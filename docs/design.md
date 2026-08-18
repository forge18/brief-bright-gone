# bbg — Design Document

Audience: an implementing LLM. This document is the authority on what to build and why. It assumes no prior context.

This specifies **the complete system**, implemented once. There are no phases. Where an ordering appears (§11) it is a dependency graph, not a release plan.

Companion documents in `docs/`:

- `communication-rules.md` — the BRIEF / BRIGHT / GONE framework. Normative.
- `sigil-system.md` — the wire format. Normative.
- `differentiation.md`, `risk-register.md` — background research. Informative only, and predate this design.

Where this document and a companion disagree, this document wins.

---

## 1. What bbg is

A communication standard for coding-agent output, plus a proxy that teaches, renders, enforces, measures, and compresses it — all at the one interception point every agent shares: the provider API.

Positioned against **Caveman** (~98K stars), which compresses output by dropping articles, prepositions, and connectors. That approach damages agent behaviour: token-level deletion removes exactly the tokens carrying action semantics (AGORA, arXiv:2605.26596). Caveman's own documentation concedes the real win is readability rather than cost.

bbg's claim is narrower and defensible: **terseness comes from cutting what shouldn't have been said, never from cutting words inside what should.** Retained sentences stay whole and grammatical.

### Optimization target

**Cost, in billed dollars.** Not token count.

Two facts govern every decision below:

- Output tokens run roughly 5x input. Output is the surface worth attacking.
- A follow-up round trip is expensive — full context re-send plus fresh generation. On a large context, one avoided follow-up outweighs a great deal of formatting overhead.

Therefore: **never trade completeness for brevity.**

---

## 2. Scope

| Component | Role |
|---|---|
| **Proxy** (`bbg-proxy`) | The primary component. Protocol adapters, sigil decode + substitution, compressor set, constraint pinning, cost telemetry and gating, transcript logging |
| **Skill** | Teaches the model the sigil encoding and the communication rules. Installed as part of proxy setup |
| **CCR store** + `bbg get` | Byte store for every transform; exact recovery |
| **`bbg lint`** | Checks output against the communication rules |
| **`bbg stats`** | Reports dollars saved — by compressor type and in total — from telemetry |
| **`bbg doctor`** | Installation diagnostics: pass/fail per check |
| **Benchmark harness** | Measures the three criteria from proxy logs |
| **`bbg wrap`** | Optional cosmetic renderer for raw-stdout CLIs. Nothing depends on it |

### The one hard requirement

**The agent must support a provider endpoint override** (`BASE_URL`, `ANTHROPIC_BASE_URL`, or equivalent). This is the system's single per-agent assumption. It is a configuration convention, not code: bbg maintains no compatibility matrix and no agent-specific branches. An agent that cannot redirect its provider endpoint cannot use bbg at all (R18).

**Out of scope:**

- **MCP.** bbg neither consumes nor provides MCP. Recovery is a CLI subcommand.
- **Trained compression models.** No Python, no torch, no model weights — bbg is a single Rust binary. This is a cost/benefit call, not a principle: a trained model means 2.5–8GB of weights and a GPU, which ends the single-binary property and the Windows story, and puts a solo project against Paritok's 4B on its own ground.
- **General prose compressor.** Excluded on evidence, not on phasing — see §5.3.
- **Mechanical rewriting of model prose.** No compressor rewrites the model's words after generation. Output brevity comes from the skill instructing the model what not to say (BRIEF). The encoding changes how prose is transmitted, not what it says.
- **Intensity levels.** One level. No lite/full/ultra tiers.
- **Per-agent support code.** No compatibility matrix, no agent-specific branches. The endpoint override above is the entire agent-facing surface.
- **Terminal stream interception as a core mechanism.** The PTY delivers agent-dependent bytes by construction — TUI agents re-render model prose before it reaches the terminal, consuming markup in the process. All identification and rendering happens at the proxy, where the model's raw bytes are available with zero identification problem. The wrapper (§8) survives only as an optional prettifier.

### Platform

