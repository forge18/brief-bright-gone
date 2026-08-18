//! Deterministic benchmark report aggregation from transcript and cost records.
use crate::{
    operations::CostRecord,
    signals::{
        AdvisoryArm, ReadinessReceipt, ReadinessSlots, ReadinessVariant, SignalReceipt, SlotState,
        TerminalState, ThrashReceipt,
    },
    transcript::TranscriptRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub turns: usize,
    pub lint_findings: usize,
    pub heuristic_findings: usize,
    /// Sum of `observed_billing_usd` from cost records whose `session_id`
    /// matches a session present in `records`. Cost records with no session id
    /// (written before that field existed) or belonging to a session absent
    /// from `records` are excluded, since the report scopes cost to the
    /// sessions it was actually asked to summarize.
    pub cost_usd: f64,
    /// Cost records that carried usage but were excluded from `cost_usd`
    /// because their session id did not match any transcript session (or was
    /// absent) — surfaced so a report never silently under-counts without a
    /// visible reason.
    pub cost_records_excluded: usize,
    pub confidence_note: String,
}

pub fn report(records: &[TranscriptRecord], costs: &[CostRecord]) -> BenchmarkReport {
    let findings = records.iter().flat_map(|r| r.lint.iter());
    let all: Vec<_> = findings.collect();
    let sessions: HashSet<&str> = records.iter().map(|r| r.session_id.as_str()).collect();
    let (matched, excluded): (Vec<_>, Vec<_>) = costs.iter().partition(|record| {
        record
            .session_id
            .as_deref()
            .is_some_and(|id| sessions.contains(id))
    });
    let cost_usd = matched
        .iter()
        .filter_map(|record| record.observed_billing_usd)
        .sum::<f64>()
        .max(0.0);
    BenchmarkReport {
        schema_version: 1,
        turns: records.len(),
        lint_findings: all.len(),
        heuristic_findings: all.iter().filter(|f| f.heuristic).count(),
        cost_usd,
        cost_records_excluded: excluded.len(),
        confidence_note: "Confidence requires repeated paired runs; this report contains observed transcript and cost-ledger counts only.".into(),
    }
}

