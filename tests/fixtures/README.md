# Fixture Layout

This directory reserves stable fixture locations for the format and transport
work described in `TODO.md`.

- `parser/` — valid, malformed, ambiguous, escaped, and fenced sigil inputs.
- `streaming/` — the same inputs split at provider chunk boundaries.
- `store/` — content-addressed blobs, hash misses, and corruption cases.
- `proxy/` — canonical provider requests, responses, and SSE events.
- `safety/` — eligible and protected payload classification cases.

Fixtures must keep action-bearing bytes exact. Add a fixture and its expected
outcome together; do not treat an unparsed or malformed input as a successful
parse.
