# brief-bright-gone (bbg)

A communication-efficiency system for coding agents: detect → safety-classify → normalize.

`bbg` makes agent conversations cheaper and clearer by compressing *what the agent reads* (input) and shaping *how it writes* (output), while keeping code, commands, errors, and other action-bearing content byte-exact.

## Design

```
payload → detect (content type) → safety gate (S0–S4) → normalize / compress
```

- **`detect`** — classifies a payload into one of 8 content types (json, code, log, diff, search-result, text, tabular, terminal). Action-sensitive types are flagged so they're never lossily transformed.
- **`safety`** — every transform declares a safety class (S0–S4). Lossy classes are reversible-only and fail closed when recovery (CCR) isn't provisioned.
- **`normalize`** — byte-safe prose cleanups: whitespace collapse, trailing-punctuation trim, polite-filler strip, profanity placeholder. Never touches code or shell.

## Status

Core engine, CLI, proxy, output-style shaping, CCR recovery, and release
hardening are implemented. See [`docs/limitations.md`](docs/limitations.md)
for the v1 support boundary.

## Building

```bash
cargo build
cargo test
```

## License

Apache-2.0

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/forge18/brief-bright-gone/main/install.sh | bash
```

Downloads the prebuilt binary for your platform from the latest release. Requires curl + tar (no Rust toolchain).

Or build from source:

```bash
cargo install brief-bright-gone
```

## Usage

`bbg` is pipe-friendly and agent-agnostic — any tool or agent that can shell out can use it:

```bash
echo 'please   fix the  bug, thank you' | bbg normalize   # fix the bug
cat file.txt | bbg detect                                  # text | code | json | ...
cat tool-output.txt | bbg stats                            # bytes before -> after
```

## Proxy mode (drop-in for any agent)

`bbg-proxy` is an OpenAI-compatible passthrough server: any agent that can point
its base URL at a local server can use it. Prose user messages are normalized
in-flight before forwarding; responses stream back unchanged.

```bash
# point at any OpenAI-compatible upstream (ollama shown here)
BBG_UPSTREAM_URL=http://localhost:11434/v1 BBG_PORT=8088 bbg-proxy

# then configure your agent's base URL to http://localhost:8088/v1
```

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
| `BBG_STORE_DIR` | `.bbg-store` | local CCR store and cost ledger |

`BBG_CONFIG` is local-only: protected constraints are injected and verified on
every supported outbound request. Provider-reported usage is recorded under the
store ledger; `bbg stats` reports configured-price observed billing separately
from transform-savings estimates. Until calibration reaches its configured
sample threshold, bbg leaves content transforms and Anthropic cache breakpoints
unchanged.

The proxy rejects upstream credentials embedded in URLs, URL queries/fragments,
non-HTTP schemes, and plain HTTP to non-loopback hosts. Redirect following is
disabled so an approved upstream cannot redirect requests or credentials. The
default local Ollama URL remains supported. Non-loopback listening is a
deliberate authenticated mode: set all three of `BBG_BIND`,
`BBG_ALLOW_NON_LOOPBACK=1`, and a strong `BBG_PROXY_TOKEN`, then send that token
as `Authorization: Bearer …` to the proxy.

Store and transcript directories are owner-only on Unix (0700), and sensitive
files are owner read/write only (0600). Symlinks and non-regular sensitive files
are rejected where the platform exposes that distinction. Transcript redaction
is defense in depth, not permission to include credentials in prompts. See
[`docs/limitations.md`](docs/limitations.md) for endpoint, host-trust, sandbox,
compaction, and history-divergence limitations.

> Note: port 8088 deliberately avoids Headroom's default ports (8787/8788).
