# Remaining TODO design review

**Scope reviewed:** `TODO.md`, `docs/score-lifecycle.md`, `src/signals.rs`, `src/benchmark.rs`, `src/proxy.rs`, and `src/operations.rs` (plus directly relevant CLI/session/transcript call sites to verify feasibility). Repository revision inspected: `ee0bde2`; the working tree was already dirty. No source or test files were modified.

## Decision boundary

Synthetic fixtures can validate deterministic scoring, schema compatibility, joins, request placement, arm assignment, and CLI rendering. They **cannot** establish prediction, correctness non-regression, provider billing savings, cache behavior, or a ship/drop decision. Those require a labelled, held-out corpus and actual provider usage/billing observations.

## Concrete recommendations by unchecked TODO

### 1. Held-out Stage 1 analysis — **BLOCKED / empirical**

**Files:** `src/benchmark.rs`, `src/bin/bbg.rs`, `docs/score-lifecycle.md`

1. Add a deterministic `stage1-analysis` report over `ReadinessObservation`, with a preregistered session-id-hash split seed, eligibility/label-coverage accounting, and both variants evaluated on the same sessions.
2. On the training partition, choose the score treatment (continuous or preregistered buckets) and the winning variant; on held-out data report: correctness association, completed-task rate, billed dollars per *completed* task, and turns only as diagnostic. Include confidence intervals/uncertainty and reject selection if labels, priced sessions, or completed-task denominators are insufficient.
3. Emit `selected_variant: null` unless the preregistered held-out criteria pass. Do not derive correctness from a terminal receipt or model text.
4. Add synthetic report tests for deterministic splits, absent/duplicate labels, zero completed denominator, unmatched costs, and a deliberately non-predictive corpus that yields `null`. The actual run must consume independently manually labelled outcomes and real provider usage.

**Risk:** current `readiness_report` is association/bucket aggregation only; it has neither a held-out split nor a selection rule. A synthetic “winner” is plumbing evidence, not Stage 1 evidence.

### 2. Eval-gated S3 advisory contract — **HIGH design prerequisite**

**Files:** `docs/score-lifecycle.md`, `src/operations.rs`, `src/signals.rs`, `src/proxy.rs`

Specify before code:

- A versioned local-only advisory config with `enabled: false` by default; a selected Stage-1 variant/template version; and a separately explicit experiment mode. Reject injection when the selection/evidence identifier is absent or unrecognized.
- A fixed, content-free template whose only variable is bounded integer score digits. No source phrase, path, tool result, terminal body, model response, or wire substring may enter it.
- `record_only`/`inject` assignment once per newly selected session, retained in the in-memory session entry for its lifetime, and recorded as a content-free arm receipt. A session collision must start a new arm rather than inherit one.
- Injection only on the first genuine user turn. Score and capture the original user transcript before mutation; append the advisory after that user content, never to system/assistant history, never as a later replay, and never into transcript/CCR/response verification.
- For Anthropic, retain the cache-control operation on the stable system prefix before adding the user-tail advisory. For OpenAI, append only after the final genuine user content. Unsupported/non-text user-content shapes must safely stay `record_only`.

**Risk:** the present `LocalConfig` has no advisory/experiment contract and `Registry` retains only history/blob state, so there is currently nowhere to hold an arm assignment. The current proxy records readiness on every genuine user record, whereas the S3 contract needs explicit first-turn gating.

### 3. Disabled-by-default S3 implementation — **implementable with synthetic upstreams**

**Files:** `src/signals.rs`, `src/operations.rs`, `src/session.rs`, `src/proxy.rs`, `src/transcript.rs`, `tests/proxy_integration.rs`

1. Add typed, serde-compatible content-free receipt fields for arm/template/selected variant (old records default to absent). Keep receipt schema/version evolution explicit.
2. Add pure functions for fixed template rendering, score bounds, deterministic arm selection, and provider-specific user-tail insertion. Return a no-op reason rather than falling back to copied content.
3. Extend the session entry—not request JSON—with assigned arm and “first user advisory attempted” state; pass this result out of `open_*_session` without exposing it to the provider.
4. In both handlers: compute the selected receipt from the unmodified genuine user text, append the arm receipt, record that original text, then conditionally append the fixed advisory immediately before forwarding. Preserve existing constraint/cache order.
5. Test with captured mock OpenAI and Anthropic payloads: default bytes unchanged; inject arm contains exactly the fixed template and score digits; user/system content is otherwise byte-preserved; unsupported content is no-op; turn two has no advisory; cached system prefix remains unchanged; and serialized transcript/store/response never contains the advisory.

