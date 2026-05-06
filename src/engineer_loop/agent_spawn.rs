use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::base_types::BaseTypeTurnInput;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::{LlmProvider, SessionBuilder};

use super::types::RepoInspection;

pub(crate) const AGENT_SESSION_TIMEOUT_SECS: u64 = 3600;

pub(crate) fn build_agent_prompt(objective: &str, inspection: &RepoInspection) -> String {
    let files = if inspection.changed_files.is_empty() {
        "none".to_string()
    } else {
        inspection.changed_files.join(", ")
    };
    let dirty = if inspection.worktree_dirty { "dirty" } else { "clean" };
    let goals: Vec<&str> = inspection.active_goals.iter().map(|g| g.title.as_str()).collect();
    let goals_list = if goals.is_empty() {
        "none".to_string()
    } else {
        goals.join("; ")
    };

    format!(
        "You are an autonomous software engineer working on a git repository.\n\
         Use your tools to implement the following objective completely and correctly.\n\
         When done, summarize what you changed.\n\n\
         Objective: {objective}\n\
         Branch: {branch}\n\
         Worktree: {dirty}\n\
         Changed files: {files}\n\
         Active goals: {goals_list}",
        objective = objective,
        branch = inspection.branch,
    )
}

/// Spawn an autonomous agent session to accomplish `objective`.
///
/// Opens an LLM agent session, sends a natural-language prompt, and waits
/// for the agent to complete, returning the execution summary.
pub fn spawn_agent_for_goal(
    objective: &str,
    inspection: &RepoInspection,
    _workspace_path: &Path,
) -> SimardResult<String> {
    let provider = LlmProvider::resolve()?;
    let mut session = SessionBuilder::new(OperatingMode::Engineer, provider)
        .node_id("engineer-agent")
        .address("engineer-agent://local")
        .adapter_tag("engineer-agent")
        .open()
        .map_err(|reason| SimardError::ActionExecutionFailed {
            action: "agent-spawn".to_string(),
            reason,
        })?;

    let prompt = build_agent_prompt(objective, inspection);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = session
            .run_turn(BaseTypeTurnInput::objective_only(prompt))
            .map(|o| o.execution_summary);
        let _ = tx.send(result);
    });
    let outcome_summary = rx
        .recv_timeout(Duration::from_secs(AGENT_SESSION_TIMEOUT_SECS))
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: "agent-spawn".to_string(),
            reason: format!("agent session timed out after {AGENT_SESSION_TIMEOUT_SECS}s"),
        })?
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: "agent-spawn".to_string(),
            reason: format!("agent session failed: {e}"),
        })?;
    Ok(outcome_summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_agent_prompt_includes_objective() {
        use crate::engineer_loop::types::RepoInspection;
        let inspection = RepoInspection {
            workspace_root: "/tmp".into(),
            repo_root: "/tmp".into(),
            branch: "main".into(),
            head: "abc".into(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let prompt = build_agent_prompt("fix the bug", &inspection);
        assert!(prompt.contains("fix the bug"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn build_agent_prompt_lists_changed_files() {
        use crate::engineer_loop::types::RepoInspection;
        let inspection = RepoInspection {
            workspace_root: "/tmp".into(),
            repo_root: "/tmp".into(),
            branch: "feat".into(),
            head: "def".into(),
            worktree_dirty: true,
            changed_files: vec!["src/lib.rs".to_string()],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let prompt = build_agent_prompt("add tests", &inspection);
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("dirty"));
    }

    #[test]
    fn build_agent_prompt_lists_active_goals() {
        use crate::engineer_loop::types::RepoInspection;
        use crate::goals::{GoalRecord, GoalStatus};
        use crate::session::{SessionId, SessionPhase};
        let inspection = RepoInspection {
            workspace_root: "/tmp".into(),
            repo_root: "/tmp".into(),
            branch: "main".into(),
            head: "abc".into(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![GoalRecord {
                slug: "self-improvement".to_string(),
                title: "Self-improvement".to_string(),
                rationale: "ongoing".to_string(),
                status: GoalStatus::Active,
                priority: 1,
                owner_identity: "simard".to_string(),
                source_session_id: SessionId::parse(
                    "00000000-0000-0000-0000-000000000001",
                )
                .unwrap(),
                updated_in: SessionPhase::Execution,
            }],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let prompt = build_agent_prompt("improve quality", &inspection);
        assert!(prompt.contains("Self-improvement"));
    }
}
