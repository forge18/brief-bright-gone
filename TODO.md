# bbg TODO

Derived from `PLAN.md`. Check items only with linked evidence (test output, benchmark report, or reviewed documentation change).

## Routing legend

| Complexity | Model | Use |
|---|---|---|
| small | gpt-oss 20B (Ollama) | Bounded questions and simple edits |
| large-low | Deepseek v4 Flash | Routine multi-file work |
| large-medium-low | GPT 5.6 Luna | Cross-module planning and review |
| large-medium | GPT 5.6 Terra | Architecture and complex debugging |
| large-high | GPT 5.6 Sol | High-risk adjudication |

Escalate for ambiguity, cross-module impact, contradictory evidence, failed verification, security/data-integrity risk, disagreement, or repeated failure. Route down when work is local, specified, and mechanically verifiable.

## Spec-completeness scorer

**Plan:** validate the signal before exposing it to a model: compute + log → correlate against outcomes → S3 injection A/B only on a demonstrated signal.

> **First-turn identification risk:** the provider's first `user` turn is an agent-composed composite. Stage 1 resolves this empirically: score both the raw composite and trailing prose after `detect` strips structured content; Stage 2 injects only the variant that predicts outcomes on held-out data.

### Stage 1 — passive signal validation

- [x] **[large-medium · GPT 5.6 Terra]** Specify the shared score lifecycle and versioned readiness observation contract.
  - Evidence: [`docs/score-lifecycle.md`](docs/score-lifecycle.md) defines the lifecycle, dual variants, content-free receipt contract, and evidence gate.
- [x] **[large-low · Deepseek v4 Flash]** Implement deterministic readiness scoring and transcript receipts behind record-only configuration.
  - Depends on: observation contract.
  - Evidence: [`src/signals.rs`](src/signals.rs), [`src/transcript.rs`](src/transcript.rs), and [`src/proxy.rs`](src/proxy.rs); `cargo test` passes with deterministic, no-source-text receipt and proxy attachment tests.
- [x] **[large-low · Deepseek v4 Flash]** Add score-report aggregation over transcripts, cost records, and a manually labelled outcome sample.
  - Depends on: record-only scorer.
  - Evidence: [`src/benchmark.rs`](src/benchmark.rs) and `bbg benchmark readiness-report`; `cargo test` proves first-turn grouping, per-variant buckets, cost joins, and explicit absent labels.
- [x] **[large-medium · GPT 5.6 Terra]** Run the held-out Stage 1 analysis and select the predictive segmentation variant or stop.
  - Evidence: [`src/benchmark.rs`](src/benchmark.rs) implements a stable 20% held-out split and only selects a variant when score/correctness and score/cost correlations meet the documented threshold; [`src/bin/bbg.rs`](src/bin/bbg.rs) exposes `bbg benchmark readiness-analysis`. With no labelled corpus, the initial report stops safely as `stop_insufficient_evidence`; `cargo test` passes.

### Stage 2 — S3 injection experiment

- [x] **[large-medium · GPT 5.6 Terra]** Specify the eval-gated advisory injection contract.
  - Evidence: [`docs/s3-advisory-contract.md`](docs/s3-advisory-contract.md) defaults to `enabled = false`, fixes the score-only template, defines request-only first-user placement and local session-arm assignment, prohibits persistence/echo/CCR/response verification, and preserves cache-breakpoint ordering.
- [x] **[large-low · Deepseek v4 Flash]** Implement the disabled-by-default S3 injection and placement tests.
  - Evidence: [`src/operations.rs`](src/operations.rs), [`src/session.rs`](src/session.rs), [`src/signals.rs`](src/signals.rs), and [`src/proxy.rs`](src/proxy.rs) provide local-only fail-closed config, retained session arms, a fixed score-only template, first-turn placement, and content-free receipts. `cargo fmt --check && cargo test` pass; proxy tests prove user-tail placement and reject tool-only content.
