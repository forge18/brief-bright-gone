//! Deterministic, content-free score receipts.
//!
//! Receipts are transcript metadata first. They never supply model-visible
//! request content; an S3 advisory requires separate evidence and wiring.

use crate::detect::{self, ContentType};
use serde::{Deserialize, Serialize};

pub const SIGNAL_RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum SignalReceipt {
    Readiness(ReadinessReceipt),
    Terminal(TerminalReceipt),
    Advisory(AdvisoryReceipt),
    Thrash(ThrashReceipt),
}

/// Content-free contribution to a session's repetition score. Digest and tool
/// identifiers stay in the in-memory session registry; only aggregate counts
/// are persisted with the transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThrashReceipt {
    pub schema_version: u32,
    pub score: u32,
    pub exact_repeated_tool_results: u32,
    pub expensive_exact_repeated_tool_results: u32,
    pub wire_cheap_exact_repeated_tool_results: u32,
    pub near_repeated_tool_calls: u32,
    pub edit_fail_edit_cycles: u32,
}

pub fn thrash_receipt(observation: crate::session::ThrashObservation) -> SignalReceipt {
    SignalReceipt::Thrash(ThrashReceipt {
        schema_version: SIGNAL_RECEIPT_SCHEMA_VERSION,
        score: observation.score(),
        exact_repeated_tool_results: observation.exact_repeated_tool_results,
        expensive_exact_repeated_tool_results: observation.expensive_exact_repeated_tool_results,
        wire_cheap_exact_repeated_tool_results: observation.wire_cheap_exact_repeated_tool_results,
        near_repeated_tool_calls: observation.near_repeated_tool_calls,
        edit_fail_edit_cycles: observation.edit_fail_edit_cycles,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisoryReceipt {
    pub schema_version: u32,
    pub arm: AdvisoryArm,
    pub variant: ReadinessVariant,
    pub template_version: u32,
    pub injected: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryArm {
    RecordOnly,
    Inject,
}

/// The sole model-visible S3 template. Its only variable is a bounded numeric
/// score; it must never include source or provider-derived text.
pub fn advisory_text(score: u8) -> String {
    format!("[bbg-readiness-v1 score={}]", score.min(100))
}

pub fn readiness_score(receipts: &[SignalReceipt], variant: ReadinessVariant) -> Option<u8> {
    receipts.iter().find_map(|receipt| match receipt {
        SignalReceipt::Readiness(receipt) if receipt.variant == variant => Some(receipt.score),
        SignalReceipt::Readiness(_)
        | SignalReceipt::Terminal(_)
        | SignalReceipt::Advisory(_)
        | SignalReceipt::Thrash(_) => None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalReceipt {
    pub schema_version: u32,
    pub state: TerminalState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Done,
    Decision,
    Blocked,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessReceipt {
    pub schema_version: u32,
    pub variant: ReadinessVariant,
    /// Whether this request contains exactly one user message and no assistant
    /// messages. Absent in legacy receipts, which Stage-1 analysis excludes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_start: Option<bool>,
    /// Applicable-slot coverage. This is an observation, not a probability.
    pub score: u8,
    pub slots: ReadinessSlots,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<ReadinessReason>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessVariant {
    RawComposite,
    TrailingProse,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    Present,
    Missing,
    NotApplicable,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessSlots {
    pub task: SlotState,
    pub target_artifact: SlotState,
    pub success_criterion: SlotState,
    pub constraints: SlotState,
    pub environment_reproduction: SlotState,
    pub unresolved_unknowns: SlotState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessReason {
    NoTask,
    NoTargetArtifact,
    NoSuccessCriterion,
    NoConstraints,
    NoEnvironmentReproduction,
    UnresolvedUnknowns,
    TrailingProseFallback,
}

/// Classify the final typed terminal using the same raw-sigil grammar as the
/// decoder and linter. This receipt never includes the terminal's body.
pub fn terminal_receipt(content: &str) -> SignalReceipt {
    let state = content
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| {
            crate::sigil::marker_body(line, '.')
                .map(|_| TerminalState::Done)
                .or_else(|| crate::sigil::marker_body(line, '?').map(|_| TerminalState::Decision))
                .or_else(|| crate::sigil::marker_body(line, 'x').map(|_| TerminalState::Blocked))
        })
        .unwrap_or(TerminalState::Missing);
    SignalReceipt::Terminal(TerminalReceipt {
        schema_version: SIGNAL_RECEIPT_SCHEMA_VERSION,
        state,
    })
}

/// Produce both Stage-1 readiness variants without a request-history claim.
/// Reports exclude these legacy-compatible receipts until the caller provides
/// an explicit conversation-start value.
pub fn readiness_receipts(content: &str) -> Vec<SignalReceipt> {
    readiness_receipts_with_start(content, None)
}

/// Produce both Stage-1 readiness variants with a content-free request-history
/// marker. A conversation start has exactly one user message and no assistant
/// messages in the provider request.
pub fn readiness_receipts_for_conversation(
    content: &str,
    conversation_start: bool,
) -> Vec<SignalReceipt> {
    readiness_receipts_with_start(content, Some(conversation_start))
}

fn readiness_receipts_with_start(
    content: &str,
    conversation_start: Option<bool>,
) -> Vec<SignalReceipt> {
    vec![
        SignalReceipt::Readiness(readiness(
            content,
            ReadinessVariant::RawComposite,
            false,
            conversation_start,
        )),
        SignalReceipt::Readiness(trailing_prose_receipt(content, conversation_start)),
    ]
}

fn trailing_prose_receipt(content: &str, conversation_start: Option<bool>) -> ReadinessReceipt {
    match trailing_prose(content) {
        Some(prose) => readiness(
            prose,
            ReadinessVariant::TrailingProse,
            false,
            conversation_start,
        ),
        None => readiness(
            content,
            ReadinessVariant::TrailingProse,
            true,
            conversation_start,
        ),
    }
}

fn readiness(
    content: &str,
    variant: ReadinessVariant,
    fallback: bool,
    conversation_start: Option<bool>,
) -> ReadinessReceipt {
    let lower = content.to_ascii_lowercase();
    let task = slot(has_task(&lower));
    let target_artifact = slot(has_target_artifact(&lower));
    let success_criterion = slot(has_success_criterion(&lower));
    let constraints = slot(has_constraints(&lower));
    let environment_reproduction = environment_slot(&lower);
    let unresolved_unknowns = if has_unresolved_unknowns(&lower) {
        SlotState::Unresolved
    } else {
        SlotState::NotApplicable
    };
    let slots = ReadinessSlots {
        task,
        target_artifact,
        success_criterion,
        constraints,
        environment_reproduction,
        unresolved_unknowns,
    };
    let mut reasons = missing_reasons(&slots);
    if fallback {
        reasons.push(ReadinessReason::TrailingProseFallback);
    }
    ReadinessReceipt {
        schema_version: SIGNAL_RECEIPT_SCHEMA_VERSION,
        variant,
        conversation_start,
        score: coverage(&slots),
        slots,
        reasons,
    }
}

fn trailing_prose(content: &str) -> Option<&str> {
    // `detect` is intentionally conservative. A mixed block is treated as
    // structured and skipped; Stage 1 compares this variant with raw input.
    content
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty() && detect::detect(block) == ContentType::Text)
        .last()
}

fn slot(present: bool) -> SlotState {
    if present {
        SlotState::Present
    } else {
        SlotState::Missing
    }
}

fn has_task(lower: &str) -> bool {
    lower.contains('?')
        || lower.contains("please ")
        || lower.starts_with("fix ")
        || lower.starts_with("add ")
        || lower.starts_with("implement ")
        || lower.starts_with("update ")
        || lower.starts_with("remove ")
        || lower.starts_with("refactor ")
        || lower.starts_with("explain ")
        || lower.contains("can you ")
        || lower.contains("could you ")
        || lower.contains("i need ")
        || lower.contains("i want ")
}

fn has_target_artifact(lower: &str) -> bool {
    lower.contains("file")
        || lower.contains("repo")
        || lower.contains("repository")
        || lower.contains("component")
        || lower.contains("module")
        || lower.contains("function")
        || lower.contains("endpoint")
        || lower.contains("api")
        || lower.contains("codebase")
        || lower.contains("test")
        || lower.contains('/')
        || lower.contains(".rs")
        || lower.contains(".ts")
        || lower.contains(".py")
}

fn has_success_criterion(lower: &str) -> bool {
    lower.contains("acceptance")
        || lower.contains("done")
        || lower.contains("expected")
        || lower.contains("should ")
        || lower.contains("must ")
        || lower.contains("pass")
        || lower.contains("verify")
        || lower.contains("working")
}

fn has_constraints(lower: &str) -> bool {
    lower.contains("do not")
        || lower.contains("don't")
        || lower.contains("must not")
        || lower.contains("only ")
        || lower.contains("without")
        || lower.contains("avoid")
        || lower.contains("preserve")
        || lower.contains("compatible")
        || lower.contains("compatibility")
        || lower.contains("keep ")
}

fn environment_slot(lower: &str) -> SlotState {
    let debugging = [
        "bug",
        "error",
        "fail",
        "crash",
        "regression",
        "reproduce",
        "stack trace",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !debugging {
        return SlotState::NotApplicable;
    }
    let evidence = lower.contains("error:")
        || lower.contains("stack trace")
        || lower.contains("reproduce")
        || lower.contains("version")
        || lower.contains(" macos")
        || lower.contains(" linux")
        || lower.contains(" windows")
        || lower.chars().any(|character| character.is_ascii_digit());
    slot(evidence)
}

fn has_unresolved_unknowns(lower: &str) -> bool {
    ["not sure", "unknown", "unclear", "don't know", "unsure"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn missing_reasons(slots: &ReadinessSlots) -> Vec<ReadinessReason> {
    let mut reasons = Vec::new();
    if slots.task == SlotState::Missing {
        reasons.push(ReadinessReason::NoTask);
    }
    if slots.target_artifact == SlotState::Missing {
        reasons.push(ReadinessReason::NoTargetArtifact);
    }
    if slots.success_criterion == SlotState::Missing {
        reasons.push(ReadinessReason::NoSuccessCriterion);
    }
    if slots.constraints == SlotState::Missing {
        reasons.push(ReadinessReason::NoConstraints);
    }
    if slots.environment_reproduction == SlotState::Missing {
        reasons.push(ReadinessReason::NoEnvironmentReproduction);
    }
    if slots.unresolved_unknowns == SlotState::Unresolved {
        reasons.push(ReadinessReason::UnresolvedUnknowns);
    }
    reasons
}

fn coverage(slots: &ReadinessSlots) -> u8 {
    let states = [
        slots.task,
        slots.target_artifact,
        slots.success_criterion,
        slots.constraints,
        slots.environment_reproduction,
    ];
    let applicable: Vec<_> = states
        .into_iter()
        .filter(|state| *state != SlotState::NotApplicable)
        .collect();
    if applicable.is_empty() {
        return 0;
    }
    let present = applicable
        .iter()
        .filter(|state| **state == SlotState::Present)
        .count();
    (present * 100 / applicable.len()) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_raw_and_trailing_prose_without_copying_prompt_text() {
        let prompt = "Fix src/signals.rs. Preserve I4. Done means cargo test passes.\n\n```text\nsecret-path=/private/value\n```\n\nDo not change the API.";
        let receipts = readiness_receipts(prompt);
        assert_eq!(receipts.len(), 2);
        let encoded = serde_json::to_string(&receipts).unwrap();
        assert!(!encoded.contains("secret-path"));
        assert!(!encoded.contains("/private/value"));
        assert!(encoded.contains("raw_composite"));
        assert!(encoded.contains("trailing_prose"));
    }

    #[test]
    fn trailing_prose_falls_back_when_no_text_segment_survives() {
        let receipts = readiness_receipts("```rust\nfn main() {}\n```");
        let SignalReceipt::Readiness(trailing) = &receipts[1] else {
            panic!("expected readiness receipt");
        };
        assert_eq!(trailing.variant, ReadinessVariant::TrailingProse);
        assert!(
            trailing
                .reasons
                .contains(&ReadinessReason::TrailingProseFallback)
        );
    }

    #[test]
    fn marks_debug_environment_missing_without_evidence() {
        let SignalReceipt::Readiness(receipt) = &readiness_receipts("Fix the bug in the file")[0]
        else {
            panic!("expected readiness receipt");
        };
        assert_eq!(receipt.slots.environment_reproduction, SlotState::Missing);
        assert!(
            receipt
                .reasons
                .contains(&ReadinessReason::NoEnvironmentReproduction)
        );
    }

    #[test]
    fn score_is_deterministic() {
        let input =
            "Implement the API in src/lib.rs. Preserve compatibility. Done when tests pass.";
        assert_eq!(readiness_receipts(input), readiness_receipts(input));
    }

    #[test]
    fn advisory_template_is_fixed_and_score_is_bounded() {
        assert_eq!(advisory_text(42), "[bbg-readiness-v1 score=42]");
        assert_eq!(advisory_text(u8::MAX), "[bbg-readiness-v1 score=100]");
        let receipts = readiness_receipts("Fix src/lib.rs. Done when tests pass.");
        assert_eq!(
            readiness_score(&receipts, ReadinessVariant::RawComposite),
            Some(75)
        );
    }

    #[test]
    fn thrash_receipt_keeps_only_aggregate_counts() {
        let receipt = thrash_receipt(crate::session::ThrashObservation {
            exact_repeated_tool_results: 1,
            expensive_exact_repeated_tool_results: 1,
            near_repeated_tool_calls: 2,
            edit_fail_edit_cycles: 1,
            ..Default::default()
        });
        let SignalReceipt::Thrash(receipt) = receipt else {
            panic!("expected thrash receipt");
        };
        assert_eq!(receipt.score, 4);
        assert_eq!(receipt.expensive_exact_repeated_tool_results, 1);
        assert_eq!(receipt.wire_cheap_exact_repeated_tool_results, 0);
        assert!(!serde_json::to_string(&receipt).unwrap().contains("digest"));
    }

    #[test]
    fn terminal_receipt_uses_final_sigil_line_without_retaining_its_body() {
        assert_eq!(
            terminal_receipt("§ Status\n. done after private-reason"),
            SignalReceipt::Terminal(TerminalReceipt {
                schema_version: SIGNAL_RECEIPT_SCHEMA_VERSION,
                state: TerminalState::Done,
            })
        );
        let encoded = serde_json::to_string(&terminal_receipt("x secret cause")).unwrap();
        assert!(!encoded.contains("secret cause"));
        assert!(matches!(
            terminal_receipt("? decision; options: a, b"),
            SignalReceipt::Terminal(TerminalReceipt {
                state: TerminalState::Decision,
                ..
            })
        ));
        assert!(matches!(
            terminal_receipt("no terminal"),
            SignalReceipt::Terminal(TerminalReceipt {
                state: TerminalState::Missing,
                ..
            })
        ));
    }
}