**Synthetic boundary:** all of the above plumbing and placement invariants are synthetic-testable. Whether the template helps is not.

### 4. Session-randomized Stage 2 benchmark/ship decision — **BLOCKED / empirical**

**Files:** `src/benchmark.rs`, `src/bin/bbg.rs`, `docs/score-lifecycle.md`

1. Add an arm-aware report that joins the content-free transcript arm receipt, manual labels, and priced cost records; reject sessions lacking a single arm, label, or valid cost as appropriate and report exclusions.
2. Pre-register randomization unit (session), eligibility, sample size/stopping rule, correctness non-inferiority margin, and the required cost-per-completed-task improvement. Stratify/report provider/model and task cohort if they differ across arms.
3. Report correctness first; only a no-regression result plus lower billed dollars per completed task can ship. Clarification/total turns and terminal/thrash measures remain diagnostics.
4. Run against real provider sessions, preserve the immutable report and labels, and mark TODO complete only from that reviewed result. If Stage 1 does not select a variant, stop rather than randomizing injection.

**Risk:** synthetic arm balance cannot demonstrate independent randomization, provider response quality, billed dollars, or no correctness regression.

### 5. Thrash score — **HIGH instrumentation gap; partly implementable**

**Files:** `src/signals.rs`, `src/proxy.rs`, `src/benchmark.rs`, `src/operations.rs`

1. Define a versioned, content-free `ThrashReceipt` made of counters/boolean pattern codes only: exact prior tool-result digest repeat, near-repeat tool-call shape, edit-fail-edit, and a per-session total. Keep digests and normalized calls in memory only; persist neither.
2. Introduce a trusted local action-event adapter/sidecar for `tool_result`, `tool_call`, `edit`, and `failure` categories. Do **not** infer semantic edit/failure actions from arbitrary provider text or persist provider tool names/arguments. Existing local attestations can support exact tool-result digest comparison but do not provide call/edit/failure semantics.
3. Add a benchmark trajectory report that joins per-session thrash counters to observed cost and labels. Classify `expensive` from observed billed cost and `wire_cheap_dedup_active` only from an explicit local compression-applied receipt/field; leave either side unknown when absent.
4. Unit-test a state machine with synthetic trusted events: exact duplicate versus same-size/different digest, near-call threshold boundary, valid and interrupted edit-fail-edit, session isolation, and no persisted raw data. Add report tests for unknown/unpriced/dedup-absent cases.

**Risk:** current transcripts omit tool messages/calls, and `CostRecord.compressor`/`estimated_savings_usd` are never populated by the proxy. Therefore the requested edit-fail-edit and wire-cheap separation cannot be truthfully delivered from the reviewed ledgers alone.

### 6. Completeness delta after clarification — **HIGH ordering/correlation gap; plumbing implementable**

**Files:** `src/signals.rs`, `src/proxy.rs`, `src/transcript.rs`, `src/operations.rs`, `src/benchmark.rs`

1. Define a content-free `CompletenessDeltaReceipt` for a `?` assistant terminal followed by the next genuine user turn in the same session: prior/readiness variant, `Missing|Unresolved → Present` slot codes, count, and no prompt text. Treat `NotApplicable` as neither a fill nor a loss unless a separately specified rule changes it.
2. Correlate only an adjacent decision-to-next-user pair; do not equate every later user turn with clarification. Capture readiness for every genuine user turn (already present), but have the report use chronological/session sequence rather than only `first_scored`.
3. Add a stable request/turn ordinal (or opaque locally generated request id) to transcript and cost records, assigned before upstream dispatch. This is needed to calculate **later** cost after a clarification under concurrent/out-of-order responses; append order and second-resolution timestamps are insufficient.
4. Add a completeness report that aggregates per-question delta, subsequent priced cost, and final optional labels; expose unknown correlations rather than substituting terminal state.
5. Synthetic tests: decision + answer fills selected slots; non-decision/ tool-only turn does not pair; no fill; multiple decisions; missing cost/label; reordered completion with ordinals; redaction/no-source-text serialization.

**Risk:** the existing readiness report intentionally keeps only the first scored turn, while `CostRecord` has no timestamp, ordinal, or transcript correlation key. It cannot currently prove “later cost” per clarification.

### 7. Per-model substitution-miss and zero-sigil health tables — **implementable with synthetic fixtures**

**Files:** `src/operations.rs`, `src/proxy.rs`, `src/bin/bbg.rs`, `tests/proxy_integration.rs`, `tests/cli_integration.rs`

