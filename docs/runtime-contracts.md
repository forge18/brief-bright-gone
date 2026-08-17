# Runtime Contracts

This document freezes the contracts that precede proxy, store, compressor, and
installer implementation. Any uncertainty follows the relevant fail-safe path.

## Compressor eligibility and I4

A payload is **eligible** only when all of these are true:

1. Its source metadata identifies it as a tool result, with a stable source
   kind and capture time.
2. The classifier identifies exactly one supported transform: lossless TOON for
   JSON/tabular data, repeated-log collapse for logs, or cross-turn dedup for a
   previously stored identical file read.
3. It contains no protected bytes: a file body in the recent window, diff,
   command string, error text, stack trace, fence, or inline-verbatim span.
4. The original bytes are durably stored before a reference is forwarded.
5. The transform is deterministic and byte-stable for the same input.

Missing, contradictory, or stale metadata makes a payload ineligible. The
proxy passes ineligible payloads through unchanged. The recent window is a
local configuration duration measured from capture time; its default is
conservatively **the active session's full lifetime**. A configured shorter
window is valid only after benchmark evidence shows it preserves I4.

## Session identity, concurrency, and GC

A session has a fingerprint, a last-seen time, history references, and liveness
pins. An incoming request matches the session with the longest matching rolling
history prefix only when that match is unique. Tied longest-prefix matches are a
**collision**: create a new isolated session, perform no cross-turn dedup, and
record the collision. Compacted histories also start a new session.

A session is active until its configurable activity window expires. Activity
windows pin every referenced blob. Served-reference recency pins are independent
of session activity. A blob is collectible only after all pins are absent;
size and TTL policies apply only then.

Session transition state machine:

`New → Active → Inactive → Collectible`

- `New → Active`: unique prefix match or first request.
- `Active → Active`: serialized update for the same session.
- `Active → Inactive`: activity window expires.
- `Inactive → Active`: a later unique prefix match.
- `Inactive → Collectible`: no session or served-reference pin remains.
- Any collision or compaction transitions the request to a new isolated
  `Active` session and disables dedup for that request.

The session registry and each session's history/pin set are shared mutable
state. Writers acquire an exclusive per-session lock before prefix selection,
store writes, history updates, or pin changes. GC uses the same registry lock
when checking and removing pins. Requests for different sessions may proceed in
parallel. A failed lock, store update, or liveness check disables transforms and
passes bytes through unchanged.

## Provider forwarding

Only OpenAI Chat Completions and Anthropic Messages are supported natively.
Adapters map both protocols to the canonical types in `src/types.rs`; no
agent-specific adapter is permitted.

- Bind to `127.0.0.1` by default. A non-loopback `BBG_BIND` is accepted only
  with `BBG_ALLOW_NON_LOOPBACK=1` and a non-empty `BBG_PROXY_TOKEN`; every route
  then requires that bearer token.
- Parse and validate the upstream URL at startup. HTTPS is required except for
  explicitly configured loopback HTTP services such as local Ollama. Reject
  userinfo credentials, queries, fragments, and non-HTTP(S) schemes, and never
  follow redirects.
- Authenticate with locally configured upstream credentials only. Never forward
  client `Authorization`, cookies, proxy credentials, or provider-specific
  secrets from inbound headers.
- Forward only the allowlisted end-to-end headers `accept`, `content-type`, and
  `user-agent`; synthesize the upstream authorization header. Drop hop-by-hop
  headers and all other inbound headers.
- Preserve upstream status, body bytes, and normalized provider usage on normal
  responses. Map connection, timeout, and body-read failures to a redacted 502
  or 504 error with a stable bbg error code.
- For SSE, forward events in arrival order and preserve the provider's terminal
  event. Never emit a synthetic successful terminal event after an upstream
  error or truncated stream.
- Do not retry a request after upstream headers or stream bytes have been
  received. Before that point, retry at most once and only a connection failure
  or idempotent 502/503/504 response; requests with a non-idempotent tool side
  effect are never retried.
- Cancellation aborts the upstream request and records a cancelled outcome; it
  does not retry.

Integration fixtures must cover both adapters' non-streaming success, SSE
success, upstream error, timeout, cancellation, usage normalization, and
redacted-error paths.

