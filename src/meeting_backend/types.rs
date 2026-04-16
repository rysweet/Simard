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

/// A structured action item extracted from meeting transcript on close.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HandoffActionItem {
    pub description: String,
    /// Assignee if mentioned in the transcript (e.g. "Alice will…").
    pub assignee: Option<String>,
    /// Deadline if mentioned (e.g. "by Friday", "next sprint").
    pub deadline: Option<String>,
    /// Slug of the linked Simard goal, if the action advances a known goal.
    pub linked_goal: Option<String>,
    /// Priority level (lower = higher priority). Defaults to None when not set.
    #[serde(default)]
    pub priority: Option<u32>,
}

/// Summary produced when a meeting is closed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub topic: String,
    pub summary_text: String,
    pub message_count: usize,
    pub duration_secs: u64,
    pub transcript_path: Option<String>,
    /// Structured action items extracted from the meeting.
    #[serde(default)]
    pub action_items: Vec<HandoffActionItem>,
    /// Key decisions recorded during the meeting.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Path to the auto-generated markdown report (if export succeeded).
    #[serde(default)]
    pub markdown_report_path: Option<String>,
    /// Open questions identified during the meeting.
    #[serde(default)]
    pub open_questions: Vec<String>,
    /// High-level themes or recurring topics from the meeting.
    #[serde(default)]
    pub themes: Vec<String>,
    /// Participants identified from the conversation.
    #[serde(default)]
    pub participants: Vec<String>,
}

/// Current status of a meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionStatus {
    pub topic: String,
    pub message_count: usize,
    pub started_at: String,
    pub is_open: bool,
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
            action_items: vec![HandoffActionItem {
                description: "Write tests".to_string(),
                assignee: Some("Alice".to_string()),
                deadline: Some("by friday".to_string()),
                linked_goal: Some("improve-testing".to_string()),
                priority: Some(1),
            }],
            decisions: vec!["Adopt TDD".to_string()],
            markdown_report_path: Some("/home/user/.simard/meetings/test_report.md".to_string()),
            open_questions: vec!["Who will lead the effort?".to_string()],
            themes: vec!["testing".to_string(), "quality".to_string()],
            participants: vec!["operator".to_string(), "simard".to_string()],
        };
        let json = serde_json::to_string(&summary).unwrap();
        let s2: MeetingSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(summary, s2);
    }

    #[test]
    fn meeting_summary_backwards_compat_deserialize() {
        // Old JSON without the new fields should deserialize via #[serde(default)]
        let json = r#"{
            "topic": "Old",
            "summary_text": "Summary",
            "message_count": 5,
            "duration_secs": 300,
            "transcript_path": null
        }"#;
        let s: MeetingSummary = serde_json::from_str(json).unwrap();
        assert!(s.action_items.is_empty());
        assert!(s.decisions.is_empty());
        assert!(s.markdown_report_path.is_none());
        assert!(s.open_questions.is_empty());
        assert!(s.themes.is_empty());
        assert!(s.participants.is_empty());
    }

    #[test]
    fn handoff_action_item_serde() {
        let item = HandoffActionItem {
            description: "Deploy to staging".to_string(),
            assignee: Some("Bob".to_string()),
            deadline: None,
            linked_goal: None,
            priority: Some(2),
        };
        let json = serde_json::to_string(&item).unwrap();
        let i2: HandoffActionItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, i2);
    }

    #[test]
    fn handoff_action_item_priority_defaults_none() {
        // Old JSON without priority field should deserialize with priority = None
        let json =
            r#"{"description":"Fix bug","assignee":null,"deadline":null,"linked_goal":null}"#;
        let item: HandoffActionItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.priority, None, "priority should default to None");
    }

    #[test]
    fn session_status_serde() {
        let status = SessionStatus {
            topic: "Retro".to_string(),
            message_count: 3,
            started_at: "2025-01-01T00:00:00Z".to_string(),
            is_open: true,
        };
        let json = serde_json::to_string(&status).unwrap();
        let s2: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, s2);
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
