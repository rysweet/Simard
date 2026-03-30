use crate::error::{SimardError, SimardResult};
use crate::goals::GoalStatus;
use crate::sanitization::sanitize_terminal_text;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedMeetingGoalUpdate {
    pub priority: u8,
    pub status: GoalStatus,
    pub title: String,
    pub rationale: String,
}

impl PersistedMeetingGoalUpdate {
    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.title)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedMeetingRecord {
    pub agenda: String,
    pub updates: Vec<String>,
    pub decisions: Vec<String>,
    pub risks: Vec<String>,
    pub next_steps: Vec<String>,
    pub open_questions: Vec<String>,
    pub goals: Vec<PersistedMeetingGoalUpdate>,
}

impl PersistedMeetingRecord {
    pub fn parse(raw: &str) -> SimardResult<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(SimardError::InvalidMeetingRecord {
                field: "record".to_string(),
                reason: "persisted meeting record cannot be empty".to_string(),
            });
        }

        let (agenda_raw, remainder) =
            take_scalar_field(trimmed, "agenda=", "; updates=", "agenda")?;
        let (updates_raw, remainder) =
            take_bracketed_field(remainder, "updates=", "; decisions=", "updates")?;
        let (decisions_raw, remainder) =
            take_bracketed_field(remainder, "decisions=", "; risks=", "decisions")?;
        let (risks_raw, remainder) =
            take_bracketed_field(remainder, "risks=", "; next_steps=", "risks")?;
        let (next_steps_raw, remainder) =
            take_bracketed_field(remainder, "next_steps=", "; open_questions=", "next_steps")?;
        let (open_questions_raw, goals_raw) =
            take_bracketed_field(remainder, "open_questions=", "; goals=", "open_questions")?;
        let goals_raw = goals_raw.trim();
        let Some(goals_raw) = goals_raw.strip_prefix("goals=") else {
            return Err(SimardError::InvalidMeetingRecord {
                field: "goals".to_string(),
                reason: "expected goals=[...] after open_questions".to_string(),
            });
        };

        Ok(Self {
            agenda: required_meeting_field("agenda", agenda_raw)?.to_string(),
            updates: parse_bracketed_text_list("updates", updates_raw)?,
            decisions: parse_bracketed_text_list("decisions", decisions_raw)?,
            risks: parse_bracketed_text_list("risks", risks_raw)?,
            next_steps: parse_bracketed_text_list("next_steps", next_steps_raw)?,
            open_questions: parse_bracketed_text_list("open_questions", open_questions_raw)?,
            goals: parse_goal_update_list("goals", goals_raw)?,
        })
    }
}

pub fn looks_like_persisted_meeting_record(value: &str) -> bool {
    [
        "agenda=",
        "updates=",
        "decisions=",
        "risks=",
        "next_steps=",
        "open_questions=",
        "goals=",
    ]
    .into_iter()
    .all(|fragment| value.contains(fragment))
}

fn take_scalar_field<'a>(
    raw: &'a str,
    prefix: &str,
    next_marker: &str,
    field: &str,
) -> SimardResult<(&'a str, &'a str)> {
    let Some(value_and_rest) = raw.strip_prefix(prefix) else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!("expected '{prefix}' at the start of the persisted meeting record"),
        });
    };
    let Some(next_index) = value_and_rest.find(next_marker) else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!("expected '{next_marker}' after {field}"),
        });
    };
    let value = &value_and_rest[..next_index];
    let rest = &value_and_rest[next_index + 2..];
    Ok((value.trim(), rest.trim()))
}

fn take_bracketed_field<'a>(
    raw: &'a str,
    prefix: &str,
    next_marker: &str,
    field: &str,
) -> SimardResult<(&'a str, &'a str)> {
    let Some(value_and_rest) = raw.strip_prefix(prefix) else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!("expected '{prefix}' in the persisted meeting record"),
        });
    };
    let Some(next_index) = value_and_rest.find(next_marker) else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!("expected '{next_marker}' after {field}"),
        });
    };
    let value = &value_and_rest[..next_index];
    let rest = &value_and_rest[next_index + 2..];
    if !(value.trim_start().starts_with('[') && value.trim_end().ends_with(']')) {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: "value must use bracketed list syntax".to_string(),
        });
    }
    Ok((value.trim(), rest.trim()))
}