Rust. Single binary. macOS, Linux, and Windows. Nothing in the core touches a PTY, so Windows no longer carries special design weight; ConPTY's synthesized-stream behaviour is confined to the optional wrapper (§8).

### License

**Apache-2.0.**

---

## 3. Architecture

One component in the data path. Everything else hangs off it.

```
            request path
agent ───────────────────────────► bbg-proxy ───► provider
        (history: markdown)     │  substitute sigil originals (§3.1)
                                │  compressors (§5.3)
                                │  constraint pinning (§5.5)
                                │  telemetry (§5.6)
                                │
            response path       │
agent ◄─────────────────────────┴─ bbg-proxy ◄─── provider
        (markdown)                 decode sigils → markdown (§3.1)
                                   originals → CCR store

CCR store ◄── bbg get <ref>        (recovery, §6)
proxy logs ──► bbg lint / benchmark (§7, §10)
bbg wrap                            (optional, §8)
```

The model emits sigil shorthand (per the skill). The agent never sees it: the proxy decodes it to markdown before the response reaches the agent, and every agent renders markdown natively — that is the one display capability that can be assumed universally. On the way back up, the proxy restores the sigil originals, so the wire history stays in the compact form.

### 3.1 The round trip

The mechanism that makes the whole design agent-agnostic. **Round-trip exactness comes from storage, not from an inverse function.**

**Response path.** The proxy decodes each assistant message's sigil content to markdown and writes the **original sigil bytes** to the CCR store, keyed by a hash of the decoded markdown. The agent receives, displays, and stores markdown.

**Request path.** For each assistant message in the incoming history, the proxy hashes the content, looks it up, and substitutes the stored sigil original.

**Hashing is over a normalized form, on both paths** — strip trailing whitespace per line, collapse runs of blank lines. Agents that trim or re-wrap assistant content before echoing it back in history would otherwise turn every cosmetic touch into a permanent hash miss, silently costing tokens for the rest of the session. Originals are stored under the normalized key. Exact bytes go upstream — I1 and I6 are satisfied by construction, with no canonical markdown→sigil transform and no inverse-function correctness burden.

**Hash miss = fail-open.** If the agent has modified, truncated, or compacted an assistant message, the hash misses and the markdown goes upstream as-is. That costs tokens; it never costs correctness. Compaction therefore degrades savings, not behaviour.

**What survives.** Output-token savings survive: the model still *generates* sigils. History savings survive: the wire history holds sigils. Display quality is solved for every agent — TUI or raw CLI — because the agent renders its native format.

**Known divergence (R19).** The agent's local history stores markdown while the wire history stores sigils. Invisible to the agent except where it computes something from its local copy expecting wire equivalence. Accepted risk; the hash-miss path is the safety net.

---

## 4. The skill

**The load-bearing artifact.** Everything on the output side works only if the model complies.

### Installation — coupled to the proxy

**The skill and the proxy are a unit.** A model emitting sigils to an agent with no proxy produces unreadable output, so the installer treats *skill installed, proxy not configured* as a misconfiguration. Skill installation happens as part of proxy setup, not independently.

`install.sh` (already in the repo) and `bbg install` configure the proxy and write the skill into common agent config locations. One `curl | bash`, and bbg is live wherever it landed.

**No supported-agents list.** The skill is one file, identical everywhere. Any agent that reads instructions from a config directory can use it.

Requirements:

- **Best-effort probe.** Check common agent config locations and install where one is found. A miss is not an error.
- **`bbg install --path <dir>`** places the skill anywhere the user names, for agents the probe does not find.
- **`bbg skill`** prints the skill to stdout, so it can be piped or pasted wherever the user wants.
- **Idempotent.** Re-running replaces cleanly; no duplicate or stale copies.
- **`bbg uninstall` removes it** from every location it was written to.
- **Version the installed copy** so an upgrade can detect and replace an older one.
- **`bbg doctor`** prints pass/fail per check: skill present and current, proxy reachable, endpoint override set in the agent's environment, store writable. This is the R17/R18 support surface — for a solo maintainer, the support burden is measured in evenings.

### Contents

