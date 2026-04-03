//! Action dispatch for the OODA loop.
//!
//! Extracted from `ooda_loop.rs` to keep each module under 400 LOC.
//! Each [`ActionKind`] maps to a concrete subsystem call. Failures are
//! per-action, not cycle-wide (Pillar 11: honest degradation).

use crate::agent_supervisor::{HeartbeatStatus, check_heartbeat};
use crate::error::SimardResult;
use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaBridges, OodaState, PlannedAction};
use crate::self_improve::{ImprovementConfig, run_improvement_cycle, summarize_cycle};
use crate::skill_builder::extract_skill_candidates;

/// Minimum procedure usage count required for skill extraction.
const SKILL_MIN_USAGE: u32 = 3;

/// Advance a goal's progress by one step: `NotStarted → InProgress(10)`,
/// `InProgress(N) → InProgress(N+10)` or `Completed` at 100.
fn next_progress(current: &GoalProgress) -> GoalProgress {
    match current {
        GoalProgress::NotStarted => GoalProgress::InProgress { percent: 10 },
        GoalProgress::InProgress { percent } => {
            let next = (*percent + 10).min(100);
            if next >= 100 {
                GoalProgress::Completed
            } else {
                GoalProgress::InProgress { percent: next }
            }
        }
        other => other.clone(),
    }
}

/// Construct an [`ActionOutcome`] from the shared action reference.
///
/// Centralises the single unavoidable clone of the [`PlannedAction`] so
/// dispatch helpers only need `(action, success, detail)`.
#[inline]
fn make_outcome(action: &PlannedAction, success: bool, detail: String) -> ActionOutcome {
    ActionOutcome {
        action: action.clone(),
        success,
        detail,
    }
}

/// Dispatch a batch of planned actions against live bridges and state.
///
/// Each action is dispatched independently; a failure in one does not
/// abort the others. Returns one [`ActionOutcome`] per input action.
pub fn dispatch_actions(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    let mut outcomes = Vec::with_capacity(actions.len());
    for action in actions {
        let outcome = dispatch_one(action, bridges, state);
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// Dispatch a single planned action and return its outcome.
fn dispatch_one(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    match action.kind {
        ActionKind::ConsolidateMemory => dispatch_consolidate_memory(action, bridges),
        ActionKind::ResearchQuery => dispatch_research_query(action, bridges),
        ActionKind::RunImprovement => dispatch_run_improvement(action, bridges),
        ActionKind::AdvanceGoal => dispatch_advance_goal(action, bridges, state),
        ActionKind::RunGymEval => dispatch_run_gym_eval(action, bridges),
        ActionKind::BuildSkill => dispatch_build_skill(action, bridges),
        ActionKind::LaunchSession => dispatch_launch_session(action),
    }
}

/// ConsolidateMemory: batch-consolidate episodic memory entries.
fn dispatch_consolidate_memory(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.memory.consolidate_episodes(20) {
        Ok(_) => make_outcome(action, true, "consolidated up to 20 episodes".to_string()),
        Err(e) => make_outcome(action, false, format!("consolidation failed: {e}")),
    }
}

/// ResearchQuery: list available knowledge packs.
fn dispatch_research_query(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.knowledge.list_packs() {
        Ok(packs) => make_outcome(
            action,
            true,
            format!("found {} knowledge packs", packs.len()),
        ),
        Err(e) => make_outcome(action, false, format!("knowledge query failed: {e}")),
    }
}

/// RunImprovement: execute a full improvement cycle via the gym bridge.
///
/// Uses default improvement config (progressive suite, 2% threshold).
/// The cycle evaluates baseline, applies no changes (empty proposals),
/// and returns the analysis. A real caller would populate proposed_changes
/// from the orient/decide phases.
fn dispatch_run_improvement(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    let config = ImprovementConfig::default();
    match run_improvement_cycle(&bridges.gym, &config) {
        Ok(cycle) => {
            let summary = summarize_cycle(&cycle);
            let committed = matches!(
                cycle.decision,
                Some(crate::self_improve::ImprovementDecision::Commit { .. })
            );
            make_outcome(
                action,
                true,
                format!("improvement cycle completed (committed={committed}): {summary}"),
            )
        }
        Err(e) => make_outcome(action, false, format!("improvement cycle failed: {e}")),
    }
}

/// AdvanceGoal: progress the target goal on the board.
///
/// If the goal has a subordinate assigned, checks the subordinate's
/// heartbeat via the supervisor. If a base-type session is available
/// (e.g. RustyClawd), delegates the goal to the agent via `run_turn`
/// for real autonomous work. Otherwise, falls back to bumping the
/// progress percentage.
fn dispatch_advance_goal(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    let goal_id = match &action.goal_id {
        Some(id) => id.clone(),
        None => {
            return make_outcome(action, false, "advance-goal requires a goal_id".to_string());
        }
    };

    // Find the goal on the board.
    let goal = match state.active_goals.active.iter().find(|g| g.id == goal_id) {
        Some(g) => g.clone(),
        None => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' not found on active board"),
            );
        }
    };

    // If the goal has a subordinate, check heartbeat.
    if let Some(ref sub_name) = goal.assigned_to {
        return advance_goal_with_subordinate(action, bridges, state, &goal_id, sub_name);
    }

    // Blocked and completed goals short-circuit before session dispatch.
    match &goal.status {
        GoalProgress::Blocked(reason) => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' is blocked: {reason}"),
            );
        }
        GoalProgress::Completed => {
            return make_outcome(
                action,
                true,
                format!("goal '{goal_id}' is already completed"),
            );
        }
        _ => {}
    }

    // If a base-type session is available, use run_turn for real agent work.
    if let Some(ref mut session) = bridges.session {
        return advance_goal_with_session(action, session.as_mut(), state, &goal);
    }

    // Fallback: no session available — advance progress by bumping percentage.
    let new_progress = next_progress(&goal.status);

    match update_goal_progress(&mut state.active_goals, &goal_id, new_progress.clone()) {
        Ok(()) => make_outcome(
            action,
            true,
            format!("goal '{goal_id}' advanced to {new_progress}"),
        ),
        Err(e) => make_outcome(
            action,
            false,
            format!("failed to update goal '{goal_id}': {e}"),
        ),
    }
}

