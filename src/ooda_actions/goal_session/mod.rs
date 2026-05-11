//! Session-based goal advancement — delegates work to a base-type agent.
//!
//! The orchestrator LLM emits **prose only** (no JSON). Two response
//! shapes are supported:
//!
//! 1. Free-form prose → dispatched as a `SpawnEngineer` task description.
//!    The engineer subprocess is itself a full coding agent that can run
//!    `gh issue create`, `gh pr comment`, edit files, open PRs, etc.
//! 2. A response containing `NO ACTION` on its own line → dispatched as
//!    a `NoAction` outcome (no engineer subprocess spawned, no work done
//!    this cycle).
//!
//! Both shapes optionally accept a `PROGRESS: NN` marker (0..=100) that
//! updates the goal's recorded completion percentage.
//!
//! See `prompt_assets/simard/goal_session_objective.md` for the operator-
//! facing version of this contract.

use crate::ooda_loop::ActionOutcome;

/// Maximum length of user-derived text (task, reason) included in outcome
/// detail strings before truncation.
pub(super) const OUTCOME_TEXT_MAX: usize = 256;

/// A decision returned by the goal-advance LLM session.
///
/// The dispatcher in `advance.rs` consumes this to either spawn a
/// subordinate engineer or record a no-op outcome.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GoalAction {
    /// Spawn a subordinate engineer to do the concrete `task`.
    SpawnEngineer {
        task: String,
        /// Reserved for future use by callers that want to seed the
        /// engineer with a file list. Currently always empty in the
        /// prose path; kept in the type signature so the downstream
        /// dispatcher in `advance_goal/mod.rs` can keep its existing
        /// destructuring pattern unchanged.
        files: Vec<String>,
        /// Optional GitHub issue number this work advances. Reserved
        /// for future structured input; currently always `None` in the
        /// prose path.
        issue: Option<u64>,
    },
    /// No engineer subprocess this cycle. The orchestrator emitted the
    /// `NO ACTION` marker. The full prose response is preserved as the
    /// `reason` so operators can audit why a cycle did nothing.
    NoAction { reason: String },
}

/// The decision the orchestrator LLM made for this cycle, paired with
/// any progress percentage extracted from a `PROGRESS: NN` marker.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OrchestratorDecision {
    pub action: GoalAction,
    /// Goal completion percentage (0..=100) extracted from a
    /// `PROGRESS: NN` marker anywhere in the response. `None` when no
    /// such marker is present.
    pub progress_pct: Option<u8>,
}

/// The outcome of a single LLM-driven goal-advance turn.
///
/// Carries both the user-visible [`ActionOutcome`] and the parsed
/// [`GoalAction`] (when the LLM emitted a non-empty response), so the
/// upstream dispatcher in `advance_goal/mod.rs` can take side-effecting
/// follow-up steps such as actually spawning the engineer subprocess.
pub(crate) struct GoalSessionResult {
    pub(super) outcome: ActionOutcome,
    pub(super) action: Option<GoalAction>,
}

/// Parse the orchestrator LLM's prose response into a structured decision.
///
/// Returns `None` only when the response trims to the empty string.
/// Every non-empty response yields a decision:
///
/// * If any line of the response trims to exactly `NO ACTION` (case-
///   insensitive, also matches `NO_ACTION`), the whole response becomes
///   the `reason` of a [`GoalAction::NoAction`] decision.
/// * Otherwise, the trimmed response becomes the `task` of a
///   [`GoalAction::SpawnEngineer`] decision.
///
/// In both branches, [`extract_progress_marker`] scans the response for
/// a `PROGRESS: NN` marker and threads it into `progress_pct`.
pub(super) fn parse_orchestrator_response(response: &str) -> Option<OrchestratorDecision> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return None;
    }

    let progress_pct = extract_progress_marker(trimmed);

    let action = if has_no_action_marker(trimmed) {
        GoalAction::NoAction {
            reason: trimmed.to_string(),
        }
    } else {
        GoalAction::SpawnEngineer {
            task: trimmed.to_string(),
            files: Vec::new(),
            issue: None,
        }
    };

    Some(OrchestratorDecision {
        action,
        progress_pct,
    })
}

/// Detect the `NO ACTION` marker.
///
/// True when any single line of the input, after trimming, equals
/// `NO ACTION` or `NO_ACTION` case-insensitively. Requires the marker
/// to appear on its own line so that prose containing the literal
/// phrase ("we should take no action against ...") does not accidentally
/// trigger a no-op.
pub(super) fn has_no_action_marker(s: &str) -> bool {
    s.lines().any(|line| {
        let upper = line.trim().to_uppercase();
        upper == "NO ACTION" || upper == "NO_ACTION"
    })
}

