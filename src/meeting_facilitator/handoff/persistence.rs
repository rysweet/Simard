use std::fs;
use std::path::{Path, PathBuf};

use super::{
    MEETING_HANDOFF_FILENAME, MEETING_SESSION_WIP_FILENAME, MeetingHandoff, derive_meeting_id,
};
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

// ---------------------------------------------------------------------------
// Per-meeting handoff bundles (~/.simard/meetings/<meeting_id>/)
// ---------------------------------------------------------------------------

/// Canonical filename for the structured handoff JSON inside a per-meeting
/// bundle directory. Matches the spec for downstream consumers.
pub const BUNDLE_HANDOFF_JSON: &str = "meeting_handoff.json";

/// Canonical filename for the human-readable handoff markdown inside a
/// per-meeting bundle directory.
pub const BUNDLE_HANDOFF_MD: &str = "meeting_handoff.md";

/// Canonical filename for the verbatim conversation transcript inside a
/// per-meeting bundle directory.
pub const BUNDLE_TRANSCRIPT_JSON: &str = "transcript.json";

/// Root directory for per-meeting handoff bundles.
///
/// Honours `SIMARD_MEETINGS_ROOT` when set (used by tests for isolation),
/// otherwise falls back to `~/.simard/meetings/`.
pub fn default_bundle_root() -> PathBuf {
    if let Some(p) = std::env::var_os("SIMARD_MEETINGS_ROOT") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".simard/meetings")
}

/// Path to the bundle directory for a given `meeting_id` under
/// [`default_bundle_root`].
pub fn meeting_bundle_dir(meeting_id: &str) -> PathBuf {
    default_bundle_root().join(meeting_id)
}

/// Path to `meeting_handoff.json` inside the bundle for `meeting_id`.
pub fn bundle_handoff_path(meeting_id: &str) -> PathBuf {
    meeting_bundle_dir(meeting_id).join(BUNDLE_HANDOFF_JSON)
}

/// Path to `meeting_handoff.md` inside the bundle for `meeting_id`.
pub fn bundle_markdown_path(meeting_id: &str) -> PathBuf {
    meeting_bundle_dir(meeting_id).join(BUNDLE_HANDOFF_MD)
}

/// Path to `transcript.json` inside the bundle for `meeting_id`.
pub fn bundle_transcript_path(meeting_id: &str) -> PathBuf {
    meeting_bundle_dir(meeting_id).join(BUNDLE_TRANSCRIPT_JSON)
}

