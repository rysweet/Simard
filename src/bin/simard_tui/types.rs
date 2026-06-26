//! Standalone serde DTOs for the TUI monitoring client.
//!
//! These types mirror `simard::goal_curation::types` without depending on
//! the library crate. All fields use `#[serde(default)]` where appropriate
//! for forward-compatibility tolerance — the daemon may add fields that
//! older TUI versions should silently ignore.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Progress state for an active goal on the board.
///
/// Mirrors `simard::goal_curation::GoalProgress`. Uses serde's default
/// externally-tagged enum representation so JSON produced by the library
/// deserializes correctly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GoalProgress {
    Proposed,
    NotStarted,
    InProgress { percent: u32 },
    Blocked(String),
    Paused,
    Completed,
}

impl fmt::Display for GoalProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proposed => f.write_str("proposed"),
            Self::NotStarted => f.write_str("not-started"),
            Self::InProgress { percent } => write!(f, "in-progress({percent}%)"),
            Self::Blocked(reason) => write!(f, "blocked: {reason}"),
            Self::Paused => f.write_str("paused"),
            Self::Completed => f.write_str("completed"),
        }
    }
}

/// A reference to work-in-progress associated with a goal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WipRef {
    pub kind: String,
    pub ref_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// An active goal on the board.
///
/// Mirrors `simard::goal_curation::ActiveGoal`. Optional fields use
/// `#[serde(default)]` so missing keys deserialize to `None`/empty rather
/// than failing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveGoal {
    pub id: String,
    pub description: String,
    pub priority: u32,
    pub status: GoalProgress,
    #[serde(default)]
    pub assigned_to: Option<String>,
    /// Target repository slug (issue #2359). `None` = the daemon's own repo.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub current_activity: Option<String>,
    #[serde(default)]
    pub wip_refs: Vec<WipRef>,
}

/// A backlog item scored for future promotion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacklogItem {
    pub id: String,
    pub description: String,
    pub source: String,
    pub score: f64,
}

