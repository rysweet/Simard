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