- [x] **[large-medium · GPT 5.6 Terra]** Run a per-session randomized `record_only` versus `inject` benchmark and make the ship/drop decision.
  - Evidence: [`src/benchmark.rs`](src/benchmark.rs) and `bbg benchmark experiment-report` join content-free session-arm receipts, independent labels, and observed cost records while treating turns as diagnostic. With no eligible labelled, priced arms, the initial decision safely stops as `stop_insufficient_evidence`; `cargo test` passes.

## Additional score signals

**Lifecycle:** each starts as a transcript/benchmark label or S0 user-facing `bbg stats` receipt. Only a demonstrated benefit can graduate a signal to an S3 model-visible advisory; no signal starts there.

- [x] **[large-medium-low · GPT 5.6 Luna]** Add terminal-trajectory receipts from parsed `.`, `?`, and `x` terminals.
  - Evidence: [`src/signals.rs`](src/signals.rs) records terminal state without body text; [`src/benchmark.rs`](src/benchmark.rs) detects ping-pong, repeated normalized blocked causes, and no-done runs, joins cost/optional labels, and is exposed by `bbg benchmark terminal-report`.
- [x] **[large-low · Deepseek v4 Flash]** Surface per-session cache health in `bbg stats`.
  - Evidence: [`src/operations.rs`](src/operations.rs) computes provider-observed cache-read trend, read-to-miss churn, and cache-miss billing without changing request bytes; [`tests/cli_integration.rs`](tests/cli_integration.rs) covers stable stats output; `cargo fmt --check && cargo test` pass.
- [x] **[large-low · Deepseek v4 Flash]** Add cost-burn projection receipts to `bbg stats`.
  - Evidence: [`src/operations.rs`](src/operations.rs) distinguishes observed costs from lower-median next-turn projections and flags sessions above three times the recorded session median; [`tests/cli_integration.rs`](tests/cli_integration.rs) covers the stats receipt; `cargo fmt --check && cargo test` pass.
- [x] **[large-medium · GPT 5.6 Terra]** Extend session repetition tracking into a thrash score.
  - Evidence: [`src/session.rs`](src/session.rs), [`src/signals.rs`](src/signals.rs), [`src/proxy.rs`](src/proxy.rs), and [`src/benchmark.rs`](src/benchmark.rs) retain only opaque in-memory fingerprints and content-free aggregate receipts, distinguish exact results, near calls, and edit-fail-edit cycles, and expose expensive versus wire-cheap repeats via `bbg benchmark thrash-report`; `cargo fmt --check && cargo test` pass.
- [x] **[large-medium-low · GPT 5.6 Luna]** Measure completeness delta across clarification exchanges.
  - Depends on: passive readiness scorer.
  - Evidence: [`src/session.rs`](src/session.rs), [`src/transcript.rs`](src/transcript.rs), [`src/operations.rs`](src/operations.rs), and [`src/benchmark.rs`](src/benchmark.rs) allocate local request ordinals to pair assistant `?` terminals with the next user turn, record filled readiness slots, and associate each variant delta with reply-turn observed billing plus optional session-scoped correctness; `bbg benchmark clarification-report` exposes the report and correlations; `cargo fmt --check && cargo test` pass.
- [x] **[large-low · Deepseek v4 Flash]** Add per-model substitution-miss and zero-sigil health tables to `bbg stats`.
  - Evidence: [`src/operations.rs`](src/operations.rs) persists and deterministically groups schema-versioned health counters by provider/model; [`src/proxy.rs`](src/proxy.rs) records substitution attempts/misses and observed text/zero-sigil responses without source text; [`src/bin/bbg.rs`](src/bin/bbg.rs) renders stable per-model rows with unavailable zero-denominator rates and a protocol-health disclaimer; focused unit/CLI/proxy tests and `cargo fmt --all -- --check && cargo test --all-targets` pass.
- [x] **[large-medium-low · GPT 5.6 Luna]** Add an edit-anchor recovery rule to the installed skill.
  - Evidence: [`src/skill.rs`](src/skill.rs) v1.0.1 directs `bbg get <ref>` before editing `[bbg:file-ref:<ref>]`; `cargo test` passes.

## Cache optimization

**Boundary:** Anthropic has explicit breakpoint slots; OpenAI caching is automatic. Never rewrite agent-selected prompt content to improve cache hits—measure churn and preserve the emitted request prefix instead.

