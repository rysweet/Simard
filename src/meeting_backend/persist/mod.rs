//! Persistence for meeting transcripts and handoff artifacts.

use std::path::PathBuf;

use tracing::{debug, info, warn};

use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingHandoff, default_handoff_dir, write_meeting_handoff,
};

use super::types::{ConversationMessage, HandoffActionItem, MeetingTranscript};

/// Maximum length for a sanitized filename component.
pub(super) const MAX_FILENAME_LEN: usize = 128;

/// Sanitize a string for safe use as a filesystem name.
///
/// Strips path separators, `..`, null bytes, and control characters. Replaces
/// spaces and unsafe characters with underscores and caps length.
pub fn sanitize_filename(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '_',
            _ => c,
        })
        .collect();
    // Remove .. sequences
    let sanitized = sanitized.replace("..", "");
    // Trim leading/trailing underscores/dots
    let sanitized = sanitized
        .trim_matches(|c: char| c == '_' || c == '.')
        .to_string();
    if sanitized.is_empty() {
        return "meeting".to_string();
    }
    if sanitized.len() > MAX_FILENAME_LEN {
        sanitized[..MAX_FILENAME_LEN].to_string()
    } else {
        sanitized
    }
}

/// Directory for meeting transcripts.
///
/// Precedence ladder (issue #1906):
/// 1. `SIMARD_MEETINGS_DIR` — narrow override (preserves backward compat
///    with the legacy env idiom used by `tests_persist_extra`).
/// 2. `SIMARD_MEETINGS_ROOT` — alias for the narrow override; same
///    semantics, used by `tests/meeting_handoff_bundle.rs` and any operator
///    that prefers the `*_ROOT` naming.
/// 3. `SIMARD_STATE_ROOT/meetings` — broad override resolved through the
///    shared [`crate::state_root`] helper so a single env var relocates
///    every Simard subsystem together.
/// 4. `~/.simard/meetings/` — default.
///
/// The narrow vars deliberately win over the broad one so a session-scoped
/// override (e.g. a single test) can still pin a specific directory without
/// fighting a global `SIMARD_STATE_ROOT` set in the parent shell.
pub(super) fn meetings_dir() -> PathBuf {
    if let Some(override_path) = std::env::var_os("SIMARD_MEETINGS_DIR") {
        let s = override_path.to_string_lossy();
        if !s.trim().is_empty() {
            return PathBuf::from(override_path);
        }
    }
    if let Some(override_path) = std::env::var_os("SIMARD_MEETINGS_ROOT") {
        let s = override_path.to_string_lossy();
        if !s.trim().is_empty() {
            return PathBuf::from(override_path);
        }
    }
    crate::state_root::resolve_subdir("meetings")
}

/// Write a JSON transcript to `~/.simard/meetings/{timestamp}_{topic}.json`.
///
/// Creates the directory if it doesn't exist. Sets file permissions to 0o600
/// on Unix.
pub fn write_transcript(transcript: &MeetingTranscript) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(&transcript.topic);
    let filename = format!("{timestamp}_{safe_topic}.json");
    let path = dir.join(&filename);

    let json = serde_json::to_string_pretty(transcript).map_err(|e| {
        SimardError::ActionExecutionFailed {
            action: "serialize-transcript".to_string(),
            reason: e.to_string(),
        }
    })?;

    std::fs::write(&path, &json).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-transcript".to_string(),
        reason: e.to_string(),
    })?;

    // Set permissions to 0o600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set transcript permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting transcript written");
    Ok(path)
}

