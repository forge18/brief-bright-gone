# Runtime limitations

These limitations are deliberate parts of the v1 support boundary. They are not
claims that bbg can transparently integrate with every agent or protect every
local execution environment.

## Endpoint override is required

The agent must support an OpenAI-compatible or Anthropic provider endpoint
override (`BASE_URL`, `ANTHROPIC_BASE_URL`, or an equivalent setting). bbg is a
proxy and does not intercept an agent's PTY, TUI, SDK, or process-local network
stack. An agent that cannot redirect its provider endpoint cannot use bbg.
There is no compatibility matrix or agent-specific interception fallback.

## Same-machine trust boundary

The default proxy bind is loopback. In that mode, bbg trusts processes that can
connect from the same machine, including any local process able to read the
proxy token or observe the configured environment. Non-loopback binding is an
explicit authenticated mode, not an anonymous service mode. Do not expose the
proxy to an untrusted network, and do not treat the proxy token as a substitute
for host or network isolation.

The upstream provider credential and protected constraints remain local runtime
secrets. A compromised agent process, user account, debugger, or host can still
read or misuse them. bbg does not provide a hardware-backed secret boundary.

## Permissions are defense in depth

On Unix, bbg creates its store and transcript directories with mode `0700` and
sensitive files with mode `0600`. Symlinks and unsafe file types are rejected
where the platform exposes that distinction; descriptor opens use no-follow
flags on supported Unix systems. These controls reduce accidental disclosure,
but they do not replace correct account, filesystem, backup, or operating-system
security. Windows and other non-Unix platforms do not receive identical Unix
mode semantics.

Transcript redaction is also defense in depth. It recognizes documented secret
forms before serialization, but unknown encodings, application-specific secret
formats, process memory, crash dumps, backups, and already-exported copies are
outside its guarantee. Treat transcripts as sensitive data.

## Sandbox and host isolation

bbg does not create or enforce a sandbox. It does not restrict filesystem
access, child processes, outbound network access, environment inspection, or
kernel capabilities of the agent, provider, or local user. Use an OS/container
sandbox, least-privilege account, firewall, and separate credential scope when
those controls are required.

The proxy validates configured upstream URLs and disables redirects, but it is
not a complete network policy engine. DNS, routing, local services, and the
host's resolver remain part of the deployment trust boundary.

## Tool-result compression is integration-API-only in v1

TOON re-encoding, log collapse, and cross-turn file dedup fire only for tool
results carried by a local attestation `(digest, locator, metadata)`.
Attestations are constructed by an integrating application through
`build_router_with_tool_result_attestations`; they are deliberately
un-forgeable from provider or client bytes, so a digest must be computed from
tool output the owner already holds.

The standalone `bbg-proxy` binary constructs no attestations and exposes no
configuration surface for them, because an operator cannot pre-compute the
digest of tool output that a future turn has not produced yet. A config list
would therefore ship unpopulatable. In v1 these three compressors are reachable
only by embedding the library and supplying attestations programmatically; the
CLI proxy forwards tool results unchanged. This is a scope reduction from the
original plan and is reflected in the README status and `design.md`.

A future *rule-based* attestation — attesting message classes (e.g. OpenAI
`role:tool`) rather than exact bytes — could reach the CLI, at the cost of
config-level defaults for the `captured_at` / `in_recent_window` inputs that
`classify`'s staleness and I4 guards otherwise take from the attestation. That
weakening is why per-digest attestation was chosen for v1. See `design.md`.

## Compaction starts a new session

Session matching depends on a unique longest prefix of the known message
history. After agent-side context compaction, that prefix may be unavailable or
ambiguous. bbg therefore starts an isolated session and disables cross-turn
file dedup for that request rather than guessing. This preserves correctness at
the cost of compression opportunities and additional storage.

A collision has the same conservative behavior: it creates a new isolated
session and records no cross-turn dedup for the ambiguous request.

## Local history and wire history can diverge

The agent sees decoded Markdown while the provider wire history can contain
compact sigils. bbg restores stored sigil originals on the request path when a
normalized hash matches. A hash miss sends the agent-visible Markdown upstream
instead; this is a safe token-cost fallback, not a correctness failure.

Agents or tools that compute semantics from their local history and assume it
is byte-identical to provider wire history can therefore observe a divergence.
This is an accepted v1 limitation. The normalization and hash-miss fallback
bound the impact, but they do not make the two histories identical in all
cases.

## Provider cache behavior is asymmetric

Anthropic exposes explicit `cache_control` breakpoints; bbg may add one to the
stable system prefix only after local calibration and only when fewer than the
provider's four slots are already reserved. Agent-provided cache controls are
preserved verbatim and count as reserved slots. bbg does not rewrite, reorder,
or otherwise canonicalize agent-selected prompt content to chase cache hits; it
reports observed cache health instead.

OpenAI caching is automatic and has no explicit breakpoint placement API in the
proxy. For OpenAI, bbg's cache behavior is limited to preserving the emitted
request prefix and reporting provider-observed cache usage in `bbg stats`.

The proxy parses JSON and serializes a new JSON request. With the default
`serde_json` map representation, object member order is deterministic but may
differ from the agent's original wire bytes. This is not a content rewrite and
is not a request-byte preservation guarantee; cache stability refers to the
provider request emitted by bbg, not original JSON member ordering.

## Sigil original recovery is first-writer-wins on key collision

Sigil originals are keyed by the normalized decoded Markdown. Each key stores
one original, and the first writer wins so concurrent requests cannot silently
overwrite an existing reversible mapping. If two distinct sigil responses ever
decode to the same normalized Markdown, only the first stored original is
recoverable; the second is not stored, and a later request-path lookup restores
the first response's bytes.

This is an accepted v1 trade-off chosen for concurrency safety. Recovery is
exact for the common one-mapping-per-key case, and the alternative (unchecked
overwrite) would corrupt recovery for the first response. Request normalization
uses a different, stricter policy: a normalization-key collision fails closed
and forwards the original bytes instead of choosing an ambiguous recovery
mapping.

## Optional `bbg wrap` decision

`bbg wrap` is not included in v1. The core proxy already returns native Markdown
for agent rendering, and the endpoint-override model avoids PTY interception.
A cosmetic stdout renderer would add a terminal/formatting dependency and
platform-specific failure surface without improving the core correctness or
safety gates. Reconsider it only when a demonstrated raw-stdout workflow has a
measured display problem that cannot be solved by the consuming terminal.