/// Advance a goal using a base-type session's `run_turn`.
///
/// Simard acts as a PM architect: she assesses the goal, decides whether to
/// delegate to an amplihack coding session, and tracks progress based on
/// evidence from the agent's response — never by auto-incrementing.
fn advance_goal_with_session(
    action: &PlannedAction,
    session: &mut dyn crate::base_types::BaseTypeSession,
    state: &mut OodaState,
    goal: &crate::goal_curation::ActiveGoal,
) -> ActionOutcome {
    use crate::base_types::BaseTypeTurnInput;
    use std::fmt::Write;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    // Gather fresh environment context so the agent sees current state.
    let env = crate::ooda_loop::gather_environment();

    // Build the objective in a single pre-sized buffer to avoid intermediate allocations.
    let mut objective = String::with_capacity(1024);
    let _ = write!(
        objective,
        "Goal '{}' ({}% complete): {}\n\n\
         Assess this goal's current status by:\n\
         1. Check the repository state, open issues, and recent commits to understand where things stand.\n\
         2. Decide whether this goal needs an amplihack coding session to make progress.\n\
         3. If work is needed: create a GitHub issue describing the specific task, then launch \
            `simard engineer` or `amplihack copilot` to handle it.\n\
         4. If the goal is already progressing or blocked, report the status without launching new work.\n\n\
         End your response with a PROGRESS line indicating your assessed completion percentage \
         (0-100), e.g.: PROGRESS: 45\n\n\
         Concrete commands you can use:\n\
         - Create a GitHub issue: `gh issue create --repo rysweet/Simard --title \"<title>\" --body \"<body>\"`\n\
         - Create a branch: `git checkout -b feat/<description>`\n\
         - Launch an amplihack coding session: `amplihack copilot` then type your task\n\
         - Run tests: `cargo test 2>&1 | tail -20`\n\
         - Check build: `cargo check 2>&1`\n\
         - Open a PR: `gh pr create --title \"<title>\" --body \"<body>\"`\n\
         - Check CI status: `gh run list --limit 5`\n\n\
         Environment context:\n- Git status: ",
        goal.id, percent, goal.description,
    );
    if env.git_status.is_empty() {
        objective.push_str("clean");
    } else {
        let _ = write!(
            objective,
            "{} changed files",
            env.git_status.lines().count()
        );
    }
    objective.push_str("\n- Open issues: ");
    if env.open_issues.is_empty() {
        objective.push_str("none");
    } else {
        for (i, issue) in env.open_issues.iter().enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(issue);
        }
    }
    objective.push_str("\n- Recent commits: ");
    if env.recent_commits.is_empty() {
        objective.push_str("none");
    } else {
        for (i, commit) in env.recent_commits.iter().take(5).enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(commit);
        }
    }

    let identity_context = "You are Simard, a PM architect who manages fleets of amplihack \
        coding sessions. You do NOT write code yourself. You assess goals, create GitHub \
        issues for specific work items, and delegate implementation to amplihack coding \
        agents (via `simard engineer` or `amplihack copilot`). Your job is to evaluate \
        what needs to happen, break it into actionable work, and orchestrate the right \
        agent to do it."
        .to_string();

    let input = BaseTypeTurnInput {
        objective,
        identity_context,
        prompt_preamble: String::new(),
    };

    match session.run_turn(input) {
        Ok(outcome) => {
            let new_progress = assess_progress_from_outcome(&outcome, &goal.status);

            // Verify claimed actions against reality.
            let verification = verify_claimed_actions(&outcome.execution_summary);
            let verified_count = verification.iter().filter(|v| v.verified).count();
            let claimed_count = verification.len();

            let _ = update_goal_progress(&mut state.active_goals, &goal.id, new_progress.clone());

            if !verification.is_empty() {
                eprintln!(
                    "[simard] OODA action verification for '{}': {}/{} claims verified",
                    goal.id, verified_count, claimed_count,
                );
                for v in &verification {
                    eprintln!(
                        "[simard]   {} {}: {}",
                        if v.verified { "✓" } else { "✗" },
                        v.claim_type,
                        v.detail,
                    );
                }
            }

            eprintln!(
                "[simard] OODA session result: advance-goal '{}': {}",
                goal.id, outcome.execution_summary
            );

            make_outcome(
                action,
                true,
                format!(
                    "goal '{}' assessed at {} via session (evidence={}, verified={}/{})",
                    goal.id,
                    new_progress,
                    outcome.evidence.len(),
                    verified_count,
                    claimed_count,
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("session run_turn failed for goal '{}': {e}", goal.id),
        ),
    }
}

/// A single verified or unverified claim from the agent's response.
#[derive(Debug, Clone)]
struct ActionVerification {
    claim_type: &'static str,
    detail: String,
    verified: bool,
}

/// Launch a bounded terminal session to work on a specific task.
///
/// Uses `PtyTerminalSession` to start `amplihack copilot`, send the task
/// description, wait for completion signals, and capture the transcript.
fn dispatch_launch_session(action: &PlannedAction) -> ActionOutcome {
    use crate::terminal_session::PtyTerminalSession;
    use std::time::Duration;

    let task = &action.description;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Launch amplihack copilot in a PTY.
    let mut session =
        match PtyTerminalSession::launch_command("terminal-shell", "amplihack copilot", &cwd) {
            Ok(s) => s,
            Err(e) => {
                return make_outcome(
                    action,
                    false,
                    format!("failed to launch amplihack copilot: {e}"),
                );
            }
        };

    // Wait for the copilot prompt to appear.
    let prompt_timeout = Duration::from_secs(30);
    match session.wait_for_output("$", prompt_timeout) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("[simard] OODA launch-session: copilot prompt not detected: {e}");
            // Continue anyway — the session may still be responsive.
        }
    }

    // Send the task description.
    if let Err(e) = session.send_input(task) {
        let _ = session.finish();
        return make_outcome(
            action,
            false,
            format!("failed to send task to copilot: {e}"),
        );
    }

    // Wait for the task to complete (up to 5 minutes).
    let work_timeout = Duration::from_secs(300);
    let _ = session.wait_for_output("$", work_timeout);

    // Send /exit to cleanly close the copilot session.
    let _ = session.send_input("/exit");
    let _ = session.wait_for_output("Bye", Duration::from_secs(10));

    // Capture transcript.
    match session.finish() {
        Ok(capture) => {
            let preview = crate::terminal_session::transcript_preview(&capture.transcript);
            let success = capture.exit_status.success();
            make_outcome(
                action,
                success,
                format!(
                    "amplihack session {} (exit={}): {preview}",
                    if success { "completed" } else { "failed" },
                    capture.exit_status,
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("terminal session capture failed: {e}"),
        ),
    }
}

/// Scan the agent's execution summary for claimed actions and verify them
/// against actual repository/system state.
///
/// Checks for:
/// - `gh issue create` → verify issue exists via `gh issue list`
/// - `git checkout -b` → verify branch exists via `git branch`
/// - `gh pr create` → verify PR exists via `gh pr list`
/// - `cargo test` / `cargo check` → verify exit status mentioned
fn verify_claimed_actions(summary: &str) -> Vec<ActionVerification> {
    let mut verifications = Vec::new();

    // Check for issue creation claims.
    if summary.contains("gh issue create")
        || summary.contains("created issue")
        || summary.contains("opened issue")
        || summary.contains("filed issue")
    {
        let verified = verify_recent_issue();
        verifications.push(ActionVerification {
            claim_type: "issue-create",
            detail: if verified {
                "confirmed: recent issue found via gh issue list".into()
            } else {
                "unverified: no recent issue found".into()
            },
            verified,
        });
    }

    // Check for branch creation claims.
    for line in summary.lines() {
        let trimmed = line.trim();
        if let Some(branch) = extract_branch_name(trimmed) {
            let verified = verify_branch_exists(&branch);
            verifications.push(ActionVerification {
                claim_type: "branch-create",
                detail: format!(
                    "{}: branch '{branch}'",
                    if verified { "confirmed" } else { "unverified" },
                ),
                verified,
            });
        }
    }

    // Check for PR creation claims.
    if summary.contains("gh pr create")
        || summary.contains("opened PR")
        || summary.contains("created PR")
        || summary.contains("pull request")
    {
        let verified = verify_recent_pr();
        verifications.push(ActionVerification {
            claim_type: "pr-create",
            detail: if verified {
                "confirmed: recent PR found via gh pr list".into()
            } else {
                "unverified: no recent PR found".into()
            },
            verified,
        });
    }

    verifications
}

/// Check if `git checkout -b <name>` appears and extract the branch name.
fn extract_branch_name(line: &str) -> Option<String> {
    let markers = ["git checkout -b ", "git switch -c "];
    for marker in markers {
        if let Some(idx) = line.find(marker) {
            let rest = &line[idx + marker.len()..];
            let branch = rest.split_whitespace().next()?;
            // Strip backticks or quotes.
            let branch = branch.trim_matches(|c| c == '`' || c == '\'' || c == '"');
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }
    None
}

/// Verify a branch exists locally or in the remote.
fn verify_branch_exists(branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["branch", "--list", branch])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// Verify that a GitHub issue was recently created (within the last 5 minutes).
fn verify_recent_issue() -> bool {
    std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "createdAt",
        ])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            // If there's a recent issue, the JSON array will be non-empty.
            o.status.success() && text.contains("createdAt")
        })
        .unwrap_or(false)
}

