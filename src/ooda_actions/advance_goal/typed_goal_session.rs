use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::identity_composition::max_subordinate_depth;
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};
use crate::typed_ooda::{
    Action, ActionKind, AdmissionSnapshot, AuthenticatedToolContext, BaseType, CapabilityGrant,
    CapabilityHandler, CapabilityPolicy, EffectExecutionError, EffectExecutor, EffectJob,
    EffectResult, EvidenceRef, GoalSessionExecutor, GoalSessionInvocation, OpaqueBytes,
    RustyClawdGoalSessionActor, TerminalKind,
};

use super::repo_resolver;
use super::spawn::{find_live_engineer_for_goal, lock_state};
use crate::ooda_actions::make_outcome;

pub(crate) fn run(
    action: &PlannedAction,
    state: &Mutex<&mut OodaState>,
    goal: &crate::goal_curation::ActiveGoal,
    repo_root: &Path,
) -> ActionOutcome {
    let session_id = format!("ooda-process-{}", std::process::id());
    let cycle_number = lock_state(state).cycle_count;
    let cycle_id = format!("cycle-{cycle_number}-{}", goal.id);
    let invocation = GoalSessionInvocation {
        session_id: session_id.clone(),
        cycle_id,
        goal_id: goal.id.clone(),
        task: OpaqueBytes::from(goal.description.as_bytes().to_vec()),
        reason: OpaqueBytes::from(action.description.as_bytes().to_vec()),
        observe_output: OpaqueBytes::from(Vec::new()),
        orient_output: OpaqueBytes::from(Vec::new()),
        decide_output: OpaqueBytes::from(Vec::new()),
    };

    let ledger_path = typed_ooda_state_root().join("typed-ooda/outcomes.sqlite3");
    if let Some(parent) = ledger_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return make_outcome(
            action,
            false,
            format!("typed goal-session ledger directory failed: {error}"),
        );
    }
    let handler = match CapabilityHandler::open(
        &ledger_path,
        CapabilityPolicy::goal_session_default("goal-session-policy-v1"),
    ) {
        Ok(handler) => handler,
        Err(error) => {
            return make_outcome(
                action,
                false,
                format!("typed goal-session ledger failed: {error}"),
            );
        }
    };

    let permits_spawn = lock_state(state).identity_cognition.permits_spawn();
    let mut grants = vec![
        CapabilityGrant::RecordNoAction,
        CapabilityGrant::RecordBlocked,
        CapabilityGrant::RecordCompleted,
    ];
    if permits_spawn {
        grants.extend([
            CapabilityGrant::RecordAction(ActionKind::SpawnEngineer),
            CapabilityGrant::RecordAction(ActionKind::FileIssue),
            CapabilityGrant::RecordAction(ActionKind::RequestMerge),
            CapabilityGrant::RecordAction(ActionKind::RequestDeploy),
        ]);
    }
    let actor_context = AuthenticatedToolContext::new("goal-session-actor", &session_id, grants);
    let admission = admission_snapshot(state, repo_root);
    let effects = LiveGoalSessionEffects {
        state,
        goal_id: goal.id.clone(),
    };
    let executor =
        GoalSessionExecutor::new(handler, actor_context, admission, Box::new(UnusedEffects));
    let actor = RustyClawdGoalSessionActor::default();
    let execution = match executor.execute_with_effects(&invocation, &effects, |received, tools| {
        actor.run(received, tools)
    }) {
        Ok(execution) => execution,
        Err(error) => {
            return make_outcome(
                action,
                false,
                format!(
                    "typed goal-session cycle failed ({:?}): {error}",
                    error.code()
                ),
            );
        }
    };

    match execution.outcome.kind {
        TerminalKind::Action => make_outcome(
            action,
            true,
            format!(
                "typed action committed and effect completed: outcome={}",
                execution.outcome.outcome_id
            ),
        ),
        TerminalKind::NoAction => make_outcome(
            action,
            true,
            format!(
                "typed no-action committed: outcome={}",
                execution.outcome.outcome_id
            ),
        ),
        TerminalKind::Blocked => {
            if let Some(goal) = lock_state(state)
                .active_goals
                .active
                .iter_mut()
                .find(|candidate| candidate.id == execution.outcome.goal_id)
            {
                goal.status = crate::goal_curation::GoalProgress::Blocked(format!(
                    "typed blocker recorded in outcome {}",
                    execution.outcome.outcome_id
                ));
            }
            make_outcome(
                action,
                true,
                format!(
                    "typed blocked terminal committed: outcome={}",
                    execution.outcome.outcome_id
                ),
            )
        }
        TerminalKind::Completed => {
            if let Some(goal) = lock_state(state)
                .active_goals
                .active
                .iter_mut()
                .find(|candidate| candidate.id == execution.outcome.goal_id)
            {
                goal.status = crate::goal_curation::GoalProgress::Completed;
            }
            make_outcome(
                action,
                true,
                format!(
                    "typed completed terminal committed: outcome={}",
                    execution.outcome.outcome_id
                ),
            )
        }
    }
}

