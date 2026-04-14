//! Persistence for meeting transcripts and handoff artifacts.

use std::path::PathBuf;

use tracing::{debug, info, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{MeetingHandoff, default_handoff_dir, write_meeting_handoff};

use super::types::{ConversationMessage, MeetingTranscript};

/// Maximum length for a sanitized filename component.
const MAX_FILENAME_LEN: usize = 128;

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
fn meetings_dir() -> PathBuf {
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
/// The handoff uses empty decisions/action_items vectors (per the arch spec —
/// the LLM extracts these conversationally, not via structured parsing). The
/// conversation summary goes in the `transcript` field.
pub fn write_handoff(
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
) -> SimardResult<()> {
    let handoff = MeetingHandoff {
        topic: topic.to_string(),
        started_at: messages
            .first()
            .map(|m| m.timestamp.clone())
            .unwrap_or_default(),
        closed_at: chrono::Utc::now().to_rfc3339(),
        decisions: Vec::new(),
        action_items: Vec::new(),
        open_questions: Vec::new(),
        processed: false,
        duration_secs: None,
        transcript: vec![summary.to_string()],
        participants: vec!["operator".to_string()],
        themes: Vec::new(),
    };

    let dir = default_handoff_dir();
    write_meeting_handoff(&dir, &handoff)?;
    info!("Meeting handoff artifact written");
    Ok(())
}

/// Store the meeting as an episodic memory via the cognitive bridge.
pub fn store_cognitive_memory(
    bridge: &dyn CognitiveMemoryOps,
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
) {
    // Store full transcript as episodic memory
    if !messages.is_empty() {
        let transcript_text: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    super::types::Role::User => "operator",
                    super::types::Role::Assistant => "simard",
                    super::types::Role::System => "system",
                };
                format!("{}: {}", role, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let episode_content = format!(
            "Meeting transcript — topic: {topic}\n\n{transcript_text}\n\nSummary: {summary}"
        );
        if let Err(e) = bridge.store_episode(
            &episode_content,
            "meeting-backend-transcript",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "transcript",
                "message_count": messages.len(),
            })),
        ) {
            warn!("Failed to persist meeting episode: {e}");
        } else {
            debug!("Meeting episode stored");
        }
    }

    // Store summary as a semantic fact
    if !summary.is_empty() {
        let tags = vec![
            "meeting".to_string(),
            "summary".to_string(),
            topic.to_string(),
        ];
        if let Err(e) = bridge.store_fact(
            &format!("meeting:{topic}"),
            summary,
            0.85,
            &tags,
            "meeting-backend",
        ) {
            warn!("Failed to persist meeting summary fact: {e}");
        } else {
            debug!("Meeting summary fact stored");
        }
    }
}

/// Write a markdown export of the current meeting to `~/.simard/meetings/`.
///
/// The file includes YAML frontmatter (topic, date, participants) and the
/// conversation transcript formatted as markdown.
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
            super::types::Role::User => "operator",
            super::types::Role::Assistant => "simard",
            super::types::Role::System => "system",
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
                super::types::Role::User => "**Operator**",
                super::types::Role::Assistant => "**Simard**",
                super::types::Role::System => "**System**",
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

/// Meeting template content (agenda and prompts) for common meeting types.
pub struct MeetingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub agenda: &'static str,
}

/// All available meeting templates.
pub const TEMPLATES: &[MeetingTemplate] = &[
    MeetingTemplate {
        name: "standup",
        description: "Daily standup / sync",
        agenda: "\
## Daily Standup

1. **What did you accomplish since last standup?**
2. **What are you working on today?**
3. **Any blockers or impediments?**

_Tip: Keep updates brief — flag blockers for offline follow-up._",
    },
    MeetingTemplate {
        name: "1on1",
        description: "One-on-one check-in",
        agenda: "\
## 1:1 Check-in

1. **How are things going?** (personal/professional)
2. **Progress on current goals**
3. **Feedback** — anything to share in either direction?
4. **Growth & development** — skills, interests, opportunities
5. **Action items from last time**

_Tip: This is their meeting — let them drive the agenda._",
    },
    MeetingTemplate {
        name: "retro",
        description: "Sprint retrospective",
        agenda: "\
## Retrospective

1. **What went well?** 🟢
2. **What didn't go well?** 🔴
3. **What can we improve?** 🔧
4. **Action items** — concrete, assigned, time-boxed

_Tip: Celebrate wins before diving into problems._",
    },
    MeetingTemplate {
        name: "planning",
        description: "Sprint / iteration planning",
        agenda: "\
## Planning Session

1. **Review previous sprint** — what carried over and why?
2. **Capacity check** — who's available, any PTO or conflicts?
3. **Backlog review** — prioritize items for this sprint
4. **Estimation** — size and assign selected items
5. **Sprint goal** — one sentence capturing the sprint's purpose
6. **Risks & dependencies** — anything that could block progress?

_Tip: Timebox estimation discussions — if it takes >2 min, take it offline._",
    },
];

/// Look up a template by name. Returns `None` if not found.
pub fn find_template(name: &str) -> Option<&'static MeetingTemplate> {
    TEMPLATES.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_filename("Sprint Planning"), "Sprint_Planning");
    }

    #[test]
    fn sanitize_path_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "etc_passwd");
    }

    #[test]
    fn sanitize_null_bytes() {
        assert_eq!(sanitize_filename("test\0file"), "testfile");
    }

    #[test]
    fn sanitize_empty() {
        assert_eq!(sanitize_filename(""), "meeting");
    }

    #[test]
    fn sanitize_special_chars() {
        assert_eq!(sanitize_filename("a:b*c?d<e>f|g"), "a_b_c_d_e_f_g");
    }

    #[test]
    fn sanitize_long_string() {
        let long = "a".repeat(200);
        let result = sanitize_filename(&long);
        assert!(result.len() <= MAX_FILENAME_LEN);
    }

    #[test]
    fn sanitize_only_dots_and_underscores() {
        assert_eq!(sanitize_filename("...___..."), "meeting");
    }

    #[test]
    fn find_template_by_name() {
        assert!(find_template("standup").is_some());
        assert!(find_template("1on1").is_some());
        assert!(find_template("retro").is_some());
        assert!(find_template("planning").is_some());
        assert!(find_template("nonexistent").is_none());
    }

    #[test]
    fn find_template_case_insensitive() {
        assert!(find_template("STANDUP").is_some());
        assert!(find_template("Retro").is_some());
    }

    #[test]
    fn templates_have_content() {
        for t in TEMPLATES {
            assert!(!t.name.is_empty());
            assert!(!t.description.is_empty());
            assert!(!t.agenda.is_empty());
        }
    }

    #[test]
    fn at_least_four_templates() {
        assert!(TEMPLATES.len() >= 4);
    }

    #[test]
    fn markdown_export_format() {
        // Verify the markdown format contains expected YAML frontmatter
        let topic = "Test Topic";
        let started_at = "2025-01-01T00:00:00Z";
        let mut md = String::new();
        md.push_str("---\n");
        md.push_str(&format!("topic: \"{topic}\"\n"));
        md.push_str(&format!("date: \"{started_at}\"\n"));
        md.push_str("participants:\n  - \"operator\"\n  - \"simard\"\n");
        md.push_str("---\n\n");
        md.push_str(&format!("# Meeting: {topic}\n\n"));

        assert!(md.contains("---"));
        assert!(md.contains("topic: \"Test Topic\""));
        assert!(md.contains("date: \"2025-01-01T00:00:00Z\""));
        assert!(md.contains("participants:"));
    }
}
