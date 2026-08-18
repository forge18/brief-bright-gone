# brief-bright-gone (bbg)

## Be Brief. Be Bright. Be Gone.

Code is only part of the job. Good engineers communicate well. Your agent should do the same.

bbg helps coding agents communicate like developers. It turns verbose responses into clear explanations, useful decisions, and practical next steps without losing the details that matter.

bbg reduces the cost of getting work done and makes conversations easier to scan. It removes wasted turns and wasted words without stripping away what developers need to act.

## Features

- **Developer-to-developer communication** — grounded in the Pyramid Pattern, PREP, SBI, HRT, and the Four-Sides model for explanations, reviews, pairing, and decisions.
- **Sigil encoding** — a compact syntax for headings, bullets, decisions, blockers, and results that gives agent responses structure without Markdown overhead.
- **Local proxy** — runs between the agent and model provider. It applies bbg’s rules, renders sigils as normal Markdown, records health, and keeps recovery copies for eligible transforms.
- **Safety and recovery** — keeps code, commands, paths, versions, errors, and line numbers exact; eligible lossy transforms store recoverable local originals first.
- **Linting** — checks output against bbg’s communication rules, including answer-first structure, terminal state, severity labels, and missed bullet structure.
- **Health telemetry** — `bbg stats` reports observed cost, cache, protocol, and versioned format health.

## See it

### Understand code

**You**
> What does `reconcileInvoice` do?

**Agent**
> It applies incoming payments to an invoice and recalculates the remaining balance.
>
> - It rejects payments for a closed invoice.
> - It marks the invoice paid when the balance reaches zero.

### Debug

**You**
> Tests pass locally but fail in CI. Where should we start?

**Agent**
> Start with the environment difference.
>
> - Compare the CI runtime, dependency lockfile, and required environment variables.
> - Run the failing test with CI's command and a clean checkout.
>
> That separates an environment failure from a test-order dependency.

### Plan

**You**
> Add CSV export to the report page.

**Agent**
> **Plan:**
>
> 1. Define the exported fields and authorization rule.
> 2. Add a streaming export endpoint and download action.
> 3. Cover empty, large, and unauthorized reports.

### Review

**You**
> Review this pull request.

**Agent**
> **Blocking:** the delete endpoint accepts a project ID without checking membership.
>
> - A signed-in user can target another project's record.
> - Check membership before loading the record.

### Status

**You**
> What changed in this branch?

**Agent**
> **Done:** report filters now persist in the URL.
>
> - Added round-trip parsing and browser-history support.
> - Covered invalid filters with a regression test.
> - No migration or API change.

## Quick start

```bash
curl -fsSL https://raw.githubusercontent.com/forge18/brief-bright-gone/main/install.sh | bash

# Route an OpenAI-compatible agent through bbg.
BBG_UPSTREAM_URL=http://localhost:11434/v1 BBG_PORT=8088 bbg-proxy
```

Point the agent's base URL at `http://localhost:8088/v1`. See [Proxy mode](#proxy-mode-drop-in-for-any-agent) for configuration and security details.

> **Requirement:** Your agent must support a configurable OpenAI- or Anthropic-compatible provider base URL. bbg does not intercept terminal or process traffic.

## How it works

```
payload → detect (content type) → safety gate (S0–S4) → normalize / compress
```

- **`detect`** classifies JSON, code, logs, diffs, search results, text, tables, and terminal output. Action-sensitive types are never lossily transformed.
- **`safety`** gives every transform a safety class (S0–S4). Lossy transforms require recovery and fail closed without it.
- **`normalize`** applies bounded prose cleanup. It never touches code or shell; the proxy persists the original before a lossy transform.

The communication stance is a collaborative engineering peer: calm, candid, respectful, and precise. It leads with facts and an explicit ask; separates observation from inference; critiques work rather than people; explains reasoning with examples when teaching; states impact, options, and uncertainty; and preserves the user's agency. It draws on HRT and egoless programming, decision-first/Pyramid writing, PREP and SBI teaching/feedback, and the Four-Sides distinction between fact, relationship, self-disclosure, and appeal. See [`docs/communication-rules.md`](docs/communication-rules.md).

## Evidence and limits

`bbg stats` reports provider-observed usage, cache health, and format health. It distinguishes observed values from estimates and does not rewrite action-bearing content to chase a metric.

Some capabilities require integration support. Standalone proxy mode forwards tool results unchanged; tool-result compression requires locally attested metadata.

## Status

Core engine, CLI, proxy, output-style shaping, CCR recovery, and release hardening are implemented. See [`docs/limitations.md`](docs/limitations.md) for support boundaries.

Tool-result compression (TOON, log collapse, cross-turn file dedup) is **library-only**: it activates only through locally-attested tool-result metadata supplied via `build_router_with_tool_result_attestations`. The standalone `bbg-proxy` binary never compresses a tool result — see the limitations doc for why and the path to relaxing it.

## Learn more

- [Design](docs/design.md)
- [Communication rules](docs/communication-rules.md)
- [Runtime limitations](docs/limitations.md)

## Building

```bash
cargo build
cargo test
```

## License

Apache-2.0

## Other installation options

The installer downloads prebuilt `bbg` and `bbg-proxy` binaries from the latest release, verifies the archive checksum, and requires curl + tar (no Rust toolchain). On Windows, run [`install.ps1`](install.ps1) from PowerShell; Git Bash users can use `install.sh`.

Or build from source:

```bash
cargo install --path .
```

## Usage

`bbg` is pipe-friendly and agent-agnostic — any tool or agent that can shell out can use it:

