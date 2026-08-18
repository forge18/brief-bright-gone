//! Deterministic decoder for the bbg sigil wire format.

/// Decode a complete sigil-formatted response to Markdown.
///
/// Decoding is per-line: a terminal-marker line (`.`/`?`/`x`) renders raw unless
/// it is the final nonblank line. This is the same rule the incremental
/// [`Decoder`] applies chunk-by-chunk, so batch and streaming decode of the same
/// bytes are byte-identical.
pub fn decode(input: &str) -> String {
    let mut decoder = Decoder::new();
    let mut output = decoder.push(input);
    output.push_str(&decoder.finish());
    output
}

/// Incremental line decoder for streaming provider responses.
///
/// Complete non-fence lines decode as soon as their newline arrives. Table runs
/// remain buffered until their boundary because column validation needs every
/// row. Fenced bytes are always forwarded verbatim. A terminal-marker line is
/// held back one line: it decodes only if it turns out to be the final nonblank
/// line, otherwise it renders raw (bounded lookahead).
#[derive(Debug, Default)]
pub struct Decoder {
    pending: String,
    table_lines: Vec<String>,
    in_fence: bool,
    nested_depth: usize,
    pending_terminal: Option<(String, String)>,
    pending_blanks: String,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a provider chunk and return newly decoded Markdown.
    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut output = String::new();

        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].to_owned();
            self.pending.drain(..=newline);
            output.push_str(&self.process_line(&line, "\n"));
        }

        output
    }

    /// Complete the stream and return the final decoded bytes.
    pub fn finish(&mut self) -> String {
        let mut output = String::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            output.push_str(&self.process_line(&line, ""));
        }
        output.push_str(&self.flush_table());
        // A still-held terminal is the final nonblank line, so it decodes.
        output.push_str(&self.flush_pending_terminal_decoded());
        output
    }

    fn process_line(&mut self, line: &str, ending: &str) -> String {
        let mut output = String::new();

        // A held terminal candidate stays pending across blank lines (it may
        // still be the final nonblank line); any nonblank line proves it was
        // not last, so it renders raw before this line is processed.
        if self.pending_terminal.is_some() {
            if line.trim().is_empty() {
                self.pending_blanks.push_str(line);
                self.pending_blanks.push_str(ending);
                return output;
            }
            output.push_str(&self.flush_pending_terminal_raw());
        }

        if self.in_fence {
            output.push_str(line);
            output.push_str(ending);
            if is_fence(line) {
                self.in_fence = false;
            }
            return output;
        }

        if is_table_line(line) {
            self.table_lines.push(format!("{line}{ending}"));
            return output;
        }

        output.push_str(&self.flush_table());
        if is_fence(line) {
            self.in_fence = true;
            self.nested_depth = 0;
            output.push_str(line);
            output.push_str(ending);
            return output;
        }

        // Hold a terminal-marker line one line: it decodes only if it proves to
        // be the final nonblank line (resolved above or in finish()).
        if is_terminal_line(line) {
            self.nested_depth = 0;
            self.pending_terminal = Some((line.to_owned(), ending.to_owned()));
            return output;
        }

        output.push_str(&decode_line(line, ending, &mut self.nested_depth));
        output
    }

    fn flush_pending_terminal_raw(&mut self) -> String {
        let mut output = match self.pending_terminal.take() {
            Some((line, ending)) => format!("{line}{ending}"),
            None => String::new(),
        };
        output.push_str(&std::mem::take(&mut self.pending_blanks));
        output
    }

    fn flush_pending_terminal_decoded(&mut self) -> String {
        let Some((line, ending)) = self.pending_terminal.take() else {
            return std::mem::take(&mut self.pending_blanks);
        };
        let mut output = decode_line(&line, &ending, &mut self.nested_depth);
        output.push_str(&std::mem::take(&mut self.pending_blanks));
        output
    }

    fn flush_table(&mut self) -> String {
        if self.table_lines.is_empty() {
            return String::new();
        }
        let lines = std::mem::take(&mut self.table_lines);
        self.nested_depth = 0;
        decode_table(&lines)
    }
}

