//! Public API for managing meeting sessions — start, record, close.

use serde_json::json;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

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
pub fn start_meeting(topic: &str, bridge: &dyn CognitiveMemoryOps) -> SimardResult<MeetingSession> {
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
        explicit_questions: Vec::new(),
        themes: Vec::new(),
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

/// Add an explicit open question to an open meeting session.
pub fn add_question(session: &mut MeetingSession, question: &str) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot add a question to a closed meeting".to_string(),
        });
    }
    required_field("question", question)?;
    session.explicit_questions.push(question.to_string());
    Ok(())
}

/// Edit an existing item (decision description, action-item description, or note)
/// by 0-based index.
pub fn edit_item(
    session: &mut MeetingSession,
    item_type: &str,
    index: usize,
    new_text: &str,
) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot edit items in a closed meeting".to_string(),
        });
    }
    required_field("new_text", new_text)?;

    match item_type {
        "decision" => {
            if index >= session.decisions.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "decision index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.decisions.len()
                    ),
                });
            }
            session.decisions[index].description = new_text.to_string();
        }
        "action" => {
            if index >= session.action_items.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "action index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.action_items.len()
                    ),
                });
            }
            session.action_items[index].description = new_text.to_string();
        }
        "note" => {
            if index >= session.notes.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "note index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.notes.len()
                    ),
                });
            }
            session.notes[index] = new_text.to_string();
        }
        "question" => {
            if index >= session.explicit_questions.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "question index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.explicit_questions.len()
                    ),
                });
            }
            session.explicit_questions[index] = new_text.to_string();
        }
        _ => {
            return Err(SimardError::InvalidMeetingRecord {
                field: "item_type".to_string(),
                reason: format!(
                    "unknown item type '{item_type}'; use decision, action, note, or question"
                ),
            });
        }
    }
    Ok(())
}

/// Remove an existing item (decision, action item, or note) by 0-based index.
pub fn remove_item(
    session: &mut MeetingSession,
    item_type: &str,
    index: usize,
) -> SimardResult<()> {
    if session.status != MeetingSessionStatus::Open {
        return Err(SimardError::InvalidMeetingRecord {
            field: "session.status".to_string(),
            reason: "cannot delete items in a closed meeting".to_string(),
        });
    }

    match item_type {
        "decision" => {
            if index >= session.decisions.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "decision index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.decisions.len()
                    ),
                });
            }
            session.decisions.remove(index);
        }
        "action" => {
            if index >= session.action_items.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "action index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.action_items.len()
                    ),
                });
            }
            session.action_items.remove(index);
        }
        "note" => {
            if index >= session.notes.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "note index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.notes.len()
                    ),
                });
            }
            session.notes.remove(index);
        }
        "question" => {
            if index >= session.explicit_questions.len() {
                return Err(SimardError::InvalidMeetingRecord {
                    field: "index".to_string(),
                    reason: format!(
                        "question index {idx} out of range (have {n})",
                        idx = index + 1,
                        n = session.explicit_questions.len()
                    ),
                });
            }
            session.explicit_questions.remove(index);
        }
        _ => {
            return Err(SimardError::InvalidMeetingRecord {
                field: "item_type".to_string(),
                reason: format!(
                    "unknown item type '{item_type}'; use decision, action, note, or question"
                ),
            });
        }
    }
    Ok(())
}

/// Close a meeting session and persist a durable summary as both an episode
/// and a semantic fact in cognitive memory.
pub fn close_meeting(
    mut session: MeetingSession,
    bridge: &dyn CognitiveMemoryOps,
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
