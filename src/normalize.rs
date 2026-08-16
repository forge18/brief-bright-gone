//! Prose normalization — the byte-safe (S2, reversible) text cleanups applied
//! to user-typed chat prose before it reaches an LLM.
//!
//! This module is deliberately conservative:
//! - Only touches plain-text prose payloads (`detect::ContentType::Text`).
//!   Code / diffs / logs / terminal / JSON are action-sensitive and are
//!   assumed to be gatekept upstream.
//! - Applies whitespace and punctuation cleanups, strips a small set of polite
//!   filler, and replaces profanity with a neutral placeholder.
//! - Every operation is reversible-in-principle (the original string can be
//!   retained for recovery); we do not drop negation or meaning-bearing words.

use crate::detect::ContentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillPoliteness {
    /// Do not strip polite filler.
    Off,
    /// Strip only the smallest set (please, thank-you/thanks).
    Narrow,
}

pub struct NormalizeOptions {
    pub strip_politeness: FillPoliteness,
    pub replace_profanity: bool,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            strip_politeness: FillPoliteness::Narrow,
            replace_profanity: true,
        }
    }
}

/// Result of normalization.
#[derive(Debug, Clone)]
pub struct Normalized {
    pub text: String,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub changed: bool,
}

/// Collapse runs of whitespace/newlines in prose.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    let mut prev_newline = 0;
    for c in s.chars() {
        if c.is_whitespace() {
            if c == '\n' {
                // Allow at most one blank line (two newlines) in a row.
                prev_newline += 1;
                if prev_newline <= 2 {
                    out.push(c);
                }
                prev_space = true;
            } else if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
            prev_newline = 0;
        }
    }
    out
}

/// Trim trailing whitespace and stray trailing punctuation per line.
fn trim_line_edges(s: &str) -> String {
    s.lines()
        .map(|line| {
            let trimmed = line.trim_end();
            let t = trimmed.trim_end_matches(|c| matches!(c, ';' | ':' | ',' | ' ' | '\t'));
            if t.is_empty() { trimmed } else { t }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_politeness(s: &str, mode: FillPoliteness) -> String {
    if mode == FillPoliteness::Off {
        return s.to_string();
    }
    // Word-boundary, case-insensitive removal of standalone polite tokens,
    // including a trailing space so wording stays clean.
    let word = |lower: &str| format!(r"(?i)\b{}\s*", lower);

    let mut t = s.to_string();
    for p in ["please", "thanks", "thank you", "thx", "ty"] {
        let pat = regex::Regex::new(&word(p)).unwrap();
        t = pat.replace_all(&t, "").to_string();
    }
    // Collapse spaces left by removals.
    let re = regex::Regex::new(r" +").unwrap();
    re.replace_all(&t.trim(), " ").to_string()
}

fn replace_profanity(s: &str) -> String {
    let re = regex::Regex::new(
        r"(?i)\b(?:fuck(?:ing|ed|er)?|shit(?:ty|s)?|bitch(?:es|ing)?|damn(?:ed|it)?|piss(?:ed|ing)?)\b",
    )
    .unwrap();
    re.replace_all(s, "@%").to_string()
}

/// Run normalization on a prose payload. Returns the original unchanged if the
/// payload is not plain prose (the caller should route based on `detect`).
pub fn normalize(src: &str, opts: &NormalizeOptions, detected: Option<ContentType>) -> Normalized {
    match detected {
        Some(ct) if ct != ContentType::Text => {
            // Action-sensitive or non-prose: do not touch.
            return Normalized { text: src.to_string(), bytes_before: src.len(), bytes_after: src.len(), changed: false };
        }
        _ => {}
    }

    let original = src.to_string();
    let mut t = collapse_whitespace(&original);
    if opts.strip_politeness != FillPoliteness::Off {
        t = strip_politeness(&t, opts.strip_politeness);
    }
    if opts.replace_profanity {
        t = replace_profanity(&t);
    }
    t = trim_line_edges(&t);
    let t = t.trim().to_string();

    let len_after = t.len();
    let changed = t != original;
    Normalized {
        text: t,
        bytes_before: original.len(),
        bytes_after: len_after,
        changed,
    }
}

/// Convenience: run detection + normalization on a payload in one step, and
/// report how many bytes were saved.
pub fn normalize_with_detect(src: &str, opts: &NormalizeOptions) -> Normalized {
    let ct = crate::detect::detect(src);
    normalize(src, opts, Some(ct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace() {
        let out = normalize_with_detect("  hey   there   \n\n\n\n   friend", &NormalizeOptions::default());
        assert_eq!(out.text, "hey there\n\nfriend");
        assert!(out.changed);
    }

    #[test]
    fn trims_trailing_punct() {
        // Internal commas are kept (lossy to remove); trailing punctuation/
        // whitespace is trimmed. Collapse also merges the comma-adjacent space.
        let out = normalize_with_detect("check this,   and that:  ", &NormalizeOptions::default());
        assert_eq!(out.text, "check this, and that");
        assert!(out.changed);
    }

    #[test]
    fn strips_politeness() {
        let out = normalize_with_detect("please fix the bug thank you", &NormalizeOptions{
            strip_politeness: FillPoliteness::Narrow,
            replace_profanity: true,
        });
        assert_eq!(out.text, "fix the bug");
    }

    #[test]
    fn replaces_profanity() {
        let out = normalize_with_detect("this is fucking ridiculous shit", &NormalizeOptions::default());
        assert_eq!(out.text, "this is @% ridiculous @%");
    }

    #[test]
    fn does_not_touch_code() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"hi\", x);\n}";
        let out = normalize_with_detect(code, &NormalizeOptions::default());
        assert!(!out.changed);
        assert_eq!(out.text, code);
    }

    #[test]
    fn tracks_bytes_and_change() {
        let out = normalize_with_detect("please    do   this", &NormalizeOptions::default());
        assert!(out.changed);
        assert!(out.bytes_after < out.bytes_before);
        assert!(out.bytes_before == "please    do   this".len());
    }
}