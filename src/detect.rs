//! Content-type detection.
//!
//! Classifies a payload so callers know *what kind* of content they have
//! before deciding whether a transform is appropriate. This is the front of
//! the pipeline and the core safety gate: code, diffs, logs, and search output
//! must be treated differently from chat prose.

use std::collections::HashSet;
use std::sync::LazyLock;

/// A detected content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentType {
    Json,
    Log,
    Code,
    Diff,
    SearchResult,
    Text,
    Tabular,
    Terminal,
}

impl ContentType {
    pub fn name(self) -> &'static str {
        match self {
            ContentType::Json => "json",
            ContentType::Log => "log",
            ContentType::Code => "code",
            ContentType::Diff => "diff",
            ContentType::SearchResult => "search-result",
            ContentType::Text => "text",
            ContentType::Tabular => "tabular",
            ContentType::Terminal => "terminal",
        }
    }

    /// Whether this content type carries action grammar that must survive
    /// verbatim (AGORA guardrail): compression that drops tokens here can
    /// destroy meaning.
    pub fn is_action_sensitive(self) -> bool {
        matches!(
            self,
            ContentType::Code | ContentType::Diff | ContentType::Terminal | ContentType::Json
        )
    }
}

static CODE_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "fn", "pub", "import", "def", "class", "function", "return", "const", "let", "var",
        "public", "private", "static", "void", "struct", "enum", "impl", "trait", "match", "async",
        "await", "package", "fun", "end", "then", "if", "else", "for", "while",
    ]
    .into_iter()
    .collect()
});

static CLIS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "sudo", "git", "npm", "pnpm", "yarn", "brew", "cd", "ls", "cat", "rg", "grep", "python",
        "node", "curl", "gh", "export", "pip", "cargo", "rustc", "make", "docker", "kubectl",
    ]
    .into_iter()
    .collect()
});

fn log_line(line: &str) -> bool {
    let trimmed = line.trim();
    let up = trimmed.to_uppercase();
    // A level token at line start.
    let has_level = up.starts_with("TRACE")
        || up.starts_with("DEBUG")
        || up.starts_with("INFO")
        || up.starts_with("WARN")
        || up.starts_with("WARNING")
        || up.starts_with("ERROR")
        || up.starts_with("FATAL")
        || up.starts_with("PANIC");
    if has_level {
        return true;
    }
    // A leading ISO timestamp is the strongest log signal.
    if trimmed.len() >= 19
        && trimmed.chars().nth(4) == Some('-')
        && trimmed.chars().nth(7) == Some('-')
        && trimmed.chars().nth(10) == Some(' ')
        && trimmed.chars().nth(13) == Some(':')
    {
        return true;
    }
    // A bracketed level/tag, e.g. `[INFO]` or `[main]` — plausible log framing.
    if trimmed.starts_with('[') && trimmed.contains(']') {
        return true;
    }
    false
}

fn code_signal(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("```") {
        return true;
    }
    // Symbols that only appear in code: `=>`, `->`, `::`, `;`, `{}`, `=`.
    if t.contains("=>") || t.contains("::") || t.contains("->") {
        return true;
    }
    if t.contains(';') {
        return true;
    }
    if t.contains("{") || t.contains("}") {
        return true;
    }
    // A line that starts with a known keyword.
    let first = t.split_whitespace().next().unwrap_or("");
    if CODE_KEYWORDS.contains(&first) {
        return true;
    }
    // A line that starts like a CLI command.
    if CLIS.contains(&first) {
        return true;
    }
    // A path-looking token (`a/b.ts`). Any slash counts, so one-line prose
    // mentioning a path ("fix the config/settings issue") also counts as a code
    // signal. That is the safe direction: it only skips normalization on prose,
    // never applies it to real code, so the failure mode is missed savings, not
    // a correctness bug.
    if t.contains('/') && !t.starts_with("http") {
        return true;
    }
    false
}

