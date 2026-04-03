//! Interactive meeting mode — structured session with decisions and action items.
//!
//! `MeetingSession` captures a running meeting with its topic, decisions made,
//! and action items assigned. The facilitator stores a durable summary into
//! cognitive memory when the meeting closes.

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single decision recorded during a meeting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingDecision {
    pub description: String,
    pub rationale: String,
    pub participants: Vec<String>,
}

/// An action item assigned during a meeting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    pub description: String,
    pub owner: String,
    pub priority: u32,
    pub due_description: Option<String>,
}

/// Status of an in-progress meeting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MeetingSessionStatus {
    Open,
    Closed,
}

impl Display for MeetingSessionStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => f.write_str("open"),
            Self::Closed => f.write_str("closed"),
        }
    }
}

/// A running or completed meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingSession {
    pub topic: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub notes: Vec<String>,
    pub status: MeetingSessionStatus,
}

impl MeetingSession {
    /// Render a concise durable summary suitable for memory storage.
    pub fn durable_summary(&self) -> String {
        let decisions = if self.decisions.is_empty() {
            "none".to_string()
        } else {
            self.decisions
                .iter()
                .map(|d| d.description.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        };
        let action_items = if self.action_items.is_empty() {
            "none".to_string()
        } else {
            self.action_items
                .iter()
                .map(|a| format!("{} (owner={})", a.description, a.owner))
                .collect::<Vec<_>>()
                .join("; ")
        };
        format!(
            "meeting topic={}; decisions=[{}]; action_items=[{}]",
            self.topic, decisions, action_items,
        )
    }
}

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

// ---------------------------------------------------------------------------
// Meeting Handoff — artifact written when a meeting closes, consumed by
// the engineer loop and the `act-on-decisions` CLI subcommand.
// ---------------------------------------------------------------------------

/// Well-known filename for meeting handoff artifacts.
pub const MEETING_HANDOFF_FILENAME: &str = "meeting_handoff.json";

/// Default directory for meeting handoff artifacts.
pub fn default_handoff_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/meeting_handoffs")
}

/// A handoff artifact produced when a meeting closes. Contains decisions,
/// action items, and open questions extracted from the meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingHandoff {
    pub topic: String,
    pub closed_at: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub processed: bool,
}

impl MeetingHandoff {
    /// Create a handoff from a closed meeting session.
    /// Notes containing `?` are extracted as open questions.
    pub fn from_session(session: &MeetingSession) -> Self {
        let open_questions: Vec<String> = session
            .notes
            .iter()
            .filter(|n| n.contains('?'))
            .cloned()
            .collect();

        Self {
            topic: session.topic.clone(),
            closed_at: Utc::now().to_rfc3339(),
            decisions: session.decisions.clone(),
            action_items: session.action_items.clone(),
            open_questions,
            processed: false,
        }
    }
}

/// Write a meeting handoff artifact to a directory.
pub fn write_meeting_handoff(dir: &Path, handoff: &MeetingHandoff) -> SimardResult<()> {
    fs::create_dir_all(dir).map_err(|e| SimardError::ArtifactIo {
        path: dir.to_path_buf(),
        reason: format!("creating handoff dir: {e}"),
    })?;
    let path = dir.join(MEETING_HANDOFF_FILENAME);
    let json = serde_json::to_string_pretty(handoff).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing handoff: {e}"),
    })?;
    fs::write(&path, json).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;
    Ok(())
}

/// Load a meeting handoff artifact from a directory. Returns `None` if the
/// file does not exist.
pub fn load_meeting_handoff(dir: &Path) -> SimardResult<Option<MeetingHandoff>> {
    let path = dir.join(MEETING_HANDOFF_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("reading handoff: {e}"),
    })?;
    let handoff: MeetingHandoff =
        serde_json::from_str(&raw).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("failed to parse handoff JSON: {e}"),
        })?;
    Ok(Some(handoff))
}

/// Mark the meeting handoff in a directory as processed. No-op if no handoff
/// file exists.
pub fn mark_meeting_handoff_processed(dir: &Path) -> SimardResult<()> {
    let path = dir.join(MEETING_HANDOFF_FILENAME);
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("reading handoff: {e}"),
    })?;
    let mut handoff: MeetingHandoff =
        serde_json::from_str(&raw).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("failed to parse handoff JSON: {e}"),
        })?;
    handoff.processed = true;
    write_meeting_handoff(dir, &handoff)
}

/// Mark an already-loaded handoff as processed and write it back, avoiding a
/// redundant file read when the caller already holds the parsed struct.
pub fn mark_handoff_processed_in_place(
    dir: &Path,
    handoff: &mut MeetingHandoff,
) -> SimardResult<()> {
    handoff.processed = true;
    write_meeting_handoff(dir, handoff)
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