/// The goal board: active goals + scored backlog.
///
/// Both fields default to empty vecs so a `{}` JSON object deserializes
/// to an empty board rather than failing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct GoalBoard {
    #[serde(default)]
    pub active: Vec<ActiveGoal>,
    #[serde(default)]
    pub backlog: Vec<BacklogItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── GoalProgress serde round-trips ──────────────────────────────

    #[test]
    fn goal_progress_proposed_roundtrip() {
        let v = GoalProgress::Proposed;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#""Proposed""#);
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_not_started_roundtrip() {
        let v = GoalProgress::NotStarted;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#""NotStarted""#);
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_in_progress_roundtrip() {
        let v = GoalProgress::InProgress { percent: 42 };
        let json = serde_json::to_string(&v).unwrap();
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
        assert!(json.contains("42"));
    }

    #[test]
    fn goal_progress_in_progress_zero_percent() {
        let v = GoalProgress::InProgress { percent: 0 };
        let json = serde_json::to_string(&v).unwrap();
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_in_progress_hundred_percent() {
        let v = GoalProgress::InProgress { percent: 100 };
        let json = serde_json::to_string(&v).unwrap();
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_blocked_roundtrip() {
        let v = GoalProgress::Blocked("waiting on review".to_string());
        let json = serde_json::to_string(&v).unwrap();
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_paused_roundtrip() {
        let v = GoalProgress::Paused;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#""Paused""#);
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn goal_progress_completed_roundtrip() {
        let v = GoalProgress::Completed;
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#""Completed""#);
        let v2: GoalProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v2);
    }

    // ── GoalProgress Display ────────────────────────────────────────

    #[test]
    fn goal_progress_display_all_variants() {
        assert_eq!(GoalProgress::Proposed.to_string(), "proposed");
        assert_eq!(GoalProgress::NotStarted.to_string(), "not-started");
        assert_eq!(
            GoalProgress::InProgress { percent: 75 }.to_string(),
            "in-progress(75%)"
        );
        assert_eq!(
            GoalProgress::Blocked("stuck".into()).to_string(),
            "blocked: stuck"
        );
        assert_eq!(GoalProgress::Paused.to_string(), "paused");
        assert_eq!(GoalProgress::Completed.to_string(), "completed");
    }

    // ── ActiveGoal ──────────────────────────────────────────────────

    fn sample_active_goal() -> ActiveGoal {
        ActiveGoal {
            repo: None,
            id: "g-1".to_string(),
            description: "Ship MVP".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 75 },
            assigned_to: Some("team-a".to_string()),
            current_activity: Some("Building feature X".to_string()),
            wip_refs: vec![WipRef {
                kind: "pr".to_string(),
                ref_id: "42".to_string(),
                label: "PR #42".to_string(),
                url: Some("https://github.com/org/repo/pull/42".to_string()),
            }],
        }
    }

    #[test]
    fn active_goal_full_roundtrip() {
        let g = sample_active_goal();
        let json = serde_json::to_string(&g).unwrap();
        let g2: ActiveGoal = serde_json::from_str(&json).unwrap();
        assert_eq!(g, g2);
    }

    #[test]
    fn active_goal_optional_fields_default() {
        // Minimal JSON: only required fields.
        let json = r#"{
            "id": "g-min",
            "description": "Minimal goal",
            "priority": 0,
            "status": "NotStarted"
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.id, "g-min");
        assert_eq!(g.description, "Minimal goal");
        assert_eq!(g.priority, 0);
        assert_eq!(g.status, GoalProgress::NotStarted);
        assert_eq!(g.assigned_to, None);
        assert_eq!(g.current_activity, None);
        assert!(g.wip_refs.is_empty());
    }

    #[test]
    fn active_goal_unknown_fields_tolerated() {
        // JSON with an extra field that doesn't exist in our struct.
        // Forward compatibility: we must silently ignore unknown fields.
        let json = r#"{
            "id": "g-future",
            "description": "Future goal",
            "priority": 1,
            "status": "Proposed",
            "some_new_field": "should be ignored",
            "another_field": 42
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.id, "g-future");
        assert_eq!(g.status, GoalProgress::Proposed);
    }

    #[test]
    fn active_goal_priority_is_u32() {
        // Priority is numeric, not a string like "p0".
        let json = r#"{
            "id": "g-p",
            "description": "Priority check",
            "priority": 3,
            "status": "NotStarted"
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.priority, 3u32);
    }

    #[test]
    fn active_goal_uses_description_not_title() {
        // The field is "description", not "title".
        let json = r#"{
            "id": "g-d",
            "description": "This is the description field",
            "priority": 1,
            "status": "Completed"
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.description, "This is the description field");
    }

    #[test]
    fn active_goal_wip_ref_with_null_url() {
        let json = r#"{
            "id": "g-wr",
            "description": "WipRef test",
            "priority": 1,
            "status": "NotStarted",
            "wip_refs": [
                {"kind": "branch", "ref_id": "main", "label": "main branch"}
            ]
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.wip_refs.len(), 1);
        assert_eq!(g.wip_refs[0].url, None);
    }

    // ── BacklogItem ─────────────────────────────────────────────────

    #[test]
    fn backlog_item_roundtrip() {
        let b = BacklogItem {
            id: "b-1".to_string(),
            description: "Refactor auth".to_string(),
            source: "review".to_string(),
            score: 0.85,
        };
        let json = serde_json::to_string(&b).unwrap();
        let b2: BacklogItem = serde_json::from_str(&json).unwrap();
        assert_eq!(b, b2);
    }

    // ── GoalBoard ───────────────────────────────────────────────────

    #[test]
    fn goal_board_empty_roundtrip() {
        let board = GoalBoard::default();
        let json = serde_json::to_string(&board).unwrap();
        let b2: GoalBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(board, b2);
        assert!(b2.active.is_empty());
        assert!(b2.backlog.is_empty());
    }

    #[test]
    fn goal_board_empty_json_object_parses() {
        // A bare `{}` must parse to an empty board.
        let board: GoalBoard = serde_json::from_str("{}").unwrap();
        assert!(board.active.is_empty());
        assert!(board.backlog.is_empty());
    }

    #[test]
    fn goal_board_populated_roundtrip() {
        let board = GoalBoard {
            active: vec![sample_active_goal()],
            backlog: vec![BacklogItem {
                id: "b-1".to_string(),
                description: "Later".to_string(),
                source: "auto".to_string(),
                score: 0.5,
            }],
        };
        let json = serde_json::to_string(&board).unwrap();
        let b2: GoalBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(board, b2);
    }

    #[test]
    fn goal_board_from_realistic_snapshot() {
        // Realistic JSON matching what the simard daemon writes to cognitive memory.
        let json = r#"{
            "active": [
                {
                    "id": "goal-ship-tui",
                    "description": "Ship the TUI monitoring client",
                    "priority": 0,
                    "status": {"InProgress": {"percent": 60}},
                    "assigned_to": "engineer-1",
                    "current_activity": "Writing tests",
                    "wip_refs": [
                        {"kind": "pr", "ref_id": "2193", "label": "PR #2193", "url": "https://github.com/rysweet/Simard/pull/2193"},
                        {"kind": "branch", "ref_id": "feat/tui", "label": "feat/tui"}
                    ]
                },
                {
                    "id": "goal-improve-memory",
                    "description": "Improve cognitive memory durability",
                    "priority": 1,
                    "status": "Paused",
                    "assigned_to": null,
                    "wip_refs": []
                },
                {
                    "id": "goal-refactor-bridge",
                    "description": "Refactor bridge layer",
                    "priority": 2,
                    "status": {"Blocked": "Waiting on lbug 0.16 release"}
                }
            ],
            "backlog": [
                {"id": "b-perf", "description": "Performance tuning", "source": "review", "score": 0.72},
                {"id": "b-docs", "description": "Update API docs", "source": "auto", "score": 0.45}
            ]
        }"#;
        let board: GoalBoard = serde_json::from_str(json).unwrap();
        assert_eq!(board.active.len(), 3);
        assert_eq!(board.backlog.len(), 2);

        // First goal: InProgress with percent
        assert_eq!(board.active[0].id, "goal-ship-tui");
        assert_eq!(board.active[0].priority, 0);
        assert_eq!(
            board.active[0].status,
            GoalProgress::InProgress { percent: 60 }
        );
        assert_eq!(board.active[0].assigned_to.as_deref(), Some("engineer-1"));
        assert_eq!(board.active[0].wip_refs.len(), 2);

        // Second goal: Paused, no assigned_to
        assert_eq!(board.active[1].status, GoalProgress::Paused);
        assert_eq!(board.active[1].assigned_to, None);

        // Third goal: Blocked with reason
        assert_eq!(
            board.active[2].status,
            GoalProgress::Blocked("Waiting on lbug 0.16 release".to_string())
        );

        // Backlog items
        assert_eq!(board.backlog[0].score, 0.72);
    }

    #[test]
    fn goal_board_with_all_progress_variants() {
        // Ensure every GoalProgress variant deserializes when embedded in ActiveGoal.
        let variants = [
            (r#""Proposed""#, GoalProgress::Proposed),
            (r#""NotStarted""#, GoalProgress::NotStarted),
            (
                r#"{"InProgress":{"percent":50}}"#,
                GoalProgress::InProgress { percent: 50 },
            ),
            (
                r#"{"Blocked":"reason"}"#,
                GoalProgress::Blocked("reason".to_string()),
            ),
            (r#""Paused""#, GoalProgress::Paused),
            (r#""Completed""#, GoalProgress::Completed),
        ];
        for (i, (status_json, expected)) in variants.iter().enumerate() {
            let json = format!(
                r#"{{"id":"g-{i}","description":"test","priority":0,"status":{status_json}}}"#
            );
            let g: ActiveGoal =
                serde_json::from_str(&json).unwrap_or_else(|e| panic!("variant {i}: {e}"));
            assert_eq!(&g.status, expected, "variant {i}");
        }
    }

    #[test]
    fn goal_board_ignores_last_progress_update_at() {
        // The library type has last_progress_update_at; our DTO doesn't.
        // We must silently ignore it.
        let json = r#"{
            "id": "g-ts",
            "description": "Timestamp test",
            "priority": 1,
            "status": "NotStarted",
            "last_progress_update_at": "2025-01-15T10:30:45Z"
        }"#;
        let g: ActiveGoal = serde_json::from_str(json).unwrap();
        assert_eq!(g.id, "g-ts");
    }
}
