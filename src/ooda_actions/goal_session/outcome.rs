//! Goal-session response contract enforcement.
//!
//! The prompt owns semantic judgment. Rust only enforces the explicit response
//! shapes the prompt promises, so invalid or ambiguous output fails loudly
//! instead of being reinterpreted by deterministic policy.

/// Maximum length of user-derived text (task, reason) included in outcome
/// detail strings before truncation.
pub(super) const OUTCOME_TEXT_MAX: usize = 256;

/// A decision returned by the goal-advance LLM session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GoalAction {
    /// Spawn a subordinate engineer to do the concrete `task`.
    SpawnEngineer {
        task: String,
        /// Reserved for future structured input; currently always empty in the
        /// explicit-contract path.
        files: Vec<String>,
        /// Optional GitHub issue number this work advances. Reserved for
        /// future structured input; currently always `None`.
        issue: Option<u64>,
    },
    /// No engineer subprocess this cycle.
    NoAction { reason: String },
}

/// The decision the orchestrator LLM made for this cycle, paired with any
/// progress percentage extracted from a `PROGRESS: NN` marker.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OrchestratorDecision {
    pub action: GoalAction,
    pub progress_pct: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum GoalSessionParse {
    Empty,
    Decision(OrchestratorDecision),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct GoalSessionParseError {
    detail: String,
}

impl GoalSessionParseError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }
}

/// Parse the orchestrator LLM's response into a structured decision.
///
/// This compatibility helper returns `None` for either empty or invalid output.
/// Callers that need the visible error detail must use
/// [`parse_orchestrator_response_strict`].
#[cfg(test)]
pub(super) fn parse_orchestrator_response(response: &str) -> Option<OrchestratorDecision> {
    match parse_orchestrator_response_strict(response) {
        Ok(GoalSessionParse::Decision(decision)) => Some(decision),
        Ok(GoalSessionParse::Empty) | Err(_) => None,
    }
}

pub(super) fn parse_orchestrator_response_strict(
    response: &str,
) -> Result<GoalSessionParse, GoalSessionParseError> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return Ok(GoalSessionParse::Empty);
    }

    let has_exact_no_action = trimmed.lines().any(|line| line.trim() == "NO ACTION");
    let action_lines: Vec<&str> = trimmed
        .lines()
        .filter(|line| line.trim_start().starts_with("ACTION:"))
        .collect();

    reject_lowercase_markers(trimmed)?;
    let progress_pct = extract_progress_marker_strict(trimmed)?;

    if has_exact_no_action && !action_lines.is_empty() {
        return Err(GoalSessionParseError::new(
            "invalid goal-session response: conflicting NO ACTION and ACTION markers",
        ));
    }

    if has_exact_no_action {
        let reason = extract_no_action_reason(trimmed)?;
        return Ok(GoalSessionParse::Decision(OrchestratorDecision {
            action: GoalAction::NoAction { reason },
            progress_pct,
        }));
    }

    if let Some(action_line) = action_lines.first() {
        if action_lines.len() > 1 {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: multiple ACTION markers",
            ));
        }
        let action = action_line.trim();
        if action != "ACTION: SPAWN_ENGINEER" {
            return Err(GoalSessionParseError::new(format!(
                "invalid goal-session response: unknown action marker '{action}'"
            )));
        }
        let task = extract_spawn_task(trimmed)?;
        return Ok(GoalSessionParse::Decision(OrchestratorDecision {
            action: GoalAction::SpawnEngineer {
                task,
                files: Vec::new(),
                issue: None,
            },
            progress_pct,
        }));
    }

    Err(GoalSessionParseError::new(
        "invalid goal-session response contract: expected 'ACTION: SPAWN_ENGINEER' with 'TASK:' or 'NO ACTION' with 'REASON:'",
    ))
}

fn reject_lowercase_markers(s: &str) -> Result<(), GoalSessionParseError> {
    for line in s.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("NO ACTION") && line != "NO ACTION" {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: NO ACTION marker must be uppercase exactly",
            ));
        }
        if line.eq_ignore_ascii_case("NO_ACTION") {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: use 'NO ACTION', not 'NO_ACTION'",
            ));
        }
        if line
            .get(.."progress:".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("progress:"))
            && !line.starts_with("PROGRESS:")
        {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: PROGRESS marker must be uppercase exactly",
            ));
        }
    }
    Ok(())
}

fn extract_no_action_reason(s: &str) -> Result<String, GoalSessionParseError> {
    let mut parts = Vec::new();
    let mut saw_reason = false;

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed == "NO ACTION" || trimmed.starts_with("PROGRESS:") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("REASON:") {
            saw_reason = true;
            let reason = rest.trim();
            if !reason.is_empty() {
                parts.push(reason.to_string());
            }
            continue;
        }
        if saw_reason && !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }

    if !saw_reason {
        return Err(GoalSessionParseError::new(
            "invalid goal-session response: NO ACTION requires a REASON: line",
        ));
    }

    let reason = parts.join("\n").trim().to_string();
    if reason.is_empty() {
        return Err(GoalSessionParseError::new(
            "invalid goal-session response: REASON must not be empty",
        ));
    }
    Ok(reason)
}

fn extract_spawn_task(s: &str) -> Result<String, GoalSessionParseError> {
    let mut task_lines = Vec::new();
    let mut in_task = false;

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed == "TASK:" {
            in_task = true;
            continue;
        }
        if trimmed == "ACTION: SPAWN_ENGINEER" || trimmed.starts_with("PROGRESS:") {
            continue;
        }
        if in_task {
            task_lines.push(line.trim_end());
        }
    }

    if !in_task {
        return Err(GoalSessionParseError::new(
            "invalid goal-session response: ACTION: SPAWN_ENGINEER requires a TASK: block",
        ));
    }

    let task = task_lines.join("\n").trim().to_string();
    if task.is_empty() {
        return Err(GoalSessionParseError::new(
            "invalid goal-session response: TASK must not be empty",
        ));
    }
    Ok(task)
}

fn extract_progress_marker_strict(s: &str) -> Result<Option<u8>, GoalSessionParseError> {
    let mut found = None;
    for line in s.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("PROGRESS:") else {
            continue;
        };
        if found.is_some() {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: duplicate PROGRESS markers",
            ));
        }
        let raw = rest.trim();
        if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_digit()) {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: PROGRESS must be an integer in 0..=100",
            ));
        }
        let parsed = raw.parse::<u16>().map_err(|_| {
            GoalSessionParseError::new("invalid goal-session response: PROGRESS out of range")
        })?;
        if parsed > 100 {
            return Err(GoalSessionParseError::new(
                "invalid goal-session response: PROGRESS out of range 0..=100",
            ));
        }
        found = Some(parsed as u8);
    }
    Ok(found)
}

/// Truncate a user-derived string for safe inclusion in outcome details / logs.
pub(super) fn truncate_for_outcome(s: &str) -> String {
    if s.len() <= OUTCOME_TEXT_MAX {
        s.to_string()
    } else {
        let mut end = OUTCOME_TEXT_MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