/// Write an auto-save transcript to `~/.simard/meetings/_autosave_{topic}.json`.
///
/// Overwrites the same file each turn. The final `write_transcript()` on
/// `/close` writes the canonical timestamped file.
pub fn write_auto_save(transcript: &MeetingTranscript) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let safe_topic = sanitize_filename(&transcript.topic);
    let filename = format!("_autosave_{safe_topic}.json");
    let path = dir.join(&filename);

    let json = serde_json::to_string_pretty(transcript).map_err(|e| {
        SimardError::ActionExecutionFailed {
            action: "serialize-autosave".to_string(),
            reason: e.to_string(),
        }
    })?;

    std::fs::write(&path, &json).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-autosave".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set autosave permissions: {e}");
        }
    }

    debug!(path = %path.display(), "Auto-save transcript written");
    Ok(path)
}

/// Optional enrichment fields carried into the persisted handoff.
///
/// Issue #1954 added `next_owner` and `artifacts` to `MeetingHandoff`.
/// Producer paths in `closing.rs` populate this struct so the persist
/// helpers don't grow yet another tuple of positional parameters.
#[derive(Clone, Debug, Default)]
pub struct HandoffEnrichment<'a> {
    /// Named owner expected to action this handoff next (e.g.
    /// `"engineer"`, `"ooda-curate"`, a GitHub handle). Set by the
    /// `/owner` slash command at the REPL or dashboard.
    pub next_owner: Option<&'a str>,
    /// Linked artifact pointers (transcript, bundle, markdown report,
    /// template agendas). Producers compute these before writing so
    /// downstream consumers can link without re-deriving paths.
    pub artifacts: Vec<crate::meeting_facilitator::HandoffArtifact>,
    /// Pre-built structured decisions (rationale + participants already
    /// extracted). When `Some`, replaces the string-list `decisions` argument
    /// of the writer — used by the closing path to thread non-placeholder
    /// rationale/participants through the partial-close fast-path. When
    /// `None`, decisions are reconstructed from `&[String]` via the
    /// existing extract helpers (legacy behaviour).
    pub structured_decisions: Option<Vec<crate::meeting_facilitator::MeetingDecision>>,
}

/// Write a `MeetingHandoff` artifact for OODA integration.
///
/// Serializes the full structured data extracted from the meeting session —
/// decisions, action items, open questions, participants, and themes — into the
/// handoff JSON. Falls back to sensible defaults when fields are empty.
pub fn write_handoff(
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
) -> SimardResult<()> {
    write_handoff_with_explicit(
        topic,
        summary,
        messages,
        action_items,
        decisions,
        &[],
        HandoffEnrichment::default(),
    )
}