1. Define a schema-versioned local health ledger event keyed by provider/model/session with counts—not source text—for: attempted assistant-history substitutions, lookup misses, observed text responses, and zero-recognized-sigil text responses. Preserve unknown/not-observed states and support old ledgers.
2. Instrument `substitute_openai_originals`/`substitute_anthropic_originals` to return counters while retaining their fail-closed rewrite behavior. Instrument raw assistant response/stream completion for zero-sigil only when text was actually observed; tool-only/non-text responses must not enter that denominator.
3. In `operations.rs`, group events by provider/model in deterministic `BTreeMap` order and expose counts/rates with zero denominators as unavailable. In `bbg stats`, print a stable table plus a note that these are protocol-format/lookup health signals, not correctness outcomes.
4. Test mocked responses/history for hit, miss, no candidate, zero/nonzero sigil, streaming/nonstreaming, provider/model separation, unknown old records, and stable CLI output.

**Risk:** no such health counters currently exist. Counting every arbitrary assistant message as a “substitution miss” would create a misleading compliance metric; the report must name it a lookup miss and retain a separate attempt denominator.

## Cross-cutting evidence and concerns

- `docs/score-lifecycle.md` correctly requires S0 observation before S3 and prohibits terminal/turn proxies for correctness. Preserve that boundary in all new reports.
- `src/signals.rs` receipts currently store only finite enums/numbers and dual readiness variants; this is a suitable pattern. Do not add raw digests, normalized causes, tool names, identifiers, or source-derived text to receipts.
- `src/benchmark.rs` costs are joined by session and labels remain optional; retain explicit exclusion/unknown counts. Stage analysis needs a new report rather than overclaiming from current buckets.
- `src/proxy.rs` already records raw assistant text and redacted user text. Advisory recording must occur before mutation and must never put the fixed advisory into stored user content.
- `src/operations.rs` has good unknown cache semantics, but cost records lack event ordering and compression provenance needed by the additional score TODOs.

## Assumptions and decision criteria

Assumptions: manual labels are independent of model terminal wording; local configuration is owner-controlled; a trusted local action adapter can exist for tool/edit semantics; and the selected readiness score remains an observation rather than a calibrated probability.

Proceed with the synthetic-testable implementation only after the S3 contract and event schemas are reviewed. Proceed to Stage 2 only if held-out Stage 1 selects a variant. Ship S3 only if the preregistered real-provider benchmark has no correctness regression and lower billed dollars per completed task; otherwise retain S0 receipts and stop.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete file-level plans and severities are provided for all seven unchecked TODO items."
    }
  ],
  "changedFiles": [
    "artifacts/todo-design-review.design-review.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short; cat TODO.md; cat docs/score-lifecycle.md",
      "result": "passed",
      "summary": "Reviewed current TODO/lifecycle and observed pre-existing dirty worktree."
    },
    {
      "command": "read src/signals.rs src/benchmark.rs src/proxy.rs src/operations.rs and relevant direct call sites",
      "result": "passed",
      "summary": "Reviewed receipt, aggregation, proxy, cost, CLI, transcript, and session feasibility."
    },
    {
      "command": "git diff --check",
      "result": "passed",
      "summary": "No whitespace errors reported in the inspected pre-existing diff."
    }
  ],
  "validationOutput": [
    "No tests run: this was a read-only design review with no implementation changes."
  ],
  "residualRisks": [
    "Stage 1 and Stage 2 decisions require real labelled provider data; synthetic fixtures cannot satisfy them.",
    "Thrash semantics and later-cost completeness correlation need new trusted instrumentation/schema fields.",
    "The working tree was already dirty, so review evidence is tied to inspected revision ee0bde2 plus uncommitted files."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added only the required design-review artifact; no source or test files were changed by this review.",
  "reviewFindings": [
    "blocker: TODO.md Stage 1/Stage 2 empirical items cannot be completed without held-out manual labels and real provider usage/billing.",
    "high: src/operations.rs CostRecord lacks request/turn ordering and compression provenance required for later-cost completeness and wire-cheap thrash claims.",
    "high: src/proxy.rs/transcript.rs do not capture trusted tool-call/edit/failure categories, so full thrash detection is not derivable from current ledgers.",
    "high: src/operations.rs and src/session.rs have no S3 advisory configuration or session arm state; contract precedes implementation."
  ],
  "manualNotes": "Synthetic fixtures are appropriate for all specified plumbing and placement tests; empirical provider outcomes remain explicitly separated above."
}
```