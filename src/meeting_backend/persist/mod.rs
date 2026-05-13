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

/// Directory for meeting transcripts: `$SIMARD_MEETINGS_DIR` if set, else
/// `~/.simard/meetings/`.
///
/// The env var override mirrors the `SIMARD_HANDOFF_DIR` idiom in
/// [`meeting_facilitator::default_handoff_dir`] so tests (and operators) can
/// redirect meeting artifacts to a tempdir without touching `$HOME`.
pub(super) fn meetings_dir() -> PathBuf {
    if let Some(override_path) = std::env::var_os("SIMARD_MEETINGS_DIR") {
        return PathBuf::from(override_path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".simard/meetings")
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
    write_handoff_with_explicit(topic, summary, messages, action_items, decisions, &[])
}

/// Variant of [`write_handoff`] that accepts a list of operator-supplied
/// explicit open questions (recorded inline via `/question`). Explicit
/// questions are prepended to the inferred ones with `explicit=true`, and
/// inferred questions whose text duplicates an explicit one are dropped.
/// Issue #1730 seam (b).
pub fn write_handoff_with_explicit(
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
    explicit_questions: &[String],
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
    // rationale context from surrounding messages when available.
    let facilitator_decisions: Vec<MeetingDecision> = decisions
        .iter()
        .map(|d| {
            let rationale = extract::extract_decision_rationale_pub(d, messages);
            MeetingDecision {
                description: d.clone(),
                rationale,
                participants: extract::extract_decision_participants_pub(d, messages),
            }
        })
        .collect();

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

    let facilitator_decisions: Vec<FacilitatorDecision> = decisions
        .iter()
        .map(|d| FacilitatorDecision {
            description: d.clone(),
            rationale: extract::extract_decision_rationale_pub(d, messages),
            participants: extract::extract_decision_participants_pub(d, messages),
        })
        .collect();

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
mod templates;

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
