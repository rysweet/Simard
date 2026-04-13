//! Types for the unified meeting backend.

use serde::{Deserialize, Serialize};

/// Role of a participant in the conversation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// A single message in the conversation history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: String,
}

/// Response from `MeetingBackend::send_message()`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingResponse {
    pub content: String,
    pub message_count: usize,
}

/// Summary produced when a meeting is closed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub topic: String,
    pub summary_text: String,
    pub message_count: usize,
    pub duration_secs: u64,
    pub transcript_path: Option<String>,
}

/// Predefined meeting template presets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeetingTemplateKind {
    Standup,
    Retro,
    Planning,
    Custom(String),
}

impl MeetingTemplateKind {
    /// Short agenda description for the template.
    pub fn agenda(&self) -> &str {
        match self {
            Self::Standup => "What did you do? What will you do? Any blockers?",
            Self::Retro => "What went well? What didn't? What to improve?",
            Self::Planning => "Goals for this sprint? Risks? Dependencies?",
            Self::Custom(desc) => desc.as_str(),
        }
    }

    /// Parse a template name string into a `MeetingTemplateKind`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "standup" => Some(Self::Standup),
            "retro" | "retrospective" => Some(Self::Retro),
            "planning" | "sprint-planning" => Some(Self::Planning),
            _ => None,
        }
    }
}

/// Current status of a meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionStatus {
    pub topic: String,
    pub message_count: usize,
    pub started_at: String,
    pub is_open: bool,
    pub duration_display: Option<String>,
    pub active_template: Option<String>,
}

/// Detailed progress snapshot for the `/progress` command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingProgress {
    pub duration_display: String,
    pub operator_messages: usize,
    pub agent_messages: usize,
    pub topics: Vec<String>,
    pub action_items: Vec<String>,
    pub pending_decisions: Vec<String>,
}

/// Persisted transcript format written to `~/.simard/meetings/`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeetingTranscript {
    pub topic: String,
    pub started_at: String,
    pub closed_at: String,
    pub duration_secs: u64,
    pub summary: String,
    pub messages: Vec<ConversationMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serde_round_trip() {
        for role in [Role::User, Role::Assistant, Role::System] {
            let json = serde_json::to_string(&role).unwrap();
            let r2: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(role, r2);
        }
    }

    #[test]
    fn conversation_message_serde() {
        let msg = ConversationMessage {
            role: Role::User,
            content: "hello".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let m2: ConversationMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, m2);
    }

    #[test]
    fn meeting_response_serde() {
        let resp = MeetingResponse {
            content: "Got it".to_string(),
            message_count: 4,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let r2: MeetingResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, r2);
    }

    #[test]
    fn meeting_summary_serde() {
        let summary = MeetingSummary {
            topic: "Sprint".to_string(),
            summary_text: "We decided things.".to_string(),
            message_count: 10,
            duration_secs: 600,
            transcript_path: Some("/home/user/.simard/meetings/test.json".to_string()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let s2: MeetingSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(summary, s2);
    }

    #[test]
    fn session_status_serde() {
        let status = SessionStatus {
            topic: "Retro".to_string(),
            message_count: 3,
            started_at: "2025-01-01T00:00:00Z".to_string(),
            is_open: true,
            duration_display: Some("5m 32s".to_string()),
            active_template: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        let s2: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, s2);
    }

    #[test]
    fn meeting_progress_serde() {
        let progress = MeetingProgress {
            duration_display: "5m 32s".to_string(),
            operator_messages: 3,
            agent_messages: 3,
            topics: vec!["API design".to_string()],
            action_items: vec!["Fix the tests".to_string()],
            pending_decisions: vec!["Choose a database".to_string()],
        };
        let json = serde_json::to_string(&progress).unwrap();
        let p2: MeetingProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(progress, p2);
    }

    #[test]
    fn meeting_transcript_serde() {
        let t = MeetingTranscript {
            topic: "Test".to_string(),
            started_at: "2025-01-01T00:00:00Z".to_string(),
            closed_at: "2025-01-01T01:00:00Z".to_string(),
            duration_secs: 3600,
            summary: "Summary text".to_string(),
            messages: vec![ConversationMessage {
                role: Role::User,
                content: "Hello".to_string(),
                timestamp: "2025-01-01T00:00:01Z".to_string(),
            }],
        };
        let json = serde_json::to_string(&t).unwrap();
        let t2: MeetingTranscript = serde_json::from_str(&json).unwrap();
        assert_eq!(t.topic, t2.topic);
        assert_eq!(t.messages.len(), t2.messages.len());
    }
}
