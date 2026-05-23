use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalStatus, GoalUpdate};

pub(crate) fn parse_goal_directive(raw: &str, default_priority: u8) -> SimardResult<GoalUpdate> {
    let mut segments = raw
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let title = segments
        .next()
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal".to_string(),
            reason: "goal entries must include a title before any attributes".to_string(),
        })?;
    let mut priority = default_priority.max(1);
    let mut status = GoalStatus::Active;
    let mut rationale = "captured as a durable Simard priority".to_string();

    for segment in segments {
        let (key, value) =
            segment
                .split_once('=')
                .ok_or_else(|| SimardError::InvalidGoalRecord {
                    field: "goal".to_string(),
                    reason: format!("goal attribute '{segment}' must look like key=value"),
                })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            return Err(SimardError::InvalidGoalRecord {
                field: key,
                reason: "goal attribute values cannot be empty".to_string(),
            });
        }
        match key.as_str() {
            "priority" => {
                priority = value
                    .parse::<u8>()
                    .map_err(|_| SimardError::InvalidGoalRecord {
                        field: "priority".to_string(),
                        reason: format!("goal priority '{value}' is not a valid integer"),
                    })?;
            }
            "status" => status = parse_goal_status(value)?,
            "rationale" => rationale = value.to_string(),
            other => {
                return Err(SimardError::InvalidGoalRecord {
                    field: other.to_string(),
                    reason: "supported goal attributes are priority=, status=, and rationale="
                        .to_string(),
                });
            }
        }
    }

    GoalUpdate::new(title, rationale, status, priority)
}

pub(crate) fn parse_goal_status(value: &str) -> SimardResult<GoalStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "candidate" => Ok(GoalStatus::Proposed),
        "hold" | "holding" => Ok(GoalStatus::Paused),
        "done" => Ok(GoalStatus::Completed),
        other => GoalStatus::parse(other).ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "status".to_string(),
            reason: format!(
                "unsupported goal status '{other}'; expected active, proposed, paused, or completed"
            ),
        }),
    }
}

// `format_items` and `format_goal_items` previously lived here as helpers
// for `StructuredMeetingNotes::concise_record`. They were exact duplicates
// of the persisted-meeting-record rendering logic in `src/meetings/mod.rs`
// and are now obsoleted by `PersistedMeetingRecord::render` (issue #2003).
// If a future writer needs the same bracketed-list semantics it should call
// `PersistedMeetingRecord::render` rather than reintroduce a parallel helper.

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_goal_directive ---

    #[test]
    fn parse_goal_directive_minimal() {
        let goal = parse_goal_directive("Ship v1", 1).unwrap();
        assert_eq!(goal.title, "Ship v1");
        assert_eq!(goal.priority, 1);
        assert_eq!(goal.status, GoalStatus::Active);
    }

    #[test]
    fn parse_goal_directive_with_all_attributes() {
        let goal = parse_goal_directive(
            "Ship v1 | priority=2 | status=proposed | rationale=roadmap",
            1,
        )
        .unwrap();
        assert_eq!(goal.title, "Ship v1");
        assert_eq!(goal.priority, 2);
        assert_eq!(goal.status, GoalStatus::Proposed);
        assert_eq!(goal.rationale, "roadmap");
    }

    #[test]
    fn parse_goal_directive_rejects_missing_title() {
        let err = parse_goal_directive("", 1).unwrap_err();
        assert!(matches!(err, SimardError::InvalidGoalRecord { .. }));
    }

    #[test]
    fn parse_goal_directive_rejects_invalid_attribute_format() {
        let err = parse_goal_directive("Title | bad-attr", 1).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("key=value"));
    }

    #[test]
    fn parse_goal_directive_rejects_empty_attribute_value() {
        let err = parse_goal_directive("Title | priority=", 1).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot be empty"));
    }

    #[test]
    fn parse_goal_directive_rejects_unsupported_attribute() {
        let err = parse_goal_directive("Title | foo=bar", 1).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("supported goal attributes"));
    }

    #[test]
    fn parse_goal_directive_rejects_invalid_priority() {
        let err = parse_goal_directive("Title | priority=abc", 1).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a valid integer"));
    }

    // --- parse_goal_status ---

    #[test]
    fn parse_goal_status_standard_values() {
        assert_eq!(parse_goal_status("active").unwrap(), GoalStatus::Active);
        assert_eq!(parse_goal_status("proposed").unwrap(), GoalStatus::Proposed);
        assert_eq!(parse_goal_status("paused").unwrap(), GoalStatus::Paused);
        assert_eq!(
            parse_goal_status("completed").unwrap(),
            GoalStatus::Completed
        );
    }

    #[test]
    fn parse_goal_status_aliases() {
        assert_eq!(
            parse_goal_status("candidate").unwrap(),
            GoalStatus::Proposed
        );
        assert_eq!(parse_goal_status("hold").unwrap(), GoalStatus::Paused);
        assert_eq!(parse_goal_status("holding").unwrap(), GoalStatus::Paused);
        assert_eq!(parse_goal_status("done").unwrap(), GoalStatus::Completed);
    }

    #[test]
    fn parse_goal_status_case_insensitive() {
        assert_eq!(parse_goal_status("ACTIVE").unwrap(), GoalStatus::Active);
        assert_eq!(parse_goal_status("Proposed").unwrap(), GoalStatus::Proposed);
    }

    #[test]
    fn parse_goal_status_invalid() {
        let err = parse_goal_status("bogus").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported goal status"));
    }

    // --- (format_items / format_goal_items tests removed alongside the
    //     helpers they covered — see the comment above the removed helpers
    //     in this file for rationale. The wire-format invariants those tests
    //     guarded are now covered by `PersistedMeetingRecord::render`
    //     round-trip tests in `src/meetings/tests.rs`.)
}