/// Variant of [`write_handoff`] that accepts a list of operator-supplied
/// explicit open questions (recorded inline via `/question`). Explicit
/// questions are prepended to the inferred ones with `explicit=true`, and
/// inferred questions whose text duplicates an explicit one are dropped.
/// Issue #1730 seam (b). Extended in issue #1954 with `HandoffEnrichment`
/// carrying `next_owner`, `artifacts`, and pre-built structured decisions.
pub fn write_handoff_with_explicit(
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
    explicit_questions: &[String],
    enrichment: HandoffEnrichment<'_>,
) -> SimardResult<()> {
    let started_at = messages
        .first()
        .map(|m| m.timestamp.clone())
        .unwrap_or_default();
    let closed_at = chrono::Utc::now().to_rfc3339();

    let duration_secs = chrono::DateTime::parse_from_rfc3339(&started_at)
        .ok()
        .map(|start| {
            chrono::Utc::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0) as u64
        });

    // Convert backend HandoffActionItems to facilitator ActionItems for the handoff.
    let facilitator_actions: Vec<crate::meeting_facilitator::ActionItem> = action_items
        .iter()
        .map(|a| ActionItem {
            description: a.description.clone(),
            owner: a
                .assignee
                .clone()
                .unwrap_or_else(|| "unassigned".to_string()),
            priority: 0,
            due_description: a.deadline.clone(),
            linked_issue: None,
        })
        .collect();

    // Convert decision strings to MeetingDecision structs, extracting
    // rationale context from surrounding messages when available — unless
    // the producer already supplied pre-built structured decisions (issue
    // #1954, which uses this to thread non-placeholder rationale through
    // the partial-close fast-path).
    let facilitator_decisions: Vec<MeetingDecision> =
        if let Some(prebuilt) = enrichment.structured_decisions.clone() {
            prebuilt
        } else {
            decisions
                .iter()
                .map(|d| {
                    let rationale = extract::extract_decision_rationale_pub(d, messages);
                    MeetingDecision {
                        description: d.clone(),
                        rationale,
                        participants: extract::extract_decision_participants_pub(d, messages),
                    }
                })
                .collect()
        };

    // Extract open questions from message content; prepend explicit ones.
    let inferred_questions = extract_open_questions(messages);
    let mut open_questions: Vec<crate::meeting_facilitator::OpenQuestion> = explicit_questions
        .iter()
        .map(|q| crate::meeting_facilitator::OpenQuestion {
            text: q.clone(),
            explicit: true,
        })
        .collect();
    for q in inferred_questions {
        let lower = q.text.to_lowercase();
        if !open_questions
            .iter()
            .any(|e| e.text.to_lowercase() == lower)
        {
            open_questions.push(q);
        }
    }

    // Collect unique participants from messages.
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            super::types::Role::User => "operator",
            super::types::Role::Assistant => "simard",
            super::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    // Also include action item assignees.
    for a in action_items {
        if let Some(ref assignee) = a.assignee
            && !participants.contains(assignee)
        {
            participants.push(assignee.clone());
        }
    }

    // Extract themes from meeting content.
    let themes = extract_themes(messages);

    let handoff = MeetingHandoff {
        meeting_id: crate::meeting_facilitator::derive_meeting_id(&started_at, topic),
        topic: topic.to_string(),
        started_at,
        closed_at,
        decisions: facilitator_decisions,
        action_items: facilitator_actions,
        open_questions,
        processed: false,
        duration_secs,
        transcript: vec![summary.to_string()],
        participants,
        themes,
        transcript_path: None,
        next_owner: enrichment.next_owner.map(|s| s.to_string()),
        artifacts: enrichment.artifacts.clone(),
    };

    let dir = default_handoff_dir();
    write_meeting_handoff(&dir, &handoff)?;
    info!("Meeting handoff artifact written");
    Ok(())
}

