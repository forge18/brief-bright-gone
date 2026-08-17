# bbg Implementation Plan

## Basis and decisions

This plan is derived from the three current normative documents in `docs/`:

- `docs/design.md`
- `docs/communication-rules.md`
- `docs/sigil-system.md`

`docs/differentiation.md` and `docs/risk-register.md` are deleted in the working tree and are intentionally excluded.

The implementation sequence below is a dependency plan, not product releases.

### Resolved specification decisions

1. **Inline emphasis:** v1 uses a prefix form: `*keyword`, where `keyword` is the maximal non-whitespace run following `*`. It decodes to `**keyword**`. This uses one marker, is unambiguous at a whitespace boundary, and is more token-efficient than paired delimiters. Literal asterisk-prefixed content must be put in inline verbatim or a code fence. The parser never interprets content inside either verbatim form.
2. **Terminal state:** every final response has exactly one terminal sigil, with optional detail on the same line:
   - `. <result>` → `**Done.** <result>`
   - `? <decision and explicit options>` → `**Decision needed:** <decision and explicit options>`
   - `x <root cause>; options: <options>` → `**Blocked:** <root cause>; **Options:** <options>`
   This preserves G1 while allowing BRIGHT's self-sufficient details and decision options.
3. **Success gates:** establish written thresholds before feature implementation. Run the probe and benchmark against those thresholds; adjust them only through a documented decision after a baseline, never silently.

## Specification issues to close before coding

| Priority | Issue | Required resolution | Plan location |
|---|---|---|---|
| Blocking | The inline `*` sigil has no grammar or example. | Add the prefix grammar and tests described above to `docs/sigil-system.md` and the installed skill. | Stage 1 |
| Blocking | The terminal sigils are not defined as a complete grammar, despite G1 requiring exactly one. | Add the state-plus-detail grammar above; define that only the last nonblank decoded block may be terminal. | Stage 1 |
| Blocking | Compressor eligibility conflicts with I4: tool results can contain files, diffs, commands, errors, and stack traces. | Define a conservative classifier and recent-window policy. An uncertain payload is ineligible and passes through unchanged. | Stage 5 |
| Blocking | “Representative tasks”, malformed-sigil detection, and pass criteria are unspecified. | Version a task corpus, oracle method, metrics, sampling count, and initial thresholds. | Stage 2 |
| Important | The table grammar lacks a formal run boundary, validation behavior for inconsistent column counts, and a fence/table interaction rule. | Define a line grammar and fail-open cases, including exact behavior in fences and verbatim spans. | Stage 1 |
| Important | Nested non-bullet block types have no explicit Markdown rendering rule. | Specify indentation and rendering for nested bullets, ordered items, blockquotes, notes, blocking items, and decisions. | Stage 1 |
| Important | Session-fingerprint collisions and concurrent requests are not addressed. | Define collision handling, isolation, locking, activity-window configuration, and fail-open behavior. | Stage 4 |
| Important | Provider forwarding leaves authentication, header allowlisting, upstream error mapping, and SSE termination/retry semantics unspecified. | Write adapter contracts and integration tests for both protocols before transforms. | Stage 3 |
| Important | The design requires Apache-2.0, while `Cargo.toml` currently declares MIT. | Change package metadata and repository license material as a dedicated, reviewed change. | Stage 9 |
| Important | Installer probing “common agent config locations” conflicts with the intent to avoid agent-specific support unless the probe contract is bounded. | Define a small, documented probe registry, install manifest, ownership rules, and uninstall behavior. | Stage 7 |
| Non-blocking | Several lint checks need explicit false-positive exclusions. | Exclude verbatim/fenced text and required identifiers from overlap, grammar, hedge, and terminal checks; label heuristic results. | Stage 8 |

## Initial quality gates

These are starting acceptance criteria, not claims about current results:

- **Format probe:** 100% parseable sigil responses for the curated happy-path corpus; 0% malformed or silently misdecoded responses. Non-sigil output is counted separately as zero-sigil noncompliance, never parsed as malformed sigil input.
- **Round trip:** property and fixture tests prove `decode(response) → substitute(request) = original sigil bytes` for every supported block type, table, escape, stream chunk boundary, inline verbatim span, and fence.
- **Safety:** 0 transformations of ineligible payloads in a labeled safety corpus; any uncertain classification fails open.
- **Protocol parity:** OpenAI Chat Completions and Anthropic Messages passthrough and streaming integration tests preserve content, usage, status, and terminal errors.
- **Benchmark:** paired, randomized repeated runs of the versioned corpus; no statistically meaningful regression in task correctness or median turns; a positive billed-cost saving with provider usage evidence. If the corpus is too small for statistical confidence, report it as inconclusive rather than successful.
- **Lint:** lint output distinguishes deterministic violations, heuristic flags, and unsupported rules; transcript checks exclude required verbatim identifiers from overlap findings.

## Dependency plan

### Stage 0 — Foundation and contracts

1. Inventory the existing Rust crate, command shape, tests, and current license files.
2. Define modules and stable data types: canonical request/response, stream event, cost record, transcript record, store reference, transform outcome, and error taxonomy.
3. Add test fixtures and property-test support before parser or proxy work.
4. Document local configuration boundaries and secret-redaction rules for logs and transcripts.

**Exit gate:** crate builds; fixtures and test scaffolding are in place; no provider request is yet transformed.

### Stage 1 — Freeze the sigil specification

