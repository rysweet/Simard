use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};
use crate::persistence::persist_json;

use super::types::ReviewArtifact;

const REVIEW_STORE_NAME: &str = "review-artifact";

pub fn review_artifacts_dir(state_root: &Path) -> PathBuf {
    state_root.join("review-artifacts")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::types::{
        ImprovementProposal, ReviewArtifact, ReviewEvidenceSummary, ReviewTargetKind,
    };
    use tempfile::tempdir;

    fn sample_artifact(review_id: &str, unix_ms: u128) -> ReviewArtifact {
        ReviewArtifact {
            review_id: review_id.to_string(),
            reviewed_at_unix_ms: unix_ms,
            target_kind: ReviewTargetKind::Session,
            target_label: "session-42".to_string(),
            identity_name: "simard".to_string(),
            session_id: "sess-001".to_string(),
            selected_base_type: "base".to_string(),
            topology: "single".to_string(),
            objective_metadata: "meta".to_string(),
            execution_summary: "ran well".to_string(),
            reflection_summary: "good".to_string(),
            summary: "All good".to_string(),
            measurement_notes: vec!["note1".to_string()],
            evidence_summary: ReviewEvidenceSummary {
                memory_records: 10,
                evidence_records: 5,
                decision_records: 2,
                benchmark_records: 1,
                exported_state: "ok".to_string(),
                session_phase: Some("execution".to_string()),
                failed_signals: vec![],
            },
            proposals: vec![ImprovementProposal {
                category: "perf".to_string(),
                title: "Cache more".to_string(),
                rationale: "Faster".to_string(),
                suggested_change: "Add LRU cache".to_string(),
                evidence: vec!["slow queries".to_string()],
            }],
        }
    }

    // ── review_artifacts_dir ────────────────────────────────────────

    #[test]
    fn review_artifacts_dir_appends_correct_subdir() {
        let root = std::path::Path::new("/state");
        let dir = review_artifacts_dir(root);
        assert_eq!(dir, std::path::PathBuf::from("/state/review-artifacts"));
    }

    // ── persist and load round-trip ─────────────────────────────────

    #[test]
    fn persist_and_load_round_trip() {
        let tmp = tempdir().unwrap();
        let state_root = tmp.path();

        let artifact = sample_artifact("rev-001", 1000);
        let path = persist_review_artifact(state_root, &artifact).unwrap();

        assert!(path.exists());
        assert!(path.to_string_lossy().contains("rev-001.json"));

        let loaded = load_review_artifact(&path).unwrap();
        assert_eq!(loaded, artifact);
    }

    #[test]
    fn load_review_artifact_nonexistent_file() {
        let result = load_review_artifact(Path::new("/nonexistent/artifact.json"));
        assert!(result.is_err());
    }

    #[test]
    fn load_review_artifact_invalid_json() {
        let tmp = tempdir().unwrap();
        let bad_file = tmp.path().join("bad.json");
        fs::write(&bad_file, "not valid json").unwrap();
        let result = load_review_artifact(&bad_file);
        assert!(result.is_err());
    }

    // ── latest_review_artifact ──────────────────────────────────────

    #[test]
    fn latest_review_artifact_no_dir() {
        let tmp = tempdir().unwrap();
        let result = latest_review_artifact(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn latest_review_artifact_empty_dir() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(review_artifacts_dir(tmp.path())).unwrap();
        let result = latest_review_artifact(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn latest_review_artifact_picks_newest() {
        let tmp = tempdir().unwrap();
        let state_root = tmp.path();

        let old = sample_artifact("rev-old", 1000);
        let new = sample_artifact("rev-new", 2000);
        persist_review_artifact(state_root, &old).unwrap();
        persist_review_artifact(state_root, &new).unwrap();

        let (path, artifact) = latest_review_artifact(state_root).unwrap().unwrap();
        assert_eq!(artifact.review_id, "rev-new");
        assert!(path.to_string_lossy().contains("rev-new"));
    }

    #[test]
    fn latest_review_artifact_ignores_non_json() {
        let tmp = tempdir().unwrap();
        let state_root = tmp.path();
        let dir = review_artifacts_dir(state_root);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("readme.txt"), "not json").unwrap();

        let result = latest_review_artifact(state_root).unwrap();
        assert!(result.is_none());
    }

    // ── render_review_text ──────────────────────────────────────────

    #[test]
    fn render_review_text_contains_key_fields() {
        let artifact = sample_artifact("rev-render", 5000);
        let text = render_review_text(&artifact);
        assert!(text.contains("Review: rev-render"));
        assert!(text.contains("Target: session-42"));
        assert!(text.contains("Target kind: session"));
        assert!(text.contains("Identity: simard"));
        assert!(text.contains("Session: sess-001"));
        assert!(text.contains("Summary: All good"));
    }

    #[test]
    fn render_review_text_contains_evidence_summary() {
        let artifact = sample_artifact("rev-ev", 5000);
        let text = render_review_text(&artifact);
        assert!(text.contains("memory_records=10"));
        assert!(text.contains("evidence_records=5"));
        assert!(text.contains("decision_records=2"));
        assert!(text.contains("benchmark_records=1"));
    }

    #[test]
    fn render_review_text_contains_proposals() {
        let artifact = sample_artifact("rev-prop", 5000);
        let text = render_review_text(&artifact);
        assert!(text.contains("[perf] Cache more -> Add LRU cache"));
    }

    #[test]
    fn render_review_text_contains_measurement_notes() {
        let artifact = sample_artifact("rev-notes", 5000);
        let text = render_review_text(&artifact);
        assert!(text.contains("Measurement notes:"));
        assert!(text.contains("- note1"));
    }

    #[test]
    fn render_review_text_empty_measurement_notes_omitted() {
        let mut artifact = sample_artifact("rev-empty", 5000);
        artifact.measurement_notes.clear();
        let text = render_review_text(&artifact);
        assert!(!text.contains("Measurement notes:"));
    }

    #[test]
    fn render_review_text_empty_proposals() {
        let mut artifact = sample_artifact("rev-noprop", 5000);
        artifact.proposals.clear();
        let text = render_review_text(&artifact);
        assert!(text.contains("Proposals:"));
    }
}
