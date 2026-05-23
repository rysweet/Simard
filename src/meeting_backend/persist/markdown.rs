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
    use crate::meeting_backend::types::{
        AppliedTemplate, ConversationMessage, HandoffActionItem, Role,
    };
    use crate::meeting_facilitator::MeetingDecision;
    use serial_test::serial;

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn setup_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SIMARD_MEETINGS_DIR", tmp.path());
        }
        tmp
    }

    fn teardown() {
        unsafe {
            std::env::remove_var("SIMARD_MEETINGS_DIR");
        }
    }

    #[test]
    #[serial(simard_env)]
    fn markdown_export_contains_frontmatter_and_transcript() {
        let _tmp = setup_dir();
        let messages = vec![
            msg(Role::User, "Hello, let's discuss performance."),
            msg(Role::Assistant, "Sure, what metrics matter?"),
        ];

        let path = write_markdown_export("perf-review", "2026-01-01T00:00:00Z", &messages).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.starts_with("---\n"));
        assert!(content.contains("topic: \"perf-review\""));
        assert!(content.contains("date: \"2026-01-01T00:00:00Z\""));
        assert!(content.contains("participants:"));
        assert!(content.contains("\"operator\""));
        assert!(content.contains("\"simard\""));
        assert!(content.contains("# Meeting: perf-review"));
        assert!(content.contains("## Transcript"));
        assert!(content.contains("**Operator**: Hello, let's discuss performance."));
        assert!(content.contains("**Simard**: Sure, what metrics matter?"));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn markdown_export_empty_messages_shows_placeholder() {
        let _tmp = setup_dir();
        let path = write_markdown_export("empty", "2026-01-01T00:00:00Z", &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_No messages recorded._"));
        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn markdown_export_roundtrip_frontmatter() {
        let _tmp = setup_dir();
        let messages = vec![msg(Role::System, "System init.")];
        let path = write_markdown_export("sys-test", "2026-01-01T00:00:00Z", &messages).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        let parts: Vec<&str> = content.splitn(3, "---\n").collect();
        assert!(parts.len() >= 3, "should have YAML frontmatter block");
        let yaml_block = parts[1];
        assert!(yaml_block.contains("topic:"));
        assert!(yaml_block.contains("\"system\""));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn markdown_export_escapes_quotes_in_topic() {
        let _tmp = setup_dir();
        let path =
            write_markdown_export("topic with \"quotes\"", "2026-01-01T00:00:00Z", &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\\\"quotes\\\""));
        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_contains_all_sections() {
        let _tmp = setup_dir();

        let messages = vec![
            msg(Role::User, "We should ship on Monday."),
            msg(Role::Assistant, "We agreed to the timeline."),
        ];
        let action_items = vec![HandoffActionItem {
            description: "Ship the release".to_string(),
            assignee: Some("Bob".to_string()),
            deadline: Some("by monday".to_string()),
            linked_goal: None,
            priority: Some(1),
        }];
        let decisions = vec![MeetingDecision {
            description: "Ship on Monday".to_string(),
            rationale: "Customers need it".to_string(),
            participants: vec!["operator".to_string()],
        }];

        let path = write_handoff_markdown_report(
            "ship-release",
            "2026-01-01T00:00:00Z",
            "We decided to ship on Monday.",
            &messages,
            &action_items,
            &decisions,
            &[],
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("## Summary"));
        assert!(content.contains("We decided to ship on Monday."));
        assert!(content.contains("## Decisions"));
        assert!(content.contains("Ship on Monday"));
        assert!(content.contains("Customers need it"));
        assert!(content.contains("## Action Items"));
        assert!(content.contains("Ship the release"));
        assert!(content.contains("Bob"));
        assert!(content.contains("## Participants"));
        assert!(content.contains("## Open Questions"));
        assert!(content.contains("## Themes"));
        assert!(content.contains("## Transcript"));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_empty_shows_placeholders() {
        let _tmp = setup_dir();

        let path = write_handoff_markdown_report(
            "empty-report",
            "2026-01-01T00:00:00Z",
            "No content.",
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("_No explicit decisions recorded._"));
        assert!(content.contains("_No action items extracted._"));
        assert!(content.contains("_No open questions identified._"));
        assert!(content.contains("_No recurring themes identified._"));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_includes_template_agenda() {
        let _tmp = setup_dir();

        let applied = vec![AppliedTemplate {
            name: "standup".to_string(),
            agenda: "## Daily Standup\n\n1. Yesterday\n2. Today\n3. Blockers".to_string(),
            applied_at: "2026-01-01T00:05:00Z".to_string(),
        }];

        let path = write_handoff_markdown_report(
            "standup",
            "2026-01-01T00:00:00Z",
            "Good standup.",
            &[msg(Role::User, "Everything is on track.")],
            &[],
            &[],
            &applied,
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("## Agenda"));
        assert!(content.contains("### Template: `standup`"));
        assert!(content.contains("Daily Standup"));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_action_items_table_format() {
        let _tmp = setup_dir();

        let action_items = vec![
            HandoffActionItem {
                description: "Task A".to_string(),
                assignee: Some("Alice".to_string()),
                deadline: Some("by friday".to_string()),
                linked_goal: Some("goal-1".to_string()),
                priority: Some(1),
            },
            HandoffActionItem {
                description: "Task B".to_string(),
                assignee: None,
                deadline: None,
                linked_goal: None,
                priority: None,
            },
        ];

        let path = write_handoff_markdown_report(
            "table-test",
            "2026-01-01T00:00:00Z",
            "summary",
            &[],
            &action_items,
            &[],
            &[],
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(content.contains("| # | Description | Assignee | Deadline | Priority | Goal |"));
        assert!(content.contains("Task A"));
        assert!(content.contains("Alice"));
        assert!(content.contains("\u{2014}"));

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_creates_json_sibling() {
        let _tmp = setup_dir();

        let messages = vec![msg(Role::User, "Let's discuss testing approaches.")];
        let path = write_handoff_markdown_report(
            "sibling-test",
            "2026-01-01T00:00:00Z",
            "summary",
            &messages,
            &[],
            &[],
            &[],
        )
        .unwrap();

        let json_path = path.with_extension("json");
        assert!(
            json_path.is_file(),
            "JSON sibling should be created at {json_path:?}"
        );

        let raw = std::fs::read_to_string(&json_path).unwrap();
        let sibling: crate::meeting_backend::persist::json_sibling::JsonHandoffSibling =
            serde_json::from_str(&raw).unwrap();
        assert_eq!(sibling.schema_version, "v1");
        assert!(!sibling.transcript_ref.is_empty());

        teardown();
    }

    #[test]
    #[serial(simard_env)]
    fn handoff_report_assignee_in_participants() {
        let _tmp = setup_dir();

        let messages = vec![msg(Role::User, "content")];
        let action_items = vec![HandoffActionItem {
            description: "task".to_string(),
            assignee: Some("ExternalHire".to_string()),
            deadline: None,
            linked_goal: None,
            priority: None,
        }];

        let path = write_handoff_markdown_report(
            "participant-test",
            "2026-01-01T00:00:00Z",
            "summary",
            &messages,
            &action_items,
            &[],
            &[],
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(
            content.contains("ExternalHire"),
            "assignee should appear in participants"
        );

        teardown();
    }
}