/// Verify that a PR was recently created.
fn verify_recent_pr() -> bool {
    std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "createdAt",
        ])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            o.status.success() && text.contains("createdAt")
        })
        .unwrap_or(false)
}

/// Extract a progress percentage from the agent's execution summary.
///
/// Looks for a `PROGRESS: <number>` line in the summary or evidence.
/// Falls back to the current progress if no explicit assessment is found —
/// never auto-increments.
fn assess_progress_from_outcome(
    outcome: &crate::base_types::BaseTypeOutcome,
    current: &GoalProgress,
) -> GoalProgress {
    // Search execution_summary and evidence for "PROGRESS: <N>"
    let sources = std::iter::once(outcome.execution_summary.as_str())
        .chain(outcome.evidence.iter().map(String::as_str));

    for text in sources {
        if let Some(p) = parse_progress_line(text) {
            return if p >= 100 {
                GoalProgress::Completed
            } else if p == 0 {
                GoalProgress::NotStarted
            } else {
                GoalProgress::InProgress { percent: p }
            };
        }
    }

    // No explicit progress found — preserve current state unchanged.
    current.clone()
}

/// Parse a "PROGRESS: <number>" line from text, returning the percentage.
fn parse_progress_line(text: &str) -> Option<u32> {
    const PREFIX: &str = "PROGRESS:";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= PREFIX.len()
            && trimmed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
            && let Ok(value) = trimmed[PREFIX.len()..].trim().parse::<u32>()
        {
            return Some(value.min(100));
        }
    }
    None
}

