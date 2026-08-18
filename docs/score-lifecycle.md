# Score-signal lifecycle

Every score is a deterministic, versioned observation. It has one initial
destination and can graduate only in this order:

1. **Benchmark receipt:** append content-free fields to a transcript record.
2. **S0 user receipt:** surface derived results in `bbg stats` or a report.
3. **S3 model advisory:** only after a preregistered, session-randomized A/B
   test shows no correctness regression and lower billed dollars per completed
   task. Defaults off. Never use raw turn count as the ship gate.

A score never jumps to model-visible injection. S0 output does not modify
provider request bytes. S3 output is request-only and must have its own
placement/safety contract.

## Receipt contract

A receipt stores a schema version, signal name, deterministic variant, finite
reason/slot codes, and numeric values needed for aggregation. It stores no
source excerpt, matched substring, user wording, path, error, identifier, or
other wire-derived text. Old transcript records remain valid when no receipts
are present.

## Readiness observation

Score the first user turn in two variants without injection:

- `raw_composite`: the provider-facing user content;
- `trailing_prose`: prose remaining after `detect` removes structured/tool/config
  content; if extraction is uncertain, emit an explicit fallback reason.

Each variant records an aggregate plus per-dimension slots: task,
target/artifact, success criterion, constraints, environment/reproduction, and
unresolved unknowns. The aggregate is an observation, not a calibrated
probability. Stage 1 selects a variant only if it predicts held-out cost or
labelled task outcome; otherwise it stops.

## Clarification delta observation

A clarification report pairs an assistant terminal `?` with the next genuine
user turn in the same session. It compares both readiness variants, records
only finite readiness-slot codes that changed from missing to present, and
reports the reply turn's observed provider billing plus the optional,
session-scoped task outcome label. A locally allocated session turn ordinal
joins user, assistant, and cost records; old records without an ordinal remain
readable but are excluded rather than ordered by JSONL position or timestamp.

This is a decision-to-next-user candidate metric, not proof that a question
caused correctness or cost changes. `bbg benchmark clarification-report`
exposes the per-exchange fields and descriptive score-delta correlations only
when enough priced or labelled observations exist.

## Thrash observation

A thrash score is the sum of three content-free per-session observations:
exact repeated tool results, near-repeated calls to the same tool, and an
edit → failed-result → edit retry cycle. The live registry holds opaque digest
and token fingerprints only long enough to avoid recounting replayed history.
Transcripts retain aggregate counts, never tool names, call arguments, result
digests, or result bodies.

Exact repeated results are split by actual request handling: an **expensive**
repeat was forwarded in full, while a **wire-cheap** repeat was reduced to an
attested cross-turn file reference. Both are diagnostic repetition; neither
proves a cost or correctness outcome. `bbg benchmark thrash-report` joins the
receipt counts to observed session billing without using either as a ship gate.

## Experiment discipline

A model-visible experiment assigns `record_only` or `inject` **per session**,
records the arm in the transcript, and uses task correctness plus billed dollars
per completed task as the decision criteria. Terminal, cost, and turn patterns
may be diagnostic or benchmark labels; they are not correctness by themselves.
