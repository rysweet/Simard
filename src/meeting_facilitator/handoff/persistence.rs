use std::fs;
use std::path::{Path, PathBuf};

use super::{MEETING_HANDOFF_FILENAME, MEETING_SESSION_WIP_FILENAME, MeetingHandoff};
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::types::MeetingSession;

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
///
/// **Note**: this is the historical "newest by filename" selector kept for
/// callers that want a single representative handoff regardless of state
/// (e.g. CLI display, observe scan). The OODA dispatch queue must use
/// [`find_oldest_unprocessed_handoff`] instead — see #1649.
pub fn find_newest_handoff(dir: &Path) -> Option<PathBuf> {
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

/// List all handoff files in a directory sorted by filename ascending
/// (oldest first, since timestamps sort lexicographically). Includes both
/// timestamped `handoff-*.json` files and the legacy `meeting_handoff.json`.
fn list_handoff_files(dir: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    let legacy = dir.join(MEETING_HANDOFF_FILENAME);
    if legacy.is_file() {
        candidates.push(legacy);
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("handoff-") && name_str.ends_with(".json") {
                candidates.push(entry.path());
            }
        }
    }

    candidates.sort(); // oldest first
    candidates
}

/// Find the **oldest unprocessed** handoff file in a directory — i.e. the
/// FIFO-next pending handoff for OODA ingestion.
///
/// This replaces the previous "newest by filename" behaviour for the
/// dispatch queue (#1649): a fresh empty handoff (e.g. emitted by a
/// dashboard chat that closes with zero items) was permanently shadowing
/// older content-rich handoffs because `find_newest_handoff` ignored the
/// `processed` flag and the older file would never be selected after a
/// newer one had been marked processed.
///
/// Each candidate file is read and parsed to inspect its `processed`
/// field; malformed JSON is skipped (an old half-written file should not
/// block dispatch). Returns `Ok(None)` when no unprocessed handoff exists.
pub fn find_oldest_unprocessed_handoff(dir: &Path) -> SimardResult<Option<PathBuf>> {
    for path in list_handoff_files(dir) {
        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping unreadable handoff while scanning for oldest unprocessed"
                );
                continue;
            }
        };
        let handoff: MeetingHandoff = match serde_json::from_str(&raw) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed handoff JSON while scanning for oldest unprocessed"
                );
                continue;
            }
        };
        if !handoff.processed {
            return Ok(Some(path));
        }
    }
    Ok(None)
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
/// Writes back to the existing file if found, otherwise create a new one.
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

// ---------------------------------------------------------------------------
// Work-in-progress session persistence (auto-save / crash resume)
// ---------------------------------------------------------------------------

/// Save the current meeting session to a WIP file in the handoff directory.
///
/// This is called periodically (every 60 s) and after every slash command so
/// that a crash loses at most the last few seconds of work.
pub fn save_session_wip(dir: &Path, session: &MeetingSession) -> SimardResult<()> {
    fs::create_dir_all(dir).map_err(|e| SimardError::ArtifactIo {
        path: dir.to_path_buf(),
        reason: format!("creating handoff dir: {e}"),
    })?;
    let path = dir.join(MEETING_SESSION_WIP_FILENAME);
    let json = serde_json::to_string_pretty(session).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing WIP session: {e}"),
    })?;
    fs::write(&path, &json).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing WIP session: {e}"),
    })?;
    Ok(())
}

/// Load a previously saved WIP session from the handoff directory.
///
/// Returns `None` if no WIP file exists. The caller should prompt the user
/// for resume vs. fresh start.
pub fn load_session_wip(dir: &Path) -> SimardResult<Option<MeetingSession>> {
    let path = dir.join(MEETING_SESSION_WIP_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("reading WIP session: {e}"),
    })?;
    let session: MeetingSession =
        serde_json::from_str(&raw).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("parsing WIP session JSON: {e}"),
        })?;
    Ok(Some(session))
}

/// Remove the WIP file from the handoff directory.
///
/// Called on clean `/close` (after writing the final handoff artifact) and
/// when the user declines to resume a stale WIP session.
pub fn remove_session_wip(dir: &Path) -> SimardResult<()> {
    let path = dir.join(MEETING_SESSION_WIP_FILENAME);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("removing WIP session: {e}"),
        })?;
    }
    Ok(())
}
