use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalStatus, GoalUpdate};
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

    /// Render this goal update in the persisted on-wire form
    /// `p<priority>:<status>:<title>:<rationale>` used inside the
    /// `goals=[...]` bracketed list of a `PersistedMeetingRecord`.
    ///
    /// Round-trips cleanly through [`parse_goal_update`]; both the REPL close
    /// path and the non-REPL `MeetingFacilitatorProgram` path use this so the
    /// on-disk format cannot drift between writers (issue #2003).
    pub fn render(&self) -> String {
        format!(
            "p{}:{}:{}:{}",
            self.priority, self.status, self.title, self.rationale
        )
    }
}

impl From<&GoalUpdate> for PersistedMeetingGoalUpdate {
    fn from(update: &GoalUpdate) -> Self {
        Self {
            priority: update.priority,
            status: update.status,
            title: update.title.clone(),
            rationale: update.rationale.clone(),
        }
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
    /// Render this record into the canonical on-wire string consumed by
    /// [`Self::parse`] and detected by [`looks_like_persisted_meeting_record`].
    ///
    /// This is the **single** writer of the persisted-meeting wire format.
    /// Both the REPL close path (`meeting_backend::persist::memory_records`)
    /// and the non-REPL `MeetingFacilitatorProgram::additional_memory_records`
    /// route through this method, eliminating the silent-drift risk between
    /// the two persistence paths that issue #2003 was filed against.
    ///
    /// Empty fields render as `[]`; the agenda falls back to `"meeting"` when
    /// the input is whitespace-only, matching the read companion's tolerance
    /// for unlabelled meetings.
    pub fn render(&self) -> String {
        let agenda = if self.agenda.trim().is_empty() {
            "meeting"
        } else {
            self.agenda.trim()
        };
        format!(
            "agenda={}; updates={}; decisions={}; risks={}; next_steps={}; open_questions={}; goals={}",
            agenda,
            render_bracketed_text(&self.updates),
            render_bracketed_text(&self.decisions),
            render_bracketed_text(&self.risks),
            render_bracketed_text(&self.next_steps),
            render_bracketed_text(&self.open_questions),
            render_bracketed_goals(&self.goals),
        )
    }

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

/// Build the canonical `PersistedMeetingRecord` wire string for the simple
/// case where a writer has only strings on hand (REPL close path: topic,
/// decisions, action item descriptions, open questions).
///
/// Sets `updates`, `risks`, and `goals` to empty — the REPL has no surface
/// for those fields today. Both the REPL close path and the non-REPL
/// `MeetingFacilitatorProgram` route through [`PersistedMeetingRecord::render`]
/// so the on-disk format cannot drift between the two writers (issue #2003).
pub fn build_persisted_meeting_record_value(
    topic: &str,
    decisions: &[String],
    action_items: &[String],
    open_questions: &[String],
) -> String {
    let record = PersistedMeetingRecord {
        agenda: topic.to_string(),
        updates: Vec::new(),
        decisions: clean_text_items(decisions),
        risks: Vec::new(),
        next_steps: clean_text_items(action_items),
        open_questions: clean_text_items(open_questions),
        goals: Vec::new(),
    };
    record.render()
}

fn clean_text_items(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn render_bracketed_text(items: &[String]) -> String {
    let cleaned = clean_text_items(items);
    if cleaned.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", cleaned.join(" | "))
    }
}

fn render_bracketed_goals(items: &[PersistedMeetingGoalUpdate]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            items
                .iter()
                .map(PersistedMeetingGoalUpdate::render)
                .collect::<Vec<_>>()
                .join(" | ")
        )
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
mod tests;