/// Heuristically classify a payload. Deterministic and cheap.
pub fn detect(input: &str) -> ContentType {
    let s = input.trim();

    // Terminal/ANSI escape is conclusive.
    if s.contains('\x1b') {
        return ContentType::Terminal;
    }

    // XML/HTML-ish.
    if s.starts_with('<') && (s.contains("</") || s.contains("/>")) {
        return ContentType::Text;
    }

    // JSON: leading `{` or `[` and parses (cheap attempt).
    if s.starts_with('{') || s.starts_with('[') {
        let v: Result<serde_json::Value, _> = serde_json::from_str(s);
        if v.is_ok() {
            return ContentType::Json;
        }
    }

    let lines: Vec<&str> = s.lines().collect();
    if lines.is_empty() {
        return ContentType::Text;
    }

    // Diff markers.
    if lines[0].starts_with("diff --git")
        || lines.iter().any(|l| l.trim_start().starts_with("@@ "))
        || lines
            .iter()
            .any(|l| l.starts_with("+++ ") || l.starts_with("--- "))
    {
        return ContentType::Diff;
    }

    // Logs: most lines carry a level token or a timestamp.
    if !lines.is_empty() && lines.len() <= 200 {
        let log_hits = lines.iter().filter(|l| log_line(l)).count();
        if log_hits as f64 / lines.len() as f64 > 0.5 {
            return ContentType::Log;
        }
    }

    // Search results: `path:line:...` or `path:line: ` — the token before the
    // first colon is a path-like run with no whitespace.
    let search_hits = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            if t.contains(':') && !t.starts_with("http") && t.len() <= 300 {
                let prefix = t.split(':').next().unwrap_or("");
                // Path-like: letter/digit/dot/slash/hyphen/underscore, no spaces,
                // and enough length to look like a file path, plus a line number
                // or colon-colon separator after it.
                let pathish = !prefix.is_empty()
                    && !prefix.chars().any(|c| c.is_whitespace())
                    && prefix
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '.' | '/' | '-' | '_' | '~'));
                if pathish {
                    let rest = &t[prefix.len()..];
                    // `:N:` or `:N ` (line-numbered search hit) is the strong signal.
                    rest.starts_with(':') && {
                        let after = rest
                            .trim_start_matches(':')
                            .split(|c: char| c == ':' || c.is_whitespace())
                            .next()
                            .unwrap_or("");
                        after.chars().all(|c| c.is_ascii_digit())
                    }
                } else {
                    false
                }
            } else {
                false
            }
        })
        .count();
    if !lines.is_empty() && search_hits as f64 / lines.len() as f64 > 0.5 {
        return ContentType::SearchResult;
    }

    // Tabular: pipe/comma separated, uniform short lines.
    if lines.len() >= 2 {
        let with_pipes = lines.iter().filter(|l| l.trim().contains('|')).count();
        let with_commas = lines
            .iter()
            .filter(|l| l.trim().matches(',').count() >= 3)
            .count();
        if with_pipes as f64 / lines.len() as f64 > 0.7
            || with_commas as f64 / lines.len() as f64 > 0.6
        {
            return ContentType::Tabular;
        }
    }

    // Code: majority of lines show code signals.
    if !lines.is_empty() {
        let code_hits = lines.iter().filter(|l| code_signal(l)).count();
        if code_hits as f64 / lines.len() as f64 > 0.4 {
            return ContentType::Code;
        }
    }

    ContentType::Text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_code_and_skips_it() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        assert_eq!(detect(code), ContentType::Code);
        assert!(ContentType::Code.is_action_sensitive());
    }

    #[test]
    fn detects_cli_and_skips_it() {
        let cli = "git commit -m \"fix\";
cd /tmp
npm run test";
        assert_eq!(detect(cli), ContentType::Code);
        assert!(detect(cli).is_action_sensitive());
    }

    #[test]
    fn detects_json() {
        assert_eq!(detect(r#"{"a":1,"b":[1,2,3]}"#), ContentType::Json);
        assert!(ContentType::Json.is_action_sensitive());
    }

    #[test]
    fn auto_detects_prose() {
        assert_eq!(
            detect("hello, can you please check this thing for me?"),
            ContentType::Text
        );
        assert!(!ContentType::Text.is_action_sensitive());
    }

    #[test]
    fn detects_diff() {
        let d = "diff --git a/x b/x\nindex 000..111\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(detect(d), ContentType::Diff);
    }
}
