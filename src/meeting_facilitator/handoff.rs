//! Meeting handoff artifacts — written when a meeting closes, consumed by
//! the engineer loop and the `act-on-decisions` CLI subcommand.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

use super::types::{ActionItem, MeetingDecision, MeetingSession};

/// Well-known filename for meeting handoff artifacts.
pub const MEETING_HANDOFF_FILENAME: &str = "meeting_handoff.json";

/// Default directory for meeting handoff artifacts.
///
/// Respects `SIMARD_HANDOFF_DIR` when set, otherwise falls back to
/// `$CARGO_MANIFEST_DIR/target/meeting_handoffs`.
pub fn default_handoff_dir() -> PathBuf {
    std::env::var_os("SIMARD_HANDOFF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/meeting_handoffs")
        })
}

/// A handoff artifact produced when a meeting closes. Contains decisions,
/// action items, and open questions extracted from the meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingHandoff {
    pub topic: String,
    pub closed_at: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub processed: bool,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    #[serde(default)]
    pub transcript: Vec<String>,
    #[serde(default)]
    pub participants: Vec<String>,
}

impl MeetingHandoff {
    /// Create a handoff from a closed meeting session.
    /// Notes containing `?` are extracted as open questions.
    pub fn from_session(session: &MeetingSession) -> Self {
        let open_questions: Vec<String> = session
            .notes
            .iter()
            .filter(|n| n.contains('?'))
            .cloned()
            .collect();

        let duration_secs = chrono::DateTime::parse_from_rfc3339(&session.started_at)
            .ok()
            .map(|start| Utc::now().signed_duration_since(start).num_seconds().max(0) as u64);

        let transcript = session.notes.clone();

        // Collect unique participants from session.participants, decision participants, and action owners.
        let mut all_participants: Vec<String> = session.participants.clone();
        for d in &session.decisions {
            for p in &d.participants {
                if !all_participants.contains(p) {
                    all_participants.push(p.clone());
                }
            }
        }
        for a in &session.action_items {
            if !all_participants.contains(&a.owner) {
                all_participants.push(a.owner.clone());
            }
        }

        Self {
            topic: session.topic.clone(),
            closed_at: Utc::now().to_rfc3339(),
            decisions: session.decisions.clone(),
            action_items: session.action_items.clone(),
            open_questions,
            processed: false,
            duration_secs,
            transcript,
            participants: all_participants,
        }
    }
}

/// Write a meeting handoff artifact to a directory.
pub fn write_meeting_handoff(dir: &Path, handoff: &MeetingHandoff) -> SimardResult<()> {
    fs::create_dir_all(dir).map_err(|e| SimardError::ArtifactIo {
        path: dir.to_path_buf(),
        reason: format!("creating handoff dir: {e}"),
    })?;
    // Use timestamped filename to avoid overwriting/appending corruption.
    let ts = handoff.closed_at.replace(':', "-").replace('+', "_");
    let filename = format!("handoff-{ts}.json");
    let path = dir.join(&filename);
    let json = serde_json::to_string_pretty(handoff).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing handoff: {e}"),
    })?;
    fs::write(&path, &json).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;
    Ok(())
}

/// Find the newest handoff file in a directory (timestamped `handoff-*.json`
/// or legacy `meeting_handoff.json`). Returns `None` if no file exists.
fn find_newest_handoff(dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // Legacy fixed filename.
    let legacy = dir.join(MEETING_HANDOFF_FILENAME);
    if legacy.is_file() {
        candidates.push(legacy);
    }

    // Timestamped files written by `write_meeting_handoff`.
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("handoff-") && name_str.ends_with(".json") {
                candidates.push(entry.path());
            }
        }
    }

    // Newest by filename (timestamps sort lexicographically).
    candidates.sort();
    candidates.pop()
}

/// Load a meeting handoff artifact from a directory. Returns `None` if no
/// handoff file exists. Scans for both legacy and timestamped filenames.
pub fn load_meeting_handoff(dir: &Path) -> SimardResult<Option<MeetingHandoff>> {
    let path = match find_newest_handoff(dir) {
        Some(p) => p,
        None => return Ok(None),
    };
    let raw = fs::read_to_string(&path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("reading handoff: {e}"),
    })?;
    let handoff: MeetingHandoff =
        serde_json::from_str(&raw).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("failed to parse handoff JSON: {e}"),
        })?;
    Ok(Some(handoff))
}

/// Mark the meeting handoff in a directory as processed. No-op if no handoff
/// file exists. Updates the file in-place (writes back to the same path).
pub fn mark_meeting_handoff_processed(dir: &Path) -> SimardResult<()> {
    let path = match find_newest_handoff(dir) {
        Some(p) => p,
        None => return Ok(()),
    };
    let raw = fs::read_to_string(&path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("reading handoff: {e}"),
    })?;
    let mut handoff: MeetingHandoff =
        serde_json::from_str(&raw).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("failed to parse handoff JSON: {e}"),
        })?;
    handoff.processed = true;
    // Write back to the same file to avoid creating duplicates.
    let json = serde_json::to_string_pretty(&handoff).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing handoff: {e}"),
    })?;
    fs::write(&path, &json).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;
    Ok(())
}

/// Mark an already-loaded handoff as processed and write it back, avoiding a
/// redundant file read when the caller already holds the parsed struct.
/// Writes back to the existing file (if found) to avoid creating duplicates.
pub fn mark_handoff_processed_in_place(
    dir: &Path,
    handoff: &mut MeetingHandoff,
) -> SimardResult<()> {
    handoff.processed = true;
    // Write back to the existing file if found, otherwise create a new one.
    let path = find_newest_handoff(dir).unwrap_or_else(|| {
        let ts = handoff.closed_at.replace(':', "-").replace('+', "_");
        dir.join(format!("handoff-{ts}.json"))
    });
    let json = serde_json::to_string_pretty(handoff).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing handoff: {e}"),
    })?;
    fs::write(&path, &json).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;
    Ok(())
}