/// Scan for a `PROGRESS: NN` marker and return the parsed percentage.
///
/// Returns the first occurrence's value, clamped to 0..=100. Matches
/// `PROGRESS:` case-insensitively, allows optional whitespace between
/// the colon and the digits, and stops at the first non-digit
/// character. Returns `None` when no such marker is present or the
/// digits parse to a value that does not fit in a `u8` after clamping
/// (impossible — a 1..3 digit string is always representable).
pub(super) fn extract_progress_marker(s: &str) -> Option<u8> {
    let lower = s.to_lowercase();
    let needle = "progress:";
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(needle) {
        let abs = search_from + rel;
        // Require the marker to be at start-of-string or preceded by
        // whitespace / punctuation, so we do not match the middle of a
        // word like `inprogress:`.
        let is_word_boundary = abs == 0
            || lower
                .as_bytes()
                .get(abs - 1)
                .is_some_and(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        if is_word_boundary {
            let after = &s[abs + needle.len()..];
            let after = after.trim_start();
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty()
                && let Ok(n) = digits.parse::<u32>()
            {
                return Some(n.min(100) as u8);
            }
        }
        search_from = abs + needle.len();
    }
    None
}

/// Truncate a user-derived string for safe inclusion in outcome details / logs.
pub(super) fn truncate_for_outcome(s: &str) -> String {
    if s.len() <= OUTCOME_TEXT_MAX {
        s.to_string()
    } else {
        // Truncate at a UTF-8 char boundary <= OUTCOME_TEXT_MAX.
        let mut end = OUTCOME_TEXT_MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

mod advance;

pub(crate) use advance::advance_goal_with_session;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_response_returns_none() {
        assert_eq!(parse_orchestrator_response(""), None);
        assert_eq!(parse_orchestrator_response("   "), None);
        assert_eq!(parse_orchestrator_response("\n\t  \r\n"), None);
    }

    #[test]
    fn pure_prose_becomes_spawn_engineer() {
        let response = "Run cargo test --lib prioritization and report which tests fail.";
        let decision = parse_orchestrator_response(response).expect("non-empty yields decision");
        assert_eq!(decision.progress_pct, None);
        match decision.action {
            GoalAction::SpawnEngineer { task, files, issue } => {
                assert_eq!(task, response);
                assert!(files.is_empty());
                assert!(issue.is_none());
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn no_action_marker_on_its_own_line_routes_to_noaction() {
        let response =
            "NO ACTION\nAnother subordinate (engineer-foo-1234) is already working this goal.";
        let decision = parse_orchestrator_response(response).expect("non-empty yields decision");
        match decision.action {
            GoalAction::NoAction { reason } => {
                assert!(reason.contains("NO ACTION"));
                assert!(reason.contains("subordinate"));
            }
            other => panic!("expected NoAction, got {other:?}"),
        }
    }

    #[test]
    fn no_action_marker_inside_a_sentence_does_not_trigger() {
        // Prose that mentions "no action" in the middle of a sentence
        // must NOT be treated as a NoAction signal — the marker must be
        // on its own line.
        let response = "We should take no action against this issue until QA confirms.";
        let decision = parse_orchestrator_response(response).expect("non-empty yields decision");
        match decision.action {
            GoalAction::SpawnEngineer { task, .. } => {
                assert_eq!(task, response);
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn no_action_marker_case_insensitive_and_underscore_form() {
        for marker in [
            "NO ACTION",
            "no action",
            "No Action",
            "NO_ACTION",
            "no_action",
        ] {
            let response = format!("{marker}\nblocked on external review");
            let decision = parse_orchestrator_response(&response).expect("yields decision");
            assert!(
                matches!(decision.action, GoalAction::NoAction { .. }),
                "marker '{marker}' should route to NoAction"
            );
        }
    }

    #[test]
    fn progress_marker_extracted_from_prose() {
        let response = "Run cargo build. PROGRESS: 60 — about two thirds done.";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, Some(60));
        assert!(matches!(decision.action, GoalAction::SpawnEngineer { .. }));
    }

    #[test]
    fn progress_marker_extracted_from_no_action() {
        let response = "NO ACTION\nWaiting on PR review. PROGRESS: 80";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, Some(80));
        assert!(matches!(decision.action, GoalAction::NoAction { .. }));
    }

    #[test]
    fn progress_marker_clamped_to_100() {
        let response = "Done. PROGRESS: 250";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, Some(100));
    }

    #[test]
    fn progress_marker_case_insensitive() {
        let response = "Working. progress:45 still going";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, Some(45));
    }

    #[test]
    fn progress_word_inside_token_does_not_match() {
        // `inprogress:` must NOT be treated as the marker — it lacks a
        // word boundary before "progress:".
        let response = "Build inprogress:waiting for tests";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, None);
    }

    #[test]
    fn no_progress_marker_means_none() {
        let response = "Just spawn the engineer to fix #1234.";
        let decision = parse_orchestrator_response(response).expect("yields decision");
        assert_eq!(decision.progress_pct, None);
    }

    #[test]
    fn truncate_handles_utf8_char_boundary() {
        // 256 bytes of ASCII + a multi-byte char — must not split the char.
        let s = format!("{}é", "x".repeat(OUTCOME_TEXT_MAX - 1));
        let truncated = truncate_for_outcome(&s);
        // The 'é' is 2 bytes; we should truncate at byte 254 to keep it whole
        // (or earlier), then append the ellipsis.
        assert!(truncated.ends_with('…'));
        // Must be valid UTF-8 (would have panicked on slice boundary otherwise).
        assert!(truncated.is_ascii() || truncated.chars().count() > 0);
    }
}
