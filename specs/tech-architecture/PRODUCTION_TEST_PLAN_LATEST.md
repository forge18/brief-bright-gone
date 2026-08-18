# Production Test Plan

## Goal

Move bbg from mostly module-unit coverage to a release-confidence suite that
exercises real process, HTTP, filesystem, and protocol boundaries without a
real provider credential. Unit tests remain the fastest layer; integration and
end-to-end tests prove the contracts that unit tests cannot.

## Risk Matrix

| Scenario ID | Behavior | Risk | Level | Target |
|---|---|---:|---|---|
| SC-PROXY-P0-01 | OpenAI request forwarding preserves supported payload fields, injects constraints, drops client credentials, and uses only the local upstream credential. | P0 | HTTP integration | proxy harness + mock upstream |
| SC-PROXY-P0-02 | Anthropic forwarding preserves string/array system content, injects constraints, and cache placement remains after final content transforms. | P0 | HTTP integration | proxy harness + mock upstream |
| SC-PROXY-P0-03 | Proxy denies non-loopback or unauthenticated access and rejects unsafe upstream URL/redirect behavior. | P0 | HTTP integration | proxy harness |
| SC-PROXY-P0-04 | SSE success, upstream error, body failure, timeout, cancellation, and truncated terminal streams preserve the contract and do not synthesize success. | P0 | HTTP integration | mock upstream streaming fixtures |
| SC-RECOVERY-P0-05 | CCR references round-trip byte-exactly; failed stdout delivery does not mark a reference served; GC preserves all pinned blobs. | P0 | CLI + filesystem integration | isolated temporary store |
| SC-FS-P0-06 | Config, transcript, ledger, receipt, blob, and sigil paths reject final-component symlinks and tighten modes on Unix. | P0 | filesystem integration | temporary private directory |
| SC-REDACTION-P0-07 | Secret material never reaches serialized transcript/operational error records for structured, multiline, nested, and malformed input. | P0 | property + regression | corpus and generated strings |
| SC-CLI-P1-08 | `skill`, install, upgrade, uninstall, doctor, get, stats, lint, and benchmark report have stable exit codes, stdout, stderr, and manifest behavior. | P1 | black-box CLI integration | spawned binary + temporary environment |
| SC-INSTALL-P1-09 | Installer accepts only a matching checksum and one expected archive entry; it rejects missing/mismatched checksums and traversal/multi-entry archives. | P1 | shell integration | local fixture HTTP server or mocked curl/tar |
| SC-SIGIL-P1-10 | Decoder never panics; streaming/chunk boundaries produce deterministic results; malformed forms fail open. | P1 | fuzz/property + corpus | `cargo-fuzz` or proptest-style harness |
| SC-COST-P1-11 | Usage normalization and observed billing stay separate from estimates across OpenAI/Anthropic fixtures and absent pricing. | P1 | integration | golden provider responses |
| SC-PORTABILITY-P1-12 | Linux, macOS, and Windows run the supported Rust test subset; Unix-only permission assertions are conditionally isolated. | P1 | CI matrix | GitHub Actions |
| SC-PERF-P2-13 | Decoder, transcript redaction, and store operations meet documented local latency/throughput budgets on fixed non-secret fixtures. | P2 | benchmark | ignored/manual performance suite |

## Test Architecture

### Layers

1. **Unit (`src/**`)**: deterministic parsing, classification, gates, adapters,
   and data shape checks. Keep these hermetic and fast.
2. **Integration (`tests/`)**: start an in-process mock upstream and exercise a
   bound proxy listener with real `reqwest` requests. Use disposable temporary
   directories and an injected configuration snapshot.
3. **CLI end-to-end (`tests/cli_*.rs`)**: spawn `bbg` through
   `CARGO_BIN_EXE_bbg`, set only explicit environment variables, and assert
   exit status plus byte-exact stdout/stderr.
4. **Shell installer integration**: run `install.sh` against local fixture
   artifacts/checksums. Never contact GitHub during tests.
5. **Generated testing**: fuzz or property-test sigil decoding and redaction.
   Seed every regression from a found counterexample into a checked-in corpus.

### Required seams

- Extract proxy construction/startup from `src/bin/bbg-proxy.rs` into a library
  module so integration tests can bind a loopback ephemeral port without
  relying on process-global environment variables.
- Provide a test-only configuration/clock/store-root injection boundary.
- Use a small mock upstream that records received requests and can emit exact
  JSON, SSE chunks, malformed chunks, status errors, redirects, and delayed
  responses.
- Use a single temporary-directory helper that removes test roots on drop.

## Fixtures

- `tests/fixtures/openai/`: non-streaming success, usage variants, error,
  redirect, timeout, and SSE chunk sequences.
- `tests/fixtures/anthropic/`: string/array system inputs, usage variants,
  cache-control behavior, error, and SSE chunk sequences.
- `tests/fixtures/redaction/`: JSONL input/output golden pairs and hostile
  malformed/quoted/multiline secret examples. Fixtures must contain synthetic
  tokens only.
- `tests/fixtures/install/`: valid single-binary archive and checksum plus
  checksum mismatch, missing checksum, traversal, and multi-entry archives.
- `tests/fixtures/cli/`: manifest, ledger, transcript, and recovery fixtures.

## CI Gates

| Tier | Command | When | Budget |
|---|---|---|---|
| Fast | `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` | every PR, all OSes | under 5 minutes |
| Integration | `cargo test --test proxy_integration --test cli_integration --test filesystem_integration` | every PR, Linux | under 10 minutes |
| Generated | deterministic property corpus/replay | every PR, Linux | under 10 minutes |
| Fuzz | bounded `cargo fuzz run` targets | scheduled/manual | bounded CPU/time |
| Performance | fixed-fixture benchmark command | scheduled/manual | trend only; no flaky PR gate |

## Non-functional acceptance

- No test requires a real provider endpoint, account, or credential.
- Tests run in isolated temporary directories and do not read the developer's
  home directory, real store, or ambient configuration.
- Network tests bind only loopback ephemeral ports and never call public hosts.
- Timeout/cancellation tests use deterministic local delays with explicit,
  small deadlines.
- Every P0 contract has at least one integration or black-box test, not only a
  unit test.
- Any discovered redaction, parser, archive, or recovery regression adds a
  minimal permanent fixture before its fix is accepted.

## Out of scope

- A real-provider benchmark is optional empirical evidence, not CI and not a
  substitute for deterministic protocol fixtures.
- End-to-end testing of every third-party coding agent is out of scope because
  bbg intentionally has no agent-specific compatibility matrix.
- Coverage-percentage targets are not release gates until the integration suite
  establishes a meaningful baseline; contract coverage is the initial metric.