1. The sigil encoding (from `sigil-system.md`), as a compact reference the model consults while writing.
2. The communication rules (from `communication-rules.md`), as behavioural instructions.
3. One line on `bbg get` (§6) so the agent knows recovery exists.

### Constraints

- **Small.** It rides in the prompt every turn. It sits in the stable prefix, so it is a cache read after turn one, but size still matters. Target well under Caveman's ~1–1.5k tokens.
- **Unambiguous.** Every sigil gets exactly one meaning and one example. Models are inconsistent about format compliance; ambiguity multiplies that.
- **Escaping made rare.** Teach the model to backtick a line beginning with a path, flag, or identifier rather than escape it. That is correct under F6 anyway, which demotes `\` to a fallback.
- **Byte-stable and prefix-positioned.** Rewriting it per turn breaks the provider's prompt cache and converts cheap cache reads into expensive cache writes.

---

## 5. The proxy (`bbg-proxy`)

The agent points at it via its endpoint override; the proxy transforms and forwards.

### 5.1 Protocol adapters

**Two wire protocols, natively: OpenAI Chat Completions and Anthropic Messages.** Together they cover effectively every agent and provider — Gemini and most local servers expose OpenAI-compatible endpoints.

Internally: one canonical request/response model with two protocol adapters. Each adapter also normalizes its protocol's usage fields (cache creation, cache read, plain input, output) into one canonical cost record for §5.6. This is protocol-count code — bounded at two — not per-agent code, and does not violate the no-compatibility-matrix rule.

### 5.2 Sigil decode and substitution

The round trip of §3.1. The decoder is a small custom line parser — one-line-per-block makes it near-trivial — living in the proxy. Fail-open: content that does not parse as sigil form passes through untouched (I7), so a non-compliant model degrades to raw-but-readable output, never to mangled output.

Streaming behaviour is specified in `sigil-system.md`: non-terminal blocks decode at `\n` (a terminal `.`/`?`/`x` line is held one line to confirm it is the final nonblank line, else it renders raw); inside a fence the proxy passes bytes through unchanged as they arrive, tracking only fence state.

### 5.3 The compressor set

**Architecture: type-specific compressors with a byte store and exact recovery.** This is Caveman's shape, and it is proven — in a pinned 54-run Claude Code benchmark they reported 33.2% fewer provider-reported input tokens while passing all 18 exact-answer checks.

Every compressor writes originals to the CCR store (§5.7) and tags output with a ref. Lossy on the wire, exactly recoverable from disk.

**Implemented compressors:**

| Type | Transform |
|---|---|
| **TOON** | JSON and tabular tool results re-encoded losslessly |
| **Logs** | Collapse repeated lines and progress output |
| **Cross-turn file dedup** | Second and later identical file reads become refs to the first |

**v1 scope: library-only.** All three activate only for tool results carried by
a local attestation `(digest, locator, metadata)`, supplied by an integrating
application through `build_router_with_tool_result_attestations`. This was a
deliberate descope from the original "shipping v1 proxy feature" plan: an
attestation pins exact tool-result bytes by digest, which only code that owns
tool execution can produce at runtime, so the standalone `bbg-proxy` binary —
which constructs no attestations — never compresses a tool result. The
un-forgeable-from-wire-data property (see the security requirement below) is
what forces this: a CLI operator cannot pre-compute digests of output that does
not exist yet.

*Future work — rule-based attestation.* The feature can be reached from the CLI
without forgeable attestations if the operator attests *classes* rather than
bytes: e.g. "`role:tool` messages on the OpenAI route are tool results, kind via
`detect`." That is still owner-supplied local config keyed on the protocol's own
provenance field, not wire data. What it cannot supply is `captured_at` /
`in_recent_window`, so `classify`'s staleness and I4 guards would need
config-level defaults — a real weakening, and the reason per-digest attestation
was chosen for v1.

Dedup matters more than it looks. Every file read stays in history and is re-sent on every subsequent turn, so content savings compound roughly quadratically across a session while a fixed-block saving compounds linearly.

**Why no general prose compressor.** It is the one type with a measured failure and no measured payoff. Compression corrupting verbatim edit anchors cut SWE-bench-derived Go patch application from 27/40 to 15/40, and an arm removing 38% of tool-output tokens cost 6.8% *more* (arXiv:2607.12161). Recovery does not rescue this: the agent does not know the bytes were altered, so it never calls `bbg get` — it emits an edit from a plausible-looking compressed copy. The protection must be structural, not behavioural.

**Eligibility is governed by I4.** Never alter bytes the agent may need to reproduce exactly: file contents in the recent window, diffs, error text, stack traces, command strings.

**Cache discipline.** Every transform changes bytes. Apply at write time, consistently, so the transformed form is canonical and byte-stable across turns. Transforming on the way out each turn rewrites the prefix and destroys the prompt cache.

**Adding compressor types** is a benchmark question — each new type is one more arm on runs you are making anyway.

### 5.4 Session identity

Cross-turn dedup requires knowing which prior request this one continues, and no protocol carries a session ID. **The proxy is per-session stateful, not a stateless passthrough.**

**Prefix fingerprinting.** Hash rolling prefixes of the message history; an incoming request belongs to the session with the longest matching known prefix.

**Compaction breaks the fingerprint — treat it as a new session.** Refs already present in retained text still resolve against the store, so nothing is lost; only future dedup opportunities reset. Consistent with the fail-open posture of §3.1.

**A session is active** while its fingerprint has been seen within a configurable activity window. GC pinning (§5.7) keys off this.

### 5.5 Constraint pinning

From the Governance Decay paper (arXiv:2606.22528). Agents obey in-context policies while visible, but compaction drops them: violations rise from 0% to 30%, reaching 59% on some models. The published fix restores violations to 0% across seven models at roughly 47 tokens, with 99% of allowed actions still completing.

The proxy maintains a protected segment carrying the user's standing rules: **injected into every outbound request, presence verified per-request, mapped by each protocol adapter to that protocol's system position.** The proxy sees requests, not agent-side execution, so per-request presence is the enforceable guarantee — it cannot gate actions, and this design does not claim to.

**Security requirement.** The segment is built **only** from local configuration the user controls. Never from bytes that crossed the wire. The same paper demonstrates a Compaction-Eviction Attack in which adversarial content inside a tool result biases the summarizer into dropping constraints.

### 5.6 Cost telemetry and gating

Read the canonical cost record (§5.1) on every response. Log it. Report dollars. **Pricing is a per-provider configuration table** — "any model" includes local models that bill nothing, so price is data, never a constant.

**Gating is runtime-calibrated, never constant-driven.** The cache-aware cost model (CAPC, arXiv:2607.15516) is real but its constants are measured on one vendor's model at one moment — a two-tier cache with a threshold near 3,500 tokens and hit rate plateauing around 0.83. Those numbers do not transfer. OpenAI caches automatically with different minimums, Gemini has its own pricing, local models have no cache billing at all.

What transfers is the model: cache hit rate is not 1.0; over-compression can push a cached prefix out of its tier and cost money; compression should be a computed decision, not an assumption.

**Implementation:** estimate hit rate and tier behaviour per provider from observed usage fields. Compress only when predicted savings exceed predicted cache-break cost. **Below the calibration sample threshold, do not compress.** The safe default is the uncalibrated default.

Price compressed content at the full base rate and a frozen prefix as a cache hit after turn one. Counting everything at list price overstates savings.

**Two health counters ride the same stream:**

- **Substitution-miss rate** — fraction of assistant messages whose hash found no stored original (§3.1). Turns R19 from a guess into a per-agent measurement.
- **Zero-sigil rate** — fraction of responses containing no sigils at all. A live R14 compliance metric per model, with no benchmark runs required.

**`bbg stats`** surfaces the ledger: dollars saved, by compressor type and in total. The receipts — for a project competing on trust, showing them differentiates more than another increment of compression.

### 5.7 CCR store

Every transform writes original bytes to the store before the transformed form goes upstream. This is what makes `bbg get` possible and every transform reversible.

**Content-addressed.** Blobs are keyed by content hash; a ref *is* the hash. Identical file reads collapse to one stored blob automatically, which makes dedup detection (§5.3) a store lookup rather than separate bookkeeping, and the served-refs ledger (§6) speaks the same keys. Sigil originals are stored under the normalized-markdown hash (§3.1).

**GC is liveness-based, never pure TTL.** Content addressing makes this load-bearing rather than hygiene: one blob can back refs in several sessions, so collecting it breaks `bbg get` for all of them at once. A blob is collectible only when no pin holds it — pins come from (a) any active session (§5.4) whose history references it, and (b) the served-refs ledger's recency window (§6). TTL and size bounds apply to unpinned blobs only. Simplifying GC to pure TTL is a correctness bug, not a tuning choice.

Local filesystem. Same machine as the proxy.

### 5.8 Transcript logging

The proxy sees every request and response, so it writes session transcripts in a bbg-defined JSONL format. This is the data source for `bbg lint --transcript` (§7) and for the benchmark harness (§10) — turns and dollars come straight from these logs, agent-agnostically.

**Each record carries the installed skill version.** Every rule change then becomes an A/B arm across historical runs automatically — the data M6 needs and otherwise does not exist.

**Passive lint-in-the-loop.** The proxy runs the single-document lint checks (§7) on each response and logs violations per turn — against the model's raw sigil output, pre-decode: that is the model's actual output, and it stays stable if the decode mapping ever changes. Longitudinal rule compliance with zero extra model runs; this is what makes M6 — any rule that raises follow-up rate gets dropped — enforceable in practice. Active enforcement (auto-retry on violation) is excluded: a retry burns a full generation, so it would need the §5.6 gating math applied to itself, and no evidence yet says it pays.

### 5.9 Cache-breakpoint injection

Anthropic's `cache_control` breakpoints are explicit, and agents place them naively or not at all. The proxy injects or repositions breakpoints using the same per-provider calibration data as §5.6.

This is config-level manipulation — no content bytes change, so none of the I4 machinery applies. The same gating rule holds: **below the calibration threshold, leave breakpoints untouched.** For agents with poor cache hygiene this lever can exceed what the compressor set saves.

**Pipeline order: breakpoints are placed last, after every content transform.** Substitution and compression change the bytes an agent-placed breakpoint sits on; positioning against pre-transform content caches the wrong prefix. Injected positions are themselves subject to I6 — a breakpoint that moves each turn breaks the cache it exists to protect.

---

## 6. `bbg get`

```
bbg get <ref>
```

Retrieves original bytes from the CCR store and writes them to stdout.

**Why a CLI and not MCP.** Every coding agent has a shell tool; MCP support varies and the protocol is churning. Recovery must work on any agent.

### The served-refs ledger

Recovery output flows back through the proxy as a tool result on the next request. If the proxy re-transforms it — dedup would fire on recovered file content identical to an earlier read — the agent loops on the same ref forever. And the marker cannot live in the bytes: recovered content is recovered *precisely because* it must be byte-exact.

**The do-not-retransform signal is out-of-band, through the shared store.** When `bbg get` serves bytes, it records the ref and a timestamp in a served-refs ledger in the CCR store — under §5.7's content addressing, the ref already *is* the content hash. Before applying any transform whose output would be a ref, the proxy hashes the candidate content against recent ledger entries; a match exempts the content from transformation. Both ends are bbg-owned, so this works identically for every agent.

### Required behaviour

**Discovery.** The agent must know the command exists. One line in the installed skill (§4). No manual file editing, so the drop-in claim holds.

**Permissions.** Agents gate shell execution. First invocation will prompt; users need to allowlist it. Sandboxed setups may block it entirely. Document this.

**Locality.** CLI and proxy share the store, so both run on the same machine.

**A miss is loud.** `bbg get` on an unknown or collected hash prints an explicit error stating the bytes are no longer held and instructing a re-read of the source — the original file is usually still on disk, so a re-read is the correct degradation, and the error must say so. Never silent, never empty output.

---

## 7. `bbg lint`

Checks output against the communication rules.

**Two modes.** Single-document mode reads stdin or a file. **`bbg lint --transcript <file>`** consumes the proxy's JSONL transcript (§5.8), which is what makes the history-dependent checks possible — B3 and G2 compare against prior turns and cannot be checked from a single document.

The single-document checks also run passively inside the proxy on every response (§5.8); the CLI and the passive path share one rule implementation.

### Lintable

| Rule | Check | Mode |
|---|---|---|
| B1 | Presence of always-safe-list categories | either |
| B2 | Preamble blocklist on line 1 | either |
| B3 | N-gram overlap with the prior turn | transcript only |
| B4 | Acknowledgment-noise phrase blocklist | either |
| B5 | **Heuristic:** flag retained sentences lacking a finite verb. Labeled heuristic in output — a model-free binary cannot check grammaticality and must not pretend to | either |
| R1 | Answer-first structure | either |
| R3 | Severity label presence on actionable statements | either |
| R5 | A failure report with no stated option is a violation | either |
| R8 | Hedge density | either |
| G1 | Exactly one typed terminal state | either |
| G2 | Restatement of an earlier point | transcript only |
| G3 | Closer blocklist | either |
| G4 | Terminal question is a decision, not an offer | either |

### Not lintable

R2, R4, R6, R7, B6, and all of STANCE. Benchmark or judgment matters. The linter must not pretend to check them.

---

## 8. `bbg wrap` — optional

A cosmetic markdown-to-TTY prettifier for raw-stdout CLIs that print the proxy's markdown as plain text. **It never sees sigils** — decoding happened at the proxy — so it is a generic markdown renderer with no stream-identification problem and no bbg-specific parsing.

```
bbg wrap -- <command>
```

One rendering crate (termimad: wrapping, table balancing, structure/skin separation). Fail-open on display: unrenderable input passes through verbatim. If ConPTY misbehaves on Windows, nothing in the core is affected.

Nothing depends on this component. Build it last or not at all.

---

## 9. Invariants

Non-negotiable. Violating any is a correctness bug, not a tuning issue.

| # | Invariant |
|---|---|
| I1 | **Round-trip exactness.** The sigil original is restored byte-exactly on the request path via CCR substitution (§3.1). The property under test in CI is decode + substitute = identity. The decoder itself needs no inverse. |
| I2 | **Bytes inside a code fence or inline verbatim span are never modified.** Indentation is meaning in Python, YAML, diffs, and Odin. |
| I3 | **No token-level deletion, ever.** Articles, prepositions, connectors, and negations survive. |
| I4 | **Never alter bytes the agent may need to reproduce exactly.** File contents in the recent window, diffs, error text, stack traces, command strings. This governs compressor eligibility. Compression corrupting edit anchors cut patch success from 27/40 to 15/40 (arXiv:2607.12161). |
| I5 | **The protected constraint segment is built only from local config.** Never from wire data. |
| I6 | **Transforms are byte-stable across turns.** Non-deterministic output destroys the prompt cache and can raise cost while lowering token count. Sigil substitution satisfies this by construction — stored originals never vary. |
| I7 | **Fail-closed on the wire, fail-open on display.** When a transform cannot be verified safe, do not transform. When decoding or rendering fails, pass through verbatim. |
| I8 | **Every proxy transform is exactly recoverable.** Originals to the CCR store before the transformed form goes upstream. |

---

## 10. Benchmark

Three criteria. Nothing else.

1. **Follows the communication rules** — `bbg lint --transcript`. No model runs required.
2. **Minimizes follow-up questions and activities** — turns per task.
3. **Reduces cost** — billed dollars per task, from provider usage fields.

Criteria 2 and 3 come from the same runs: execute a task set with the skill on and with it off, count turns, sum dollars. **The proxy's transcript logs (§5.8) are the data source** — the harness needs no agent instrumentation.

**Only real requirement:** the same tasks both ways, repeated enough that run-to-run variance does not swamp the result. Agent runs are non-deterministic.

If the tasks have a pass/fail outcome, record it. Free from the same runs, and it catches the case where the agent is confidently wrong and therefore generates no follow-ups.

Each compressor type is an additional arm on the same harness.

**Do not rebuild ConstraintRot.** It exists, from the Governance Decay paper, and tests constraint survival directly.

**Transform correctness is unit tests, not benchmark.** Encode, decode, assert byte-identical. CI, free.

---

## 11. Dependency order

Not phases. What must exist before what.

1. **R14 compliance probe.** Skill text in a system prompt, raw API calls against a handful of representative tasks, for a selected evaluation model: count zero-sigil rate and malformed-sigil rate. Throwaway scripting; no bbg infrastructure. bbg remains model-agnostic, so this is empirical evidence for the selected model rather than a universal compatibility gate. This runs first when provider access is available because its failure mode is fixed in skill wording or the encoding itself, and encoding changes get expensive the moment fixtures and property tests are committed.
2. **Passthrough proxy + both protocol adapters.** Proving the loop on real agents *is* the endpoint-override verification — the only per-agent assumption gets tested first.
3. **Sigil decoder + CCR store + substitution.** The §3.1 round trip, with decode+substitute=identity property tests (I1).
4. **The skill + installer coupling + `bbg doctor`.** Needs the encoding; ships with proxy setup.
5. **`bbg get` + served-refs ledger.** Needs the store.
6. **Session fingerprinting (§5.4).** Needed by dedup.
7. **Compressors** — TOON, logs, dedup. Need the store, recovery, and fingerprinting.
8. **Constraint pinning.** Independent; needs only the adapters.
9. **Cost telemetry and gating + `bbg stats` + cache-breakpoint injection.** Needs transforms to gate; breakpoint injection reuses the calibration data.
10. **Transcript logging + `bbg lint`** (both modes).
11. **Benchmark harness.** Needs everything else to measure it.
12. **`bbg wrap`.** Optional; nothing waits on it.

---

## 12. Prior art — do not rebuild

| Project | Relevance |
|---|---|
| **Caveman** | The incumbent, and the architectural reference for §5.3. Nine type-specific compressors, byte store, exact recovery. Proxy and Engine are BSL-1.1 — the design is copyable, the code is not. |
| **Paritok** (Apache-2.0) | Proxy with `read_original` recovery and cache-stable tool filtering. Not a dependency — Python, 4B LoRA over Qwen3, GPU. Its cache-aware `/stats` accounting is worth copying conceptually. |
| **AGORA** | Take the structural-floor concept. Not the code: license unclear, Python, tuned for WebShop rather than coding agents. |
| **TOON** | Use it. |
| **ConstraintRot** | Use it. |
| **Governance Decay, CAPC, VISTA, arXiv:2607.12161** | Real papers, accurate figures. Constraint Pinning and the CAPC cost model are published solutions, not bbg inventions — cite them. |

---

## 13. Risks

| # | Risk | Mitigation |
|---|---|---|
| R9 | Tool-schema bloat is a decaying lever — Claude Code defers MCP schemas by default, and the MCP spec went stateless in the 2026-07-28 release | Do not build value on tool-schema compression |
| R10 | Compression corrupting verbatim anchors halves patch success | I4; task success recorded in the benchmark |
| R11 | Cache cost constants are vendor- and time-specific | Runtime calibration; do not compress when uncalibrated |
| R12 | The proxy is a prompt-injection target | I5; content *type* decides transforms, never content itself |
| R13 | Solo maintainer against commercializing incumbents | Scope discipline |
| R14 | Model non-compliance with the encoding | Fail-open decode (I7): unparsed content passes through as readable text. Keep the skill small and unambiguous |
| R17 | Agents move or rename their config directories, so the installer's probe misses them | No per-agent code to rot. A miss degrades to `bbg install --path`, and installs are idempotent so re-running repairs |
| R18 | An agent with no provider endpoint override cannot use bbg at all | This is the system's single hard requirement. Document it prominently. No workaround exists by design — the alternative is per-agent interception code, which is out of scope |
| R19 | Local/wire history divergence: the agent stores markdown, the wire stores sigils. An agent computing over its local copy and expecting wire equivalence may behave unexpectedly | Accepted. The hash-miss path (§3.1) fails open to markdown upstream — costs tokens, never correctness |

---

## 14. Open items

None.
