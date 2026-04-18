//! Core types for meeting sessions.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

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

/// An open question recorded during or inferred from a meeting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub text: String,
    /// `true` when the user explicitly typed `/question`, `false` when inferred
    /// from notes heuristics (contains `?`, starts with `OPEN:`, etc.).
    pub explicit: bool,
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
    pub started_at: String,
    pub participants: Vec<String>,
    /// Questions explicitly added via `/question`.
    #[serde(default)]
    pub explicit_questions: Vec<String>,
    /// Themes explicitly recorded via `/theme`.
    #[serde(default)]
    pub themes: Vec<String>,
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
        let participants = if self.participants.is_empty() {
            "none".to_string()
        } else {
            self.participants.join(", ")
        };
        let duration = if !self.started_at.is_empty() {
            if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&self.started_at) {
                let elapsed = chrono::Utc::now().signed_duration_since(start);
                format!("{}s", elapsed.num_seconds())
            } else {
                "unknown".to_string()
            }
        } else {
            "unknown".to_string()
        };
        format!(
            "meeting topic={}; duration={}; participants=[{}]; decisions=[{}]; action_items=[{}]",
            self.topic, duration, participants, decisions, action_items,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MeetingDecision ─────────────────────────────────────────────

    #[test]
    fn meeting_decision_round_trip_serde() {
        let d = MeetingDecision {
            description: "Use Rust".to_string(),
            rationale: "Memory safety".to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
        };
        let json = serde_json::to_string(&d).unwrap();
        let d2: MeetingDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(d, d2);
    }

    #[test]
    fn meeting_decision_empty_participants() {
        let d = MeetingDecision {
            description: "No attendees".to_string(),
            rationale: "Testing".to_string(),
            participants: vec![],
        };
        let json = serde_json::to_string(&d).unwrap();
        let d2: MeetingDecision = serde_json::from_str(&json).unwrap();
        assert!(d2.participants.is_empty());
    }

    // ── ActionItem ──────────────────────────────────────────────────

    #[test]
    fn action_item_round_trip_serde() {
        let a = ActionItem {
            description: "Write tests".to_string(),
            owner: "dev".to_string(),
            priority: 1,
            due_description: Some("next sprint".to_string()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let a2: ActionItem = serde_json::from_str(&json).unwrap();
        assert_eq!(a, a2);
    }

    #[test]
    fn action_item_due_description_none() {
        let a = ActionItem {
            description: "Fix bug".to_string(),
            owner: "ops".to_string(),
            priority: 3,
            due_description: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        let a2: ActionItem = serde_json::from_str(&json).unwrap();
        assert_eq!(a2.due_description, None);
    }

    #[test]
    fn action_item_zero_priority() {
        let a = ActionItem {
            description: "Low".to_string(),
            owner: "x".to_string(),
            priority: 0,
            due_description: None,
        };
        assert_eq!(a.priority, 0);
    }

    // ── OpenQuestion ────────────────────────────────────────────────

    #[test]
    fn open_question_explicit_flag() {
        let q = OpenQuestion {
            text: "Why?".to_string(),
            explicit: true,
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: OpenQuestion = serde_json::from_str(&json).unwrap();
        assert!(q2.explicit);
    }

    #[test]
    fn open_question_inferred_flag() {
        let q = OpenQuestion {
            text: "What about X?".to_string(),
            explicit: false,
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: OpenQuestion = serde_json::from_str(&json).unwrap();
        assert!(!q2.explicit);
    }

    // ── MeetingSessionStatus ────────────────────────────────────────

    #[test]
    fn status_display_open() {
        assert_eq!(MeetingSessionStatus::Open.to_string(), "open");
    }

    #[test]
    fn status_display_closed() {
        assert_eq!(MeetingSessionStatus::Closed.to_string(), "closed");
    }

    #[test]
    fn status_serde_round_trip() {
        for status in [MeetingSessionStatus::Open, MeetingSessionStatus::Closed] {
            let json = serde_json::to_string(&status).unwrap();
            let s2: MeetingSessionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, s2);
        }
    }

    // ── MeetingSession ──────────────────────────────────────────────

    fn sample_session() -> MeetingSession {
        MeetingSession {
            topic: "Sprint planning".to_string(),
            decisions: vec![MeetingDecision {
                description: "Adopt TDD".to_string(),
                rationale: "Better quality".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: vec![ActionItem {
                description: "Set up CI".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("Friday".to_string()),
            }],
            notes: vec!["Good discussion".to_string()],
            status: MeetingSessionStatus::Open,
            started_at: "2025-01-01T00:00:00Z".to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
            explicit_questions: vec!["What about testing?".to_string()],
            themes: vec!["performance".to_string()],
        }
    }

    #[test]
    fn session_round_trip_serde() {
        let s = sample_session();
        let json = serde_json::to_string(&s).unwrap();
        let s2: MeetingSession = serde_json::from_str(&json).unwrap();
        assert_eq!(s, s2);
    }

    #[test]
    fn session_explicit_questions_default() {
        // explicit_questions and themes have #[serde(default)], so missing fields default to empty vec
        let json = r#"{
            "topic": "test",
            "decisions": [],
            "action_items": [],
            "notes": [],
            "status": "Open",
            "started_at": "",
            "participants": []
        }"#;
        let s: MeetingSession = serde_json::from_str(json).unwrap();
        assert!(s.explicit_questions.is_empty());
        assert!(s.themes.is_empty());
    }

    #[test]
    fn durable_summary_contains_topic() {
        let s = sample_session();
        let summary = s.durable_summary();
        assert!(summary.contains("Sprint planning"));
    }

    #[test]
    fn durable_summary_contains_decisions() {
        let s = sample_session();
        let summary = s.durable_summary();
        assert!(summary.contains("Adopt TDD"));
    }

    #[test]
    fn durable_summary_contains_action_items_with_owner() {
        let s = sample_session();
        let summary = s.durable_summary();
        assert!(summary.contains("Set up CI"));
        assert!(summary.contains("owner=bob"));
    }

    #[test]
    fn durable_summary_contains_participants() {
        let s = sample_session();
        let summary = s.durable_summary();
        assert!(summary.contains("alice"));
        assert!(summary.contains("bob"));
    }

    #[test]
    fn durable_summary_empty_session() {
        let s = MeetingSession {
            topic: "empty".to_string(),
            decisions: vec![],
            action_items: vec![],
            notes: vec![],
            status: MeetingSessionStatus::Open,
            started_at: "".to_string(),
            participants: vec![],
            explicit_questions: vec![],
            themes: vec![],
        };
        let summary = s.durable_summary();
        assert!(summary.contains("decisions=[none]"));
        assert!(summary.contains("action_items=[none]"));
        assert!(summary.contains("participants=[none]"));
        assert!(summary.contains("duration=unknown"));
    }

    #[test]
    fn durable_summary_invalid_started_at_shows_unknown_duration() {
        let s = MeetingSession {
            topic: "bad-ts".to_string(),
            decisions: vec![],
            action_items: vec![],
            notes: vec![],
            status: MeetingSessionStatus::Open,
            started_at: "not-a-date".to_string(),
            participants: vec![],
            explicit_questions: vec![],
            themes: vec![],
        };
        let summary = s.durable_summary();
        assert!(summary.contains("duration=unknown"));
    }

    #[test]
    fn session_themes_default_empty() {
        let json = r#"{
            "topic": "old-format",
            "decisions": [],
            "action_items": [],
            "notes": [],
            "status": "Open",
            "started_at": "",
            "participants": [],
            "explicit_questions": []
        }"#;
        let s: MeetingSession = serde_json::from_str(json).unwrap();
        assert!(
            s.themes.is_empty(),
            "themes should default to [] for old JSON"
        );
    }
}
