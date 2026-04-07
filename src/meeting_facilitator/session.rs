//! Public API for managing meeting sessions — start, record, close.

use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::types::{ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus};

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn required_field(field: &str, value: &str) -> SimardResult<()> {
    if value.trim().is_empty() {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(())
}

fn validate_decision(decision: &MeetingDecision) -> SimardResult<()> {
    required_field("decision.description", &decision.description)?;
    required_field("decision.rationale", &decision.rationale)?;
    Ok(())
}

fn validate_action_item(item: &ActionItem) -> SimardResult<()> {
    required_field("action_item.description", &item.description)?;
    required_field("action_item.owner", &item.owner)?;
    if item.priority == 0 {
        return Err(SimardError::InvalidMeetingRecord {
            field: "action_item.priority".to_string(),
            reason: "priority must be at least 1".to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start a new meeting session. Records a sensory observation in cognitive
/// memory so the meeting start is captured for recall.
pub fn start_meeting(topic: &str, bridge: &CognitiveMemoryBridge) -> SimardResult<MeetingSession> {
    required_field("topic", topic)?;

    bridge.record_sensory("meeting-start", &format!("Meeting started: {topic}"), 3600)?;

    Ok(MeetingSession {
        topic: topic.to_string(),
        decisions: Vec::new(),
        action_items: Vec::new(),
        notes: Vec::new(),
        status: MeetingSessionStatus::Open,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: Vec::new(),
    })
}

/// Record a decision in an open meeting session.
pub fn record_decision(
    session: &mut MeetingSession,
    decision: MeetingDecision,
) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot record a decision in a closed meeting".to_string(),
        });
    }
    validate_decision(&decision)?;
    session.decisions.push(decision);
    Ok(())
}

/// Record an action item in an open meeting session.
pub fn record_action_item(session: &mut MeetingSession, item: ActionItem) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot record an action item in a closed meeting".to_string(),
        });
    }
    validate_action_item(&item)?;
    session.action_items.push(item);
    Ok(())
}

/// Add a free-form note to an open meeting session.
pub fn add_note(session: &mut MeetingSession, note: &str) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot add a note to a closed meeting".to_string(),
        });
    }
    required_field("note", note)?;
    session.notes.push(note.to_string());
    Ok(())
}

/// Close a meeting session and persist a durable summary as both an episode
/// and a semantic fact in cognitive memory.
pub fn close_meeting(
    mut session: MeetingSession,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<MeetingSession> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "meeting is already closed".to_string(),
        });
    }

    session.status = MeetingSessionStatus::Closed;
    let summary = session.durable_summary();

    // Store as an episodic memory for future recall.
    bridge.store_episode(
        &summary,
        "meeting-facilitator",
        Some(&json!({"topic": session.topic})),
    )?;

    // Store a semantic fact capturing the key decisions.
    if !session.decisions.is_empty() {
        let decision_text = session
            .decisions
            .iter()
            .map(|d| d.description.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        bridge.store_fact(
            &format!("meeting:{}", session.topic),
            &format!("Decisions: {decision_text}"),
            0.85,
            &["meeting".to_string(), "decision".to_string()],
            "meeting-facilitator",
        )?;
    }

    // Store action items as a prospective memory so they trigger later.
    for item in &session.action_items {
        bridge.store_prospective(
            &format!("Action: {}", item.description),
            &format!("owner={} starts work", item.owner),
            &format!("remind {} about: {}", item.owner, item.description),
            i64::from(item.priority),
        )?;
    }

    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use serde_json::json;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-meeting", |method, _params| match method {
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
    fn start_and_close_meeting_round_trip() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Sprint planning", &bridge).unwrap();
        assert_eq!(session.status, MeetingSessionStatus::Open);

        record_decision(
            &mut session,
            MeetingDecision {
                description: "Ship phase 8".to_string(),
                rationale: "Unblocks goal curation".to_string(),
                participants: vec!["alice".to_string()],
            },
        )
        .unwrap();

        record_action_item(
            &mut session,
            ActionItem {
                description: "Write tests".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("end of sprint".to_string()),
            },
        )
        .unwrap();

        let closed = close_meeting(session, &bridge).unwrap();
        assert_eq!(closed.status, MeetingSessionStatus::Closed);
        assert_eq!(closed.decisions.len(), 1);
        assert_eq!(closed.action_items.len(), 1);
    }

    #[test]
    fn cannot_add_to_closed_meeting() {
        let bridge = mock_bridge();
        let session = start_meeting("Retro", &bridge).unwrap();
        let mut closed = close_meeting(session, &bridge).unwrap();

        let err = record_decision(
            &mut closed,
            MeetingDecision {
                description: "late".to_string(),
                rationale: "oops".to_string(),
                participants: vec![],
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("closed meeting"));
    }

    #[test]
    fn rejects_empty_topic() {
        let bridge = mock_bridge();
        let err = start_meeting("", &bridge).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_zero_priority_action_item() {
        let bridge = mock_bridge();
        let mut session = start_meeting("Check", &bridge).unwrap();
        let err = record_action_item(
            &mut session,
            ActionItem {
                description: "task".to_string(),
                owner: "me".to_string(),
                priority: 0,
                due_description: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("priority"));
    }
}
