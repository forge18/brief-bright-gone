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

Core engine implemented (Rust, no unsafe). CLI, proxy, output-style shaping, and CCR recovery are next.

## Building

```bash
cargo build
cargo test
```

## License

MIT
