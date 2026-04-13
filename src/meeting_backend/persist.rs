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
}
