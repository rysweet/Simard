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

    #[test]
    fn goal_slug_normalizes_title_text() {
        assert_eq!(
            goal_slug("Keep Simard's Top 5 Goals Honest!"),
            "keep-simard-s-top-5-goals-honest"
        );
    }
}