struct UnusedEffects;

impl EffectExecutor for UnusedEffects {
    fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
        Err(EffectExecutionError::permanent(
            "external live effect executor was not supplied",
        ))
    }
}

fn admission_snapshot(state: &Mutex<&mut OodaState>, repo_root: &Path) -> AdmissionSnapshot {
    let guard = lock_state(state);
    let concurrent_engineers = guard
        .active_goals
        .active
        .iter()
        .filter(|goal| goal.assigned_to.is_some())
        .count();
    let active_claims = guard
        .active_goals
        .active
        .iter()
        .filter(|goal| goal.assigned_to.is_some())
        .map(|goal| {
            format!(
                "{}:{}",
                goal.repo.as_deref().unwrap_or("rysweet/Simard"),
                goal.id
            )
        })
        .collect();
    drop(guard);
    AdmissionSnapshot {
        concurrent_engineers,
        disk_used_percent: disk_used_percent(repo_root).unwrap_or(100),
        active_claims,
        policy_revision: "live-admission-v1".to_string(),
    }
}

fn disk_used_percent(path: &Path) -> Option<u8> {
    let stats = nix::sys::statvfs::statvfs(path).ok()?;
    let blocks = stats.blocks();
    if blocks == 0 {
        return None;
    }
    let used = blocks.saturating_sub(stats.blocks_available());
    Some(((used.saturating_mul(100) / blocks).min(100)) as u8)
}

struct LiveGoalSessionEffects<'state, 'value> {
    state: &'value Mutex<&'state mut OodaState>,
    goal_id: String,
}

impl EffectExecutor for LiveGoalSessionEffects<'_, '_> {
    fn execute(&self, job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
        match &job.action {
            Action::SpawnEngineer(spawn) => self.spawn_engineer(spawn),
            Action::RequestMerge(_) | Action::RequestDeploy(_) => Ok(EffectResult::Succeeded {
                evidence: Vec::new(),
            }),
            Action::FileIssue(issue) => self.file_issue(issue),
        }
    }
}