```bash
echo 'please   fix the  bug, thank you' | bbg normalize   # fix the bug
cat file.txt | bbg detect                                  # text | code | json | ...
bbg stats                                                  # observed billing from the cost ledger
```

## Proxy mode (drop-in for any agent)

`bbg-proxy` is an OpenAI-compatible passthrough server: any agent that can point
its base URL at a local server can use it. Prose user messages are normalized
in-flight before forwarding; assistant responses are sigil-decoded to Markdown
on the way back (streaming and non-streaming alike).

```bash
# point at any OpenAI-compatible upstream (ollama shown here)
BBG_UPSTREAM_URL=http://localhost:11434/v1 BBG_PORT=8088 bbg-proxy

# then configure your agent's base URL to http://localhost:8088/v1
```

If your agent honors the standard base-URL environment variables
(`ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, or `BASE_URL`) but does not expose a
settings field for them, `bbg run` sets them for you and launches the agent
against the already-running proxy:

```bash
bbg-proxy &                      # start the proxy (loopback default)
bbg run -- your-agent --flags     # runs your-agent with its base URL pointed at bbg
```

`bbg run` only fills a base-URL variable you have not already set, so an
explicit value you exported still wins. It targets the local loopback proxy; a
non-loopback proxy is configured manually.

| Env | Default | Purpose |
| --- | --- | --- |
| `BBG_UPSTREAM_URL` | `OPENAI_BASE_URL` or `http://localhost:11434/v1` | real provider |
| `BBG_UPSTREAM_KEY` | `OPENAI_API_KEY` | provider key |
| `BBG_PORT` | `8088` | listen port (avoids Headroom's 8787/8788) |
| `BBG_BIND` | `127.0.0.1` | listen IP; loopback is the safe default |
| `BBG_ALLOW_NON_LOOPBACK` | off | must be `1` to opt into a non-loopback bind |
| `BBG_PROXY_TOKEN` | unset | required bearer token for non-loopback mode; when set, protects every route |
| `BBG_DRY` | off | `1` = normalize + drop (no forward), for testing |
| `BBG_CONFIG` | unset | local JSON protected constraints, provider pricing, and calibration |
| `BBG_STORE_DIR` | `~/.bbg-store` | local CCR store, cost ledger, and skill manifest — home-anchored so every `bbg`/`bbg-proxy` invocation shares one store regardless of launch directory |
| `BBG_TRANSCRIPT` | on | `0` or `off` disables transcript capture; otherwise a size-capped, auto-rotated ledger under the store dir |

`BBG_CONFIG` is local-only: protected constraints are injected and verified on
every supported outbound request. Provider-reported usage is recorded under the
store ledger; `bbg stats` reports configured-price observed billing separately
from transform-savings estimates. Until calibration reaches its configured
sample threshold, bbg leaves content transforms and Anthropic cache breakpoints
unchanged.

### Pi / OpenAI Codex Responses

bbg also accepts Pi's `openai-codex` protocol at
`/v1/codex/responses`. Configure the upstream as the Codex backend and override
Pi's existing provider base URL with `http://127.0.0.1:8088/v1`; Pi then sends
requests to bbg's `/v1/codex/responses`, which forwards them to
`https://chatgpt.com/backend-api/codex/responses`. Start bbg with that upstream;
leave `BBG_UPSTREAM_KEY` unset because Pi supplies its managed OAuth bearer.

```bash
BBG_UPSTREAM_URL=https://chatgpt.com/backend-api \
BBG_PROXY_TOKEN="$BBG_PROXY_TOKEN" \
bbg-proxy
```

```ts
// Pi extension: preserve Pi-managed OAuth; do not put its bearer in env/config.
pi.registerProvider("openai-codex", {
  baseUrl: "http://127.0.0.1:8088/v1",
  headers: { "x-bbg-proxy-token": "$BBG_PROXY_TOKEN" }, // only if token protection is enabled
});
```

For this route, `Authorization`, `ChatGPT-Account-Id`, `OpenAI-Beta`, and
`session-id` are forwarded upstream. If `BBG_PROXY_TOKEN` is set, Pi must use
`x-bbg-proxy-token`; its `Authorization` header remains Pi's upstream OAuth
bearer. Codex Responses request bodies may use `Content-Encoding: zstd` (1 MiB
compressed / 8 MiB decompressed limits). Responses SSE is currently forwarded
unchanged, so sigil decoding and per-stream usage accounting remain unavailable
on this protocol.

The proxy rejects upstream credentials embedded in URLs, URL queries/fragments,
non-HTTP schemes, and plain HTTP to non-loopback hosts. Redirect following is
disabled so an approved upstream cannot redirect requests or credentials. The
default local Ollama URL remains supported. Non-loopback listening is a
deliberate authenticated mode: set all three of `BBG_BIND`,
`BBG_ALLOW_NON_LOOPBACK=1`, and a strong `BBG_PROXY_TOKEN`, then send that token
as `Authorization: Bearer …` to the proxy. Codex clients instead send Pi's
OAuth `Authorization` upstream and use `x-bbg-proxy-token` for bbg itself.

Store and transcript directories are owner-only on Unix (0700), and sensitive
files are owner read/write only (0600). Symlinks and non-regular sensitive files
are rejected where the platform exposes that distinction. Transcript redaction
is defense in depth, not permission to include credentials in prompts. See
[`docs/limitations.md`](docs/limitations.md) for endpoint, host-trust, sandbox,
compaction, and history-divergence limitations.

> Note: port 8088 deliberately avoids Headroom's default ports (8787/8788).
