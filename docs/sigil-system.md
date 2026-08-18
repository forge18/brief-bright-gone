# bbg Sigil System

The wire format the model emits and the proxy decodes.

## Purpose

The model emits shorthand. The proxy decodes it to markdown on the response path; the agent displays markdown, which every agent renders natively. Formatted markup is therefore never generated as output tokens and never stored in wire history — only the shorthand is, restored on the request path via CCR substitution (design.md §3.1).

**Round-trip exactness comes from storage, not inversion.** The decoder is one-way (sigils → markdown); the original sigil bytes come back from the store. The decoder needs no inverse, and the markdown mapping below is presentation, not contract — changing it never affects stored originals. The property under test in CI is decode + substitute = identity (I1).

**Governing principle: transmit structure, generate spacing.** If the decoder can derive whitespace from a sigil, the model does not send it.

**Design rule: line-initial sigils, not paired delimiters.** A marker at line start merges with the newline and costs about one token. A paired construct such as `**bold**` costs 2–4.

**Scope: formatting only, never content.** Nothing is deleted, so the action-grammar failure mode (AGORA) does not apply.

---

## Block sigils

Line-initial. One per line.

| Sigil | Meaning | Decodes to |
|---|---|---|
| `§` | Section heading | `##` heading |
| `-` | Bullet | `-` list item |
| `>` | Consequence / downside | `>` blockquote |
| `!` | Blocking issue | `> **Blocking:**` prefixed blockquote |
| `~` | Non-blocking note | `> Note:` prefixed blockquote |
| `.` | Done | `**Done.**` terminal line |
| `x` | Blocked | `**Blocked:**` terminal line |

Lines with no sigil are body text. That is the common case and costs nothing.

### Recognition requires trailing whitespace

**A block sigil is recognized only when followed by whitespace or end of line.** For nested forms, the rule applies after the dash run and optional type sigil: `-! text`, `--# text`.

This closes an entire class of silent misparses that fail-open cannot catch, because they parse *successfully* into the wrong thing:

- `x86 hosts…` — body text, not a Blocked state.
- `...continuing` — body text, not Done.
- `--force does X` — body text, not a depth-2 bullet. Flag-initial lines need no escape (though backticking them remains correct under F6).

---

## Nesting

A leading run of `-` sets depth. An optional type sigil follows, then required whitespace. Parse left to right: count the dashes, read the next character, require the terminator.

```
- text       bullet, depth 1
-- text      bullet, depth 2
--- text     bullet, depth 3
-! text      blocking issue, depth 1
--~ text     non-blocking note, depth 2
```

Any non-terminal block type can nest, not only bullets. Terminal states are top-level only.

**Ordered lists:** `-#` at depth 1, `--#` at depth 2. The model emits no numbers. The decoder counts, which saves tokens and eliminates renumbering errors.

### V1 nested rendering

The decoder renders depth `d` with `2 × (d - 1)` leading spaces. It then uses
the following Markdown prefix; the source body follows unchanged except for
inline decoding outside verbatim spans.

| Nested sigil | Markdown prefix |
|---|---|
| `-` | `- ` |
| `-#` | `1. ` (counter restarts for each sibling list) |
| `->` | `> ` |
| `-!` | `> **Blocking:** ` |
| `-~` | `> Note: ` |

A nested source block is valid only when its parent depth exists in the same
contiguous block sequence. A depth jump larger than one, a nested terminal,
or a nested table fails open for that line. Nested decision content uses a
normal body line or one of the non-terminal prefixes; only a final top-level
`?` is a decision-needed terminal.

Indentation is never transmitted. Repeated sigils are cheaper than leading spaces and immune to whitespace stripping anywhere in the pipeline.

---

## Terminal state

Every sigil response has exactly one terminal state. The terminal is the last
nonblank top-level line; it cannot be nested or followed by another nonblank
top-level line.

```abnf
terminal-line  = done-line / decision-line / blocked-line
done-line      = "." [SP terminal-detail]
decision-line  = "?" [SP terminal-detail]
blocked-line   = "x" [SP terminal-detail]
terminal-detail = 1*non-newline
```

| Sigil | Meaning | Decodes to |
|---|---|---|
| `. <result>` | Done | `**Done.** <result>` |
| `? <decision and explicit options>` | Decision needed | `**Decision needed:** <decision and explicit options>` |
| `x <root cause>; options: <options>` | Blocked | `**Blocked:** <root cause>; **Options:** <options>` |

A bare terminal sigil is permitted; same-line detail is optional. The `?` sigil
is terminal-only. Earlier decisions belong in ordinary body text or a
non-terminal nested block.

## Tables

Markdown semantics, sigil form. Three things removed: the separator row,
alignment padding, and the trailing pipe.

```
|Rule|Source|Check
|B2 No preamble|BLUF|Lintable
|B5 Sentence integrity|AGORA|Lintable
```

### V1 table-run grammar

A table run is a maximal contiguous run of top-level lines that begin with `|`,
outside a fenced code block. Its first line is the header and it must contain
at least one following data row. Headerless tables are inexpressible.

```abnf
table-run        = table-header LF 1*table-row
table-header     = table-row
table-row        = "|" table-cell *("|" table-cell)
table-cell       = *table-char
table-char       = %x00-0A / %x0C-7E / UTF8-2 / UTF8-3 / UTF8-4
```