/// True when the decoder recognizes at least one sigil in the response. This
/// mirrors the decoder's line grammar, including nested-depth validation and
/// fenced/table boundaries, without relying only on whether the rendered bytes
/// changed (a valid `- item` list renders with the same prefix).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TableHealth {
    pub table_runs: u64,
    pub malformed_table_runs: u64,
}

/// Count explicit table runs using the same validation as the decoder. The
/// response bytes remain fail-open; this is passive health telemetry only.
pub fn table_health(content: &str) -> TableHealth {
    let mut health = TableHealth::default();
    let mut in_fence = false;
    let mut table_lines = Vec::new();

    for segment in content.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if in_fence {
            if is_fence(line) {
                in_fence = false;
            }
            continue;
        }
        if is_table_line(line) {
            table_lines.push(segment.to_owned());
            continue;
        }
        if !table_lines.is_empty() {
            health.table_runs += 1;
            if decode_table(&table_lines) == table_lines.concat() {
                health.malformed_table_runs += 1;
            }
            table_lines.clear();
        }
        if is_fence(line) {
            in_fence = true;
        }
    }
    if !table_lines.is_empty() {
        health.table_runs += 1;
        if decode_table(&table_lines) == table_lines.concat() {
            health.malformed_table_runs += 1;
        }
    }
    health
}

pub fn uses_sigils(content: &str) -> bool {
    let mut in_fence = false;
    let mut nested_depth = 0;
    let mut table_lines = Vec::new();

    let segments = content.split_inclusive('\n').collect::<Vec<_>>();
    for (index, segment) in segments.iter().enumerate() {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if in_fence {
            if is_fence(line) {
                in_fence = false;
            }
            continue;
        }
        if is_table_line(line) {
            table_lines.push((*segment).to_owned());
            continue;
        }
        if !table_lines.is_empty() {
            let source = table_lines.concat();
            if decode_table(&table_lines) != source {
                return true;
            }
            table_lines.clear();
        }
        if is_fence(line) {
            in_fence = true;
            nested_depth = 0;
            continue;
        }
        let has_marker = marker_body(line, '§').is_some()
            || marker_body(line, '>').is_some()
            || marker_body(line, '!').is_some()
            || marker_body(line, '~').is_some()
            || marker_body(line, '.').is_some()
            || marker_body(line, '?').is_some()
            || marker_body(line, 'x').is_some();
        let final_nonblank = !segments[index + 1..]
            .iter()
            .any(|later| !later.strip_suffix('\n').unwrap_or(later).trim().is_empty());
        if has_marker && (!is_terminal_line(line) || final_nonblank) {
            return true;
        }
        if !line.starts_with('\\') && decode_nested(line, &mut nested_depth).is_some() {
            return true;
        }
        nested_depth = 0;
        let inline_source = line.strip_prefix('\\').unwrap_or(line);
        if decode_inline(inline_source) != inline_source {
            return true;
        }
    }

    if !table_lines.is_empty() {
        let source = table_lines.concat();
        decode_table(&table_lines) != source
    } else {
        false
    }
}

/// A terminal-marker line (`.`/`?`/`x`) is the only line kind subject to the
/// final-nonblank-line rule. Escaped lines are never terminals.
///
/// `pub(crate)`: shared with `lint`'s G1 check, so the passive linter detects
/// terminals using the same raw-sigil grammar the decoder does, rather than a
/// second hand-rolled approximation that can drift from it.
pub(crate) fn is_terminal_line(line: &str) -> bool {
    !line.starts_with('\\')
        && ['.', '?', 'x']
            .iter()
            .any(|marker| marker_body(line, *marker).is_some())
}

fn is_fence(line: &str) -> bool {
    line.starts_with("```")
}

fn is_table_line(line: &str) -> bool {
    line.starts_with('|')
}

fn split_line_ending(line: &str) -> (&str, &str) {
    match line.strip_suffix('\r') {
        Some(without_cr) => (without_cr, "\r"),
        None => (line, ""),
    }
}