1. Amend `docs/sigil-system.md` with an ABNF-like line grammar for block sigils, nesting, terminal lines, table runs, escapes, fences, inline verbatim, and prefix emphasis.
2. Define malformed input and all fail-open conditions, including inconsistent tables and ambiguous prefixes.
3. Specify decoded Markdown for every nesting/type combination, with normative examples.
4. Create parser fixtures for valid, ambiguous, malformed, escaped, fenced, and chunked input.
5. Implement the pure incremental decoder and its fence state machine.

**Exit gate:** fixture and property tests cover the grammar; decoder behavior is deterministic and fail-open.

### Stage 2 — Define measurement before optimization

1. Version a representative task corpus with expected task outcomes and a method for classifying follow-up activity.
2. Define the R14 probe runner, target models, prompt setup, response capture, parse categories, and initial thresholds.
3. Define paired benchmark execution, randomization, repetition count, billing calculation, confidence reporting, and baseline storage.
4. Run the probe before committing the format as an ecosystem contract; revise the skill/format only with recorded evidence.

**Exit gate:** a reproducible baseline report exists, or the project stops with a documented encoding-compliance failure.

### Stage 3 — Build a transparent proxy

1. Implement a canonical model plus isolated adapters for OpenAI Chat Completions and Anthropic Messages.
2. Forward headers and authentication safely; redact credentials from errors, telemetry, and transcripts.
3. Support non-streaming and SSE streaming responses without changing content or provider usage fields.
4. Specify upstream timeout, cancellation, retry, and error propagation behavior.
5. Run real endpoint-override integration tests with disposable credentials and fixtures.

**Exit gate:** passthrough is protocol-correct; both adapters have verified streaming and error tests.

### Stage 4 — Add CCR storage and sigil round trip

1. Implement content-addressed blob storage, normalized-markdown keys for sigil originals, atomic writes, and corruption handling.
2. Decode response sigils to Markdown while storing original sigil bytes.
3. On requests, normalize assistant-message Markdown, look up originals, and substitute only on a hit; record misses and pass Markdown unchanged on a miss.
4. Define per-session state, longest-prefix matching, collision behavior, concurrency, and liveness pins.
5. Add round-trip, normalization, restart, collision, and hash-miss tests.

**Exit gate:** I1, I6, I7, and relevant I8 behavior are demonstrated in unit and integration tests.

### Stage 5 — Implement safe, recoverable compressors

1. Define a payload classifier, source metadata requirements, and recent-window policy that enforce I4 conservatively.
2. Implement served-reference tracking and `bbg get <ref>` before any reference-producing transform.
3. Implement TOON only for explicitly eligible JSON/tabular payloads, with exact recovery tests.
4. Implement repeated-log collapse only for explicitly eligible logs, with original-byte recovery tests.
5. Implement cross-turn file dedup only after fingerprinting and served-reference exemptions are verified.
6. Add transform accounting and require every transform to persist originals before forwarding the reference.

**Exit gate:** every transform is exact-recoverable; adversarial safety fixtures prove protected content is not compressed.

### Stage 6 — Pin local constraints and add calibrated cost logic

1. Define the local-only protected-segment config format and enforce source provenance.
2. Inject and verify the segment in each adapter's system position without duplicating it.
3. Normalize provider usage into canonical cost records and make pricing provider configuration.
4. Implement calibration storage, minimum sample threshold, conservative uncalibrated behavior, and predicted-savings decisions.
5. Add cache-breakpoint placement only after transformed content is final and only when calibrated.

**Exit gate:** no wire data can alter pinned constraints; uncalibrated operation does not compress or move cache breakpoints.

### Stage 7 — Ship the skill and installation lifecycle

1. Generate a compact, versioned skill from the frozen communication and sigil rules.
2. Implement `bbg skill`, explicit-path install, bounded best-effort probe, manifest-backed idempotency, uninstall, and upgrade replacement.
3. Implement `bbg doctor` checks for skill currency, proxy reachability, endpoint override, and store writability.
4. Document endpoint-override, same-machine locality, shell permission, and sandbox limitations.

**Exit gate:** install/uninstall/upgrade is idempotent and doctor reports each check honestly.

### Stage 8 — Add observability, linting, and benchmark reporting

1. Define schema-versioned JSONL transcripts with redaction, skill version, transform receipts, cost records, and passive-lint results.
2. Implement shared lint rules for stdin/file and transcript modes; mark grammar and semantic checks as heuristic where applicable.
3. Add false-positive exclusions for code fences, inline verbatim, paths, identifiers, commands, errors, and required terminal state text.
4. Implement `bbg stats` from canonical telemetry, showing actual observed usage, assumptions, and per-compressor estimates separately.
5. Implement the benchmark harness and report correctness, turn count, costs, confidence, sigil compliance, and residual risks.

**Exit gate:** reports are reproducible from logs and never label estimates as provider-billed facts.

### Stage 9 — Release hardening and optional rendering

1. Update licensing from MIT to Apache-2.0 across package metadata and repository license material, after confirming intended scope.
2. Run cross-platform CI for parser, store, proxy, installer, CLI, and benchmark fixtures.
3. Conduct security review of proxy headers, local storage permissions, transcript redaction, local config provenance, and recovery behavior.
4. Implement `bbg wrap` only if core gates pass; keep it isolated and fail-open.
5. Publish operations documentation and known limitations, including agents without endpoint override and local/wire history divergence.

**Exit gate:** all mandatory quality gates pass; optional wrapper failure cannot affect core proxy behavior.

## Explicit non-goals

Do not add MCP, trained models, mechanical prose rewriting, intensity modes, a per-agent compatibility matrix, PTY interception as a core path, or an implementation of external research systems. Do not begin a compressor until I4 eligibility and recovery behavior are testable.
