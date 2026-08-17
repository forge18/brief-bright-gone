//! Deterministic decoder for the bbg sigil wire format.

/// Decode a complete sigil-formatted response to Markdown.
pub fn decode(input: &str) -> String {
    if !has_valid_terminal_structure(input) {
        return input.to_owned();
    }

    let mut decoder = Decoder::new();
    let mut output = decoder.push(input);
    output.push_str(&decoder.finish());
    output
}

/// Incremental line decoder for streaming provider responses.
///
/// Complete non-fence lines decode as soon as their newline arrives. Table runs
/// remain buffered until their boundary because column validation needs every
/// row. Fenced bytes are always forwarded verbatim.
#[derive(Debug, Default)]
pub struct Decoder {
    pending: String,
    table_lines: Vec<String>,
    in_fence: bool,
    nested_depth: usize,
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
        output
    }

    fn process_line(&mut self, line: &str, ending: &str) -> String {
        if self.in_fence {
            let output = format!("{line}{ending}");
            if is_fence(line) {
                self.in_fence = false;
            }
            return output;
        }

        if is_table_line(line) {
            self.table_lines.push(format!("{line}{ending}"));
            return String::new();
        }

        let mut output = self.flush_table();
        if is_fence(line) {
            self.in_fence = true;
            self.nested_depth = 0;
            output.push_str(line);
            output.push_str(ending);
            return output;
        }

        output.push_str(&decode_line(line, ending, &mut self.nested_depth));
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

fn is_fence(line: &str) -> bool {
    line.starts_with("```")
}

fn is_table_line(line: &str) -> bool {
    line.starts_with('|')
}

/// A complete sigil response either has no terminal yet (useful for partial
/// streams) or has exactly one terminal as its final nonblank top-level line.
/// Any other terminal layout is malformed and the whole response fails open.
fn has_valid_terminal_structure(input: &str) -> bool {
    let mut in_fence = false;
    let mut terminal_lines = Vec::new();
    let mut last_nonblank = None;

    for (index, raw_line) in input.lines().enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if !line.trim().is_empty() {
            last_nonblank = Some(index);
        }
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence
            && !line.starts_with('\\')
            && ['.', '?', 'x']
                .iter()
                .any(|marker| marker_body(line, *marker).is_some())
        {
            terminal_lines.push(index);
        }
    }

    terminal_lines.is_empty()
        || (terminal_lines.len() == 1 && terminal_lines[0] == last_nonblank.unwrap_or_default())
}

fn split_line_ending(line: &str) -> (&str, &str) {
    match line.strip_suffix('\r') {
        Some(without_cr) => (without_cr, "\r"),
        None => (line, ""),
    }
}

fn marker_body(line: &str, marker: char) -> Option<&str> {
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
    Some(format!("{indent}{prefix}{}", decode_inline(body)))
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
            let after_marker = cursor + 1;
            let span_end = input[after_marker..]
                .char_indices()
                .find_map(|(offset, value)| value.is_whitespace().then_some(after_marker + offset))
                .unwrap_or(input.len());
            if span_end > after_marker {
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
    use super::{Decoder, decode};

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
    fn decodes_valid_tables_and_fails_open_on_invalid_runs() {
        assert_eq!(
            decode("|Name|Count\n|one|2\n. done"),
            "|Name|Count|\n|---|---:|\n|one|2|\n**Done.** done"
        );
        let malformed = "|Name|Count\n|one\n. done";
        assert_eq!(decode(malformed), "|Name|Count\n|one\n**Done.** done");
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
    fn invalid_terminal_layout_fails_open_for_the_complete_response() {
        let malformed = ". first\nbody after terminal";
        assert_eq!(decode(malformed), malformed);
    }

    #[test]
    fn decoding_is_deterministic() {
        let source = "§ Status\n- *keep\n. done";
        assert_eq!(decode(source), decode(source));
    }
}
