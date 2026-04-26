//! Interactive meeting mode — structured session with decisions and action items.
//!
//! `MeetingSession` captures a running meeting with its topic, decisions made,
//! and action items assigned. The facilitator stores a durable summary into
//! cognitive memory when the meeting closes.

mod handoff;
mod session;
#[cfg(test)]
mod tests_handoff;
#[cfg(test)]
mod tests_handoff_extra;
#[cfg(test)]
mod tests_session;
mod types;

// Re-export all public items so `crate::meeting_facilitator::X` still works.
pub use handoff::{
    MEETING_HANDOFF_FILENAME, MEETING_SESSION_WIP_FILENAME, MeetingHandoff, default_handoff_dir,
    load_meeting_handoff, load_session_wip, mark_handoff_processed_in_place,
    mark_meeting_handoff_processed, remove_session_wip, save_session_wip, write_meeting_handoff,
};
pub use session::{
    add_note, add_question, close_meeting, edit_item, record_action_item, record_decision,
    remove_item, start_meeting,
};
pub use types::{ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus, OpenQuestion};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-meeting-mod", |method, _params| match method {
                "memory.record_sensory" => Ok(json!({"id": "sen_m1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_m1"})),
                "memory.store_fact" => Ok(json!({"id": "sem_m1"})),
                "memory.store_prospective" => Ok(json!({"id": "pro_m1"})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    #[test]
    fn start_meeting_creates_open_session() {
        let bridge = mock_bridge();
        let session = start_meeting("Architecture review", &bridge).unwrap();
        assert_eq!(session.topic, "Architecture review");
        assert_eq!(session.status, MeetingSessionStatus::Open);
        assert!(session.decisions.is_empty());
        assert!(session.action_items.is_empty());
    }

    #[test]
    fn start_meeting_rejects_empty_topic() {
        let bridge = mock_bridge();
        assert!(start_meeting("", &bridge).is_err());
        assert!(start_meeting("   ", &bridge).is_err());
    }

    #[test]
    fn record_decision_adds_to_session() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Test", &bridge).unwrap();
        record_decision(
            &mut session,
            MeetingDecision {
                description: "Use Rust".to_string(),
                rationale: "Safety".to_string(),
                participants: vec!["dev".to_string()],
            },
        )
        .unwrap();
        assert_eq!(session.decisions.len(), 1);
        assert_eq!(session.decisions[0].description, "Use Rust");
    }

    #[test]
    fn record_action_item_validates_priority() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Test", &bridge).unwrap();
        let result = record_action_item(
            &mut session,
            ActionItem {
                description: "Do thing".to_string(),
                owner: "dev".to_string(),
                priority: 0,
                due_description: None,
            },
        );
        assert!(result.is_err(), "priority 0 should be rejected");
    }

    #[test]
    fn add_note_to_open_session() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Test", &bridge).unwrap();
        add_note(&mut session, "Important observation").unwrap();
        assert_eq!(session.notes.len(), 1);
    }

    #[test]
    fn add_question_to_open_session() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Test", &bridge).unwrap();
        add_question(&mut session, "What about scaling?").unwrap();
        assert_eq!(session.explicit_questions.len(), 1);
        assert_eq!(session.explicit_questions[0], "What about scaling?");
    }

    #[test]
    fn durable_summary_includes_key_fields() {
        let session = MeetingSession {
            topic: "Planning".to_string(),
            decisions: vec![MeetingDecision {
                description: "Ship it".to_string(),
                rationale: "Ready".to_string(),
                participants: vec![],
            }],
            action_items: vec![ActionItem {
                description: "Deploy".to_string(),
                owner: "ops".to_string(),
                priority: 1,
                due_description: None,
            }],
            notes: vec![],
            status: MeetingSessionStatus::Open,
            started_at: "".to_string(),
            participants: vec!["alice".to_string()],
            explicit_questions: vec![],
            themes: vec![],
        };
        let summary = session.durable_summary();
        assert!(summary.contains("Planning"));
        assert!(summary.contains("Ship it"));
        assert!(summary.contains("owner=ops"));
        assert!(summary.contains("alice"));
    }

    #[test]
    fn meeting_session_status_display() {
        assert_eq!(MeetingSessionStatus::Open.to_string(), "open");
        assert_eq!(MeetingSessionStatus::Closed.to_string(), "closed");
    }
}