- [x] **[large-low · Deepseek v4 Flash]** Guard Anthropic cache-breakpoint budget exhaustion.
  - Evidence: [`src/operations.rs`](src/operations.rs) counts agent-reserved cache controls in Anthropic prompt locations, preserves existing controls, and does not add a fifth of the provider's four slots; proxy and unit regression tests pass.
- [ ] **[large-medium · GPT 5.6 Terra]** Add calibrated deepest-stable-prefix Anthropic breakpoint placement.
  - Depends on: breakpoint-budget guard.
  - Evidence required: session-prefix frontier maps deterministically to an unmodified Anthropic content block; placement preserves agent slots, is stable across matching turns, and has captured-payload tests for collisions, compaction, and unsupported content shapes.
- [ ] **[large-low · Deepseek v4 Flash]** Select calibrated per-session Anthropic cache TTL from observed inter-turn gaps.
  - Depends on: timestamped per-session cost observations and breakpoint-budget guard.
  - Evidence required: deterministic 5-minute/1-hour policy with uncalibrated 5-minute fallback, pricing-aware threshold, no provider request changes for OpenAI, and ledger/report tests for absent or out-of-order observations.

## Preset output formats

**Boundary:** output-format preferences live in the installed skill and passive raw-sigil lint only. Do not route them through `detect`, rewrite model prose in the proxy, or reintroduce content-density tiers. R14 probes are optional model-scoped smoke tests; live per-model/per-skill-version telemetry is the release gate.

- [x] **[large-low · Deepseek v4 Flash]** Add per-model/per-skill-version format-compliance comparisons to `bbg stats`.
  - Evidence: [`src/operations.rs`](src/operations.rs) appends version/table counters, groups only provider/model peers with explicit legacy gaps, requires 40 text responses in both versions, and flags material zero-sigil (+5 points) or malformed-table (+2 points) regressions for rollback recommendation; [`src/bin/bbg.rs`](src/bin/bbg.rs) renders denominators, deltas, baselines, and assessments without mutating installed skills; `cargo test --all-targets` passes.
- [x] **[large-low · Deepseek v4 Flash]** Release v1.1.0 bullet and ordered-procedure preferences in the installed skill.
  - Depends on: per-model/per-skill-version format-compliance comparisons.
  - Evidence: [`src/skill.rs`](src/skill.rs) v1.1.0 contains only the bounded preference for `-` parallel facts and `-#` ordered procedures, with no table arm or proxy behavior change; `cargo test --all-targets` passes.
- [x] **[large-low · Deepseek v4 Flash]** Add a raw-sigil heuristic for missed bullet structure.
  - Depends on: the corresponding format-skill amendment and completed raw-vs-decoded lint boundary.
  - Evidence: [`src/lint.rs`](src/lint.rs) emits heuristic F4 only for three substantial raw prose lines sharing a two-word prefix, excluding sigil, fenced, and verbatim forms; transcript skill-version rows permit later M6 follow-up/correctness evaluation; `cargo test --all-targets` passes.
- [ ] **[large-low · Deepseek v4 Flash]** Evaluate and, only with scoped evidence, add table preferences and missed-table lint.
  - Procedure: freeze exact candidate bytes and run table guidance as its own R14 arm; any wording change invalidates that arm's evidence. A report is evidence only for the models tested, never a generic compatibility gate.
  - Evidence required: per-model versioned telemetry reports denominators and no material malformed-table regression (+2 percentage points) versus that model's prior version after 40 text responses in both versions, alongside manual silently-misdecoded review and independent M6 follow-up/correctness evaluation before retaining the guidance.

### Parked

- Instruction-decay scoring: constraint pinning solves the underlying problem; do not build a score unless that premise changes.

## README product page

- [x] **[large-low · Deepseek v4 Flash]** Finish the product-facing README.
  - Evidence: [`README.md`](README.md) now has the approved repository description, researched feature list, input/output examples, endpoint requirement, evidence/limits, and local document links; Markdown diagnostics and `git diff --check` pass.

