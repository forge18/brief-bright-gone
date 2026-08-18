# Impact: model-agnostic format-compliance telemetry

## Target

- `src/operations.rs`: append-only health ledger fields and deterministic
  provider/model/skill-version aggregation.
- `src/proxy.rs`: records raw assistant-format observations only; it must not
  alter request or response bytes.
- `src/sigil.rs`: recognizes malformed table runs using the decoder's existing
  grammar.
- `src/bin/bbg.rs`: renders per-version compliance deltas and a rollback
  recommendation.
- `src/skill.rs` and `src/lint.rs`: follow only after telemetry is available.

## Dependents (high fan-in)

- Health records flow from both streaming and non-streaming OpenAI/Anthropic
  proxy paths into `bbg stats`.
- The health ledger is backward-compatible JSONL; older rows lack the new
  skill-version/table fields and must remain readable without being treated as
  a versioned baseline.
- `src/sigil.rs` also drives response decoding and lint semantics, so malformed
  detection must share its grammar and remain display fail-open.
- CLI integration tests pin stable `bbg stats` output.

## Affected stories

No release plan or epic capsules exist. `TODO.md` orders this telemetry before
v1.1.0 bullet/ordered-list guidance. Table guidance remains a separately
observable future arm.

## Test coverage

- `src/operations.rs`: cost/cache/health grouping unit tests.
- `src/proxy.rs`: health recording and raw-content privacy tests.
- `src/sigil.rs`: valid/malformed table fixtures and deterministic decoding.
- `tests/cli_integration.rs`: stable stats rendering.

Gaps to close: skill-version grouping, malformed-table counters, baseline
comparison, insufficient-sample reporting, and captured streaming/non-streaming
health persistence.

## Risk: Medium

The change touches shared proxy, ledger, decoder, and CLI paths, but it is
append-only and observational. The fail-open decoder guarantees that a
non-compliant model degrades display formatting rather than task correctness.

## Recommended action

Persist the installed skill version and malformed-table-run counts in each
health record. Compare only versions of the same provider/model, report
observed denominators, and mark comparisons insufficient until both versions
have at least 40 text responses. A material increase over the model's own prior
version (5 percentage points zero-sigil; 2 points malformed-table) produces a
rollback recommendation, not an automatic rewrite of installed skills. State
that maintainer sessions before broad use are real production evidence with a
small-sample limitation. Reword the R14 probe as optional model-scoped smoke
coverage, then release v1.1.0 only after this telemetry is present.