/// Build a [`MeetingHandoff`] from a closing meeting and write it to the
/// per-meeting bundle directory under `~/.simard/meetings/<meeting_id>/`.
///
/// Returns the bundle directory path on success. The `started_at` timestamp
/// is taken from `started_at_override` when provided (to match the backend's
/// session-creation time) and otherwise inferred from the first message.
///
/// Does NOT touch the legacy `default_handoff_dir()` artifact — that is
/// still written by [`write_handoff`] for OODA queue compatibility.
#[allow(clippy::too_many_arguments)]
pub fn write_handoff_bundle(
    topic: &str,
    summary: &str,
    started_at_override: Option<&str>,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
    open_questions: Vec<crate::meeting_facilitator::OpenQuestion>,
    themes: Vec<String>,
    participants: Vec<String>,
    enrichment: HandoffEnrichment<'_>,
) -> SimardResult<std::path::PathBuf> {
    use crate::meeting_facilitator::{
        ActionItem as FacilitatorActionItem, MeetingDecision as FacilitatorDecision,
        derive_meeting_id, write_meeting_bundle,
    };

    let started_at = started_at_override
        .map(|s| s.to_string())
        .or_else(|| messages.first().map(|m| m.timestamp.clone()))
        .unwrap_or_default();
    let closed_at = chrono::Utc::now().to_rfc3339();
    let duration_secs = chrono::DateTime::parse_from_rfc3339(&started_at)
        .ok()
        .map(|start| {
            chrono::Utc::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0) as u64
        });

    let facilitator_actions: Vec<FacilitatorActionItem> = action_items
        .iter()
        .map(|a| FacilitatorActionItem {
            description: a.description.clone(),
            owner: a
                .assignee
                .clone()
                .unwrap_or_else(|| "unassigned".to_string()),
            priority: a.priority.unwrap_or(0),
            due_description: a.deadline.clone(),
            linked_issue: None,
        })
        .collect();

    // Honour pre-built structured decisions when the producer supplied
    // them (issue #1954) — otherwise rebuild from the string list using
    // the heuristic extractors.
    let facilitator_decisions: Vec<FacilitatorDecision> =
        if let Some(prebuilt) = enrichment.structured_decisions.clone() {
            prebuilt
        } else {
            decisions
                .iter()
                .map(|d| FacilitatorDecision {
                    description: d.clone(),
                    rationale: extract::extract_decision_rationale_pub(d, messages),
                    participants: extract::extract_decision_participants_pub(d, messages),
                })
                .collect()
        };

    let meeting_id = derive_meeting_id(&started_at, topic);
    let mut handoff = MeetingHandoff {
        meeting_id,
        topic: topic.to_string(),
        started_at,
        closed_at,
        decisions: facilitator_decisions,
        action_items: facilitator_actions,
        open_questions,
        processed: false,
        duration_secs,
        transcript: vec![summary.to_string()],
        participants,
        themes,
        transcript_path: None,
        next_owner: enrichment.next_owner.map(|s| s.to_string()),
        artifacts: enrichment.artifacts.clone(),
    };

    let lines: Vec<crate::meeting_facilitator::BundleTranscriptLine> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                super::types::Role::User => "operator",
                super::types::Role::Assistant => "simard",
                super::types::Role::System => "system",
            };
            crate::meeting_facilitator::BundleTranscriptLine {
                role: role.to_string(),
                content: m.content.clone(),
                timestamp: m.timestamp.clone(),
            }
        })
        .collect();

    let dir = write_meeting_bundle(&mut handoff, &lines)?;
    info!(meeting_id = %handoff.meeting_id, dir = %dir.display(), "Meeting handoff bundle written");
    Ok(dir)
}

mod markdown;
pub use markdown::{write_handoff_markdown_report, write_markdown_export};
mod cognitive;
mod extract;
mod json_sibling;
mod memory_records;
mod templates;

pub use memory_records::{MEMORY_RECORDS_FILENAME, write_meeting_memory_records};

pub use cognitive::{store_cognitive_memory, store_enriched_cognitive_memory};
// re-exported for cfg(test) consumers in meeting_backend/tests_persist.rs (false-positive of clippy unused_imports on lib pass — see #1405)
#[allow(unused_imports)]
pub(crate) use extract::{
    clean_action_description, extract_assignee, extract_deadline, split_sentences,
};
pub use extract::{
    extract_action_items, extract_decision_participants_pub, extract_decision_rationale_pub,
    extract_decisions, extract_open_questions, extract_themes, link_action_items_to_goals,
};
pub use json_sibling::{JsonHandoffActionItem, JsonHandoffSibling};
pub use templates::{MeetingTemplate, TEMPLATES, find_template};

// ─── Public wrappers around extract helpers used by the inline /action ───
// command path (issue #1730 seam (b)). Kept thin so the heuristic logic
// stays in one place and any future tweak to the extractors automatically
// flows through to operator-typed action items.

/// Public wrapper around [`extract::extract_assignee`] for use by the
/// `MeetingBackend::push_explicit_action_item` inline-recording path.
pub fn extract_assignee_pub(sentence: &str) -> Option<String> {
    extract::extract_assignee(sentence)
}

/// Public wrapper around [`extract::extract_deadline`] for use by the
/// `MeetingBackend::push_explicit_action_item` inline-recording path.
pub fn extract_deadline_pub(lower_sentence: &str) -> Option<String> {
    extract::extract_deadline(lower_sentence)
}

