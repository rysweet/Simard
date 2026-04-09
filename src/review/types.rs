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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_evidence_summary() -> ReviewEvidenceSummary {
        ReviewEvidenceSummary {
            memory_records: 10,
            evidence_records: 5,
            decision_records: 2,
            benchmark_records: 3,
            exported_state: "ready".to_string(),
            session_phase: Some("execution".to_string()),
            failed_signals: vec!["signal-a".to_string()],
        }
    }

    #[test]
    fn review_target_kind_as_str() {
        assert_eq!(ReviewTargetKind::Session.as_str(), "session");
        assert_eq!(ReviewTargetKind::Benchmark.as_str(), "benchmark");
    }

    #[test]
    fn review_target_kind_serde_roundtrip() {
        let kind = ReviewTargetKind::Benchmark;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"benchmark\"");
        let back: ReviewTargetKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn review_target_kind_kebab_case_session() {
        let json = "\"session\"";
        let kind: ReviewTargetKind = serde_json::from_str(json).unwrap();
        assert_eq!(kind, ReviewTargetKind::Session);
    }

    #[test]
    fn review_signal_serde_roundtrip() {
        let signal = ReviewSignal {
            id: "sig-1".to_string(),
            passed: true,
            detail: "all good".to_string(),
        };
        let json = serde_json::to_string(&signal).unwrap();
        let back: ReviewSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, signal);
    }

    #[test]
    fn improvement_proposal_serde_roundtrip() {
        let proposal = ImprovementProposal {
            category: "perf".to_string(),
            title: "cache lookups".to_string(),
            rationale: "reduce latency".to_string(),
            suggested_change: "add LRU cache".to_string(),
            evidence: vec!["bench-1".to_string()],
        };
        let json = serde_json::to_string(&proposal).unwrap();
        let back: ImprovementProposal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, proposal);
    }

    #[test]
    fn review_evidence_summary_serde_roundtrip() {
        let summary = sample_evidence_summary();
        let json = serde_json::to_string(&summary).unwrap();
        let back: ReviewEvidenceSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back, summary);
    }

    #[test]
    fn review_artifact_concise_record_no_proposals() {
        let artifact = ReviewArtifact {
            review_id: "r1".to_string(),
            reviewed_at_unix_ms: 1000,
            target_kind: ReviewTargetKind::Session,
            target_label: "sess-1".to_string(),
            identity_name: "id-1".to_string(),
            session_id: "s-1".to_string(),
            selected_base_type: "bt".to_string(),
            topology: "single".to_string(),
            objective_metadata: "obj".to_string(),
            execution_summary: "exec".to_string(),
            reflection_summary: "reflect".to_string(),
            summary: "sum".to_string(),
            measurement_notes: vec![],
            evidence_summary: sample_evidence_summary(),
            proposals: vec![],
        };
        let record = artifact.concise_record();
        assert!(record.contains("target=sess-1"));
        assert!(record.contains("evidence_records=5"));
        assert!(record.contains("failed_signals=1"));
        assert!(record.contains("proposals=[]"));
    }

    #[test]
    fn review_artifact_concise_record_with_proposals() {
        let artifact = ReviewArtifact {
            review_id: "r2".to_string(),
            reviewed_at_unix_ms: 2000,
            target_kind: ReviewTargetKind::Benchmark,
            target_label: "bench-1".to_string(),
            identity_name: "id-2".to_string(),
            session_id: "s-2".to_string(),
            selected_base_type: "bt".to_string(),
            topology: "single".to_string(),
            objective_metadata: "obj".to_string(),
            execution_summary: "exec".to_string(),
            reflection_summary: "reflect".to_string(),
            summary: "sum".to_string(),
            measurement_notes: vec![],
            evidence_summary: sample_evidence_summary(),
            proposals: vec![ImprovementProposal {
                category: "perf".to_string(),
                title: "optimize".to_string(),
                rationale: "faster".to_string(),
                suggested_change: "add cache".to_string(),
                evidence: vec![],
            }],
        };
        let record = artifact.concise_record();
        assert!(record.contains("optimize: add cache"));
    }

    #[test]
    fn review_request_construction() {
        let req = ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "sess".to_string(),
            execution_summary: "exec".to_string(),
            reflection_summary: "reflect".to_string(),
            measurement_notes: vec!["note".to_string()],
            signals: vec![],
        };
        assert_eq!(req.target_kind, ReviewTargetKind::Session);
        assert!(req.signals.is_empty());
    }
}
