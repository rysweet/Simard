//! Meeting handoff artifacts — written when a meeting closes, consumed by
//! the engineer loop and the `act-on-decisions` CLI subcommand.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

use super::types::{ActionItem, MeetingDecision, MeetingSession, OpenQuestion};

/// Well-known filename for meeting handoff artifacts.
pub const MEETING_HANDOFF_FILENAME: &str = "meeting_handoff.json";

/// Well-known filename for the work-in-progress session snapshot.
pub const MEETING_SESSION_WIP_FILENAME: &str = "meeting_session_wip.json";

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
    pub started_at: String,
    pub closed_at: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub open_questions: Vec<OpenQuestion>,
    #[serde(default)]
    pub processed: bool,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    #[serde(default)]
    pub transcript: Vec<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    /// High-level themes or recurring topics identified during the meeting.
    #[serde(default)]
    pub themes: Vec<String>,
}

/// Check whether a note looks like a rhetorical question (short, common
/// filler phrases) so we can filter it out of open questions.
fn is_rhetorical(note: &str) -> bool {
    let trimmed = note.trim().trim_end_matches('?').trim();
    // Very short questions are usually rhetorical ("Why not?", "Right?").
    if trimmed.len() < 15 {
        return true;
    }
    let lower = note.trim().to_lowercase();
    let rhetorical_patterns = [
        "right?",
        "isn't it?",
        "aren't they?",
        "don't you think?",
        "wouldn't you say?",
        "isn't that so?",
        "why not?",
        "who knows?",
        "who cares?",
        "what else?",
        "so what?",
        "how about that?",
    ];
    rhetorical_patterns
        .iter()
        .any(|p| lower == *p || lower.ends_with(&format!(" {p}")))
}

/// Prefixes (case-insensitive) that mark a note as an explicit open question
/// even when it does not contain a `?`.
const OPEN_QUESTION_PREFIXES: &[&str] = &["open:", "todo:", "question:", "tbd:", "unresolved:"];

/// Returns `true` if `note` should be extracted as an open question.
pub(super) fn is_open_question(note: &str) -> bool {
    let lower = note.trim().to_lowercase();

    // Explicit markers always count.
    for prefix in OPEN_QUESTION_PREFIXES {
        if lower.starts_with(prefix) {
            return true;
        }
    }

    // Notes with `?` count unless they look rhetorical.
    if note.contains('?') && !is_rhetorical(note) {
        return true;
    }

    false
}

impl MeetingHandoff {
    /// Create a handoff from a closed meeting session.
    ///
    /// Open questions are extracted from two sources:
    /// 1. **Explicit** — questions added via `/question` during the meeting.
    /// 2. **Inferred** — notes containing `?` (unless rhetorical) or notes
    ///    starting with explicit markers (`OPEN:`, `TODO:`, `QUESTION:`,
    ///    `TBD:`, `UNRESOLVED:`).
    pub fn from_session(session: &MeetingSession) -> Self {
        // Explicit questions from /question command.
        let mut open_questions: Vec<OpenQuestion> = session
            .explicit_questions
            .iter()
            .map(|q| OpenQuestion {
                text: q.clone(),
                explicit: true,
            })
            .collect();

        // Inferred questions from notes heuristics.
        let inferred: Vec<OpenQuestion> = session
            .notes
            .iter()
            .filter(|n| is_open_question(n))
            .map(|n| OpenQuestion {
                text: n.clone(),
                explicit: false,
            })
            .collect();
        open_questions.extend(inferred);

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

        // Extract themes from notes; use decision/action text if notes
        // are empty (common in the backend code path which uses messages, not notes).
        // Explicit /theme entries from session always take priority.
        let inferred: Vec<String> = {
            let mut t = Self::extract_themes_from_notes(&session.notes);
            if t.is_empty() {
                let fallback_texts: Vec<String> = session
                    .decisions
                    .iter()
                    .map(|d| d.description.clone())
                    .chain(session.action_items.iter().map(|a| a.description.clone()))
                    .collect();
                t = Self::extract_themes_from_notes(&fallback_texts);
            }
            t
        };
        let mut themes: Vec<String> = session.themes.clone();
        for t in inferred {
            let lower = t.to_lowercase();
            if !themes.iter().any(|e| e.to_lowercase() == lower) {
                themes.push(t);
            }
        }

        Self {
            topic: session.topic.clone(),
            started_at: session.started_at.clone(),
            closed_at: Utc::now().to_rfc3339(),
            decisions: session.decisions.clone(),
            action_items: session.action_items.clone(),
            open_questions,
            processed: false,
            duration_secs,
            transcript,
            participants: all_participants,
            themes,
        }
    }

    /// Extract recurring theme keywords from meeting notes.
    fn extract_themes_from_notes(notes: &[String]) -> Vec<String> {
        use std::collections::HashMap;

        const STOP_WORDS: &[&str] = &[
            "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with",
            "by", "is", "it", "that", "this", "was", "are", "be", "has", "have", "had", "not",
            "we", "they", "you", "will", "can", "should", "would", "could", "do", "does", "did",
            "from", "about", "into", "out", "if", "then", "so", "up", "one", "all", "been", "just",
            "also", "than", "like", "more", "some", "what", "when", "how", "who", "which", "there",
            "their", "our", "i", "my", "me", "your", "its",
        ];

        let mut word_freq: HashMap<String, usize> = HashMap::new();
        for note in notes {
            let mut seen = std::collections::HashSet::new();
            let words: Vec<String> = note
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '-')
                .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
                .map(String::from)
                .collect();
            for w in words {
                if seen.insert(w.clone()) {
                    *word_freq.entry(w).or_insert(0) += 1;
                }
            }
        }

        let min_freq = 2;
        let mut themes: Vec<(String, usize)> = word_freq
            .into_iter()
            .filter(|(_, count)| *count >= min_freq)
            .collect();
        themes.sort_by_key(|a| std::cmp::Reverse(a.1));
        themes.truncate(10);
        themes.into_iter().map(|(word, _)| word).collect()
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
pub(super) fn find_newest_handoff(dir: &Path) -> Option<PathBuf> {
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
