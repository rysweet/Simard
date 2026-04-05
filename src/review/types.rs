use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewTargetKind {
    Session,
    Benchmark,
}

impl ReviewTargetKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Benchmark => "benchmark",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewSignal {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewRequest {
    pub target_kind: ReviewTargetKind,
    pub target_label: String,
    pub execution_summary: String,
    pub reflection_summary: String,
    pub measurement_notes: Vec<String>,
    pub signals: Vec<ReviewSignal>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub category: String,
    pub title: String,
    pub rationale: String,
    pub suggested_change: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewEvidenceSummary {
    pub memory_records: usize,
    pub evidence_records: usize,
    pub decision_records: usize,
    pub benchmark_records: usize,
    pub exported_state: String,
    pub session_phase: Option<String>,
    pub failed_signals: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewArtifact {
    pub review_id: String,
    pub reviewed_at_unix_ms: u128,
    pub target_kind: ReviewTargetKind,
    pub target_label: String,
    pub identity_name: String,
    pub session_id: String,
    pub selected_base_type: String,
    pub topology: String,
    pub objective_metadata: String,
    pub execution_summary: String,
    pub reflection_summary: String,
    pub summary: String,
    pub measurement_notes: Vec<String>,
    pub evidence_summary: ReviewEvidenceSummary,
    pub proposals: Vec<ImprovementProposal>,
}

impl ReviewArtifact {
    pub fn concise_record(&self) -> String {
        let proposals = self
            .proposals
            .iter()
            .map(|proposal| format!("{}: {}", proposal.title, proposal.suggested_change))
            .collect::<Vec<_>>();
        format!(
            "review-summary | target={} | evidence_records={} | failed_signals={} | proposals=[{}]",
            self.target_label,
            self.evidence_summary.evidence_records,
            self.evidence_summary.failed_signals.len(),
            proposals.join(" | ")
        )
    }
}
