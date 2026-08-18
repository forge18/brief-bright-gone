//! Deterministic benchmark report aggregation from transcript and cost records.
use crate::{operations::CostRecord, transcript::TranscriptRecord};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
            session_id: session_id.map(str::to_owned),
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
}
