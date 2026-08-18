# S3 advisory injection contract

This contract is the only route from passive readiness receipts to a
model-visible advisory. It applies only after a reviewed Stage-1 report names a
selected variant. Until then, bbg remains record-only.

## Local configuration

`BBG_CONFIG` is owner-controlled. The optional `advisory` object is ignored
unless all gates below pass:

```json
{
  "advisory": {
    "enabled": false,
    "experiment": "record_only",
    "selected_variant": "raw_composite",
    "template_version": 1,
    "stage1_evidence_id": "reviewed-report-id"
  }
}
```

`enabled` defaults to `false`. `experiment` is `record_only` or `randomized`;
unknown values, missing evidence identifiers, unsupported template versions,
or missing selected variants fail closed to `record_only`. Wire bytes, headers,
transcripts, provider metadata, and environment values other than the local
config path cannot alter this configuration.

## Fixed advisory

The only v1 template is exactly:

```text
[bbg-readiness-v1 score=<0..100>]
```

Its sole variable is an integer score from the selected passive receipt. The
block contains no user wording, paths, tool data, terminal details, identifiers,
model output, or other wire-derived substring. It is request-only: it is never
stored in a transcript or CCR, echoed to a response, linted, or response-
verified.

## Session arm and placement

1. Assign `record_only` or `inject` once when a new local session is selected.
   Retain it in local session state for that session's lifetime. A collision
   creates an isolated new session and receives a new assignment.
2. Record a content-free arm/template/variant receipt. A receipt records neither
   the advisory nor user content.
3. Only a genuine provider conversation start is eligible: the request has
   exactly one user message and no assistant messages. A new local session after
   compaction, eviction, or restart is not sufficient. Score and record the
   original text before any mutation. Tool-only/non-text turns are ineligible.
4. For an `inject` arm with all gates satisfied, append the fixed block after
   that user text immediately before forwarding. Never add it to a system or
   assistant message, and never replay it on later turns. Record `injected`
   only when this mutation succeeds.
5. Constraints remain in the system position. Anthropic cache placement remains
   on the stable system prefix before the user-tail advisory; OpenAI appends only
   to the final genuine user content.

The injected block is request-only, so the agent's next history omits it. That
can invalidate the treated first turn's cached user-prefix on the next request;
the dollars-based experiment gate includes this cost rather than treating it as
an advisory-free baseline effect.

Default and ineligible cases preserve provider request bytes. Injection is
advisory-only and does not alter cache-breakpoint ordering.

## Evaluation gate

A randomized experiment is only eligible after Stage 1 selects a variant from
held-out labelled outcomes. Randomize per session. Ship only when a reviewed
paired report finds no correctness regression and lower billed dollars per
completed task; clarification and total turns are diagnostic only. Otherwise
retain record-only operation and stop.