/// Manually supplied benchmark label. Omitted fields stay unknown; reports
/// never infer a task outcome from terminal text or model self-report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskOutcomeLabel {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correct: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessObservation {
    pub session_id: String,
    pub variant: ReadinessVariant,
    pub score: u8,
    pub turns: usize,
    pub observed_billing_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TaskOutcomeLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessBucket {
    pub variant: ReadinessVariant,
    pub score: u8,
    pub sessions: usize,
    pub observed_billing_usd: f64,
    pub labelled_sessions: usize,
    pub completed_sessions: usize,
    pub correct_sessions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessReport {
    pub schema_version: u32,
    pub observations: Vec<ReadinessObservation>,
    pub buckets: Vec<ReadinessBucket>,
    pub unlabelled_sessions: usize,
    pub confidence_note: String,
}

/// A conservative Stage-1 conclusion. `stop_*` is a successful safety outcome:
/// no advisory may be enabled without a held-out winner.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessAnalysisDecision {
    SelectRawComposite,
    SelectTrailingProse,
    StopInsufficientEvidence,
    StopNoPredictiveVariant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessHeldOutVariant {
    pub variant: ReadinessVariant,
    pub held_out_sessions: usize,
    pub labelled_sessions: usize,
    pub completed_sessions: usize,
    pub correct_sessions: usize,
    pub observed_billing_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billed_dollars_per_completed_task: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_correctness_correlation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_cost_correlation: Option<f64>,
    pub diagnostic_mean_turns: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessHeldOutAnalysis {
    pub schema_version: u32,
    pub decision: ReadinessAnalysisDecision,
    pub training_sessions: usize,
    pub held_out_sessions: usize,
    pub labelled_held_out_sessions: usize,
    pub variants: Vec<ReadinessHeldOutVariant>,
    pub confidence_note: String,
}

/// Join the first scored user turn per session with cost and optional manual
/// outcome labels. This reports association inputs; it does not claim that a
/// score is predictive without held-out analysis.
pub fn readiness_report(
    records: &[TranscriptRecord],
    costs: &[CostRecord],
    labels: &[TaskOutcomeLabel],
) -> ReadinessReport {
    let mut turns = HashMap::<&str, usize>::new();
    let mut first_scored = BTreeMap::<String, Vec<ReadinessReceipt>>::new();
    for record in records {
        *turns.entry(&record.session_id).or_default() += 1;
        if record.role != "user" || first_scored.contains_key(&record.session_id) {
            continue;
        }
        let receipts = record
            .receipts
            .iter()
            .filter_map(|receipt| match receipt {
                SignalReceipt::Readiness(receipt) if receipt.conversation_start == Some(true) => {
                    Some(receipt.clone())
                }
                SignalReceipt::Readiness(_)
                | SignalReceipt::Terminal(_)
                | SignalReceipt::Advisory(_)
                | SignalReceipt::Thrash(_) => None,
            })
            .collect::<Vec<_>>();
        if !receipts.is_empty() {
            first_scored.insert(record.session_id.clone(), receipts);
        }
    }

    let mut costs_by_session = HashMap::<&str, f64>::new();
    for cost in costs {
        if let Some(session_id) = cost.session_id.as_deref() {
            *costs_by_session.entry(session_id).or_default() +=
                cost.observed_billing_usd.unwrap_or(0.0).max(0.0);
        }
    }
    let labels_by_session: HashMap<&str, &TaskOutcomeLabel> = labels
        .iter()
        .map(|label| (label.session_id.as_str(), label))
        .collect();

    let mut observations = Vec::new();
    for (session_id, receipts) in first_scored {
        for receipt in receipts {
            observations.push(ReadinessObservation {
                turns: turns.get(session_id.as_str()).copied().unwrap_or_default(),
                observed_billing_usd: costs_by_session
                    .get(session_id.as_str())
                    .copied()
                    .unwrap_or_default(),
                outcome: labels_by_session.get(session_id.as_str()).cloned().cloned(),
                session_id: session_id.clone(),
                variant: receipt.variant,
                score: receipt.score,
            });
        }
    }

    let mut buckets = BTreeMap::<(ReadinessVariant, u8), ReadinessBucket>::new();
    for observation in &observations {
        let bucket = buckets
            .entry((observation.variant, observation.score))
            .or_insert(ReadinessBucket {
                variant: observation.variant,
                score: observation.score,
                sessions: 0,
                observed_billing_usd: 0.0,
                labelled_sessions: 0,
                completed_sessions: 0,
                correct_sessions: 0,
            });
        bucket.sessions += 1;
        bucket.observed_billing_usd += observation.observed_billing_usd;
        if let Some(outcome) = &observation.outcome {
            bucket.labelled_sessions += 1;
            bucket.completed_sessions += usize::from(outcome.completed == Some(true));
            bucket.correct_sessions += usize::from(outcome.correct == Some(true));
        }
    }
    let unlabelled_sessions = observations
        .iter()
        .filter(|observation| observation.outcome.is_none())
        .map(|observation| observation.session_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    ReadinessReport {
        schema_version: 1,
        observations,
        buckets: buckets.into_values().collect(),
        unlabelled_sessions,
        confidence_note: "Association only: use held-out labelled outcomes before selecting a readiness variant or enabling injection.".into(),
    }
}

/// Run the deterministic Stage-1 held-out comparison. The split is stable by
/// session id (20% held out), so rerunning the same ledger produces the same
/// conclusion. Fewer than eight fully labelled held-out sessions stop safely.
pub fn readiness_held_out_analysis(
    records: &[TranscriptRecord],
    costs: &[CostRecord],
    labels: &[TaskOutcomeLabel],
) -> ReadinessHeldOutAnalysis {
    const MIN_LABELLED_HELD_OUT: usize = 8;
    let report = readiness_report(records, costs, labels);
    let mut held_out = Vec::new();
    let mut training_sessions = HashSet::new();
    for observation in report.observations {
        if is_held_out(&observation.session_id) {
            held_out.push(observation);
        } else {
            training_sessions.insert(observation.session_id.clone());
        }
    }
    let held_out_sessions = held_out
        .iter()
        .map(|observation| observation.session_id.as_str())
        .collect::<HashSet<_>>();
    let labelled_held_out_sessions = held_out_sessions
        .iter()
        .filter(|session_id| {
            held_out.iter().any(|observation| {
                observation.session_id == **session_id
                    && observation.outcome.as_ref().is_some_and(|outcome| {
                        outcome.completed.is_some() && outcome.correct.is_some()
                    })
            })
        })
        .count();
    let variants = [
        ReadinessVariant::RawComposite,
        ReadinessVariant::TrailingProse,
    ]
    .into_iter()
    .map(|variant| held_out_variant(variant, &held_out))
    .collect::<Vec<_>>();
    let decision = if labelled_held_out_sessions < MIN_LABELLED_HELD_OUT {
        ReadinessAnalysisDecision::StopInsufficientEvidence
    } else {
        select_predictive_variant(&variants)
            .unwrap_or(ReadinessAnalysisDecision::StopNoPredictiveVariant)
    };
    ReadinessHeldOutAnalysis {
        schema_version: 1,
        decision,
        training_sessions: training_sessions.len(),
        held_out_sessions: held_out_sessions.len(),
        labelled_held_out_sessions,
        variants,
        confidence_note: "Stage 1 is passive: selection requires held-out score/correctness and score/cost correlation of at least 0.30 in the expected directions. A stop decision forbids advisory injection.".into(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentArm {
    RecordOnly,
    Inject,
}

impl From<AdvisoryArm> for ExperimentArm {
    fn from(arm: AdvisoryArm) -> Self {
        match arm {
            AdvisoryArm::RecordOnly => Self::RecordOnly,
            AdvisoryArm::Inject => Self::Inject,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperimentArmSummary {
    pub arm: ExperimentArm,
    pub assigned_sessions: usize,
    pub eligible_sessions: usize,
    pub completed_sessions: usize,
    pub correct_sessions: usize,
    pub observed_billing_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billed_dollars_per_completed_task: Option<f64>,
    pub diagnostic_mean_turns: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentDecision {
    StopInsufficientEvidence,
    ReviewRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperimentReport {
    pub schema_version: u32,
    pub decision: ExperimentDecision,
    pub excluded_sessions: usize,
    pub arms: Vec<ExperimentArmSummary>,
    pub confidence_note: String,
}

/// Aggregate content-free S3 arm receipts with optional manual labels and cost
/// records. It deliberately never infers correctness from terminals, and never
/// declares a ship decision: reviewed real-provider evidence is required.
pub fn experiment_report(
    records: &[TranscriptRecord],
    costs: &[CostRecord],
    labels: &[TaskOutcomeLabel],
) -> ExperimentReport {
    let mut arms = BTreeMap::<String, Option<ExperimentArm>>::new();
    let mut turns = HashMap::<&str, usize>::new();
    for record in records {
        *turns.entry(&record.session_id).or_default() += 1;
        for receipt in &record.receipts {
            let SignalReceipt::Advisory(receipt) = receipt else {
                continue;
            };
            let arm = ExperimentArm::from(receipt.arm);
            let entry = arms.entry(record.session_id.clone()).or_insert(Some(arm));
            if *entry != Some(arm) {
                *entry = None;
            }
        }
    }
    let labels = labels
        .iter()
        .map(|label| (label.session_id.as_str(), label))
        .collect::<HashMap<_, _>>();
    let mut billed = HashMap::<&str, f64>::new();
    for cost in costs {
        if let Some(session_id) = cost.session_id.as_deref() {
            *billed.entry(session_id).or_default() +=
                cost.observed_billing_usd.unwrap_or_default().max(0.0);
        }
    }
    let mut summaries = BTreeMap::<ExperimentArm, ExperimentArmSummary>::new();
    let mut excluded_sessions = 0;
    for (session_id, arm) in arms {
        let Some(arm) = arm else {
            excluded_sessions += 1;
            continue;
        };
        let summary = summaries.entry(arm).or_insert(ExperimentArmSummary {
            arm,
            assigned_sessions: 0,
            eligible_sessions: 0,
            completed_sessions: 0,
            correct_sessions: 0,
            observed_billing_usd: 0.0,
            billed_dollars_per_completed_task: None,
            diagnostic_mean_turns: 0.0,
        });
        summary.assigned_sessions += 1;
        let Some(label) = labels.get(session_id.as_str()) else {
            excluded_sessions += 1;
            continue;
        };
        let Some(cost) = billed.get(session_id.as_str()) else {
            excluded_sessions += 1;
            continue;
        };
        summary.eligible_sessions += 1;
        summary.completed_sessions += usize::from(label.completed == Some(true));
        summary.correct_sessions += usize::from(label.correct == Some(true));
        summary.observed_billing_usd += *cost;
        summary.diagnostic_mean_turns +=
            turns.get(session_id.as_str()).copied().unwrap_or_default() as f64;
    }
    let mut arms = [ExperimentArm::RecordOnly, ExperimentArm::Inject]
        .into_iter()
        .map(|arm| {
            summaries.remove(&arm).unwrap_or(ExperimentArmSummary {
                arm,
                assigned_sessions: 0,
                eligible_sessions: 0,
                completed_sessions: 0,
                correct_sessions: 0,
                observed_billing_usd: 0.0,
                billed_dollars_per_completed_task: None,
                diagnostic_mean_turns: 0.0,
            })
        })
        .collect::<Vec<_>>();
    for summary in &mut arms {
        if summary.eligible_sessions > 0 {
            summary.diagnostic_mean_turns /= summary.eligible_sessions as f64;
        }
        summary.billed_dollars_per_completed_task = (summary.completed_sessions > 0)
            .then_some(summary.observed_billing_usd / summary.completed_sessions as f64);
    }
    let decision = if arms.iter().all(|arm| arm.eligible_sessions >= 8) {
        ExperimentDecision::ReviewRequired
    } else {
        ExperimentDecision::StopInsufficientEvidence
    };
    ExperimentReport {
        schema_version: 1,
        decision,
        excluded_sessions,
        arms,
        confidence_note: "Correctness and observed billed dollars per completed task are the ship criteria. This report is a review input; turns are diagnostic only.".into(),
    }
}

/// A content-free readiness dimension that a clarification reply can fill.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessSlot {
    Task,
    TargetArtifact,
    SuccessCriterion,
    Constraints,
    EnvironmentReproduction,
    UnresolvedUnknowns,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClarificationObservation {
    pub session_id: String,
    pub question_turn: u64,
    pub reply_turn: u64,
    pub variant: ReadinessVariant,
    pub before_score: u8,
    pub after_score: u8,
    pub score_delta: i16,
    pub filled_slots: Vec<ReadinessSlot>,
    pub resolved_unresolved_unknowns: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_turn_observed_billing_usd: Option<f64>,
    pub reply_turn_cost_records: usize,
    pub reply_turn_priced_cost_records: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TaskOutcomeLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClarificationReport {
    pub schema_version: u32,
    pub observations: Vec<ClarificationObservation>,
    pub unpaired_decision_turns: usize,
    pub incomplete_readiness_pairs: usize,
    pub legacy_records_excluded: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_delta_reply_turn_cost_correlation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_delta_correctness_correlation: Option<f64>,
    pub confidence_note: String,
}

#[derive(Default)]
struct ReplyCost {
    observed_billing_usd: f64,
    records: usize,
    priced_records: usize,
}

/// Measure candidate assistant `?` decision turns against the next user turn
/// in the same local session. The local turn ordinal, rather than JSONL append
/// order or timestamps, defines the causal ordering. Each readiness variant is
/// reported independently; task labels remain session-scoped context only.
pub fn clarification_report(
    records: &[TranscriptRecord],
    costs: &[CostRecord],
    labels: &[TaskOutcomeLabel],
) -> ClarificationReport {
    let mut readiness =
        BTreeMap::<String, BTreeMap<u64, BTreeMap<ReadinessVariant, ReadinessReceipt>>>::new();
    let mut decisions = BTreeMap::<String, BTreeSet<u64>>::new();
    let mut legacy_records_excluded = 0;
    for record in records {
        let Some(session_turn) = record.session_turn else {
            legacy_records_excluded += 1;
            continue;
        };
        if record.role == "user" {
            let receipts = record.receipts.iter().filter_map(|receipt| match receipt {
                SignalReceipt::Readiness(receipt) => Some(receipt.clone()),
                SignalReceipt::Terminal(_)
                | SignalReceipt::Advisory(_)
                | SignalReceipt::Thrash(_) => None,
            });
            let by_variant = readiness
                .entry(record.session_id.clone())
                .or_default()
                .entry(session_turn)
                .or_default();
            for receipt in receipts {
                by_variant.entry(receipt.variant).or_insert(receipt);
            }
        }
        if record.role == "assistant"
            && record.receipts.iter().any(|receipt| {
                matches!(
                    receipt,
                    SignalReceipt::Terminal(receipt) if receipt.state == TerminalState::Decision
                )
            })
        {
            decisions
                .entry(record.session_id.clone())
                .or_default()
                .insert(session_turn);
        }
    }

    let mut costs_by_turn = BTreeMap::<(String, u64), ReplyCost>::new();
    for cost in costs {
        let (Some(session_id), Some(session_turn)) =
            (cost.session_id.as_deref(), cost.session_turn)
        else {
            continue;
        };
        let joined = costs_by_turn
            .entry((session_id.to_owned(), session_turn))
            .or_default();
        joined.records += 1;
        if let Some(billing) = cost
            .observed_billing_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
        {
            joined.priced_records += 1;
            joined.observed_billing_usd += billing;
        }
    }
    let labels = labels
        .iter()
        .map(|label| (label.session_id.as_str(), label))
        .collect::<HashMap<_, _>>();

    let mut observations = Vec::new();
    let mut unpaired_decision_turns = 0;
    let mut incomplete_readiness_pairs = 0;
    for (session_id, question_turns) in decisions {
        let Some(user_turns) = readiness.get(&session_id) else {
            unpaired_decision_turns += question_turns.len();
            continue;
        };
        for question_turn in question_turns {
            let Some((_, before)) = user_turns.range(..=question_turn).next_back() else {
                incomplete_readiness_pairs += 1;
                continue;
            };
            let Some((reply_turn, after)) = user_turns
                .range((
                    std::ops::Bound::Excluded(question_turn),
                    std::ops::Bound::Unbounded,
                ))
                .next()
            else {
                unpaired_decision_turns += 1;
                continue;
            };
            for variant in [
                ReadinessVariant::RawComposite,
                ReadinessVariant::TrailingProse,
            ] {
                let (Some(before), Some(after)) = (before.get(&variant), after.get(&variant))
                else {
                    incomplete_readiness_pairs += 1;
                    continue;
                };
                let (filled_slots, resolved_unresolved_unknowns) =
                    slot_delta(&before.slots, &after.slots);
                let cost = costs_by_turn.get(&(session_id.clone(), *reply_turn));
                observations.push(ClarificationObservation {
                    session_id: session_id.clone(),
                    question_turn,
                    reply_turn: *reply_turn,
                    variant,
                    before_score: before.score,
                    after_score: after.score,
                    score_delta: i16::from(after.score) - i16::from(before.score),
                    filled_slots,
                    resolved_unresolved_unknowns,
                    reply_turn_observed_billing_usd: cost
                        .filter(|cost| cost.priced_records > 0)
                        .map(|cost| cost.observed_billing_usd),
                    reply_turn_cost_records: cost.map(|cost| cost.records).unwrap_or_default(),
                    reply_turn_priced_cost_records: cost
                        .map(|cost| cost.priced_records)
                        .unwrap_or_default(),
                    outcome: labels.get(session_id.as_str()).cloned().cloned(),
                });
            }
        }
    }
    let cost_points = observations
        .iter()
        .filter_map(|observation| {
            observation
                .reply_turn_observed_billing_usd
                .map(|cost| (f64::from(observation.score_delta), cost))
        })
        .collect::<Vec<_>>();
    let correctness_points = observations
        .iter()
        .filter_map(|observation| {
            observation.outcome.as_ref().and_then(|outcome| {
                outcome.correct.map(|correct| {
                    (
                        f64::from(observation.score_delta),
                        if correct { 1.0 } else { 0.0 },
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    ClarificationReport {
        schema_version: 1,
        observations,
        unpaired_decision_turns,
        incomplete_readiness_pairs,
        legacy_records_excluded,
        score_delta_reply_turn_cost_correlation: pearson(&cost_points),
        score_delta_correctness_correlation: pearson(&correctness_points),
        confidence_note: "Decision-to-next-user candidates are diagnostic only. Reply-turn cost is provider-observed when priced; correctness labels are session-scoped and do not establish causation.".into(),
    }
}

fn slot_delta(before: &ReadinessSlots, after: &ReadinessSlots) -> (Vec<ReadinessSlot>, bool) {
    let filled_slots = [
        (ReadinessSlot::Task, before.task, after.task),
        (
            ReadinessSlot::TargetArtifact,
            before.target_artifact,
            after.target_artifact,
        ),
        (
            ReadinessSlot::SuccessCriterion,
            before.success_criterion,
            after.success_criterion,
        ),
        (
            ReadinessSlot::Constraints,
            before.constraints,
            after.constraints,
        ),
        (
            ReadinessSlot::EnvironmentReproduction,
            before.environment_reproduction,
            after.environment_reproduction,
        ),
    ]
    .into_iter()
    .filter_map(|(slot, before, after)| {
        (before == SlotState::Missing && after == SlotState::Present).then_some(slot)
    })
    .collect();
    (
        filled_slots,
        before.unresolved_unknowns == SlotState::Unresolved
            && after.unresolved_unknowns == SlotState::NotApplicable,
    )
}

fn is_held_out(session_id: &str) -> bool {
    // FNV-1a is explicit and stable across Rust releases and processes.
    let hash = session_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    hash % 5 == 0
}

fn held_out_variant(
    variant: ReadinessVariant,
    observations: &[ReadinessObservation],
) -> ReadinessHeldOutVariant {
    let observations = observations
        .iter()
        .filter(|observation| observation.variant == variant)
        .collect::<Vec<_>>();
    let labelled = observations
        .iter()
        .filter(|observation| {
            observation
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.completed.is_some() && outcome.correct.is_some())
        })
        .collect::<Vec<_>>();
    let completed = labelled
        .iter()
        .filter(|observation| observation.outcome.as_ref().unwrap().completed == Some(true))
        .collect::<Vec<_>>();
    let correctness = labelled
        .iter()
        .map(|observation| {
            (
                f64::from(observation.score),
                if observation.outcome.as_ref().unwrap().correct == Some(true) {
                    1.0
                } else {
                    0.0
                },
            )
        })
        .collect::<Vec<_>>();
    let completed_costs = completed
        .iter()
        .map(|observation| {
            (
                f64::from(observation.score),
                observation.observed_billing_usd,
            )
        })
        .collect::<Vec<_>>();
    let observed_billing_usd = observations
        .iter()
        .map(|observation| observation.observed_billing_usd)
        .sum();
    ReadinessHeldOutVariant {
        variant,
        held_out_sessions: observations.len(),
        labelled_sessions: labelled.len(),
        completed_sessions: completed.len(),
        correct_sessions: labelled
            .iter()
            .filter(|observation| observation.outcome.as_ref().unwrap().correct == Some(true))
            .count(),
        observed_billing_usd,
        billed_dollars_per_completed_task: (!completed.is_empty())
            .then_some(observed_billing_usd / completed.len() as f64),
        score_correctness_correlation: pearson(&correctness),
        score_cost_correlation: pearson(&completed_costs),
        diagnostic_mean_turns: if observations.is_empty() {
            0.0
        } else {
            observations
                .iter()
                .map(|observation| observation.turns)
                .sum::<usize>() as f64
                / observations.len() as f64
        },
    }
}

fn select_predictive_variant(
    variants: &[ReadinessHeldOutVariant],
) -> Option<ReadinessAnalysisDecision> {
    let candidate = variants
        .iter()
        .filter_map(|variant| {
            let (correctness, cost) = (
                variant.score_correctness_correlation?,
                variant.score_cost_correlation?,
            );
            (correctness >= 0.30 && cost <= -0.30).then_some((variant.variant, correctness - cost))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))?
        .0;
    Some(match candidate {
        ReadinessVariant::RawComposite => ReadinessAnalysisDecision::SelectRawComposite,
        ReadinessVariant::TrailingProse => ReadinessAnalysisDecision::SelectTrailingProse,
    })
}

fn pearson(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let (mean_x, mean_y) = points.iter().fold((0.0, 0.0), |(x, y), point| {
        (
            x + point.0 / points.len() as f64,
            y + point.1 / points.len() as f64,
        )
    });
    let (covariance, variance_x, variance_y) = points.iter().fold(
        (0.0, 0.0, 0.0),
        |(covariance, variance_x, variance_y), point| {
            let x = point.0 - mean_x;
            let y = point.1 - mean_y;
            (covariance + x * y, variance_x + x * x, variance_y + y * y)
        },
    );
    (variance_x > 0.0 && variance_y > 0.0).then_some(covariance / (variance_x * variance_y).sqrt())
}

/// Automatic, diagnostic trajectory label derived from assistant terminals.
/// It is not a correctness label and never replaces manual task outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalTrajectory {
    pub session_id: String,
    pub terminals: Vec<TerminalState>,
    pub decision_ping_pong: bool,
    pub repeated_blocked_cause: bool,
    pub long_run_without_done: bool,
    pub turns: usize,
    pub observed_billing_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TaskOutcomeLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalTrajectoryReport {
    pub schema_version: u32,
    pub trajectories: Vec<TerminalTrajectory>,
    pub confidence_note: String,
}

/// Aggregate one session's persisted thrash-receipt contributions. Result
/// digests and tool arguments are intentionally unavailable at report time;
/// only the score and content-free category counts are retained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThrashSessionObservation {
    pub session_id: String,
    pub score: u32,
    pub exact_repeated_tool_results: u32,
    pub expensive_exact_repeated_tool_results: u32,
    pub wire_cheap_exact_repeated_tool_results: u32,
    pub near_repeated_tool_calls: u32,
    pub edit_fail_edit_cycles: u32,
    pub observed_billing_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThrashReport {
    pub schema_version: u32,
    pub sessions: Vec<ThrashSessionObservation>,
    pub confidence_note: String,
}

/// Report repeated tool activity without treating it as correctness. Exact
/// repetitions remain useful diagnostic work even after file-reference dedup;
/// those wire-cheap occurrences are reported separately from full-cost ones.
pub fn thrash_report(records: &[TranscriptRecord], costs: &[CostRecord]) -> ThrashReport {
    let mut costs_by_session = HashMap::<&str, f64>::new();
    for cost in costs {
        if let Some(session_id) = cost.session_id.as_deref() {
            *costs_by_session.entry(session_id).or_default() +=
                cost.observed_billing_usd.unwrap_or_default().max(0.0);
        }
    }
    let mut sessions = BTreeMap::<String, ThrashSessionObservation>::new();
    for record in records {
        let observation = sessions
            .entry(record.session_id.clone())
            .or_insert_with(|| ThrashSessionObservation {
                observed_billing_usd: costs_by_session
                    .get(record.session_id.as_str())
                    .copied()
                    .unwrap_or_default(),
                session_id: record.session_id.clone(),
                score: 0,
                exact_repeated_tool_results: 0,
                expensive_exact_repeated_tool_results: 0,
                wire_cheap_exact_repeated_tool_results: 0,
                near_repeated_tool_calls: 0,
                edit_fail_edit_cycles: 0,
            });
        for receipt in &record.receipts {
            let SignalReceipt::Thrash(receipt) = receipt else {
                continue;
            };
            add_thrash_receipt(observation, receipt);
        }
    }
    ThrashReport {
        schema_version: 1,
        sessions: sessions.into_values().collect(),
        confidence_note: "Thrash is a diagnostic repetition score, not a correctness or efficiency conclusion; billed dollars are observed session totals.".into(),
    }
}

fn add_thrash_receipt(observation: &mut ThrashSessionObservation, receipt: &ThrashReceipt) {
    observation.score = observation.score.saturating_add(receipt.score);
    observation.exact_repeated_tool_results = observation
        .exact_repeated_tool_results
        .saturating_add(receipt.exact_repeated_tool_results);
    observation.expensive_exact_repeated_tool_results = observation
        .expensive_exact_repeated_tool_results
        .saturating_add(receipt.expensive_exact_repeated_tool_results);
    observation.wire_cheap_exact_repeated_tool_results = observation
        .wire_cheap_exact_repeated_tool_results
        .saturating_add(receipt.wire_cheap_exact_repeated_tool_results);
    observation.near_repeated_tool_calls = observation
        .near_repeated_tool_calls
        .saturating_add(receipt.near_repeated_tool_calls);
    observation.edit_fail_edit_cycles = observation
        .edit_fail_edit_cycles
        .saturating_add(receipt.edit_fail_edit_cycles);
}

/// Derive session trajectories from terminal receipts, falling back to raw
/// sigil parsing for historical transcripts that predate receipts.
pub fn terminal_trajectory_report(
    records: &[TranscriptRecord],
    costs: &[CostRecord],
    labels: &[TaskOutcomeLabel],
) -> TerminalTrajectoryReport {
    let mut by_session = BTreeMap::<String, Vec<&TranscriptRecord>>::new();
    for record in records {
        by_session
            .entry(record.session_id.clone())
            .or_default()
            .push(record);
    }
    let mut costs_by_session = HashMap::<&str, f64>::new();
    for cost in costs {
        if let Some(session_id) = cost.session_id.as_deref() {
            *costs_by_session.entry(session_id).or_default() +=
                cost.observed_billing_usd.unwrap_or_default().max(0.0);
        }
    }
    let labels_by_session: HashMap<&str, &TaskOutcomeLabel> = labels
        .iter()
        .map(|label| (label.session_id.as_str(), label))
        .collect();
    let trajectories = by_session
        .into_iter()
        .map(|(session_id, session_records)| {
            let assistant = session_records
                .iter()
                .filter(|record| record.role == "assistant")
                .collect::<Vec<_>>();
            let terminals = assistant
                .iter()
                .map(|record| terminal_state(record))
                .collect::<Vec<_>>();
            let causes = assistant
                .iter()
                .filter_map(|record| blocked_cause_signature(&record.content))
                .collect::<Vec<_>>();
            TerminalTrajectory {
                decision_ping_pong: terminals
                    .windows(3)
                    .any(|window| window.iter().all(|state| *state == TerminalState::Decision)),
                repeated_blocked_cause: causes
                    .iter()
                    .enumerate()
                    .any(|(index, cause)| causes[..index].iter().any(|prior| prior == cause)),
                long_run_without_done: terminals.len() >= 5
                    && !terminals.contains(&TerminalState::Done),
                turns: session_records.len(),
                observed_billing_usd: costs_by_session
                    .get(session_id.as_str())
                    .copied()
                    .unwrap_or_default(),
                outcome: labels_by_session.get(session_id.as_str()).cloned().cloned(),
                session_id,
                terminals,
            }
        })
        .collect();
    TerminalTrajectoryReport {
        schema_version: 1,
        trajectories,
        confidence_note: "Terminal history is an automatic trajectory label, not task correctness."
            .into(),
    }
}

fn terminal_state(record: &TranscriptRecord) -> TerminalState {
    record
        .receipts
        .iter()
        .find_map(|receipt| match receipt {
            SignalReceipt::Terminal(receipt) => Some(receipt.state),
            SignalReceipt::Readiness(_) | SignalReceipt::Advisory(_) | SignalReceipt::Thrash(_) => {
                None
            }
        })
        .unwrap_or_else(|| match crate::signals::terminal_receipt(&record.content) {
            SignalReceipt::Terminal(receipt) => receipt.state,
            SignalReceipt::Readiness(_) | SignalReceipt::Advisory(_) | SignalReceipt::Thrash(_) => {
                unreachable!("terminal receipt is terminal")
            }
        })
}

/// Exact normalized comparison is deliberately conservative. It is calculated
/// only in-memory while reporting and never emitted as a cause string.
fn blocked_cause_signature(content: &str) -> Option<String> {
    let line = content.lines().rev().find(|line| !line.trim().is_empty())?;
    let body = crate::sigil::marker_body(line, 'x')?;
    let normalized = body
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|word| word.len() > 2)
        .collect::<Vec<_>>()
        .join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Provider, Usage};

    fn transcript(session_id: &str) -> TranscriptRecord {
        TranscriptRecord::new(
            "t".into(),
            session_id.into(),
            "assistant".into(),
            "§ Status\n. done".into(),
            None,
        )
    }

    fn cost(session_id: Option<&str>, billing_usd: f64) -> CostRecord {
        CostRecord {
            schema_version: 1,
            provider: Provider::OpenAi,
            model: "m".into(),
            recorded_at_secs: None,
            session_id: session_id.map(str::to_owned),
            session_turn: None,
            usage: Usage::default(),
            observed_billing_usd: Some(billing_usd),
            estimated_savings_usd: None,
            compressor: None,
        }
    }

    #[test]
    fn sums_cost_records_matching_a_transcript_session() {
        let records = vec![transcript("sess-1"), transcript("sess-1")];
        let costs = vec![cost(Some("sess-1"), 0.01), cost(Some("sess-1"), 0.02)];
        let report = report(&records, &costs);
        assert_eq!(report.turns, 2);
        assert!((report.cost_usd - 0.03).abs() < 1e-9);
        assert_eq!(report.cost_records_excluded, 0);
    }

    #[test]
    fn excludes_cost_records_from_other_sessions_and_without_a_session_id() {
        let records = vec![transcript("sess-1")];
        let costs = vec![
            cost(Some("sess-1"), 0.01),
            cost(Some("sess-2"), 0.05),
            cost(None, 0.07),
        ];
        let report = report(&records, &costs);
        // Only the matching session's cost is counted...
        assert!((report.cost_usd - 0.01).abs() < 1e-9);
        // ...and the excluded records are surfaced, not silently dropped.
        assert_eq!(report.cost_records_excluded, 2);
    }

    #[test]
    fn empty_report_has_zero_cost_and_no_excluded_records() {
        let report = report(&[], &[]);
        assert_eq!(report.turns, 0);
        assert_eq!(report.cost_usd, 0.0);
        assert_eq!(report.cost_records_excluded, 0);
    }

    #[test]
    fn experiment_report_keeps_missing_arms_and_evidence_explicit() {
        let report = experiment_report(&[transcript("s")], &[], &[]);
        assert_eq!(
            report.decision,
            ExperimentDecision::StopInsufficientEvidence
        );
        assert_eq!(report.arms.len(), 2);
        assert!(report.arms.iter().all(|arm| arm.assigned_sessions == 0));
    }

    #[test]
    fn held_out_analysis_stops_safely_without_labelled_evidence() {
        let report = readiness_held_out_analysis(&[transcript("s")], &[], &[]);
        assert_eq!(
            report.decision,
            ReadinessAnalysisDecision::StopInsufficientEvidence
        );
        assert_eq!(report.labelled_held_out_sessions, 0);
        assert_eq!(report.variants.len(), 2);
    }

    #[test]
    fn terminal_trajectory_flags_ping_pong_and_repeated_blocked_causes() {
        let assistant = |content: &str| {
            TranscriptRecord::new(
                "t".into(),
                "s".into(),
                "assistant".into(),
                content.into(),
                None,
            )
        };
        let records = vec![
            assistant("? need scope"),
            assistant("? need scope"),
            assistant("? need scope"),
            assistant("x dependency unavailable"),
            assistant("x dependency unavailable"),
        ];
        let report = terminal_trajectory_report(&records, &[], &[]);
        let trajectory = &report.trajectories[0];
        assert!(trajectory.decision_ping_pong);
        assert!(trajectory.repeated_blocked_cause);
        assert!(trajectory.long_run_without_done);
        assert_eq!(trajectory.terminals.len(), 5);
    }

    #[test]
    fn clarification_report_joins_decision_reply_delta_cost_and_session_label() {
        let user = |turn: u64, content: &str| {
            TranscriptRecord::new("t".into(), "s".into(), "user".into(), content.into(), None)
                .with_session_turn(turn)
                .with_receipts(crate::signals::readiness_receipts(content))
        };
        let decision = TranscriptRecord::new(
            "t".into(),
            "s".into(),
            "assistant".into(),
            "? Which target file and acceptance check apply?".into(),
            None,
        )
        .with_session_turn(1)
        .with_receipts(vec![crate::signals::terminal_receipt("? clarification")]);
        let records = vec![
            user(1, "Fix the bug."),
            decision,
            user(
                2,
                "Fix the bug in src/lib.rs. Preserve compatibility. Done when cargo test passes.",
            ),
        ];
        let labels = vec![TaskOutcomeLabel {
            session_id: "s".into(),
            completed: Some(true),
            correct: Some(true),
        }];
        let report = clarification_report(
            &records,
            &[cost(Some("s"), 0.02).with_session_turn(2)],
            &labels,
        );
        assert_eq!(report.unpaired_decision_turns, 0);
        assert_eq!(report.incomplete_readiness_pairs, 0);
        assert_eq!(report.observations.len(), 2);
        let raw = report
            .observations
            .iter()
            .find(|observation| observation.variant == ReadinessVariant::RawComposite)
            .unwrap();
        assert_eq!(raw.question_turn, 1);
        assert_eq!(raw.reply_turn, 2);
        assert!(raw.score_delta > 0);
        assert!(raw.filled_slots.contains(&ReadinessSlot::TargetArtifact));
        assert!(raw.filled_slots.contains(&ReadinessSlot::SuccessCriterion));
        assert!(raw.filled_slots.contains(&ReadinessSlot::Constraints));
        assert_eq!(raw.reply_turn_observed_billing_usd, Some(0.02));
        assert_eq!(raw.reply_turn_priced_cost_records, 1);
        assert_eq!(raw.outcome.as_ref().unwrap().correct, Some(true));
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("src/lib.rs"));
        assert!(!encoded.contains("Which target"));
    }

    #[test]
    fn clarification_report_excludes_legacy_records_and_unpaired_decisions() {
        let decision = TranscriptRecord::new(
            "t".into(),
            "s".into(),
            "assistant".into(),
            "? need detail".into(),
            None,
        )
        .with_session_turn(1)
        .with_receipts(vec![crate::signals::terminal_receipt("? need detail")]);
        let legacy = TranscriptRecord::new(
            "t".into(),
            "legacy".into(),
            "assistant".into(),
            "? old".into(),
            None,
        )
        .with_receipts(vec![crate::signals::terminal_receipt("? old")]);
        let report = clarification_report(&[decision, legacy], &[], &[]);
        assert_eq!(report.unpaired_decision_turns, 1);
        assert_eq!(report.legacy_records_excluded, 1);
        assert!(report.observations.is_empty());
    }

    #[test]
    fn thrash_report_separates_expensive_and_wire_cheap_exact_repetition() {
        let receipt = crate::signals::thrash_receipt(crate::session::ThrashObservation {
            exact_repeated_tool_results: 2,
            expensive_exact_repeated_tool_results: 1,
            wire_cheap_exact_repeated_tool_results: 1,
            near_repeated_tool_calls: 1,
            edit_fail_edit_cycles: 1,
        });
        let records = vec![
            transcript("s").with_receipts(vec![receipt]),
            transcript("quiet"),
        ];
        let report = thrash_report(&records, &[cost(Some("s"), 0.02)]);
        assert_eq!(report.sessions.len(), 2);
        let thrashing = report
            .sessions
            .iter()
            .find(|observation| observation.session_id == "s")
            .unwrap();
        assert_eq!(thrashing.score, 4);
        assert_eq!(thrashing.exact_repeated_tool_results, 2);
        assert_eq!(thrashing.expensive_exact_repeated_tool_results, 1);
        assert_eq!(thrashing.wire_cheap_exact_repeated_tool_results, 1);
        assert_eq!(thrashing.near_repeated_tool_calls, 1);
        assert_eq!(thrashing.edit_fail_edit_cycles, 1);
        assert!((thrashing.observed_billing_usd - 0.02).abs() < 1e-9);
        assert_eq!(
            report
                .sessions
                .iter()
                .find(|observation| observation.session_id == "quiet")
                .unwrap()
                .score,
            0
        );
    }

    #[test]
    fn readiness_report_uses_first_scored_turn_and_keeps_missing_labels_unknown() {
        let user = |session_id: &str, content: &str| {
            TranscriptRecord::new(
                "t".into(),
                session_id.into(),
                "user".into(),
                content.into(),
                None,
            )
            .with_receipts(crate::signals::readiness_receipts_for_conversation(
                content, true,
            ))
        };
        let records = vec![
            user(
                "s1",
                "Fix src/lib.rs. Preserve compatibility. Done when tests pass.",
            ),
            transcript("s1"),
            user("s2", "Explain this."),
        ];
        let labels = vec![TaskOutcomeLabel {
            session_id: "s1".into(),
            completed: Some(true),
            correct: Some(true),
        }];
        let report = readiness_report(
            &records,
            &[cost(Some("s1"), 0.02), cost(Some("s2"), 0.03)],
            &labels,
        );
        assert_eq!(report.observations.len(), 4, "two variants per session");
        assert_eq!(report.unlabelled_sessions, 1);
        let raw_s1 = report
            .observations
            .iter()
            .find(|observation| {
                observation.session_id == "s1"
                    && observation.variant == ReadinessVariant::RawComposite
            })
            .unwrap();
        assert_eq!(raw_s1.turns, 2);
        assert!((raw_s1.observed_billing_usd - 0.02).abs() < 1e-9);
        assert_eq!(raw_s1.outcome.as_ref().unwrap().correct, Some(true));
        assert!(
            report
                .observations
                .iter()
                .filter(|observation| observation.session_id == "s2")
                .all(|observation| observation.outcome.is_none())
        );
    }

    #[test]
    fn readiness_report_excludes_non_start_and_legacy_receipts() {
        let non_start = TranscriptRecord::new(
            "t".into(),
            "continued".into(),
            "user".into(),
            "Continue the task.".into(),
            None,
        )
        .with_receipts(crate::signals::readiness_receipts_for_conversation(
            "Continue the task.",
            false,
        ));
        let legacy = TranscriptRecord::new(
            "t".into(),
            "legacy".into(),
            "user".into(),
            "Old observation.".into(),
            None,
        )
        .with_receipts(crate::signals::readiness_receipts("Old observation."));
        let report = readiness_report(&[non_start, legacy], &[], &[]);
        assert!(report.observations.is_empty());
    }
}
