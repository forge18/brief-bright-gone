# bbg Domain Context

## Language

**Terminal State**:
The one final outcome marker in a sigil response: Done (`.`), Decision Needed (`?`), or Blocked (`x`).
_Avoid_: terminal block, intermediate status

**Sigil Response**:
A response encoded in the bbg line-oriented sigil format before it is decoded to Markdown.

**Prefix-Emphasis Span**:
The maximal non-whitespace run after `*`, rendered as a load-bearing Markdown bold span.
_Avoid_: paired emphasis, literal asterisk

**Eligible Payload**:
A tool-result payload proven safe for one supported reversible compressor.
_Avoid_: compressible payload, likely-safe payload

**Session**:
An isolated, active-window-scoped continuation identified by a unique longest history-prefix fingerprint.
_Avoid_: provider session, client session

**Installation Record**:
The manifest-owned record of a skill file written by bbg, including its path, version, and digest.
_Avoid_: discovered agent configuration, unmanaged skill

## Invariants

- Each **Sigil Response** has exactly one **Terminal State**.
- A **Terminal State** is the last nonblank top-level line.
- Only `.`, `?`, and `x` can express a **Terminal State**; `?` is not an intermediate block type.
- A **Prefix-Emphasis Span** begins with `*`, contains at least one following non-whitespace character, and ends at the next whitespace boundary.
- Inline-verbatim spans and fenced code blocks preserve their bytes and do not contain parsed **Prefix-Emphasis Spans**.
- An **Eligible Payload** has source metadata, no I4-protected content, deterministic output, and persisted originals before forwarding; every other payload passes unchanged.
- A **Session** is selected only by a unique longest prefix match; a collision or compaction creates an isolated new session without cross-turn dedup.
- A store blob is collectible only when no active-session or served-reference pin remains.
- An **Installation Record** owns only the exact file and digest it wrote; uninstall and upgrade never remove or overwrite an unowned or modified file.

## State Machine

`Open response → Final(. | ? | x)`

A final response has no further nonblank top-level sigil lines.

`New Session → Active → Inactive → Collectible`

A unique match activates or reactivates a session. A collision or compaction
creates a separate active session. Expiration makes a session inactive; it
becomes collectible only after all liveness pins are gone.

## Concurrency

| Shared mutable location | Readers/writers | Synchronization | Risk control |
|---|---|---|---|
| Session registry | Proxy requests and GC | Registry lock | Resolve ties as collisions; do not reuse ambiguous state. |
| Per-session history and pins | Matched proxy requests and GC | Per-session exclusive lock | Serialize prefix, store, history, and pin updates. |
| CCR store and served-ref ledger | Proxy, `bbg get`, and GC | Store transaction/lock | Persist originals before references; collect only unpinned blobs. |
| Installer manifest | Install, upgrade, uninstall | Manifest lock plus atomic replacement | Mutated or unowned targets fail loudly. |