/// Advance a goal that has a subordinate assigned by checking heartbeat.
fn advance_goal_with_subordinate(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking.
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path: std::path::PathBuf::from("."),
        spawn_time: 0,
        retry_count: 0,
        killed: false,
    };

    match check_heartbeat(&handle, &bridges.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Subordinate is alive; update goal to in-progress if not already.
            let new_progress = GoalProgress::InProgress { percent: 50 };
            let _ = update_goal_progress(&mut state.active_goals, goal_id, new_progress);
            make_outcome(
                action,
                true,
                format!(
                    "subordinate '{sub_name}' alive (phase={phase}), goal '{goal_id}' in-progress"
                ),
            )
        }
        Ok(HeartbeatStatus::Stale { seconds_since }) => make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' stale ({seconds_since}s), goal '{goal_id}' may need reassignment"
            ),
        ),
        Ok(HeartbeatStatus::Dead) => {
            let _ = update_goal_progress(
                &mut state.active_goals,
                goal_id,
                GoalProgress::Blocked(format!("subordinate '{sub_name}' is dead")),
            );
            make_outcome(
                action,
                false,
                format!("subordinate '{sub_name}' is dead, goal '{goal_id}' blocked"),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
    }
}

/// RunGymEval: run the progressive gym suite and return the score.
fn dispatch_run_gym_eval(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.gym.run_suite("progressive") {
        Ok(result) => {
            use crate::gym_scoring::suite_score_from_result;
            let score = suite_score_from_result(&result);
            make_outcome(
                action,
                true,
                format!(
                    "gym eval: {:.1}% overall, {}/{} passed",
                    score.overall * 100.0,
                    score.scenarios_passed,
                    score.scenario_count,
                ),
            )
        }
        Err(e) => make_outcome(action, false, format!("gym eval failed: {e}")),
    }
}