/// A single conversation line for the per-meeting transcript artifact.
///
/// This is intentionally a thin, public-API-safe shape so callers in the
/// `meeting_backend` crate can convert their richer `ConversationMessage`
/// values without exposing backend-private types here.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BundleTranscriptLine {
    /// Logical role: `operator`, `simard`, or `system`.
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Write a self-contained per-meeting handoff bundle.
///
/// Creates `<root>/<meeting_id>/` and writes:
/// * `meeting_handoff.json` — the canonical structured artifact.
/// * `meeting_handoff.md` — a human-readable rendering.
/// * `transcript.json` — verbatim conversation lines (may be empty).
///
/// The handoff's `meeting_id` is filled in (and any embedded
/// `transcript_path` is rewritten to point inside the bundle) before the
/// JSON is serialized so the artifact is internally consistent.
///
/// Returns the bundle directory path.
pub fn write_meeting_bundle(
    handoff: &mut MeetingHandoff,
    transcript_lines: &[BundleTranscriptLine],
) -> SimardResult<PathBuf> {
    if handoff.meeting_id.is_empty() {
        handoff.meeting_id = derive_meeting_id(&handoff.started_at, &handoff.topic);
    }
    let dir = meeting_bundle_dir(&handoff.meeting_id);
    fs::create_dir_all(&dir).map_err(|e| SimardError::ArtifactIo {
        path: dir.clone(),
        reason: format!("creating bundle dir: {e}"),
    })?;

    // Write transcript first so we know its on-disk path before serializing
    // the handoff (the handoff records `transcript_path`).
    let transcript_path = dir.join(BUNDLE_TRANSCRIPT_JSON);
    let transcript_json = serde_json::to_string_pretty(&BundleTranscript {
        meeting_id: handoff.meeting_id.clone(),
        topic: handoff.topic.clone(),
        started_at: handoff.started_at.clone(),
        closed_at: handoff.closed_at.clone(),
        lines: transcript_lines.to_vec(),
    })
    .map_err(|e| SimardError::ArtifactIo {
        path: transcript_path.clone(),
        reason: format!("serializing transcript: {e}"),
    })?;
    fs::write(&transcript_path, &transcript_json).map_err(|e| SimardError::ArtifactIo {
        path: transcript_path.clone(),
        reason: format!("writing transcript: {e}"),
    })?;
    handoff.transcript_path = Some(transcript_path.to_string_lossy().to_string());

    // Write the structured handoff JSON.
    let handoff_path = dir.join(BUNDLE_HANDOFF_JSON);
    let handoff_json =
        serde_json::to_string_pretty(handoff).map_err(|e| SimardError::ArtifactIo {
            path: handoff_path.clone(),
            reason: format!("serializing handoff: {e}"),
        })?;
    fs::write(&handoff_path, &handoff_json).map_err(|e| SimardError::ArtifactIo {
        path: handoff_path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;

    // Write the human-readable markdown.
    let md_path = dir.join(BUNDLE_HANDOFF_MD);
    let md = render_bundle_markdown(handoff, transcript_lines);
    fs::write(&md_path, &md).map_err(|e| SimardError::ArtifactIo {
        path: md_path.clone(),
        reason: format!("writing markdown: {e}"),
    })?;

    // 0o600 on Unix — handoffs may contain operator-private text.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        for p in [&handoff_path, &md_path, &transcript_path] {
            if let Err(e) = std::fs::set_permissions(p, perms.clone()) {
                tracing::warn!(path = %p.display(), error = %e, "failed to set 0o600 on bundle file");
            }
        }
    }

    tracing::info!(
        meeting_id = %handoff.meeting_id,
        bundle = %dir.display(),
        "Meeting handoff bundle written"
    );
    Ok(dir)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BundleTranscript {
    meeting_id: String,
    topic: String,
    started_at: String,
    closed_at: String,
    lines: Vec<BundleTranscriptLine>,
}

fn render_bundle_markdown(
    handoff: &MeetingHandoff,
    transcript_lines: &[BundleTranscriptLine],
) -> String {
    use std::fmt::Write as _;
    let mut md = String::with_capacity(4096);

    let _ = writeln!(md, "# Meeting handoff: {}", handoff.topic);
    let _ = writeln!(md);
    let _ = writeln!(md, "- **Meeting ID:** `{}`", handoff.meeting_id);
    let _ = writeln!(md, "- **Started:** {}", handoff.started_at);
    let _ = writeln!(md, "- **Ended:** {}", handoff.closed_at);
    if let Some(secs) = handoff.duration_secs {
        let _ = writeln!(md, "- **Duration:** {secs}s");
    }
    if !handoff.participants.is_empty() {
        let _ = writeln!(
            md,
            "- **Participants:** {}",
            handoff.participants.join(", ")
        );
    }
    if let Some(ref p) = handoff.transcript_path {
        let _ = writeln!(md, "- **Transcript:** `{p}`");
    }
    let _ = writeln!(md);

    let _ = writeln!(md, "## Decisions");
    let _ = writeln!(md);
    if handoff.decisions.is_empty() {
        let _ = writeln!(md, "_None recorded._");
    } else {
        for (i, d) in handoff.decisions.iter().enumerate() {
            let _ = writeln!(md, "{}. **{}**", i + 1, d.description);
            if !d.rationale.is_empty() {
                let _ = writeln!(md, "   - *Rationale:* {}", d.rationale);
            }
            if !d.participants.is_empty() {
                let _ = writeln!(md, "   - *By:* {}", d.participants.join(", "));
            }
        }
    }
    let _ = writeln!(md);

    let _ = writeln!(md, "## Action items");
    let _ = writeln!(md);
    if handoff.action_items.is_empty() {
        let _ = writeln!(md, "_None recorded._");
    } else {
        let _ = writeln!(md, "| # | Owner | Description | Due | Linked issue |");
        let _ = writeln!(md, "|---|-------|-------------|-----|--------------|");
        for (i, a) in handoff.action_items.iter().enumerate() {
            let due = a.due_description.as_deref().unwrap_or("—");
            let issue = a.linked_issue.as_deref().unwrap_or("—");
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} |",
                i + 1,
                a.owner,
                a.description.replace('|', "\\|"),
                due.replace('|', "\\|"),
                issue.replace('|', "\\|"),
            );
        }
    }
    let _ = writeln!(md);

    let _ = writeln!(md, "## Open questions");
    let _ = writeln!(md);
    if handoff.open_questions.is_empty() {
        let _ = writeln!(md, "_None recorded._");
    } else {
        for q in &handoff.open_questions {
            let tag = if q.explicit { " *(explicit)*" } else { "" };
            let _ = writeln!(md, "- {}{}", q.text, tag);
        }
    }
    let _ = writeln!(md);

    if !handoff.themes.is_empty() {
        let _ = writeln!(md, "## Themes");
        let _ = writeln!(md);
        for t in &handoff.themes {
            let _ = writeln!(md, "- {t}");
        }
        let _ = writeln!(md);
    }

    if !transcript_lines.is_empty() {
        let _ = writeln!(md, "## Transcript");
        let _ = writeln!(md);
        for line in transcript_lines {
            let role = match line.role.as_str() {
                "operator" | "user" => "**Operator**",
                "simard" | "assistant" => "**Simard**",
                _ => "**System**",
            };
            let _ = writeln!(md, "{role} ({}): {}", line.timestamp, line.content);
            let _ = writeln!(md);
        }
    }

    md
}

