# Spec-completeness scorer (proposal, 2026-08-18)

**Goal:** give the LLM an explained, advisory estimate of whether the *collected task context* is sufficient. The LLM decides to elicit or proceed. The proxy never blocks, asks, or manufactures a reply.

## Research position

Agent-internal ask/assume work: BALAR <https://arxiv.org/abs/2605.05386>, Active Task Disambiguation <https://arxiv.org/abs/2502.04485>, Ask or Assume <https://arxiv.org/abs/2603.26233>, Clarify When Necessary <https://arxiv.org/abs/2311.09469>. Ambig-SWE <https://arxiv.org/abs/2502.13069> found models struggle to distinguish well- from under-specified instructions — the case for an external signal. Elicitation work/benchmarks: Curiosity by Design <https://arxiv.org/abs/2507.21285>, Python Code Gen by Clarifying <https://arxiv.org/abs/2212.09885>, HumanEvalComm <https://arxiv.org/abs/2406.00215>, ClarQ-LLM <https://arxiv.org/abs/2409.06097>. Beyond Expert Users <https://arxiv.org/abs/2606.30863> warns against over-elicitation.

**Gap:** no found system exposes an external, deterministic completeness score + reason codes as non-binding model context.

## Scoring model

Deterministic only; reuse `detect`; no scorer LLM call. Record a versioned slot vector before trusting an aggregate:

`task | target/artifact | success criterion | constraints | environment/repro | unresolved unknowns`

A slot is `present`, `missing`, or `not-applicable`; unknowns are a risk flag, not score credit. v1's provisional score is applicable-slot coverage. Do not claim it is calibrated probability; retain the vector so Stage 1 can test whether slots or weights predict cost/outcome.

### Input-identification risk (named)

The first provider `user` turn is commonly an **agent-composed composite**, not the human request: harness preamble, repo/environment dumps, and prior context may make a naive score falsely high. v1 has no bounded-history input (resolves the prior "first turn + bounded prior" contradiction). On every user turn it computes and logs two content-free feature records:

- `full_user`: final user message as received;
- `trailing_prose`: final unstructured prose after `detect` excludes known structured/tool/config blocks; falls back to `full_user` if unsure.

Analysis selects a segmenter only if it survives held-out evidence. The first user turn of a session is an approximation of a task boundary; task-level claims require the existing manually labelled benchmark corpus.

## Two-stage release

### Stage 1 — observe, do not inject

Default behavior: compute and log score version, segmenter, slot states, and aggregate — **never model-visible**. Persist no source excerpts, matched strings, paths, values, or other wire-derived text. Existing redacted user transcripts can be backfilled; new records preserve the scoring receipt. Turns and billed dollars come from existing ledgers; correctness/completion remains the benchmark's manual outcome label.

Test whether low first-task scores predict subsequent cost per completed task, failed/incorrect outcomes, or follow-up-heavy sessions. Turns are a diagnostic only. If no predictive signal survives held-out data, stop: the heuristics are noise and no user session was exposed to an intervention.

### Stage 2 — inject only after Stage 1 signal

The feature is S3: it changes model-visible bytes behaviorally, so `spec_readiness.enabled` defaults to `false` and is eval-gated. Randomize `record_only` vs `inject` on the labelled corpus.

**Ship gate:** no task-correctness regression and lower billed dollars per completed task. Report turns and clarification turns, but do not reject the feature merely because a useful clarification adds a turn. Drop it if cost/correctness does not improve.

## Injection contract (Stage 2 only)

- **Fixed output only:** local template, fixed vocabulary, version, score digits, and finite reason codes. No byte sequence is copied verbatim from wire content — including matched snippets, paths, errors, identifiers, or user wording. This is I5-adjacent; wire data may classify but never supply output text.
- **Request-only:** append once after the final user content; do not persist, transcript-record, replay, echo, CCR-store, or response-verify the block. Unit-test its construction/placement; no constraint-style runtime acknowledgement is needed.
- **Cache-safe placement:** outside the stable system prefix and cached zone. It runs after final user content and before the existing final cache-breakpoint placement (§5.9), which must leave it outside the cached prefix.

Example (all non-score words are fixed):
```
[bbg:readiness v1]
score: 0.60; band: low
missing: success, environment
unresolved: none
Advisory metadata, not user-authored and not binding. Ask only for material missing information; proceed when clear.
```

**Failure behavior:** scorer/segmenter/injection failure = omit the feature and forward normally.