/// `pub(crate)`: shared with `lint`'s R3 check (severity labels are the raw
/// `!`/`~` markers, not the decoded "Blocking:"/"Note:" prose).
pub(crate) fn marker_body(line: &str, marker: char) -> Option<&str> {
    let (body, cr) = split_line_ending(line);
    let remainder = body.strip_prefix(marker)?;
    if remainder.is_empty() || remainder.starts_with(char::is_whitespace) {
        Some(if cr.is_empty() {
            remainder
        } else {
            &line[marker.len_utf8()..]
        })
    } else {
        None
    }
}

fn decode_line(line: &str, ending: &str, nested_depth: &mut usize) -> String {
    if let Some(escaped) = line.strip_prefix('\\') {
        *nested_depth = 0;
        return format!("{}{}", decode_inline(escaped), ending);
    }

    if let Some(rendered) = decode_nested(line, nested_depth) {
        return format!("{rendered}{ending}");
    }

    *nested_depth = 0;
    let rendered = if let Some(body) = marker_body(line, '§') {
        format!("##{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, '>') {
        format!(">{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, '!') {
        format!("> **Blocking:**{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, '~') {
        format!("> Note:{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, '.') {
        format!("**Done.**{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, '?') {
        format!("**Decision needed:**{}", decode_inline(body))
    } else if let Some(body) = marker_body(line, 'x') {
        format!("**Blocked:**{}", decode_inline(body))
    } else {
        decode_inline(line)
    };
    format!("{rendered}{ending}")
}

fn decode_nested(line: &str, previous_depth: &mut usize) -> Option<String> {
    let depth = line
        .chars()
        .take_while(|character| *character == '-')
        .count();
    if depth == 0 {
        return None;
    }

    let rest = &line[depth..];
    let (prefix, body) = if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        ("- ", consume_required_space(rest))
    } else {
        let typed = rest.chars().next()?;
        let after_type = &rest[typed.len_utf8()..];
        if !(after_type.is_empty() || after_type.starts_with(char::is_whitespace)) {
            return None;
        }
        let prefix = match typed {
            '#' => "1. ",
            '>' => "> ",
            '!' => "> **Blocking:** ",
            '~' => "> Note: ",
            _ => return None,
        };
        (prefix, consume_required_space(after_type))
    };

    if depth > *previous_depth + 1 {
        *previous_depth = 0;
        return None;
    }

    *previous_depth = depth;
    let indent = "  ".repeat(depth.saturating_sub(1));
    Some(format!("{indent}{prefix}{}", decode_nested_inline(body)))
}

fn decode_nested_inline(input: &str) -> String {
    if let Some(rest) = input.strip_prefix('*')
        && rest.chars().next().is_some_and(char::is_alphanumeric)
    {
        let end = rest
            .char_indices()
            .find_map(|(offset, value)| value.is_whitespace().then_some(offset))
            .unwrap_or(rest.len());
        return format!("**{}**{}", &rest[..end], &rest[end..]);
    }
    decode_inline(input)
}

fn decode_table(lines: &[String]) -> String {
    let source = lines.concat();
    if lines.len() < 2 {
        return source;
    }

    let mut rows = Vec::with_capacity(lines.len());
    for line in lines {
        let (line_without_newline, ending) = if let Some(stripped) = line.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (line.as_str(), "")
        };
        let (text, carriage_return) = split_line_ending(line_without_newline);
        if !carriage_return.is_empty() && ending.is_empty() {
            return source;
        }
        if !text.starts_with('|') || (text.len() > 1 && text.ends_with('|')) {
            return source;
        }
        rows.push(text[1..].split('|').map(str::to_owned).collect::<Vec<_>>());
    }

    let column_count = rows[0].len();
    if column_count == 0 || rows.iter().any(|row| row.len() != column_count) {
        return source;
    }

    let alignments = (0..column_count)
        .map(|column| {
            let nonempty = rows.iter().skip(1).filter_map(|row| {
                let cell = row[column].trim();
                (!cell.is_empty()).then_some(cell)
            });
            let cells = nonempty.collect::<Vec<_>>();
            !cells.is_empty() && cells.iter().all(|cell| cell.parse::<f64>().is_ok())
        })
        .collect::<Vec<_>>();

    let mut output = String::new();
    output.push_str(&render_table_row(&rows[0]));
    output.push('\n');
    output.push('|');
    for numeric in &alignments {
        output.push_str(if *numeric { "---:|" } else { "---|" });
    }
    output.push('\n');
    for row in rows.iter().skip(1) {
        output.push_str(&render_table_row(row));
        output.push('\n');
    }
    if !lines.last().is_some_and(|line| line.ends_with('\n')) {
        output.pop();
    }
    output
}

fn consume_required_space(value: &str) -> &str {
    value.strip_prefix(char::is_whitespace).unwrap_or(value)
}

fn render_table_row(row: &[String]) -> String {
    format!("|{}|", row.join("|"))
}

fn decode_inline(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while cursor < input.len() {
        let remainder = &input[cursor..];
        let character = remainder.chars().next().expect("cursor is on a boundary");
        if character == '`' {
            if let Some(close_offset) = remainder[1..].find('`') {
                let end = cursor + close_offset + 2;
                output.push_str(&input[cursor..end]);
                cursor = end;
                continue;
            }
            output.push_str(remainder);
            break;
        }
        if character == '*' {
            let preceded_by_word = input[..cursor]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let after_marker = cursor + 1;
            let first = input[after_marker..].chars().next();
            let span_end = input[after_marker..]
                .char_indices()
                .find_map(|(offset, value)| value.is_whitespace().then_some(after_marker + offset))
                .unwrap_or(input.len());
            // A load-bearing span opens at the line/segment start or after
            // whitespace — matching the grammar, which has no special case for
            // position 0 (so top-level `*word` bolds just like nested `- *word`).
            let at_boundary = cursor == 0
                || input[..cursor]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            if !preceded_by_word
                && at_boundary
                && first.is_some_and(char::is_alphanumeric)
                && span_end > after_marker
            {
                output.push_str("**");
                output.push_str(&input[after_marker..span_end]);
                output.push_str("**");
                cursor = span_end;
                continue;
            }
        }
        output.push(character);
        cursor += character.len_utf8();
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{Decoder, TableHealth, decode, table_health, uses_sigils};

    #[test]
    fn decodes_a_heading_and_done_terminal() {
        assert_eq!(
            decode("§ Status\n. shipped"),
            "## Status\n**Done.** shipped"
        );
    }

    #[test]
    fn decodes_nested_blocks_and_inline_emphasis() {
        assert_eq!(
            decode("- *keep\n--! blocked\n. done"),
            "- **keep**\n  > **Blocking:** blocked\n**Done.** done"
        );
    }

    #[test]
    fn preserves_inline_verbatim_and_fences() {
        assert_eq!(
            decode("`*literal` *live\n```rust\n*unchanged\n```\n. done"),
            "`*literal` **live**\n```rust\n*unchanged\n```\n**Done.** done"
        );
    }

    #[test]
    fn does_not_treat_ordinary_asterisks_as_emphasis() {
        // `5*6` is preceded by a word; `*.rs` is followed by punctuation, not a
        // load-bearing span opener.
        assert_eq!(decode("5*6 *.rs\n. done"), "5*6 *.rs\n**Done.** done");
    }

    #[test]
    fn line_initial_emphasis_bolds_at_top_level_like_nested_bodies() {
        // A top-level line opening with `*word` bolds (grammar has no position-0
        // exception); `* item` (asterisk then space) is not a span.
        assert_eq!(
            decode("*keyword rest\n. done"),
            "**keyword** rest\n**Done.** done"
        );
        assert_eq!(decode("* item\n. done"), "* item\n**Done.** done");
    }

    #[test]
    fn prefix_emphasis_applies_to_pointer_like_spans_per_grammar() {
        // Per the V1 emphasis grammar, a whitespace-preceded `*word` is always a
        // load-bearing span — there is no special case for pointer-deref syntax.
        assert_eq!(
            decode("deref *ptr and *self\n. done"),
            "deref **ptr** and **self**\n**Done.** done"
        );
        // Literal `*ptr` uses inline verbatim, the documented escape.
        assert_eq!(
            decode("deref `*ptr`\n. done"),
            "deref `*ptr`\n**Done.** done"
        );
    }

    #[test]
    fn decodes_valid_tables_and_fails_open_on_invalid_runs() {
        assert_eq!(
            decode("|Name|Count\n|one|2\n. done"),
            "|Name|Count|\n|---|---:|\n|one|2|\n**Done.** done"
        );
        let malformed = "|Name|Count\n|one\n. done";
        assert_eq!(decode(malformed), "|Name|Count\n|one\n**Done.** done");
    }

    #[test]
    fn table_health_uses_the_decoder_validation_and_ignores_fences() {
        assert_eq!(
            table_health("|Name|Count\n|one|2\n. done"),
            TableHealth {
                table_runs: 1,
                malformed_table_runs: 0,
            }
        );
        assert_eq!(
            table_health("|Name|Count\n|one\n. done"),
            TableHealth {
                table_runs: 1,
                malformed_table_runs: 1,
            }
        );
        assert_eq!(
            table_health("```text\n|not|a table\n```\n. done"),
            TableHealth::default()
        );
    }

    #[test]
    fn streams_complete_lines_and_buffers_chunk_boundaries() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.push("§ Sta"), "");
        assert_eq!(decoder.push("tus\n. do"), "## Status\n");
        assert_eq!(decoder.push("ne"), "");
        assert_eq!(decoder.finish(), "**Done.** done");
    }

    #[test]
    fn malformed_prefixes_and_depth_jumps_fail_open_per_line() {
        assert_eq!(
            decode("x86 host\n--- jump\n. done"),
            "x86 host\n--- jump\n**Done.** done"
        );
    }

    #[test]
    fn non_final_terminal_line_renders_raw_but_does_not_block_other_decoding() {
        // The mid-response terminal stays raw; the heading after it still
        // decodes, instead of the whole response failing open.
        assert_eq!(
            decode(". first\n§ heading\n. done"),
            ". first\n## heading\n**Done.** done"
        );
        // A code-ish line opening with a terminal marker is preserved verbatim.
        assert_eq!(
            decode("x = 5 is wrong\n. done"),
            "x = 5 is wrong\n**Done.** done"
        );
    }

    #[test]
    fn terminal_held_across_trailing_blank_lines_still_decodes() {
        assert_eq!(decode("§ H\n. done\n\n"), "## H\n**Done.** done\n\n");
    }

    #[test]
    fn batch_and_incremental_decode_are_byte_identical_regardless_of_chunking() {
        let source = ". mid\n§ Status\n- *keep\n. done\nx = 5\n. really done";
        let batch = decode(source);
        for split in 1..source.len() {
            if !source.is_char_boundary(split) {
                continue;
            }
            let mut decoder = Decoder::new();
            let mut streamed = decoder.push(&source[..split]);
            streamed.push_str(&decoder.push(&source[split..]));
            streamed.push_str(&decoder.finish());
            assert_eq!(streamed, batch, "divergence at split {split}");
        }
    }

    #[test]
    fn uses_sigils_agrees_with_decoder_grammar() {
        assert!(!uses_sigils("plain prose with no markers"));
        assert!(!uses_sigils("x86 host\n--- jump\njust text"));
        assert!(uses_sigils("§ Status\n. done"));
        assert!(uses_sigils("? decision; options: a"));
        assert!(uses_sigils("- bullet"));
        assert!(uses_sigils("-# ordered"));
        assert!(uses_sigils("|a|b\n|1|2"));
        assert!(uses_sigils("*word emphasized"));
        assert!(uses_sigils("! blocking\n~ note"));
        // Escaped line markers are not sigils; emphasis on an escaped line is.
        assert!(!uses_sigils("\\§ literal heading"));
        assert!(uses_sigils("\\*word"));
        // Word-internal asterisks and markerless prose are not sigils.
        assert!(!uses_sigils("five * six"));
        assert!(!uses_sigils("x = 5 is wrong\nplain prose"));
    }

    #[test]
    fn decoding_is_deterministic() {
        let source = "§ Status\n- *keep\n. done";
        assert_eq!(decode(source), decode(source));
    }
}