#[cfg(test)]
mod bundle_tests {
    use super::*;
    use crate::meeting_facilitator::{ActionItem, MeetingDecision, OpenQuestion};
    use serial_test::serial;

    fn temp_root(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{label}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_handoff() -> MeetingHandoff {
        MeetingHandoff {
            meeting_id: String::new(), // exercise auto-fill
            topic: "Sprint planning".to_string(),
            started_at: "2026-05-13T07:00:00Z".to_string(),
            closed_at: "2026-05-13T07:30:00Z".to_string(),
            decisions: vec![MeetingDecision {
                description: "Adopt structured handoff bundles".to_string(),
                rationale: "Downstream engineer loop needs a stable shape".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: vec![ActionItem {
                description: "Wire bundle writer into close() flow".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("by friday".to_string()),
                linked_issue: Some("rysweet/Simard#1730".to_string()),
            }],
            open_questions: vec![OpenQuestion {
                text: "Should we keep the legacy handoff dir for OODA?".to_string(),
                explicit: true,
            }],
            processed: false,
            duration_secs: Some(1800),
            transcript: vec!["operator: hi".to_string()],
            participants: vec!["alice".to_string(), "bob".to_string()],
            themes: vec!["handoff".to_string()],
            transcript_path: None,
        }
    }

    fn sample_lines() -> Vec<BundleTranscriptLine> {
        vec![
            BundleTranscriptLine {
                role: "operator".to_string(),
                content: "Let's plan the handoff.".to_string(),
                timestamp: "2026-05-13T07:00:01Z".to_string(),
            },
            BundleTranscriptLine {
                role: "simard".to_string(),
                content: "Agreed — what should the bundle contain?".to_string(),
                timestamp: "2026-05-13T07:00:05Z".to_string(),
            },
        ]
    }

    #[test]
    #[serial(simard_meetings_root_env)]
    fn write_meeting_bundle_creates_canonical_files() {
        let root = temp_root("bundle-canonical");
        // SAFETY: tests in this binary that touch SIMARD_MEETINGS_ROOT serialize
        // via the temp_root suffix being unique per call; we still need the env
        // override during the brief write. set_var is unsafe in 2024-edition.
        unsafe {
            std::env::set_var("SIMARD_MEETINGS_ROOT", &root);
        }
        let mut handoff = sample_handoff();
        let lines = sample_lines();

        let dir = write_meeting_bundle(&mut handoff, &lines).expect("write bundle");

        assert_eq!(dir, root.join(&handoff.meeting_id));
        assert!(dir.join("meeting_handoff.json").is_file());
        assert!(dir.join("meeting_handoff.md").is_file());
        assert!(dir.join("transcript.json").is_file());

        // meeting_id was synthesized
        assert!(handoff.meeting_id.starts_with("20260513T070000Z-"));
        // transcript_path now points inside the bundle
        let tp = handoff.transcript_path.as_deref().unwrap();
        assert!(tp.ends_with("transcript.json"), "got transcript_path={tp}");

        unsafe {
            std::env::remove_var("SIMARD_MEETINGS_ROOT");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    #[serial(simard_meetings_root_env)]
    fn write_meeting_bundle_round_trips_handoff_json() {
        let root = temp_root("bundle-roundtrip");
        unsafe {
            std::env::set_var("SIMARD_MEETINGS_ROOT", &root);
        }
        let mut handoff = sample_handoff();
        let dir = write_meeting_bundle(&mut handoff, &sample_lines()).expect("write bundle");

        let raw = std::fs::read_to_string(dir.join("meeting_handoff.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["meeting_id"], handoff.meeting_id);
        assert_eq!(parsed["topic"], "Sprint planning");
        assert_eq!(parsed["started_at"], "2026-05-13T07:00:00Z");
        assert_eq!(parsed["closed_at"], "2026-05-13T07:30:00Z");
        assert!(parsed["decisions"].as_array().unwrap().len() == 1);
        assert!(parsed["action_items"].as_array().unwrap().len() == 1);
        assert_eq!(parsed["action_items"][0]["owner"], "bob");
        assert_eq!(
            parsed["action_items"][0]["linked_issue"],
            "rysweet/Simard#1730"
        );
        assert!(parsed["open_questions"].as_array().unwrap().len() == 1);
        assert!(parsed["transcript_path"].is_string());

        // Strict round-trip via the typed struct.
        let typed: MeetingHandoff = serde_json::from_str(&raw).unwrap();
        assert_eq!(typed.meeting_id, handoff.meeting_id);
        assert_eq!(
            typed.action_items[0].linked_issue.as_deref(),
            Some("rysweet/Simard#1730")
        );

        unsafe {
            std::env::remove_var("SIMARD_MEETINGS_ROOT");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    #[serial(simard_meetings_root_env)]
    fn write_meeting_bundle_markdown_contains_sections() {
        let root = temp_root("bundle-md");
        unsafe {
            std::env::set_var("SIMARD_MEETINGS_ROOT", &root);
        }
        let mut handoff = sample_handoff();
        let dir = write_meeting_bundle(&mut handoff, &sample_lines()).expect("write bundle");

        let md = std::fs::read_to_string(dir.join("meeting_handoff.md")).unwrap();
        assert!(md.contains("# Meeting handoff: Sprint planning"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("## Action items"));
        assert!(md.contains("## Open questions"));
        assert!(md.contains("rysweet/Simard#1730"));
        assert!(md.contains("Wire bundle writer into close() flow"));

        unsafe {
            std::env::remove_var("SIMARD_MEETINGS_ROOT");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn legacy_handoff_without_meeting_id_or_linked_issue_deserializes() {
        // Old artifact missing meeting_id, linked_issue, transcript_path —
        // also using the legacy `closed_at` name for the end timestamp.
        let json = r#"{
            "topic": "Legacy",
            "started_at": "2025-01-01T00:00:00Z",
            "closed_at": "2025-01-01T00:30:00Z",
            "decisions": [],
            "action_items": [
                {"description": "Old item", "owner": "alice", "priority": 1, "due_description": null}
            ],
            "open_questions": []
        }"#;
        let h: MeetingHandoff = serde_json::from_str(json).unwrap();
        assert!(h.meeting_id.is_empty());
        assert!(h.transcript_path.is_none());
        assert_eq!(h.action_items[0].linked_issue, None);
    }

    #[test]
    fn derive_meeting_id_is_stable_for_same_inputs() {
        let a = derive_meeting_id("2026-05-13T07:00:00Z", "Sprint planning!");
        let b = derive_meeting_id("2026-05-13T07:00:00Z", "Sprint planning!");
        assert_eq!(a, b);
        assert!(a.starts_with("20260513T070000Z-"));
        // Slug is filesystem-safe.
        assert!(!a.contains('!'));
        assert!(!a.contains(' '));
    }

    #[test]
    fn derive_meeting_id_handles_empty_topic_and_invalid_started_at() {
        let id = derive_meeting_id("not-a-date", "");
        // Should not panic; should produce a 16-char timestamp prefix even
        // when started_at is bad.
        assert!(id.len() >= 16);
    }
}
