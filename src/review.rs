use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::MemoryScope;
use crate::persistence::persist_json;

const REVIEW_STORE_NAME: &str = "review-artifact";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewTargetKind {
    Session,
    Benchmark,
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

pub fn review_artifacts_dir(state_root: &Path) -> PathBuf {
    state_root.join("review-artifacts")
}

pub fn build_review_artifact(
    request: ReviewRequest,
    handoff: &RuntimeHandoffSnapshot,
) -> SimardResult<ReviewArtifact> {
    let session = handoff
        .session
        .as_ref()
        .ok_or_else(|| SimardError::InvalidHandoffSnapshot {
            field: "session".to_string(),
            reason: "review requires an exported session boundary".to_string(),
        })?;
    let failed_signals = request
        .signals
        .iter()
        .filter(|signal| !signal.passed)
        .map(|signal| format!("{}: {}", signal.id, signal.detail))
        .collect::<Vec<_>>();
    let evidence_summary = ReviewEvidenceSummary {
        memory_records: handoff.memory_records.len(),
        evidence_records: handoff.evidence_records.len(),
        decision_records: handoff
            .memory_records
            .iter()
            .filter(|record| record.scope == MemoryScope::Decision)
            .count(),
        benchmark_records: handoff
            .memory_records
            .iter()
            .filter(|record| record.scope == MemoryScope::Benchmark)
            .count(),
        exported_state: handoff.exported_state.to_string(),
        session_phase: handoff
            .session
            .as_ref()
            .map(|record| record.phase.to_string()),
        failed_signals,
    };
    let proposals = improvement_proposals(&request, handoff, &evidence_summary);
    let summary = review_summary(&request, &evidence_summary, &proposals);

    Ok(ReviewArtifact {
        review_id: format!("{}-review", session.id),
        reviewed_at_unix_ms: now_unix_ms()?,
        target_kind: request.target_kind,
        target_label: request.target_label,
        identity_name: handoff.identity_name.clone(),
        session_id: session.id.to_string(),
        selected_base_type: handoff.selected_base_type.to_string(),
        topology: handoff.topology.to_string(),
        objective_metadata: session.objective.clone(),
        execution_summary: request.execution_summary,
        reflection_summary: request.reflection_summary,
        summary,
        measurement_notes: request.measurement_notes,
        evidence_summary,
        proposals,
    })
}

pub fn persist_review_artifact(
    state_root: &Path,
    artifact: &ReviewArtifact,
) -> SimardResult<PathBuf> {
    let artifact_path =
        review_artifacts_dir(state_root).join(format!("{}.json", artifact.review_id));
    persist_json(REVIEW_STORE_NAME, &artifact_path, artifact)?;
    Ok(artifact_path)
}

pub fn load_review_artifact(path: &Path) -> SimardResult<ReviewArtifact> {
    let contents = fs::read(path).map_err(|error| SimardError::PersistentStoreIo {
        store: REVIEW_STORE_NAME.to_string(),
        action: "read".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    serde_json::from_slice(&contents).map_err(|error| SimardError::PersistentStoreIo {
        store: REVIEW_STORE_NAME.to_string(),
        action: "deserialize".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

pub fn latest_review_artifact(
    state_root: &Path,
) -> SimardResult<Option<(PathBuf, ReviewArtifact)>> {
    let artifact_dir = review_artifacts_dir(state_root);
    if !artifact_dir.exists() {
        return Ok(None);
    }

    let entries = fs::read_dir(&artifact_dir).map_err(|error| SimardError::PersistentStoreIo {
        store: REVIEW_STORE_NAME.to_string(),
        action: "read-dir".to_string(),
        path: artifact_dir.clone(),
        reason: error.to_string(),
    })?;
    let mut latest = None;

    for entry in entries {
        let entry = entry.map_err(|error| SimardError::PersistentStoreIo {
            store: REVIEW_STORE_NAME.to_string(),
            action: "read-dir-entry".to_string(),
            path: artifact_dir.clone(),
            reason: error.to_string(),
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }

        let artifact = load_review_artifact(&path)?;
        let is_newer = latest
            .as_ref()
            .map(|(_, current): &(PathBuf, ReviewArtifact)| {
                artifact.reviewed_at_unix_ms > current.reviewed_at_unix_ms
            })
            .unwrap_or(true);
        if is_newer {
            latest = Some((path, artifact));
        }
    }

    Ok(latest)
}

pub fn render_review_text(artifact: &ReviewArtifact) -> String {
    let mut lines = vec![
        format!("Review: {}", artifact.review_id),
        format!("Target: {}", artifact.target_label),
        format!("Target kind: {}", artifact.target_kind.as_str()),
        format!("Identity: {}", artifact.identity_name),
        format!("Session: {}", artifact.session_id),
        format!(
            "Evidence summary: memory_records={}, evidence_records={}, decision_records={}, benchmark_records={}",
            artifact.evidence_summary.memory_records,
            artifact.evidence_summary.evidence_records,
            artifact.evidence_summary.decision_records,
            artifact.evidence_summary.benchmark_records
        ),
        format!("Summary: {}", artifact.summary),
        "Proposals:".to_string(),
    ];
    for proposal in &artifact.proposals {
        lines.push(format!(
            "- [{}] {} -> {}",
            proposal.category, proposal.title, proposal.suggested_change
        ));
    }
    if !artifact.measurement_notes.is_empty() {
        lines.push("Measurement notes:".to_string());
        for note in &artifact.measurement_notes {
            lines.push(format!("- {note}"));
        }
    }
    lines.join("\n")
}

impl ReviewTargetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Benchmark => "benchmark",
        }
    }
}

fn improvement_proposals(
    request: &ReviewRequest,
    handoff: &RuntimeHandoffSnapshot,
    summary: &ReviewEvidenceSummary,
) -> Vec<ImprovementProposal> {
    let mut proposals = Vec::new();

    if summary.evidence_records <= 4 || failed_signal(&request.signals, "evidence") {
        proposals.push(ImprovementProposal {
            category: "evidence-capture".to_string(),
            title: "Capture denser execution evidence".to_string(),
            rationale: format!(
                "This review only had {} persisted evidence records, which leaves the operator with a thin trail when checking what really happened.",
                summary.evidence_records
            ),
            suggested_change: "Record one concise evidence item per major phase boundary or adapter turn so reviews can cite planning, execution, and persistence evidence separately.".to_string(),
            evidence: handoff
                .evidence_records
                .iter()
                .take(3)
                .map(|record| record.detail.clone())
                .collect(),
        });
    }

    if request.target_kind == ReviewTargetKind::Session && summary.benchmark_records == 0 {
        proposals.push(ImprovementProposal {
            category: "benchmark-coverage".to_string(),
            title: "Promote this pattern into a repeatable benchmark".to_string(),
            rationale: "This review covered a one-off session with no benchmark-scoped memory, so future regressions would be hard to compare objectively.".to_string(),
            suggested_change: "Turn the reviewed operator flow into a named benchmark scenario so the same task can be replayed before accepting follow-on prompt or policy changes.".to_string(),
            evidence: vec![format!(
                "target={} exported benchmark records={}",
                request.target_label, summary.benchmark_records
            )],
        });
    }

    if request
        .measurement_notes
        .iter()
        .any(|note| note.contains("unnecessary_action_count"))
    {
        proposals.push(ImprovementProposal {
            category: "operator-metrics".to_string(),
            title: "Measure unnecessary action count explicitly".to_string(),
            rationale: "The benchmark report still admits that unnecessary actions are unmeasured, so review decisions cannot separate wasteful behavior from necessary work.".to_string(),
            suggested_change: "Add a bounded action counter in the gym runner and emit it as structured review evidence instead of leaving it as a free-form note.".to_string(),
            evidence: request
                .measurement_notes
                .iter()
                .filter(|note| note.contains("unnecessary_action_count"))
                .cloned()
                .collect(),
        });
    }

    if request
        .measurement_notes
        .iter()
        .any(|note| note.contains("retry_count"))
    {
        proposals.push(ImprovementProposal {
            category: "retry-policy".to_string(),
            title: "Track bounded retries in benchmark runs".to_string(),
            rationale: "The current benchmark foundation cannot show whether a scenario only succeeded because it would have needed a retry loop, so promotion decisions are missing an important quality signal.".to_string(),
            suggested_change: "Add an explicit retry-and-replan path for benchmark scenarios and persist the observed retry count as review evidence.".to_string(),
            evidence: request
                .measurement_notes
                .iter()
                .filter(|note| note.contains("retry_count"))
                .cloned()
                .collect(),
        });
    }

    if failed_signal(&request.signals, "reflection") || request.reflection_summary.trim().len() < 80
    {
        proposals.push(ImprovementProposal {
            category: "reflection-calibration".to_string(),
            title: "Tighten reflection summaries around operator-visible facts".to_string(),
            rationale: "The current reflection summary is too thin to explain why the session should count as successful or where it struggled.".to_string(),
            suggested_change: "Require reflection summaries to mention the concrete outcome, evidence density, and the next operator check instead of only restating the runtime wiring.".to_string(),
            evidence: vec![request.reflection_summary.clone()],
        });
    }

    if proposals.is_empty() {
        proposals.push(ImprovementProposal {
            category: "review-promotion".to_string(),
            title: "Carry accepted findings into the next run deliberately".to_string(),
            rationale: "This review did not surface an obvious failure, but it still produced evidence that should gate the next change instead of being discarded.".to_string(),
            suggested_change: "Use the persisted review artifact as an explicit approval checkpoint before promoting prompt, policy, or orchestration changes into the next benchmark cycle.".to_string(),
            evidence: vec![format!(
                "reviewed target={} with {} evidence records and {} failed signals",
                request.target_label,
                summary.evidence_records,
                summary.failed_signals.len()
            )],
        });
    }

    proposals.truncate(3);
    proposals
}

fn failed_signal(signals: &[ReviewSignal], needle: &str) -> bool {
    signals
        .iter()
        .any(|signal| !signal.passed && signal.id.contains(needle))
}

fn review_summary(
    request: &ReviewRequest,
    summary: &ReviewEvidenceSummary,
    proposals: &[ImprovementProposal],
) -> String {
    format!(
        "{} review for '{}' inspected {} evidence records, {} memory records, and {} failed signals, then emitted {} concrete proposal(s).",
        request.target_kind.as_str(),
        request.target_label,
        summary.evidence_records,
        summary.memory_records,
        summary.failed_signals.len(),
        proposals.len()
    )
}

fn now_unix_ms() -> SimardResult<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SimardError::ClockBeforeUnixEpoch {
            reason: error.to_string(),
        })?
        .as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeId;
    use crate::evidence::{EvidenceRecord, EvidenceSource};
    use crate::identity::OperatingMode;
    use crate::memory::{MemoryRecord, MemoryScope};
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
    use crate::session::{SessionId, SessionPhase, SessionRecord};
    use tempfile::TempDir;

    fn make_session_id() -> SessionId {
        SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_session_record() -> SessionRecord {
        SessionRecord {
            id: make_session_id(),
            mode: OperatingMode::Engineer,
            objective: "test objective".to_string(),
            phase: SessionPhase::Complete,
            selected_base_type: BaseTypeId::new("test-base"),
            evidence_ids: vec![],
            memory_keys: vec![],
        }
    }

    fn make_handoff() -> RuntimeHandoffSnapshot {
        let node = RuntimeNodeId::new("test-node");
        RuntimeHandoffSnapshot {
            exported_state: RuntimeState::Active,
            identity_name: "test-identity".to_string(),
            selected_base_type: BaseTypeId::new("test-base"),
            topology: RuntimeTopology::SingleProcess,
            source_runtime_node: node.clone(),
            source_mailbox_address: RuntimeAddress::local(&node),
            session: Some(make_session_record()),
            memory_records: vec![],
            evidence_records: vec![],
            copilot_submit_audit: None,
        }
    }

    fn make_handoff_with_records() -> RuntimeHandoffSnapshot {
        let sid = make_session_id();
        let mut handoff = make_handoff();
        handoff.memory_records = vec![
            MemoryRecord {
                key: "decision-1".to_string(),
                scope: MemoryScope::Decision,
                value: "decided something".to_string(),
                session_id: sid.clone(),
                recorded_in: SessionPhase::Execution,
            },
            MemoryRecord {
                key: "bench-1".to_string(),
                scope: MemoryScope::Benchmark,
                value: "benchmark data".to_string(),
                session_id: sid.clone(),
                recorded_in: SessionPhase::Execution,
            },
            MemoryRecord {
                key: "scratch-1".to_string(),
                scope: MemoryScope::SessionScratch,
                value: "scratch".to_string(),
                session_id: sid.clone(),
                recorded_in: SessionPhase::Execution,
            },
        ];
        handoff.evidence_records = vec![
            EvidenceRecord {
                id: "ev-1".to_string(),
                session_id: sid.clone(),
                phase: SessionPhase::Execution,
                detail: "first evidence".to_string(),
                source: EvidenceSource::Runtime,
            },
            EvidenceRecord {
                id: "ev-2".to_string(),
                session_id: sid.clone(),
                phase: SessionPhase::Execution,
                detail: "second evidence".to_string(),
                source: EvidenceSource::Runtime,
            },
        ];
        handoff
    }

    fn make_request() -> ReviewRequest {
        ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "test-session".to_string(),
            execution_summary: "Executed successfully".to_string(),
            reflection_summary: "Reflection on the session with enough detail to pass \
                the length check for the improvement proposal threshold"
                .to_string(),
            measurement_notes: vec!["latency=100ms".to_string()],
            signals: vec![
                ReviewSignal {
                    id: "signal-pass".to_string(),
                    passed: true,
                    detail: "all good".to_string(),
                },
                ReviewSignal {
                    id: "signal-fail".to_string(),
                    passed: false,
                    detail: "something went wrong".to_string(),
                },
            ],
        }
    }

    fn make_artifact_direct(review_id: &str, timestamp: u128) -> ReviewArtifact {
        ReviewArtifact {
            review_id: review_id.to_string(),
            reviewed_at_unix_ms: timestamp,
            target_kind: ReviewTargetKind::Session,
            target_label: "label".to_string(),
            identity_name: "id".to_string(),
            session_id: "s1".to_string(),
            selected_base_type: "base".to_string(),
            topology: "single".to_string(),
            objective_metadata: "obj".to_string(),
            execution_summary: "exec".to_string(),
            reflection_summary: "refl".to_string(),
            summary: "summary".to_string(),
            measurement_notes: vec![],
            evidence_summary: ReviewEvidenceSummary {
                memory_records: 0,
                evidence_records: 0,
                decision_records: 0,
                benchmark_records: 0,
                exported_state: "active".to_string(),
                session_phase: None,
                failed_signals: vec![],
            },
            proposals: vec![],
        }
    }

    // 1. review_artifacts_dir returns correct path
    #[test]
    fn review_artifacts_dir_appends_subdir() {
        let root = PathBuf::from("/state/root");
        assert_eq!(review_artifacts_dir(&root), root.join("review-artifacts"));
    }

    // 2. build_review_artifact with mixed signals extracts failed ones correctly
    #[test]
    fn build_artifact_extracts_failed_signals() {
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let failed = &artifact.evidence_summary.failed_signals;
        assert_eq!(failed.len(), 1);
        assert!(failed[0].contains("signal-fail"));
        assert!(failed[0].contains("something went wrong"));
    }

    #[test]
    fn build_artifact_counts_memory_scopes() {
        let handoff = make_handoff_with_records();
        let artifact = build_review_artifact(make_request(), &handoff).unwrap();
        assert_eq!(artifact.evidence_summary.memory_records, 3);
        assert_eq!(artifact.evidence_summary.evidence_records, 2);
        assert_eq!(artifact.evidence_summary.decision_records, 1);
        assert_eq!(artifact.evidence_summary.benchmark_records, 1);
    }

    // 3. build_review_artifact generates a review_id and sets reviewed_at_unix_ms
    #[test]
    fn build_artifact_sets_id_and_timestamp() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        assert!(!artifact.review_id.is_empty());
        assert!(artifact.review_id.contains("review"));
        assert!(artifact.reviewed_at_unix_ms >= before);
        assert!(artifact.reviewed_at_unix_ms <= after);
    }

    #[test]
    fn build_artifact_fails_without_session() {
        let mut handoff = make_handoff();
        handoff.session = None;
        let result = build_review_artifact(make_request(), &handoff);
        assert!(result.is_err());
    }

    // 4. concise_record contains target label and signal counts
    #[test]
    fn concise_record_includes_target_and_counts() {
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let record = artifact.concise_record();
        assert!(record.contains("test-session"), "missing target label");
        assert!(
            record.contains("failed_signals=1"),
            "missing failed signal count"
        );
        assert!(
            record.contains("evidence_records=0"),
            "missing evidence count"
        );
    }

    #[test]
    fn concise_record_includes_proposal_text() {
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let record = artifact.concise_record();
        assert!(record.contains("proposals=["), "missing proposals bracket");
        // The artifact should have at least one proposal
        assert!(!artifact.proposals.is_empty());
        assert!(
            record.contains(&artifact.proposals[0].title),
            "missing first proposal title"
        );
    }

    // 5. render_review_text contains header, proposals section, measurement notes
    #[test]
    fn render_review_text_has_sections() {
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let text = render_review_text(&artifact);
        assert!(text.contains("Review:"), "missing Review header");
        assert!(text.contains("Target: test-session"), "missing target");
        assert!(text.contains("Identity: test-identity"), "missing identity");
        assert!(text.contains("Proposals:"), "missing Proposals section");
        assert!(
            text.contains("Measurement notes:"),
            "missing Measurement notes section"
        );
        assert!(text.contains("latency=100ms"), "missing measurement note");
    }

    #[test]
    fn render_review_text_shows_target_kind() {
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
        let text = render_review_text(&artifact);
        assert!(text.contains("Target kind: session"), "missing target kind");
    }

    // 6. persist + load roundtrip
    #[test]
    fn persist_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();

        let path = persist_review_artifact(dir.path(), &artifact).unwrap();
        assert!(path.exists());
        assert!(path.extension().unwrap() == "json");

        let loaded = load_review_artifact(&path).unwrap();
        assert_eq!(artifact, loaded);
    }

    // 7. latest_review_artifact returns None on empty dir, newest when multiple exist
    #[test]
    fn latest_review_artifact_none_when_no_dir() {
        let dir = TempDir::new().unwrap();
        let result = latest_review_artifact(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn latest_review_artifact_none_when_dir_empty() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(review_artifacts_dir(dir.path())).unwrap();
        let result = latest_review_artifact(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn latest_review_artifact_returns_newest() {
        let dir = TempDir::new().unwrap();
        let older = make_artifact_direct("older-review", 1000);
        let newer = make_artifact_direct("newer-review", 2000);

        persist_review_artifact(dir.path(), &older).unwrap();
        persist_review_artifact(dir.path(), &newer).unwrap();

        let (path, latest) = latest_review_artifact(dir.path()).unwrap().unwrap();
        assert_eq!(latest.review_id, "newer-review");
        assert_eq!(latest.reviewed_at_unix_ms, 2000);
        assert!(path.to_string_lossy().contains("newer-review"));
    }

    // 8. ReviewTargetKind as_str shows "session" / "benchmark"
    #[test]
    fn target_kind_as_str_values() {
        assert_eq!(ReviewTargetKind::Session.as_str(), "session");
        assert_eq!(ReviewTargetKind::Benchmark.as_str(), "benchmark");
    }

    #[test]
    fn target_kind_serde_roundtrip() {
        let session_json = serde_json::to_string(&ReviewTargetKind::Session).unwrap();
        assert_eq!(session_json, "\"session\"");
        let benchmark_json = serde_json::to_string(&ReviewTargetKind::Benchmark).unwrap();
        assert_eq!(benchmark_json, "\"benchmark\"");

        let parsed: ReviewTargetKind = serde_json::from_str("\"session\"").unwrap();
        assert_eq!(parsed, ReviewTargetKind::Session);
        let parsed: ReviewTargetKind = serde_json::from_str("\"benchmark\"").unwrap();
        assert_eq!(parsed, ReviewTargetKind::Benchmark);
    }
}
