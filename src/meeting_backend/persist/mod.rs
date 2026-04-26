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

/// Directory for meeting transcripts: `~/.simard/meetings/`.
pub(super) fn meetings_dir() -> PathBuf {
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

    // Extract open questions from message content.
    let open_questions = extract_open_questions(messages);

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
    };

    let dir = default_handoff_dir();
    write_meeting_handoff(&dir, &handoff)?;
    info!("Meeting handoff artifact written");
    Ok(())
}

mod markdown;
pub use markdown::{write_handoff_markdown_report, write_markdown_export};
mod cognitive;
mod extract;
mod templates;

pub use cognitive::{store_cognitive_memory, store_enriched_cognitive_memory};
pub(crate) use extract::{
    clean_action_description, extract_assignee, extract_deadline, split_sentences,
};
pub use extract::{
    extract_action_items, extract_decision_participants_pub, extract_decision_rationale_pub,
    extract_decisions, extract_open_questions, extract_themes, link_action_items_to_goals,
};
pub use templates::{MeetingTemplate, TEMPLATES, find_template};