## Log and transcript redaction

Logs and transcripts are separate trust surfaces. Operational logs use an
allowlist: stable error codes, request/session identifiers, status, timings,
byte/token counts, transform names, and content digests. They never contain
request/response bodies, prompt text, local constraint text, complete upstream
URLs with query strings, cookies, or credential/header values.

Transcript records intentionally contain model-visible content for linting and
benchmarking. Before a record is serialized, bbg replaces recognized secret
material with `[REDACTED]`:

- `Authorization` and `Proxy-Authorization` values, including bearer tokens;
- `x-api-key` and equivalent `api_key`/`api-key` assignments;
- token, password, secret, cookie, and session assignments in header,
  environment, query, quoted/nested JSON-style syntax;
- PEM private-key bodies and HTTP(S) URI userinfo credentials.

Redaction happens before lint findings or any other derived record is written.
The raw secret is never retained alongside the redacted record. Redaction
preserves record and line boundaries but does not preserve secret length.
Unknown secret formats cannot be proven safe; callers must not place credentials
in prompt content. Store/transcript directories are created or tightened to
0700 and sensitive files to 0600 on Unix; symlinks and unsafe file types are
rejected where practical cross-platform. Error paths emit stable local codes
and generic messages rather than serializing upstream client errors, which may
contain URLs or headers.

## Local configuration provenance

`BBG_CONFIG`, when set, names one explicit local JSON file. The proxy loads it
from the local filesystem at startup into an immutable configuration snapshot.
Only that snapshot may supply protected constraints, provider pricing,
calibration, and cache policy. Request bodies, response bodies, headers, tool
results, transcripts, environment values other than the local file path, and
provider metadata are untrusted wire data and can never create or override
those fields.

A malformed or unreadable explicit config fails closed at startup. A missing
path means the conservative empty configuration: no protected segment, no
compression or cache movement, and no observed-billing calculation without
pricing. Each outbound request injects protected constraints from the snapshot
into the protocol system position and verifies the exact marker and bytes before
forwarding. Verification failure blocks that request. Configuration values are
never logged or copied into transcripts; only provenance (`local_file` or
`empty_default`) and non-sensitive version/digest metadata may be recorded.
Anthropic protected constraints are prepended as a dedicated text block; any
existing system string is retained byte-for-byte as the following text block,
and every existing system array element is retained exactly and in order.

## Local constraints, pricing, and runtime gates

`BBG_CONFIG`, when set, names a local JSON configuration file. It may contain
`protected_constraints`, provider-keyed `pricing`, and `calibration`; inbound
request and response bytes never supply or modify any of those values. A
non-empty protected segment is injected at the system position for each
supported protocol and its exact marker and content are verified before the
request is forwarded. Verification failure returns a local error rather than
forwarding an unprotected request.

Provider usage is normalized into a canonical cost record including input,
output, cache-read, and cache-creation tokens where the provider reports them.
Configured prices turn those reported fields into an observed-billing estimate;
transform savings remain a separate optional estimate and are never labelled as
provider billing.

A content-changing transform and an Anthropic cache breakpoint require a local
calibration whose sample count reaches `min_samples`. The transform gate also
requires predicted savings to exceed the estimated cache-break cost. Missing or
insufficient calibration therefore leaves content and existing breakpoints
unchanged. Cache placement runs after content transforms and constraint
injection so its position is stable against the final bytes.

## Installer probe registry and ownership

The probe registry is a versioned, bounded local list of directories. Each
entry contains a path, expected skill filename, and a non-mutating presence
check. It has no network discovery, no recursive home-directory search, and no
agent-specific behavior beyond locating a known skill directory.

`bbg install` writes only locations explicitly passed with `--path` or found by
the registry. Each write records a manifest entry with the absolute path, skill
filename, content digest, installed skill version, and installer version.
Rerunning install replaces only a manifest-owned file, atomically; a modified
or unowned target fails loudly and requires an explicit path decision.

`bbg uninstall` removes only manifest-owned files whose digest still matches.
`bbg upgrade` replaces a manifest-owned older version atomically and updates
its digest. A probe miss is a successful no-op with guidance to use
`bbg install --path`; it is never an implicit unsupported-agent claim.