fn parse_bracketed_text_list(field: &str, raw: &str) -> SimardResult<Vec<String>> {
    parse_bracketed_items(field, raw)
}

fn parse_goal_update_list(field: &str, raw: &str) -> SimardResult<Vec<PersistedMeetingGoalUpdate>> {
    parse_bracketed_items(field, raw)?
        .into_iter()
        .map(|item| parse_goal_update(field, &item))
        .collect()
}

fn parse_bracketed_items(field: &str, raw: &str) -> SimardResult<Vec<String>> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: "value must use bracketed list syntax".to_string(),
        });
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner
        .split(" | ")
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect())
}

fn parse_goal_update(field: &str, raw: &str) -> SimardResult<PersistedMeetingGoalUpdate> {
    let sanitized = sanitize_terminal_text(raw);
    let segments = sanitized.splitn(4, ':').collect::<Vec<_>>();
    if segments.len() != 4 {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!(
                "goal update '{}' must use p<priority>:<status>:<title>:<rationale>",
                sanitized.trim()
            ),
        });
    }

    let Some(priority_raw) = segments[0].trim().strip_prefix('p') else {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!(
                "goal update '{}' must start with p<priority>",
                sanitized.trim()
            ),
        });
    };
    let priority = priority_raw
        .parse::<u8>()
        .map_err(|_| SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!("goal update '{}' has an invalid priority", sanitized.trim()),
        })?;
    if priority == 0 {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!(
                "goal update '{}' must use priority 1 or greater",
                sanitized.trim()
            ),
        });
    }

    let status =
        GoalStatus::parse(segments[1]).ok_or_else(|| SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: format!(
                "goal update '{}' uses an unsupported status",
                sanitized.trim()
            ),
        })?;

    Ok(PersistedMeetingGoalUpdate {
        priority,
        status,
        title: required_meeting_field(field, segments[2])?.to_string(),
        rationale: required_meeting_field(field, segments[3])?.to_string(),
    })
}

fn required_meeting_field<'a>(field: &str, value: &'a str) -> SimardResult<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidMeetingRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::{PersistedMeetingGoalUpdate, PersistedMeetingRecord};
    use crate::error::SimardError;
    use crate::goals::GoalStatus;

    #[test]
    fn parses_persisted_meeting_record_for_readback() {
        let record = PersistedMeetingRecord::parse(
            "agenda=align the next Simard workstream; updates=[durable memory merged]; decisions=[preserve meeting-to-engineer continuity]; risks=[workflow routing is still unreliable]; next_steps=[keep durable priorities visible]; open_questions=[how aggressively should Simard reprioritize?]; goals=[p1:active:Preserve meeting handoff:meeting decisions must shape later work]",
        )
        .expect("persisted meeting record should parse");

        assert_eq!(record.agenda, "align the next Simard workstream");
        assert_eq!(record.updates, vec!["durable memory merged"]);
        assert_eq!(
            record.decisions,
            vec!["preserve meeting-to-engineer continuity"]
        );
        assert_eq!(record.risks, vec!["workflow routing is still unreliable"]);
        assert_eq!(record.next_steps, vec!["keep durable priorities visible"]);
        assert_eq!(
            record.open_questions,
            vec!["how aggressively should Simard reprioritize?"]
        );
        assert_eq!(
            record.goals,
            vec![PersistedMeetingGoalUpdate {
                priority: 1,
                status: GoalStatus::Active,
                title: "Preserve meeting handoff".to_string(),
                rationale: "meeting decisions must shape later work".to_string(),
            }]
        );
        assert_eq!(
            record.goals[0].concise_label(),
            "p1 [active] Preserve meeting handoff"
        );
    }

    #[test]
    fn rejects_malformed_persisted_meeting_goal_update() {
        let error = PersistedMeetingRecord::parse(
            "agenda=align the next Simard workstream; updates=[]; decisions=[preserve meeting-to-engineer continuity]; risks=[]; next_steps=[]; open_questions=[]; goals=[p0:active:Preserve meeting handoff:meeting decisions must shape later work]",
        )
        .expect_err("malformed goal update should fail");

        assert_eq!(
            error,
            SimardError::InvalidMeetingRecord {
                field: "goals".to_string(),
                reason: "goal update 'p0:active:Preserve meeting handoff:meeting decisions must shape later work' must use priority 1 or greater".to_string(),
            }
        );
    }
}
