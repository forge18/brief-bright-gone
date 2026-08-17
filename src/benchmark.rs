//! Deterministic benchmark report aggregation from transcript records.
use crate::transcript::TranscriptRecord;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub turns: usize,
    pub lint_findings: usize,
    pub heuristic_findings: usize,
    pub cost_usd: f64,
    pub confidence_note: String,
}
pub fn report(records: &[TranscriptRecord]) -> BenchmarkReport {
    let findings = records.iter().flat_map(|r| r.lint.iter());
    let all: Vec<_> = findings.collect();
    BenchmarkReport{schema_version:1,turns:records.len(),lint_findings:all.len(),heuristic_findings:all.iter().filter(|f|f.heuristic).count(),cost_usd:0.0,confidence_note:"Confidence requires repeated paired runs; this report contains observed transcript counts only.".into()}
}