- Every row in a run must have the header's exact column count.
- Empty cells are valid; a trailing `|` is not.
- No separator row, alignment padding, or alignment syntax is transmitted.
- A blank line, a non-`|` line, or a fence boundary ends the run.
- A line inside a fence is never a table row. A line escaped with leading `\`
  is a body line, not a table row.

The decoder emits standard Markdown: header, synthesized separator, then data
rows. It right-aligns a column only when every nonempty data cell in that column is
numeric; otherwise it left-aligns it. An invalid run (missing data row,
trailing separator, inconsistent column count, or unterminated fence) fails
open as its original bytes; no partial table decoding occurs.

---

## Inline

| Sigil | Meaning | Decodes to |
|---|---|---|
| `` ` `` | Verbatim — paths, identifiers, commands, error strings. Byte-exact, never reflowed (F6). | `` ` `` unchanged |
| `*` | Load-bearing keyword | `**bold**` |

### V1 emphasis grammar

Outside an inline-verbatim span or fenced code block, an asterisk followed by a
non-whitespace character starts a prefix-emphasis span. The span is the maximal
following run of non-whitespace characters.

```abnf
emphasis       = "*" load-bearing-span
load-bearing-span = 1*non-whitespace
non-whitespace = %x21-7E / UTF8-2 / UTF8-3 / UTF8-4
```

The decoder renders `*keyword` as `**keyword**`. Because the span ends only at
whitespace, punctuation is part of the emphasized text: `*fixed,` becomes
`**fixed,**`. Paired emphasis is not syntax: in `*foo*`, the trailing asterisk
belongs to the load-bearing span. Literal asterisks require inline verbatim or
a fenced code block. The parser never interprets emphasis inside either
verbatim form.

### Inline verbatim density

Verbatim is paired, so it costs ~2 tokens, and agent output is saturated with paths and identifiers. This is the encoding's dominant markup cost.

**Decision: explicit backticks, as specified above.** Correct, cheap to build, and no ambiguity about what is byte-exact.

Two cheaper alternatives exist if measured density on real transcripts later justifies the complexity: decoder-side auto-detection of obvious identifiers (contains `/`, file extension, snake_case, camelCase, leading `--`), or a per-turn declaration line applied to every occurrence. Neither is in the spec.

---

## Code blocks

Standard fenced blocks with a language tag. Contents are byte-exact. The decoder does not modify bytes inside a fence — this is a hard rule, not a heuristic, because indentation is meaning in Python, YAML, diffs, and Odin.

Fences are not compressed. They are mostly content already, and mangling the fence risks the verbatim guarantee.

---

## Whitespace

### Never transmitted (presentation)

Blank lines above sections, padding around code fences, spacing between bullets, visual indentation. All derivable from the sigil; all synthesized by the decoder in its markdown output.

### Transmitted as structure

- **One `\n` = one block boundary.** Every line is its own block. Blank lines are never needed for prose. This halves the most frequent whitespace cost, since a paragraph break drops from two newline tokens to one.
- **Nesting depth** via repeated sigil, not indentation.
- **No manual line wrapping.** One block, one physical line, however long. Models habitually hard-wrap at 80 columns, which costs a token per break and fights the renderer. The agent's own renderer wraps to display width.

### Byte-exact (semantic)

Inside fenced code blocks and inline verbatim spans.

### Stripped (waste)

Trailing whitespace, runs of three or more newlines, trailing newlines at end of turn.

---

## Escaping

**`\` at line start only.** The leading `\` is consumed; the rest of the line is body text. `\\` at line start emits a literal backslash.

Chosen because:

- It is markdown's own escape character, so the model already knows the convention — no distribution shift.
- Line-initial only, so `C:\Users\...` mid-line is untouched.
- Not in the sigil set and not the start of a code fence.
- Body lines genuinely starting with a backslash are rare, and the cases that exist (UNC paths, LaTeX) belong in verbatim or a fence, where the decoder does not touch bytes.

**Scope: block interpretation only.** `\` suppresses the line's sigil reading; inline parsing continues normally. `\*foo* bar` is a body line in which `*foo*` still renders as a keyword. For a fully literal line, use verbatim or a fence.

**Skill guidance:** models are inconsistent about escaping. Make it moot — the trailing-whitespace rule already returns most ambiguous lines to body text, and teaching the model to backtick a line beginning with a path, flag, or identifier covers the rest. That is correct under F6 anyway, which makes the escape a rare fallback rather than a routine operation.

---

## Streaming

The proxy decodes incrementally within the provider's streaming response.

**Block completion.** One-line-per-block means a non-terminal block is complete — and decodes — the moment `\n` arrives, unlike markdown, where a paragraph's end and an unclosed fence are both ambiguous mid-stream. The one exception is a terminal-marker line (`.`/`?`/`x`): because a terminal is only valid as the final nonblank line, the decoder holds such a line back exactly one line. If another nonblank line follows, the held line was not the terminal and renders **raw**; if the stream (or the block) ends first, it decodes. This bounded one-line lookahead is the per-line form of the terminal grammar above, and it makes streaming and whole-buffer decode of the same bytes byte-identical — there is no separate whole-response validation pass.

**Code fences.** The proxy tracks fence state and passes fence bytes through unchanged as they arrive — inside a fence there is nothing to decode, so no buffering is needed. Display-side wrapping and redraw are the agent renderer's problem, not bbg's.

---

## The decoder

A small custom line parser in the proxy — one-line-per-block makes it near-trivial. It is **not** a markdown parser and no markdown crate applies to the sigil side; markdown appears only as the decoder's *output*. The optional `bbg wrap` (design.md §8) renders that markdown with a single crate and never sees sigils.

**Failure posture: fail-open on decode.** Content that does not parse as sigil form passes through untouched. A display-path error must never look like a content error.

---

## Not an encoding question

Whether the model reads its own shorthand history well — terse structured context may help (denser) or hurt (further from training distribution) — is a benchmark question about model behaviour, not a gap in this spec. It does not change the format.
