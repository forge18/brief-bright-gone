# bbg — Risk Register & Mitigations (v2)

**Date:** 2026-08-16
**Status:** Answers to the material risks, with research-backed mitigations.

---

## The accepted framing

bbg = **Caveman's context benefits (compression + economics) with cleaner, more readable output.** A proxy-design alternative for people who don't like Caveman's telegraphic syntax. Not a competitor; an alternative.

---

## R3 — Compression can cost MORE (cache break)

**The problem:** (Token Reduction/2607.12161, Don't Break Cache/2601.06007) aggressive compression changes the cached prefix → breaks cache hits → billed cost rises.

**The fix (CAPC/2607.15516):**
1. **Freeze the stable prefix** — never rewrite the `system` + `tools[]` + stable-history block once it's sent. Cache hits rely on it being byte-stable.
2. **Query-agnostic compression only** for the stable prefix — deterministic (not per-query rewrites), so it stays cacheable.
3. **Tier-preserving ratio bound** — don't over-compress such that a cached prefix drops below the ~3,500-token hot-tier threshold.
4. **Compress only volatile content** (new user msg, fresh tool results) that isn't inside the cached region.
5. **Measure real cost, not tokens** — report "$ saved" (inferred) from cache-hit rate + tier, not token deltas.

---

## R4 — Compression degrades behavior (AGORA)

**The problem:** (AGORA/2605.26596) token deletion removes action grammar (identifiers, brackets, verbs) → agent reward collapses to ≤0.05.

**The fix — content-type gating + action-skeleton invariant:**
1. **Never compress action-sensitive types** — code, shell, JSON-schema, diffs (our `detect` already classifies these).
2. **Action-skeleton proof:** before/after compression, extract the skeleton (negations `not/never`, numbers, verb phrases, identifiers, error codes). If any vanishes, **refuse** the transform (return original).
3. **Protected segments byte-for-byte:** system constraints, latest user message, diff headers survive untouched (tokenfold's invariants + Ghost-in-the-Context's protected placement).
4. **Reversible (CCR):** originals always stored; nothing unrecoverable.
5. **Eval-gated:** measure on a benchmark that compressed prompts produce the same tool calls/output (like paritok's SWE-bench eval) before claims.

---

## R5 — Constraints silently erased by compaction (Governance Decay)

**The problem — RESEARCH DONE:** (Governance Decay/2606.22528) violations rise 0% → 30% (59% some models) after compaction. **Critical detail:** "when the constraint **survives**, violation stays **0%**; when **dropped**, violation 38%." Ghost in the Context (2605.12535) confirms: **protected-placement preserves policy**, task-local placement gets evicted.

**The fix — guaranteed constraint survival:**
1. **Protected policy segment** — governance constraints live in a dedicated, typed, top-of-context block that compaction/eviction must NEVER drop (byte-survival guaranteed).
2. **Typed provenance** — each constraint tagged (`[POLICY]`, `[SAFETY]`, `[IRREVERSIBLE]`) so the policy assembler recognizes and preserves them.
3. **Presence verification before action** — before any tool call that touches the constrained surface, verify the policy block is still present in context (re-inject if evicted).
4. **Fail-closed** — if the protected segment can't survive, don't compact; abort rather than lose constraints.

---

## R6 — Brevity degrades reasoning (Moonshot citation)

**The fix (precise):** the citation targets *aggressive* brevity that mangles syntax. Our `lite`/`full` with Be-Brief-Bright-Gone keeps grammar — terse, not telegraphic. Bake a hard rule: **no dropped articles/conjunctions/negations that change comprehension.** "Full" must never cross into syntax-eating.

---

## R7 — "You don't change your agent" is partly true

**Clarified scope:**
- **Proxy-only (zero agent change):** compression (input) + cost model + visibility injection (we add a budget note to the system prompt via rewriting).
- **Needs agent cooperation:** on-demand `read_original` recovery (agent must know the tool), memory/tiering (agent must call back or use MCP/hooks).
- **Honest scoping:** ship the proxy-only core first; treat recovery/memory as feature-gated on agent support, not claimed as drop-in.

---

## R8 — Correctness burden

**The fix: bounded, verifiable correctness:**
1. **Round-trip/parse-equal tests** for every transform (nothing escapes that isn't provably equivalent).
2. **Fail-closed by default** — when shape/content is uncertain, skip compression (return original).
3. **Protected segments** (constraints, latest user, diffs) never touched.
4. **`inferred` vs `verified` labels** — only claim what we measured (Caveman's honest-evidence practice).
5. **Narrow allowlists** for value compression (prose strings only, guarded against IDs/errors).

---

## The safe build path

1. **Proxy-only core first** (compression + cost model + visibility injection) — the only scope that needs zero agent cooperation.
2. **Protected constraint segment** from day one (R5) — it's the safety differentiator and cheap to do now.
3. **Action-skeleton gate (R4)** + frozen-prefix cost model (R3) built into the engine.
4. **CCR recovery + memory** as feature-gated later.
5. **Eval harness** (SWE-bench-style + ConstraintRot-style) before claiming savings or safety.

---

## Sources for R5
- Governance Decay / ConstraintRot: https://arxiv.org/abs/2606.22528
- Ghost in the Context / Policy-Carriage: https://arxiv.org/abs/2605.12535
- (Other papers as noted inline)
