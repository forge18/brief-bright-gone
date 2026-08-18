//! Shared document/transcript communication-rule linter.
//!
//! Rules operate on the model's raw sigil output, not decoded Markdown
//! (design.md §5.8): the raw form is the model's actual output and stays
//! stable if the decode mapping ever changes. `lint_document` is meant to run
//! only on assistant turns — the rules (typed terminal, severity labels, ...)
//! are contracts on sigil-formatted model output, not on user prose.
use crate::sigil;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

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

fn is_structured_sigil_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('|')
        || line.starts_with('-')
        || ['§', '>', '!', '~', '.', '?', 'x']
            .iter()
            .any(|marker| sigil::marker_body(line, *marker).is_some())
}

/// Find a deliberately narrow parallel-prose shape: three substantial prose
/// lines sharing their first two content words. It is a suggestion, never a
/// correctness claim; raw sigil lines and verbatim/fenced text are excluded.
fn parallel_prose_line(lines: &[&str]) -> Option<usize> {
    let mut prefixes = BTreeMap::<(String, String), Vec<usize>>::new();
    let mut in_fence = false;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || is_structured_sigil_line(line) {
            continue;
        }
        let cleaned = without_verbatim(line);
        let words = words(&cleaned);
        if words.len() < 4 {
            continue;
        }
        prefixes
            .entry((words[0].clone(), words[1].clone()))
            .or_default()
            .push(index + 1);
    }
    prefixes
        .into_values()
        .find(|line_numbers| line_numbers.len() >= 3)
        .and_then(|line_numbers| line_numbers.into_iter().next())
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
    // Raw-sigil terminal detection (`.`/`?`/`x`, line-initial, marker-body
    // aware) — shared with the decoder so this can't drift from what actually
    // parses as a terminal.
    if !lines.iter().any(|line| sigil::is_terminal_line(line)) {
        f.push(Finding {
            rule: "G1".into(),
            message: "missing typed terminal".into(),
            heuristic: false,
            line: None,
        });
    }
    // Severity labels are the raw `!` (blocking) and `~` (note) markers, not
    // decoded prose — R3 checks for those markers directly.
    let actionable = ["must", "need to", "should", "fix", "change", "run"];
    let has_severity_marker = lines.iter().any(|line| {
        sigil::marker_body(line, '!').is_some() || sigil::marker_body(line, '~').is_some()
    });
    if lines
        .iter()
        .any(|x| actionable.iter().any(|a| x.to_lowercase().contains(a)))
        && !has_severity_marker
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
    if let Some(line) = parallel_prose_line(&lines) {
        f.push(Finding {
            rule: "F4".into(),
            message: "parallel prose may be clearer as bullets (heuristic)".into(),
            heuristic: true,
            line: Some(line),
        });
    }
    f
}
/// Transcript-level checks: the single-document rules plus turn-to-turn
/// overlap (B3/G2), which needs history a lone document doesn't have. Both
/// only run on assistant turns — user turns are prose, not sigil output, and
/// "prior turn" for overlap means the agent's own last turn, not whatever
/// record precedes it (a user turn in between would make an overlap check
/// against user prose meaningless).
pub fn lint_transcript(records: &[crate::transcript::TranscriptRecord]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut prior: Option<HashSet<String>> = None;
    for (i, record) in records.iter().enumerate() {
        if record.role != "assistant" {
            continue;
        }
        let cur = words(&without_verbatim(&record.content));
        let set: HashSet<_> = cur.iter().cloned().collect();
        if let Some(prior_set) = &prior
            && !prior_set.is_empty()
            && set.intersection(prior_set).count() >= 3
        {
            out.push(Finding {
                rule: "B3/G2".into(),
                message: "overlap with prior assistant turn (heuristic)".into(),
                heuristic: true,
                line: Some(i + 1),
            });
        }
        out.extend(lint_document(&record.content));
        prior = Some(set);
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

    #[test]
    fn f4_suggests_bullets_only_for_parallel_raw_prose() {
        let prose = "Cache reads retain stable prefixes.\nCache reads reduce repeated input cost.\nCache reads expose prefix churn.\n. done";
        let finding = lint_document(prose)
            .into_iter()
            .find(|finding| finding.rule == "F4")
            .expect("parallel prose should be measured");
        assert!(finding.heuristic);
        assert_eq!(finding.line, Some(1));

        for structured in [
            "- Cache reads retain stable prefixes.\n- Cache reads reduce repeated input cost.\n- Cache reads expose prefix churn.\n. done",
            "```text\nCache reads retain stable prefixes.\nCache reads reduce repeated input cost.\nCache reads expose prefix churn.\n```\n. done",
            "`Cache reads retain stable prefixes`\n`Cache reads reduce repeated input cost`\n`Cache reads expose prefix churn`\n. done",
        ] {
            assert!(
                lint_document(structured)
                    .iter()
                    .all(|finding| finding.rule != "F4"),
                "structured/verbatim content must not receive F4: {structured}"
            );
        }
    }

    #[test]
    fn g1_recognizes_a_raw_sigil_terminal_and_fires_on_decoded_markdown() {
        // Raw sigil terminal: recognized, no G1 finding.
        assert!(
            lint_document("§ Status\n. done")
                .iter()
                .all(|f| f.rule != "G1")
        );
        // The decoded form of the same response has no line-initial `.`/`?`/`x`
        // — `**Done.**` starts with `*` — so G1 correctly fires on decoded
        // input. This is exactly why the passive linter must run on raw
        // content, not the post-decode rewrite: linting decoded output would
        // false-positive G1 on every compliant response.
        assert!(
            lint_document("## Status\n**Done.** done")
                .iter()
                .any(|f| f.rule == "G1")
        );
    }

    #[test]
    fn r3_requires_a_raw_severity_marker_not_the_decoded_word() {
        // Actionable content with a raw `!` blocking marker: no R3 finding.
        assert!(
            lint_document("! must fix the config\n. done")
                .iter()
                .all(|f| f.rule != "R3")
        );
        // A raw `~` note marker also satisfies R3.
        assert!(
            lint_document("~ should run the migration\n. done")
                .iter()
                .all(|f| f.rule != "R3")
        );
        // Actionable content with no marker at all: R3 fires.
        assert!(
            lint_document("must fix the config\n. done")
                .iter()
                .any(|f| f.rule == "R3")
        );
        // The literal English word "blocking:" is not a severity label in the
        // raw grammar — only the `!`/`~` markers are — so it does not suppress
        // R3 on its own.
        assert!(
            lint_document("blocking: must fix the config\n. done")
                .iter()
                .any(|f| f.rule == "R3")
        );
    }

    fn record(role: &str, content: &str) -> crate::transcript::TranscriptRecord {
        crate::transcript::TranscriptRecord::new(
            "t".into(),
            "s".into(),
            role.into(),
            content.into(),
            None,
        )
    }

    #[test]
    fn lint_transcript_skips_user_turns_for_document_rules() {
        let records = vec![
            record("user", "please fix the config, no terminal here"),
            record("assistant", "§ Status\n. done"),
        ];
        let findings = lint_transcript(&records);
        // The user turn has no typed terminal and an unlabeled actionable verb,
        // but it must not produce G1/R3 — those rules are contracts on the
        // model's sigil output, not on user prose.
        assert!(findings.is_empty(), "user turn linted: {findings:?}");
    }

    #[test]
    fn lint_transcript_still_flags_a_noncompliant_assistant_turn() {
        let records = vec![
            record("user", "hello"),
            record("assistant", "no terminal in this response at all"),
        ];
        let findings = lint_transcript(&records);
        assert!(findings.iter().any(|f| f.rule == "G1"));
    }

    #[test]
    fn overlap_check_compares_against_the_prior_assistant_turn_across_a_user_turn() {
        let records = vec![
            record(
                "assistant",
                "unusual overlapping repeated wording here\n. done",
            ),
            record("user", "totally different unrelated reply"),
            record(
                "assistant",
                "unusual overlapping repeated wording again\n. done",
            ),
        ];
        let findings = lint_transcript(&records);
        assert!(
            findings.iter().any(|f| f.rule == "B3/G2"),
            "expected overlap with the prior assistant turn, skipping the user turn between them: {findings:?}"
        );
    }
}
