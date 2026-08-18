# bbg Test-Coverage Review (production-readiness)

Date: 2026-08-18 · Method: `cargo tarpaulin --all-targets` + manual source audit + independent subagent review.

## Headline numbers

- Overall line coverage: **76.6%** (1877/2450) across all targets.
- Per-file hot spots: proxy.rs **87%**, store.rs 90%, sigil.rs 97%, transcript.rs 99%.
- **Thin files:** skill.rs **18%**, bin/bbg.rs main **2%**, bin/bbg-proxy.rs **0%**, benchmark.rs **0%**, lint.rs **68%**, safety.rs 61%, detect.rs 82%, compress.rs 82%.
- All CI-equivalent local gates pass: `cargo test` (95 lib + 26 integration/fixture), `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt` has uncommitted drift (see note).

---

## High-severity findings

### H1 — Proxy request-time bearer authentication is untested (the #1 gap)
`src/proxy.rs:200` `client_authorized` and `:213` `constant_time_eq` are **never exercised with a real token**. Every integration test in `tests/proxy_integration.rs` starts the proxy with `proxy_token: None`, and the only auth-adjacent unit test (`non_loopback_bind_requires_explicit_opt_in_and_authentication`) tests `resolve_bind`, not request-time 401 rejection.
- **Risk (High):** non-loopback bind is explicitly gated by `BBG_PROXY_TOKEN`, but none of the request paths actually verifies that a wrong/missing token yields `HTTP 401`. A regression here silently disables the only network trust boundary.
- **Fix:** add loopback integration tests that (a) set a real proxy token, assert valid `Bearer` passes and reaches upstream, (b) missing token → 401, (c) wrong token → 401, and confirm `constant_time_eq` on equal/unequal/empty inputs at the unit level.

### H2 — CCR blob-integrity failure path untested
`src/store.rs:105-115` (`get` returns `InvalidData "CCR integrity check failed"` when stored bytes don't hash to the requested digest) has **no test**. `content_addressing_and_integrity` only tests an exact round-trip; `rejects_invalid_digest` only tests a non-digest string.
- **Risk (High):** silent corruption of a stored original produces a wrong/blended recovery. The fail-closed path is the whole recovery guarantee.
- **Fix:** write a blob then overwrite/truncate the file so the hash mismatches; assert `get` errors with `InvalidData` and recovery is refused.

### H3 — skill lifecycle trust logic is almost untested
`src/skill.rs` at **18%**. Uncovered: `install` refusing a modified unowned skill (`:132-136`), `upgrade` refusing a modified managed skill (`:171-176`), `uninstall` removing only if digest matches (`:152-164`), manifest digest verification, and install-to-explicit-path idempotence.
- **Risk (Medium-High):** these refusals are the protection against clobbering a user-edited `BBG_SKILL.md`; unbound behavior here can overwrite user files.
- **Fix:** isolated-tempdir unit tests for modified-unowned refusal, modified-managed upgrade refusal, uninstall-by-matching-digest, and manifest round-trip.

---

## Medium findings

### M1 — `lint_transcript` (B3/G2 cross-turn overlap) has zero coverage
`src/lint.rs:139` `lint_transcript` is never called by any test. `lint_document` is covered, but the whole transcript-mode path (prior-turn overlap detection, per-record doc linting) is untested. Production reports (`bbg lint --transcript`, benchmark harness) depend on it.
- **Fix:** unit tests for 0, 1, and >1 records, overlap-triggering and no-overlap cases.

### M2 — `benchmark::report` has no direct unit test
`src/benchmark.rs` is 0/4. It is only touched through the CLI integration test. Fine, but a direct fixture test (turns counting, lint/hedge aggregation, estimate-vs-billing separation) is cheap and protects the report contract.
- **Fix:** unit test on a small `TranscriptRecord` slice.

### M3 — `detect.rs` classification branches untested
Uncovered: yaml/date-line diff detection (`:86`), log, terminal, search-result, and several `ContentType::Text` fallback returns (`:102,125,136,141,154,211`). Classification governs whether lossy transforms run, so a misclassification is a safety issue.
- **Fix:** targeted inputs per branch (log line, diff header, search-result, terminal, yaml, prose fallback).

### M4 — `compress.rs` pass-through / fail-closed branches
Uncovered: `classify` returning `None` for stale capture, protected data, non-UTF8, non-matching content-type, and the `put`/verify failure paths (`:98-100,124-133,148-150`).
- **Fix:** assert no-transform/pass-through for each disqualified classifier input.

### M5 — `safety::Class::name` and 401 helper internals
`Class::name` match arms (S0–S4) and the `unauthorized()` JSON body are untested. Low risk but trivial to add.

### M6 — CLI/main and proxy main are uncovered by themselves
`bbg.rs` main is 2% and `bbg-proxy.rs` main is 0%. These are thin arg-dispatch shells; the meaningful behavior is covered via `CARGO_BIN_EXE_bbg` integration tests (`cli_integration.rs`). Acceptable, but a smoke test that `bbg-proxy` starts and serves `/health` under a temp store would pin the startup seam (`SC-PROXY-P0` seam).

---

## Well covered (do not duplicate)
- Sigil decoding/streaming: 97% + generated 512-case determinism (tests/redaction_sigil_regression.rs, sigil_fixtures.rs, sigil_round_trip.rs).
- Transcript redaction: 99% + checked-in corpus (tests/fixtures/redaction/corpus.json).
- Store served-ledger compaction, GC pinning, normalization collision: covered.
- SSE proxy contracts (order, error, timeout, cancellation, truncation): `tests/proxy_integration.rs` (9).
- Session registry prefix matching + TTL eviction + pinned digests.
- Transcript `append_capped` rotation.
- Installer archive/checksum/traversal rejection: `tests/installer_integration.rs`.

---

## Recommended priority order
1. **H1** bearer-token auth (unit + integration 401). [High]
2. **H2** blob-integrity fail-closed (corrupt blob). [High]
3. **H3** skill lifecycle refusals. [Medium-High]
4. **M1** `lint_transcript`. [Medium]
5. **M2** `benchmark::report`. [Medium]
6. **M3/M4/M5** detect/compress/class-name branches. [Low-Med]
7. **M6** proxy `/health` startup smoke. [Low]

## Notes / caveats
- The working tree contains substantial **uncommitted concurrent changes**; `cargo fmt --check` currently reports drift, which the `fast` CI gate would reject. Coverage reflects the current on-disk tree.
- Recommend committing/formatting before adding tests so the additions land on a stable baseline, and re-running tarpaulin after each H-item to confirm the targeted gain.
