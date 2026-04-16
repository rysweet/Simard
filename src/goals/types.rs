use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::session::{SessionId, SessionPhase};

/// Lifecycle status of a goal in the goal curation system.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GoalStatus {
    Proposed,
    Active,
    Paused,
    Completed,
}

impl GoalStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "proposed" => Some(Self::Proposed),
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    pub(super) fn rank(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Proposed => 1,
            Self::Paused => 2,
            Self::Completed => 3,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl Display for GoalStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
        };
        f.write_str(label)
    }
}

/// A proposed change to a goal (parsed from agent output).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalUpdate {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
}

impl GoalUpdate {
    pub fn new(
        title: impl Into<String>,
        rationale: impl Into<String>,
        status: GoalStatus,
        priority: u8,
    ) -> SimardResult<Self> {
        let title = required_goal_field("title", title.into())?;
        let rationale = required_goal_field("rationale", rationale.into())?;
        validate_priority(priority)?;

        Ok(Self {
            slug: goal_slug(&title),
            title,
            rationale,
            status,
            priority,
        })
    }
}

/// Persisted goal with ownership and provenance metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
    pub owner_identity: String,
    pub source_session_id: SessionId,
    pub updated_in: SessionPhase,
}

impl GoalRecord {
    pub fn from_update(
        update: GoalUpdate,
        owner_identity: impl Into<String>,
        source_session_id: SessionId,
        updated_in: SessionPhase,
    ) -> SimardResult<Self> {
        let owner_identity = required_goal_field("owner_identity", owner_identity.into())?;
        Ok(Self {
            slug: required_goal_field("slug", update.slug)?,
            title: required_goal_field("title", update.title)?,
            rationale: required_goal_field("rationale", update.rationale)?,
            status: update.status,
            priority: update.priority,
            owner_identity,
            source_session_id,
            updated_in,
        })
    }

    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.title)
    }
}

pub fn goal_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn required_goal_field(field: &str, value: String) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidGoalRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

fn validate_priority(priority: u8) -> SimardResult<()> {
    if priority == 0 {
        return Err(SimardError::InvalidGoalRecord {
            field: "priority".to_string(),
            reason: "priority must be at least 1".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn goal_slug_normalizes_title_text() {
        assert_eq!(
            goal_slug("Keep Simard's Top 5 Goals Honest!"),
            "keep-simard-s-top-5-goals-honest"
        );
    }

    #[test]
    fn goal_slug_handles_edge_cases() {
        assert_eq!(goal_slug(""), "");
        assert_eq!(goal_slug("---"), "");
        assert_eq!(goal_slug("  spaces  "), "spaces");
        assert_eq!(goal_slug("ALLCAPS"), "allcaps");
        assert_eq!(goal_slug("a--b"), "a-b");
    }

    // ---- GoalStatus ----

    #[test]
    fn goal_status_parse_all_variants() {
        assert_eq!(GoalStatus::parse("proposed"), Some(GoalStatus::Proposed));
        assert_eq!(GoalStatus::parse("active"), Some(GoalStatus::Active));
        assert_eq!(GoalStatus::parse("paused"), Some(GoalStatus::Paused));
        assert_eq!(GoalStatus::parse("completed"), Some(GoalStatus::Completed));
    }

    #[test]
    fn goal_status_parse_case_insensitive() {
        assert_eq!(GoalStatus::parse("ACTIVE"), Some(GoalStatus::Active));
        assert_eq!(GoalStatus::parse("Proposed"), Some(GoalStatus::Proposed));
        assert_eq!(GoalStatus::parse("  active  "), Some(GoalStatus::Active));
    }

    #[test]
    fn goal_status_parse_unknown_returns_none() {
        assert_eq!(GoalStatus::parse("unknown"), None);
        assert_eq!(GoalStatus::parse(""), None);
    }

    #[test]
    fn goal_status_is_active() {
        assert!(GoalStatus::Active.is_active());
        assert!(!GoalStatus::Proposed.is_active());
        assert!(!GoalStatus::Paused.is_active());
        assert!(!GoalStatus::Completed.is_active());
    }

    #[test]
    fn goal_status_display() {
        assert_eq!(GoalStatus::Proposed.to_string(), "proposed");
        assert_eq!(GoalStatus::Active.to_string(), "active");
        assert_eq!(GoalStatus::Paused.to_string(), "paused");
        assert_eq!(GoalStatus::Completed.to_string(), "completed");
    }

    #[test]
    fn goal_status_serde_round_trip() {
        for status in [
            GoalStatus::Proposed,
            GoalStatus::Active,
            GoalStatus::Paused,
            GoalStatus::Completed,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: GoalStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, parsed);
        }
    }

    // ---- GoalUpdate ----

    #[test]
    fn goal_update_new_valid() {
        let update = GoalUpdate::new("Fix bugs", "They cause crashes", GoalStatus::Active, 1);
        assert!(update.is_ok());
        let u = update.unwrap();
        assert_eq!(u.title, "Fix bugs");
        assert_eq!(u.slug, "fix-bugs");
        assert_eq!(u.priority, 1);
    }

    #[test]
    fn goal_update_rejects_empty_title() {
        let result = GoalUpdate::new("", "rationale", GoalStatus::Active, 1);
        assert!(result.is_err());
    }

    #[test]
    fn goal_update_rejects_empty_rationale() {
        let result = GoalUpdate::new("title", "", GoalStatus::Active, 1);
        assert!(result.is_err());
    }

    #[test]
    fn goal_update_rejects_zero_priority() {
        let result = GoalUpdate::new("title", "rationale", GoalStatus::Active, 0);
        assert!(result.is_err());
    }

    #[test]
    fn goal_update_trims_whitespace() {
        let u = GoalUpdate::new("  padded title  ", "  padded  ", GoalStatus::Active, 5).unwrap();
        assert_eq!(u.title, "padded title");
        assert_eq!(u.rationale, "padded");
    }

    // ---- GoalRecord ----

    #[test]
    fn goal_record_from_update() {
        let update =
            GoalUpdate::new("Improve X", "self-improvement", GoalStatus::Active, 1).unwrap();
        let session_id = SessionId::from_uuid(Uuid::nil());
        let record = GoalRecord::from_update(update, "simard", session_id, SessionPhase::Execution);
        assert!(record.is_ok());
        let r = record.unwrap();
        assert_eq!(r.title, "Improve X");
        assert_eq!(r.owner_identity, "simard");
    }

    #[test]
    fn goal_record_from_update_rejects_empty_owner() {
        let update = GoalUpdate::new("Goal", "reason", GoalStatus::Active, 1).unwrap();
        let session_id = SessionId::from_uuid(Uuid::nil());
        let result = GoalRecord::from_update(update, "", session_id, SessionPhase::Execution);
        assert!(result.is_err());
    }

    #[test]
    fn goal_record_concise_label() {
        let update =
            GoalUpdate::new("Fix memory leaks", "important", GoalStatus::Active, 2).unwrap();
        let session_id = SessionId::from_uuid(Uuid::nil());
        let record =
            GoalRecord::from_update(update, "simard", session_id, SessionPhase::Execution).unwrap();
        let label = record.concise_label();
        assert_eq!(label, "p2 [active] Fix memory leaks");
    }
}