/// Public wrapper around [`extract::clean_action_description`] for use by
/// the `MeetingBackend::push_explicit_action_item` inline-recording path.
pub fn clean_action_description_pub(sentence: &str) -> String {
    extract::clean_action_description(sentence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, Role};
    use serial_test::serial;

    fn temp_meetings_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("persist-mod-{label}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_messages() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage {
                role: Role::User,
                content: "Let's discuss testing.".to_string(),
                timestamp: "2026-01-15T10:00:00Z".to_string(),
            },
            ConversationMessage {
                role: Role::Assistant,
                content: "We decided to adopt TDD.".to_string(),
                timestamp: "2026-01-15T10:01:00Z".to_string(),
            },
        ]
    }

    // ── sanitize_filename ───────────────────────────────────────────

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize_filename("a/b\\c"), "a_b_c");
    }

    #[test]
    fn sanitize_strips_null_bytes_and_control_chars() {
        assert_eq!(sanitize_filename("ab\0cd\x01ef"), "abcdef");
    }

    #[test]
    fn sanitize_removes_dot_dot_sequences() {
        assert_eq!(sanitize_filename("../etc/passwd"), "etc_passwd");
    }

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
    }

    #[test]
    fn sanitize_trims_leading_trailing_underscores_dots() {
        assert_eq!(sanitize_filename("___hello___"), "hello");
        assert_eq!(sanitize_filename("...hello..."), "hello");
    }

    #[test]
    fn sanitize_empty_input_returns_meeting() {
        assert_eq!(sanitize_filename(""), "meeting");
        assert_eq!(sanitize_filename("///"), "meeting");
    }

    #[test]
    fn sanitize_caps_length() {
        let long = "a".repeat(200);
        let result = sanitize_filename(&long);
        assert!(result.len() <= MAX_FILENAME_LEN);
    }

    #[test]
    fn sanitize_spaces_become_underscores() {
        assert_eq!(sanitize_filename("sprint planning"), "sprint_planning");
    }

    // ── write_transcript ────────────────────────────────────────────

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_transcript_creates_json_file() {
        let dir = temp_meetings_dir("transcript");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let transcript = crate::meeting_backend::types::MeetingTranscript {
            topic: "Sprint retro".to_string(),
            started_at: "2026-01-15T10:00:00Z".to_string(),
            closed_at: "2026-01-15T11:00:00Z".to_string(),
            duration_secs: 3600,
            summary: "Went well.".to_string(),
            messages: sample_messages(),
        };
        let path = write_transcript(&transcript).unwrap();
        assert!(path.exists());
        assert!(path.extension().is_some_and(|e| e == "json"));

        let contents = std::fs::read_to_string(&path).unwrap();
        let rt: crate::meeting_backend::types::MeetingTranscript =
            serde_json::from_str(&contents).unwrap();
        assert_eq!(rt.topic, "Sprint retro");

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_transcript_error_on_unwritable_dir() {
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", "/proc/1/nonexistent") };
        let transcript = crate::meeting_backend::types::MeetingTranscript {
            topic: "fail".to_string(),
            started_at: String::new(),
            closed_at: String::new(),
            duration_secs: 0,
            summary: String::new(),
            messages: vec![],
        };
        assert!(write_transcript(&transcript).is_err());
        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
    }

    // ── write_auto_save ─────────────────────────────────────────────

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_auto_save_overwrites_same_file() {
        let dir = temp_meetings_dir("autosave");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let mut transcript = crate::meeting_backend::types::MeetingTranscript {
            topic: "auto-topic".to_string(),
            started_at: "2026-01-15T10:00:00Z".to_string(),
            closed_at: "2026-01-15T11:00:00Z".to_string(),
            duration_secs: 60,
            summary: "first".to_string(),
            messages: vec![],
        };
        let path1 = write_auto_save(&transcript).unwrap();
        transcript.summary = "second".to_string();
        let path2 = write_auto_save(&transcript).unwrap();
        assert_eq!(path1, path2, "auto-save should overwrite same file");

        let contents = std::fs::read_to_string(&path2).unwrap();
        assert!(contents.contains("second"));

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── write_handoff ───────────────────────────────────────────────

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_success_writes_artifact() {
        let dir = temp_meetings_dir("handoff");
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", &dir) };

        let msgs = sample_messages();
        let items = vec![HandoffActionItem {
            description: "Write tests".to_string(),
            assignee: Some("Alice".to_string()),
            deadline: Some("by friday".to_string()),
            linked_goal: None,
            priority: None,
        }];
        let result = write_handoff("Sprint", "Summary", &msgs, &items, &["Use TDD".to_string()]);
        assert!(result.is_ok(), "write_handoff should succeed: {result:?}");

        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("handoff-"))
            .collect();
        assert!(!files.is_empty(), "handoff file should exist");

        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_error_on_read_only_dir() {
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", "/proc/1/no_such_dir") };
        let result = write_handoff("topic", "summary", &[], &[], &[]);
        assert!(
            result.is_err(),
            "write_handoff should surface error, not silently drop"
        );
        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_empty_messages() {
        let dir = temp_meetings_dir("handoff-empty");
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", &dir) };

        let result = write_handoff("empty", "No messages", &[], &[], &[]);
        assert!(result.is_ok(), "empty-message handoff should succeed");

        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── write_handoff_bundle ────────────────────────────────────────

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_bundle_creates_sibling_files() {
        let dir = temp_meetings_dir("bundle");
        unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", &dir) };

        let msgs = sample_messages();
        let items = vec![HandoffActionItem {
            description: "Deploy".to_string(),
            assignee: Some("Bob".to_string()),
            deadline: None,
            linked_goal: None,
            priority: Some(1),
        }];
        let decisions = vec!["Adopt TDD".to_string()];
        let oq = vec![crate::meeting_facilitator::OpenQuestion {
            text: "What about coverage?".to_string(),
            explicit: true,
        }];
        let themes = vec!["testing".to_string()];
        let participants = vec!["operator".to_string(), "simard".to_string()];

        let bundle_dir = write_handoff_bundle(
            "Sprint",
            "Summary",
            Some("2026-01-15T10:00:00Z"),
            &msgs,
            &items,
            &decisions,
            oq,
            themes,
            participants,
            HandoffEnrichment::default(),
        )
        .unwrap();

        assert!(
            bundle_dir.join("meeting_handoff.json").is_file(),
            "bundle must contain meeting_handoff.json"
        );
        assert!(
            bundle_dir.join("transcript.json").is_file(),
            "bundle must contain transcript.json"
        );

        unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_bundle_error_on_unwritable() {
        unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", "/proc/1/nowrite") };

        let result = write_handoff_bundle(
            "topic",
            "summary",
            None,
            &[],
            &[],
            &[],
            vec![],
            vec![],
            vec![],
            HandoffEnrichment::default(),
        );
        assert!(
            result.is_err(),
            "bundle write to unwritable dir should error, not silently drop"
        );

        unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };
    }

    // ── extract_assignee_pub / extract_deadline_pub ─────────────────

    #[test]
    fn extract_assignee_pub_detects_name() {
        let result = extract_assignee_pub("Alice will write the tests");
        assert_eq!(result, Some("Alice".to_string()));
    }

    #[test]
    fn extract_assignee_pub_none_without_name() {
        let result = extract_assignee_pub("we should do this");
        assert_eq!(result, None);
    }

    #[test]
    fn extract_deadline_pub_detects_by_friday() {
        let result = extract_deadline_pub("finish by friday please");
        assert_eq!(result, Some("by friday".to_string()));
    }

    #[test]
    fn extract_deadline_pub_none_when_absent() {
        let result = extract_deadline_pub("no deadline here");
        assert_eq!(result, None);
    }
}
