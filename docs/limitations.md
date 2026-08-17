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

## Optional `bbg wrap` decision

`bbg wrap` is not included in v1. The core proxy already returns native Markdown
for agent rendering, and the endpoint-override model avoids PTY interception.
A cosmetic stdout renderer would add a terminal/formatting dependency and
platform-specific failure surface without improving the core correctness or
safety gates. Reconsider it only when a demonstrated raw-stdout workflow has a
measured display problem that cannot be solved by the consuming terminal.
