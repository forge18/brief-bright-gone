# Release Security Review

Reviewed proxy headers, local-store permissions, recovery behavior, transcript redaction, and protected-constraint provenance.

## Resolved findings

- **CWE-306 — credential-backed proxy exposure:** the proxy binds to loopback by default; explicit non-loopback binding requires opt-in and separate inbound authentication.
- **CWE-918 — upstream redirect SSRF:** upstream URLs are validated, plaintext HTTP is limited to explicit loopback use, and redirects are disabled.
- **CWE-732/CWE-59 — sensitive local files:** store, transcript, ledger, and configuration data use owner-only Unix modes; sensitive reads/appends use descriptor opens with `O_NOFOLLOW` and reject non-regular files.
- **CWE-532 — transcript credential exposure:** structured JSON is recursively redacted; unstructured headers, quoted assignments, cookies/sessions, URI userinfo, and private-key bodies are redacted before serialization.
- **CWE-693 — constraint provenance integrity:** protected constraints come only from the immutable local configuration snapshot; Anthropic string and array system content is preserved exactly after the protected block is prepended and verified.
- **CWE-664 — recovery state ordering:** `bbg get` records a served reference only after stdout write and flush succeed.
- **Sensitive release artifacts:** `.pi/`, `.bbg-store/`, and `.DS_Store` are ignored; pre-existing staged runtime artifacts were removed from the index without deleting local files.
- **Release follow-up:** calibrated transform gates now receive a real normalization savings estimate, cache breakpoint placement uses a calibration-only gate, skill upgrades refuse modified managed files, and release archives are checksum-verified and path-constrained before installation.

## Verification

- `cargo test --all-targets`: 66 tests passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `python3 -m unittest discover -s benchmarks -p 'test_*.py'`: 4 tests passed.
- `bash -n install.sh`: passed.
- `git diff --check`: passed.
- Independent adversarial reviews confirmed network, redirect, descriptor-open, Anthropic preservation, and recovery-ordering fixes. The final narrow review found no remaining code blocker in the requested scope.
- `git diff --cached --name-only` contains no `.pi`, `.bbg-store`, or `.DS_Store` artifacts.

## Residual risks

- Non-Unix platforms receive file-type and symlink checks but not Unix mode enforcement.
- Redaction is defense in depth; unknown or encoded secret formats may evade pattern recognition, so transcripts remain sensitive local data.
- DNS rebinding is reduced by URL and redirect policy but fully pinning resolved addresses would require connection-level address validation.
- Streaming and non-streaming telemetry behavior should receive route-level integration coverage before a production release.
