//! Markdown export writers for meeting transcripts and handoffs.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{ActionItem, MeetingDecision, MeetingHandoff, OpenQuestion};

use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, MeetingTranscript};
use super::extract::{
    extract_action_items, extract_decision_participants_pub, extract_decision_rationale_pub,
    extract_decisions, extract_open_questions, extract_themes,
};
use super::{meetings_dir, sanitize_filename};

pub fn write_markdown_export(
    topic: &str,
    started_at: &str,
    messages: &[ConversationMessage],
) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(topic);
    let filename = format!("{timestamp}_{safe_topic}.md");
    let path = dir.join(&filename);

    let mut md = String::with_capacity(4096);
    // YAML frontmatter
    md.push_str("---\n");
    md.push_str(&format!("topic: \"{}\"\n", topic.replace('"', "\\\"")));
    md.push_str(&format!("date: \"{started_at}\"\n"));
    // Collect unique participants from messages
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            crate::meeting_backend::types::Role::User => "operator",
            crate::meeting_backend::types::Role::Assistant => "simard",
            crate::meeting_backend::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    md.push_str("participants:\n");
    for p in &participants {
        md.push_str(&format!("  - \"{p}\"\n"));
    }
    md.push_str("---\n\n");

    // Title and transcript
    md.push_str(&format!("# Meeting: {topic}\n\n"));
    md.push_str(&format!("**Date:** {started_at}\n\n"));

    if messages.is_empty() {
        md.push_str("_No messages recorded._\n");
    } else {
        md.push_str("## Transcript\n\n");
        for msg in messages {
            let role_label = match msg.role {
                crate::meeting_backend::types::Role::User => "**Operator**",
                crate::meeting_backend::types::Role::Assistant => "**Simard**",
                crate::meeting_backend::types::Role::System => "**System**",
            };
            md.push_str(&format!("{role_label}: {}\n\n", msg.content));
        }
    }

    std::fs::write(&path, &md).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-markdown-export".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set export file permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting markdown export written");
    Ok(path)
}

pub fn write_handoff_markdown_report(
    topic: &str,
    started_at: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[MeetingDecision],
) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(topic);
    let filename = format!("{timestamp}_{safe_topic}_report.md");
    let path = dir.join(&filename);

    let mut md = String::with_capacity(8192);

    // YAML frontmatter
    md.push_str("---\n");
    md.push_str(&format!("topic: \"{}\"\n", topic.replace('"', "\\\"")));
    md.push_str(&format!("date: \"{started_at}\"\n"));
    md.push_str("type: meeting-report\n");
    md.push_str("---\n\n");

    md.push_str(&format!("# Meeting Report: {topic}\n\n"));
    md.push_str(&format!("**Date:** {started_at}\n\n"));

    // Participants section
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            crate::meeting_backend::types::Role::User => "operator",
            crate::meeting_backend::types::Role::Assistant => "simard",
            crate::meeting_backend::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    for a in action_items {
        if let Some(ref assignee) = a.assignee
            && !participants.contains(assignee)
        {
            participants.push(assignee.clone());
        }
    }
    if !participants.is_empty() {
        md.push_str("## Participants\n\n");
        for p in &participants {
            md.push_str(&format!("- {p}\n"));
        }
        md.push('\n');
    }

    md.push_str("## Summary\n\n");
    md.push_str(summary);
    md.push_str("\n\n");

    md.push_str("## Decisions\n\n");
    if decisions.is_empty() {
        md.push_str("_No explicit decisions recorded._\n\n");
    } else {
        for (i, d) in decisions.iter().enumerate() {
            md.push_str(&format!("{}. **{}**\n", i + 1, d.description));
            if !d.rationale.is_empty() {
                md.push_str(&format!("   - *Rationale:* {}\n", d.rationale));
            }
            if !d.participants.is_empty() {
                md.push_str(&format!("   - *By:* {}\n", d.participants.join(", ")));
            }
        }
        md.push('\n');
    }

    md.push_str("## Action Items\n\n");
    if action_items.is_empty() {
        md.push_str("_No action items extracted._\n\n");
    } else {
        md.push_str("| # | Description | Assignee | Deadline | Priority | Goal |\n");
        md.push_str("|---|-------------|----------|----------|----------|------|\n");
        for (i, item) in action_items.iter().enumerate() {
            let assignee = item.assignee.as_deref().unwrap_or("\u{2014}");
            let deadline = item.deadline.as_deref().unwrap_or("\u{2014}");
            let priority = item
                .priority
                .map(|p| p.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string());
            let goal = item.linked_goal.as_deref().unwrap_or("\u{2014}");
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                i + 1,
                item.description,
                assignee,
                deadline,
                priority,
                goal
            ));
        }
        md.push('\n');
    }

    // Open questions extracted from transcript.
    let open_questions = extract_open_questions(messages);
    md.push_str("## Open Questions\n\n");
    if open_questions.is_empty() {
        md.push_str("_No open questions identified._\n\n");
    } else {
        for q in &open_questions {
            let tag = if q.explicit { " *(explicit)*" } else { "" };
            md.push_str(&format!("- {}{tag}\n", q.text));
        }
        md.push('\n');
    }

    // Themes extracted from meeting content.
    let themes = extract_themes(messages);
    md.push_str("## Themes\n\n");
    if themes.is_empty() {
        md.push_str("_No recurring themes identified._\n\n");
    } else {
        for t in &themes {
            md.push_str(&format!("- {t}\n"));
        }
        md.push('\n');
    }

    if !messages.is_empty() {
        md.push_str("## Transcript\n\n");
        for msg in messages {
            let role_label = match msg.role {
                crate::meeting_backend::types::Role::User => "**Operator**",
                crate::meeting_backend::types::Role::Assistant => "**Simard**",
                crate::meeting_backend::types::Role::System => "**System**",
            };
            md.push_str(&format!("{role_label}: {}\n\n", msg.content));
        }
    }

    std::fs::write(&path, &md).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-handoff-report".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set report file permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting handoff report written");
    Ok(path)
}

