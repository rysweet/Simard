//! Markdown export writers for meeting transcripts and handoffs.

use std::path::PathBuf;

use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::MeetingDecision;

use super::extract::{extract_open_questions, extract_themes};
use super::{meetings_dir, sanitize_filename};
use crate::meeting_backend::types::{AppliedTemplate, ConversationMessage, HandoffActionItem};

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
    applied_templates: &[AppliedTemplate],
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

    // Agenda from any templates applied during the meeting. Renders the
    // template name plus its full agenda body so reviewers can see the
    // intended structure of the discussion. Skipped entirely if no
    // template was applied (avoid an empty section).
    if !applied_templates.is_empty() {
        md.push_str("## Agenda\n\n");
        for tmpl in applied_templates {
            md.push_str(&format!(
                "### Template: `{}` (applied {})\n\n",
                tmpl.name, tmpl.applied_at
            ));
            md.push_str(tmpl.agenda.trim_end());
            md.push_str("\n\n");
        }
    }

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

    // Emit the structured JSON sibling artifact alongside the markdown
    // report (issue #1646). Markdown remains the canonical artifact —
    // a JSON write failure is logged and skipped, never propagated.
    match super::json_sibling::write_json_sibling_for_markdown(
        &path,
        messages,
        action_items,
        decisions,
    ) {
        Ok(json_path) => {
            info!(path = %json_path.display(), "Meeting handoff JSON sibling written");
        }
        Err(e) => {
            warn!(error = %e, "Failed to write JSON sibling for handoff report — markdown remains canonical");
        }
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, Role};
    use crate::meeting_facilitator::MeetingDecision;
    use serial_test::serial;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("md-{label}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-15T10:00:00Z".to_string(),
        }
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_markdown_export_creates_md_file() {
        let dir = temp_dir("export");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let msgs = vec![msg(Role::User, "Hello"), msg(Role::Assistant, "Hi there")];
        let path = write_markdown_export("Sprint", "2026-01-15T10:00:00Z", &msgs).unwrap();
        assert!(path.exists());
        assert_eq!(path.extension().unwrap(), "md");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("topic: \"Sprint\""));
        assert!(content.contains("**Operator**"));
        assert!(content.contains("**Simard**"));

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_markdown_export_empty_messages() {
        let dir = temp_dir("export-empty");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let path = write_markdown_export("Empty", "2026-01-15T10:00:00Z", &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("No messages recorded"));

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_markdown_report_creates_report() {
        let dir = temp_dir("report");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let msgs = vec![
            msg(Role::User, "Let's plan the sprint."),
            msg(
                Role::Assistant,
                "Decision: adopt structured handoff bundles.",
            ),
        ];
        let items = vec![HandoffActionItem {
            description: "Write integration tests".to_string(),
            assignee: Some("Alice".to_string()),
            deadline: Some("by friday".to_string()),
            linked_goal: None,
            priority: Some(1),
        }];
        let decisions = vec![MeetingDecision {
            description: "Adopt TDD".to_string(),
            rationale: "Better quality".to_string(),
            participants: vec!["operator".to_string()],
        }];

        let path = write_handoff_markdown_report(
            "Sprint Planning",
            "2026-01-15T10:00:00Z",
            "Good meeting.",
            &msgs,
            &items,
            &decisions,
            &[],
        )
        .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("## Summary"));
        assert!(content.contains("## Decisions"));
        assert!(content.contains("Adopt TDD"));
        assert!(content.contains("## Action Items"));
        assert!(content.contains("Write integration tests"));
        assert!(content.contains("Alice"));
        assert!(content.contains("## Open Questions"));
        assert!(content.contains("## Themes"));

        let json_path = path.with_extension("json");
        assert!(json_path.exists(), "JSON sibling should exist");

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_markdown_report_empty_sections() {
        let dir = temp_dir("report-empty");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let path = write_handoff_markdown_report(
            "Empty",
            "2026-01-15T10:00:00Z",
            "Nothing.",
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("No explicit decisions recorded"));
        assert!(content.contains("No action items extracted"));

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial(simard_meetings_dir_env)]
    fn write_handoff_markdown_report_includes_templates() {
        let dir = temp_dir("report-tmpl");
        unsafe { std::env::set_var("SIMARD_MEETINGS_DIR", &dir) };

        let templates = vec![AppliedTemplate {
            name: "retro".to_string(),
            agenda: "## Retrospective\n1. What went well?".to_string(),
            applied_at: "2026-01-15T10:05:00Z".to_string(),
        }];

        let path = write_handoff_markdown_report(
            "Retro",
            "2026-01-15T10:00:00Z",
            "Good retro.",
            &[msg(Role::User, "went well")],
            &[],
            &[],
            &templates,
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("## Agenda"));
        assert!(content.contains("Template: `retro`"));

        unsafe { std::env::remove_var("SIMARD_MEETINGS_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