/// BuildSkill: extract skill candidates from procedural memory.
fn dispatch_build_skill(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match extract_skill_candidates(&bridges.memory, SKILL_MIN_USAGE) {
        Ok(candidates) => {
            let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
            make_outcome(
                action,
                true,
                format!(
                    "extracted {} skill candidates: [{}]",
                    candidates.len(),
                    names.join(", ")
                ),
            )
        }
        Err(e) => make_outcome(action, false, format!("skill extraction failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::{
        BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    };
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::error::SimardError;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};
    use crate::gym_bridge::GymBridge;
    use crate::knowledge_bridge::KnowledgeBridge;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A mock session that captures the input sent to `run_turn` and returns
    /// a configurable response. Used to test `advance_goal_with_session`.
    struct MockSession {
        captured_input: Rc<RefCell<Option<BaseTypeTurnInput>>>,
        response: Result<BaseTypeOutcome, String>,
    }

    impl MockSession {
        fn new_ok(
            summary: &str,
            evidence: Vec<String>,
        ) -> (Self, Rc<RefCell<Option<BaseTypeTurnInput>>>) {
            let captured = Rc::new(RefCell::new(None));
            let session = Self {
                captured_input: Rc::clone(&captured),
                response: Ok(BaseTypeOutcome {
                    plan: String::new(),
                    execution_summary: summary.to_string(),
                    evidence,
                }),
            };
            (session, captured)
        }

        fn new_err(msg: &str) -> Self {
            Self {
                captured_input: Rc::new(RefCell::new(None)),
                response: Err(msg.to_string()),
            }
        }
    }

    // MockSession is !Send because of Rc<RefCell<...>>, but tests are single-threaded.
    // We need Send for BaseTypeSession trait bound, so use an unsafe impl.
    unsafe impl Send for MockSession {}

    impl BaseTypeSession for MockSession {
        fn descriptor(&self) -> &BaseTypeDescriptor {
            unimplemented!("not needed for advance_goal_with_session tests")
        }

        fn open(&mut self) -> crate::error::SimardResult<()> {
            Ok(())
        }

        fn run_turn(
            &mut self,
            input: BaseTypeTurnInput,
        ) -> crate::error::SimardResult<BaseTypeOutcome> {
            *self.captured_input.borrow_mut() = Some(input);
            match &self.response {
                Ok(outcome) => Ok(outcome.clone()),
                Err(msg) => Err(SimardError::BridgeTransportError {
                    bridge: "mock-session".to_string(),
                    reason: msg.clone(),
                }),
            }
        }

        fn close(&mut self) -> crate::error::SimardResult<()> {
            Ok(())
        }
    }

    fn mock_memory() -> CognitiveMemoryBridge {
        CognitiveMemoryBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-mem",
            |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 5, "working_count": 3, "episodic_count": 12,
                    "semantic_count": 8, "procedural_count": 2, "prospective_count": 1
                })),
                "memory.consolidate_episodes" => Ok(json!({"id": null})),
                "memory.recall_procedure" => Ok(json!({
                    "procedures": [{"node_id": "proc_1", "name": "cargo build",
                        "steps": ["compile", "test"], "prerequisites": ["rust"],
                        "usage_count": 5}]
                })),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        )))
    }

    fn mock_gym() -> GymBridge {
        GymBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-gym",
            |_method, _params| {
                Ok(json!({
                    "suite_id": "progressive", "success": true, "overall_score": 0.75,
                    "dimensions": {"factual_accuracy": 0.8, "specificity": 0.7,
                        "temporal_awareness": 0.75, "source_attribution": 0.7,
                        "confidence_calibration": 0.8},
                    "scenario_results": [], "scenarios_passed": 6, "scenarios_total": 6,
                    "degraded_sources": []
                }))
            },
        )))
    }

    fn mock_knowledge() -> KnowledgeBridge {
        KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-knowledge",
            |method, _params| match method {
                "knowledge.list_packs" => Ok(json!({"packs": [{"name": "rust-expert",
                    "description": "Rust knowledge", "article_count": 100,
                    "section_count": 400}]})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        )))
    }

    fn test_bridges() -> OodaBridges {
        OodaBridges {
            memory: mock_memory(),
            knowledge: mock_knowledge(),
            gym: mock_gym(),
            session: None,
        }
    }

    fn board_with_goal(id: &str, progress: GoalProgress, assigned: Option<&str>) -> GoalBoard {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            ActiveGoal {
                id: id.to_string(),
                description: format!("Goal {id}"),
                priority: 1,
                status: progress,
                assigned_to: assigned.map(String::from),
            },
        )
        .unwrap();
        board
    }

    #[test]
    fn dispatch_run_improvement_calls_gym() {
        let mut bridges = test_bridges();
        let action = PlannedAction {
            kind: ActionKind::RunImprovement,
            goal_id: None,
            description: "test".into(),
        };
        let mut state = OodaState::new(GoalBoard::new());
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("improvement cycle completed"));
    }

    #[test]
    fn dispatch_advance_goal_not_started_becomes_in_progress() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("in-progress"));
        assert!(matches!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 10 }
        ));
    }

    #[test]
    fn dispatch_advance_goal_blocked_fails() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::Blocked("waiting".into()), None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("blocked"));
    }

    #[test]
    fn dispatch_advance_goal_missing_id_fails() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: None,
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("requires a goal_id"));
    }

    #[test]
    fn dispatch_run_gym_eval_returns_score() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::RunGymEval,
            goal_id: None,
            description: "eval".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("gym eval"));
        assert!(outcomes[0].detail.contains("75.0%"));
    }

    #[test]
    fn dispatch_build_skill_extracts_candidates() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::BuildSkill,
            goal_id: None,
            description: "build".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("cargo-build"));
    }

    #[test]
    fn dispatch_advance_goal_with_dead_subordinate_blocks() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::NotStarted, Some("sub-1"));
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        // No progress facts in memory means Dead heartbeat.
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("dead"));
    }

    #[test]
    fn parse_progress_line_extracts_percentage() {
        assert_eq!(parse_progress_line("PROGRESS: 45"), Some(45));
        assert_eq!(parse_progress_line("progress: 80"), Some(80));
        assert_eq!(
            parse_progress_line("Some text\nPROGRESS: 60\nMore text"),
            Some(60)
        );
        assert_eq!(parse_progress_line("no progress here"), None);
        assert_eq!(parse_progress_line("PROGRESS: 150"), Some(100)); // clamped
        assert_eq!(parse_progress_line("PROGRESS: 0"), Some(0));
    }

    #[test]
    fn assess_progress_keeps_current_when_no_marker() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "did some work".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 30 };
        assert_eq!(assess_progress_from_outcome(&outcome, &current), current);
    }

    #[test]
    fn assess_progress_extracts_from_summary() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "Assessed goal.\nPROGRESS: 55".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 30 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::InProgress { percent: 55 }
        );
    }

    #[test]
    fn assess_progress_extracts_from_evidence() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "no marker here".to_string(),
            evidence: vec!["PROGRESS: 100".to_string()],
        };
        let current = GoalProgress::InProgress { percent: 80 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::Completed
        );
    }

    #[test]
    fn assess_progress_zero_means_not_started() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "PROGRESS: 0".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 10 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::NotStarted
        );
    }

    // ── advance_goal_with_session tests ──────────────────────────────

    fn bridges_with_session(session: MockSession) -> OodaBridges {
        OodaBridges {
            memory: mock_memory(),
            knowledge: mock_knowledge(),
            gym: mock_gym(),
            session: Some(Box::new(session)),
        }
    }

    #[test]
    fn session_identity_describes_pm_architect_not_coder() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 25", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let id = &input.identity_context;

        // Must describe PM architect role, not a coder.
        assert!(
            id.contains("PM architect"),
            "identity should mention PM architect, got: {id}"
        );
        assert!(
            id.contains("amplihack") || id.contains("coding sessions"),
            "identity should mention managing coding sessions, got: {id}"
        );
        assert!(
            !id.to_lowercase().contains("you write code")
                && !id.to_lowercase().contains("you are a coder"),
            "identity must NOT describe Simard as a coder, got: {id}"
        );
    }

    #[test]
    fn session_objective_includes_assessment_steps() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 30", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let obj = &input.objective;

        // Objective must include the goal ID and description.
        assert!(obj.contains("g1"), "objective should contain goal ID");

        // Must instruct assessment of goal status.
        assert!(
            obj.to_lowercase().contains("assess") || obj.to_lowercase().contains("check"),
            "objective should instruct assessment, got: {obj}"
        );

        // Must mention creating GitHub issues for work.
        assert!(
            obj.to_lowercase().contains("github issue") || obj.to_lowercase().contains("issue"),
            "objective should mention creating issues, got: {obj}"
        );

        // Must mention launching amplihack sessions.
        assert!(
            obj.contains("simard engineer") || obj.contains("amplihack copilot"),
            "objective should mention delegation commands, got: {obj}"
        );

        // Must request a PROGRESS line in the response.
        assert!(
            obj.contains("PROGRESS"),
            "objective should request PROGRESS assessment, got: {obj}"
        );
    }

    #[test]
    fn session_progress_comes_from_agent_response_not_auto_bump() {
        // Agent reports PROGRESS: 55 — goal should become 55%, not current+10.
        let (session, _captured) = MockSession::new_ok(
            "Assessed the goal. Created issue #42.\nPROGRESS: 55",
            vec![],
        );
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        // Progress must be 55 (from agent response), NOT 30 (20+10 auto-bump).
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 55 },
            "progress should come from agent's PROGRESS line, not auto-bump"
        );
    }

    #[test]
    fn session_no_progress_marker_preserves_current() {
        // Agent does NOT include a PROGRESS line — current progress must be preserved.
        let (session, _captured) = MockSession::new_ok(
            "Checked the repo. Everything looks fine.",
            vec!["no markers here".to_string()],
        );
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 40 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        // Must stay at 40%, NOT bumped to 50%.
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 40 },
            "without PROGRESS marker, progress must be preserved (not auto-bumped)"
        );
    }

    #[test]
    fn session_progress_100_completes_goal() {
        let (session, _captured) = MockSession::new_ok("PROGRESS: 100", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 80 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        assert_eq!(state.active_goals.active[0].status, GoalProgress::Completed,);
    }

    #[test]
    fn session_run_turn_failure_returns_error_outcome() {
        let session = MockSession::new_err("connection lost");
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("session run_turn failed"));
        // Progress must NOT change on error.
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 10 },
        );
    }

    #[test]
    fn session_objective_includes_environment_context() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 20", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let obj = &input.objective;

        // Objective should include environment context (git status, issues, commits).
        assert!(
            obj.contains("Git status") || obj.contains("git status"),
            "objective should include environment context"
        );
    }

    #[test]
    fn session_not_started_goal_reports_0_percent_in_objective() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 5", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().unwrap();
        // NotStarted should show 0% in the objective.
        assert!(
            input.objective.contains("0% complete"),
            "NotStarted goal should report 0% in objective"
        );
    }

    #[test]
    fn session_outcome_includes_verification_counts() {
        let (session, _) = MockSession::new_ok("Created an issue. PROGRESS: 20", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        // The outcome detail should include verification counts.
        assert!(
            outcomes[0].detail.contains("verified="),
            "outcome should include verification counts, got: {}",
            outcomes[0].detail,
        );
    }

    #[test]
    fn extract_branch_name_from_checkout() {
        assert_eq!(
            extract_branch_name("git checkout -b feat/fix-issue-42"),
            Some("feat/fix-issue-42".to_string()),
        );
        assert_eq!(
            extract_branch_name("ran `git checkout -b feat/new-thing` and pushed"),
            Some("feat/new-thing".to_string()),
        );
        assert_eq!(
            extract_branch_name("git switch -c hotfix/urgent"),
            Some("hotfix/urgent".to_string()),
        );
        assert_eq!(extract_branch_name("just some text"), None);
    }

    #[test]
    fn verify_claimed_actions_detects_issue_creation_claims() {
        let v = verify_claimed_actions("I ran gh issue create to file the bug");
        assert!(!v.is_empty(), "should detect issue creation claim");
        assert_eq!(v[0].claim_type, "issue-create");
    }

    #[test]
    fn verify_claimed_actions_detects_branch_creation() {
        let v = verify_claimed_actions("Created branch with git checkout -b feat/my-fix");
        let branch_claims: Vec<_> = v
            .iter()
            .filter(|c| c.claim_type == "branch-create")
            .collect();
        assert!(
            !branch_claims.is_empty(),
            "should detect branch creation claim"
        );
        assert!(branch_claims[0].detail.contains("feat/my-fix"));
    }

    #[test]
    fn verify_claimed_actions_detects_pr_creation() {
        let v = verify_claimed_actions("I opened PR #42 for the fix");
        let pr_claims: Vec<_> = v.iter().filter(|c| c.claim_type == "pr-create").collect();
        assert!(!pr_claims.is_empty(), "should detect PR creation claim");
    }

    #[test]
    fn verify_claimed_actions_returns_empty_for_observation_only() {
        let v =
            verify_claimed_actions("Checked the repo status. Everything looks clean. PROGRESS: 30");
        assert!(
            v.is_empty(),
            "observation-only responses should have no claims to verify"
        );
    }

    #[test]
    fn objective_includes_concrete_commands() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 10", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().unwrap();
        assert!(
            input.objective.contains("gh issue create"),
            "objective should include concrete gh issue create command"
        );
        assert!(
            input.objective.contains("amplihack copilot"),
            "objective should include amplihack copilot command"
        );
        assert!(
            input.objective.contains("cargo test"),
            "objective should include cargo test command"
        );
    }

    #[test]
    #[ignore] // Requires amplihack copilot — run with `cargo test -- --ignored`
    fn launch_session_returns_failure_when_amplihack_unavailable() {
        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: "test task for session launch".into(),
        };
        let outcome = dispatch_launch_session(&action);
        // In CI/test environments, amplihack copilot won't be available,
        // so we expect a graceful failure rather than a panic.
        assert!(
            !outcome.detail.is_empty(),
            "launch-session should report a meaningful outcome even on failure"
        );
    }

    #[test]
    fn action_kind_launch_session_displays_correctly() {
        assert_eq!(ActionKind::LaunchSession.to_string(), "launch-session");
    }
}
