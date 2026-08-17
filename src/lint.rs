//! Shared document/transcript communication-rule linter.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub rule: String,
    pub message: String,
    pub heuristic: bool,
    pub line: Option<usize>,
}
fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.len() > 2)
        .collect()
}
fn without_verbatim(s: &str) -> String {
    let mut out = String::new();
    let mut tick = false;
    for c in s.chars() {
        if c == '`' {
            tick = !tick;
            out.push(' ')
        } else if !tick {
            out.push(c)
        } else {
            out.push(' ')
        }
    }
    out
}
pub fn lint_document(input: &str) -> Vec<Finding> {
    let mut f = Vec::new();
    let lines: Vec<_> = input.lines().collect();
    if lines
        .first()
        .map(|x| x.to_lowercase())
        .unwrap_or_default()
        .contains("here's")
        || lines
            .first()
            .map(|x| x.to_lowercase())
            .unwrap_or_default()
            .starts_with("sure")
    {
        f.push(Finding {
            rule: "B2".into(),
            message: "preamble on first line".into(),
            heuristic: false,
            line: Some(1),
        });
    }
    let low = input.to_lowercase();
    for phrase in ["got it", "sounds good", "great question", "thanks for"] {
        if low.contains(phrase) {
            f.push(Finding {
                rule: "B4".into(),
                message: format!("acknowledgment noise: {phrase}"),
                heuristic: false,
                line: None,
            });
        }
    }
    let hedge = ["perhaps", "maybe", "might", "possibly", "i think"];
    let n = hedge.iter().filter(|x| low.contains(**x)).count();
    if n >= 3 {
        f.push(Finding {
            rule: "R8".into(),
            message: "high hedge density".into(),
            heuristic: false,
            line: None,
        });
    }
    if !lines.iter().any(|x| {
        [".", "?", "x"]
            .iter()
            .any(|t| x.trim_start().starts_with(t))
    }) {
        f.push(Finding {
            rule: "G1".into(),
            message: "missing typed terminal".into(),
            heuristic: false,
            line: None,
        });
    }
    let actionable = ["must", "need to", "should", "fix", "change", "run"];
    if lines
        .iter()
        .any(|x| actionable.iter().any(|a| x.to_lowercase().contains(a)))
        && !["blocking:", "non-blocking:", "warning:", "suggestion:"]
            .iter()
            .any(|x| low.contains(x))
    {
        f.push(Finding {
            rule: "R3".into(),
            message: "actionable statement lacks severity label".into(),
            heuristic: false,
            line: None,
        });
    }
    for (i, l) in lines.iter().enumerate() {
        let clean = without_verbatim(l);
        if clean.trim().is_empty() || l.trim_start().starts_with('`') {
            continue;
        }
        let ws = words(&clean);
        if ws.len() >= 4
            && !ws.iter().any(|w| {
                [
                    "is", "are", "was", "were", "be", "been", "run", "add", "use", "fix", "check",
                    "pass", "fails", "failed", "works", "will", "can", "has", "have", "need",
                ]
                .contains(&w.as_str())
            })
        {
            f.push(Finding {
                rule: "B5".into(),
                message: "retained sentence may lack a finite verb (heuristic)".into(),
                heuristic: true,
                line: Some(i + 1),
            });
        }
    }
    f
}
pub fn lint_transcript(records: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut prior = HashSet::new();
    for (i, r) in records.iter().enumerate() {
        let cur = words(&without_verbatim(r));
        let set: HashSet<_> = cur.iter().cloned().collect();
        if i > 0 && !prior.is_empty() && set.intersection(&prior).count() >= 3 {
            out.push(Finding {
                rule: "B3/G2".into(),
                message: "overlap with prior turn (heuristic)".into(),
                heuristic: true,
                line: Some(i + 1),
            });
        }
        out.extend(lint_document(r));
        prior = set;
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verbatim_is_excluded() {
        assert!(
            lint_document("`opaque_identifier` has value\n.")
                .iter()
                .all(|f| f.rule != "B5")
        );
    }
    #[test]
    fn heuristic_label() {
        assert!(
            lint_document("Thing unusual surprising outcome\n.")
                .iter()
                .any(|f| f.heuristic)
        );
    }
}