impl LiveGoalSessionEffects<'_, '_> {
    fn file_issue(
        &self,
        issue: &crate::typed_ooda::FileIssueAction,
    ) -> Result<EffectResult, EffectExecutionError> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let title = std::str::from_utf8(issue.title.as_bytes()).map_err(|error| {
            EffectExecutionError::permanent(format!("issue title is not valid UTF-8: {error}"))
        })?;
        let body = std::str::from_utf8(issue.body.as_bytes()).map_err(|error| {
            EffectExecutionError::permanent(format!("issue body is not valid UTF-8: {error}"))
        })?;
        let request = serde_json::to_vec(&serde_json::json!({
            "title": title,
            "body": body,
            "labels": issue.labels,
        }))
        .map_err(|error| {
            EffectExecutionError::permanent(format!("issue request encoding failed: {error}"))
        })?;
        let endpoint = format!(
            "repos/{}/{}/issues",
            issue.repository.owner, issue.repository.name
        );
        let mut child = Command::new("gh")
            .args(["api", "--method", "POST", &endpoint, "--input", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                EffectExecutionError::permanent(format!(
                    "gh issue request failed to start: {error}"
                ))
            })?;
        child
            .stdin
            .take()
            .ok_or_else(|| EffectExecutionError::permanent("gh issue stdin was unavailable"))?
            .write_all(&request)
            .map_err(|error| {
                EffectExecutionError::permanent(format!("gh issue request write failed: {error}"))
            })?;
        let output = child.wait_with_output().map_err(|error| {
            EffectExecutionError::permanent(format!("gh issue request wait failed: {error}"))
        })?;
        if !output.status.success() {
            return Err(EffectExecutionError::permanent(format!(
                "gh issue request exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        #[derive(serde::Deserialize)]
        struct CreatedIssue {
            number: u64,
        }
        let created: CreatedIssue = serde_json::from_slice(&output.stdout).map_err(|error| {
            EffectExecutionError::permanent(format!(
                "GitHub issue response was invalid application data: {error}"
            ))
        })?;
        Ok(EffectResult::Succeeded {
            evidence: vec![EvidenceRef::Issue {
                repository: issue.repository.clone(),
                number: created.number,
            }],
        })
    }

    fn spawn_engineer(
        &self,
        spawn: &crate::typed_ooda::SpawnEngineerAction,
    ) -> Result<EffectResult, EffectExecutionError> {
        if spawn.base_type != BaseType::Copilot {
            return Err(EffectExecutionError::permanent(
                "live typed spawn currently supports the Copilot engineer base type",
            ));
        }
        let task = std::str::from_utf8(spawn.task.as_bytes()).map_err(|error| {
            EffectExecutionError::permanent(format!(
                "engineer task is not valid UTF-8 for the downstream agent: {error}"
            ))
        })?;
        let (goal_repo, already_assigned) = {
            let guard = lock_state(self.state);
            let goal = guard
                .active_goals
                .active
                .iter()
                .find(|goal| goal.id == self.goal_id)
                .ok_or_else(|| EffectExecutionError::permanent("goal disappeared before spawn"))?;
            (goal.repo.clone(), goal.assigned_to.is_some())
        };
        if already_assigned {
            return Err(EffectExecutionError::permanent(
                "goal already has an assigned engineer",
            ));
        }
        let expected_repo = goal_repo.as_deref().unwrap_or("rysweet/Simard");
        let expected_repo = if expected_repo == "Simard" {
            "rysweet/Simard"
        } else {
            expected_repo
        };
        let requested_repo = format!("{}/{}", spawn.repository.owner, spawn.repository.name);
        if requested_repo != expected_repo {
            return Err(EffectExecutionError::permanent(format!(
                "typed spawn repository {requested_repo:?} does not match goal repository {expected_repo:?}"
            )));
        }

        let current_depth = std::env::var("SIMARD_SUBORDINATE_DEPTH")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        if current_depth >= max_subordinate_depth() {
            return Err(EffectExecutionError::permanent(
                "subordinate depth limit reached",
            ));
        }
        let state_root = typed_ooda_state_root();
        if find_live_engineer_for_goal(&state_root, &self.goal_id).is_some() {
            return Err(EffectExecutionError::permanent(
                "a live engineer already holds this goal",
            ));
        }
        let parent_repo =
            repo_resolver::resolve_goal_repo(goal_repo.as_deref()).map_err(|error| {
                EffectExecutionError::permanent(format!(
                    "target repository resolution failed: {error}"
                ))
            })?;
        let worktree = crate::engineer_worktree::EngineerWorktree::allocate(
            &parent_repo,
            &state_root,
            &self.goal_id,
        )
        .map_err(|error| {
            EffectExecutionError::permanent(format!("worktree allocation failed: {error}"))
        })?;
        let agent_name = format!("engineer-{}-{}", self.goal_id, job_safe_suffix());
        let config = SubordinateConfig {
            agent_name: agent_name.clone(),
            goal: format!(
                "{task}\n\n[target repo: {requested_repo} — work in this worktree at {}]",
                worktree.path().display()
            ),
            role: AgentRole::Engineer,
            worktree_path: worktree.path().to_path_buf(),
            current_depth,
        };
        let handle = match spawn_subordinate(&config) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = worktree.cleanup();
                return Err(EffectExecutionError::permanent(format!(
                    "engineer process launch failed: {error}"
                )));
            }
        };
        {
            let mut guard = lock_state(self.state);
            let goal = guard
                .active_goals
                .active
                .iter_mut()
                .find(|goal| goal.id == self.goal_id)
                .ok_or_else(|| {
                    EffectExecutionError::permanent("goal disappeared after engineer spawn")
                })?;
            goal.assigned_to = Some(agent_name.clone());
            guard
                .engineer_worktrees
                .insert(self.goal_id.clone(), worktree);
        }
        Ok(EffectResult::Succeeded {
            evidence: vec![EvidenceRef::EngineerRun {
                session_id: if handle.session_name.is_empty() {
                    agent_name
                } else {
                    handle.session_name
                },
                claim_key: spawn.claim_key.clone(),
            }],
        })
    }
}

fn typed_ooda_state_root() -> PathBuf {
    std::env::var_os("SIMARD_STATE_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SIMARD_HOME").map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".simard")
        })
}

fn job_safe_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
