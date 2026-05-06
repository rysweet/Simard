---
title: spawn_agent_for_goal — API Reference
description: Rust API reference for the spawn_agent_for_goal function that delegates engineer work to a subordinate autonomous agent session.
last_updated: 2026-05-06
owner: simard
doc_type: reference
related:
  - ../architecture/engineer-agent-orchestration.md
  - ./simard-engineer-step.md
---

# `spawn_agent_for_goal` — API Reference

`spawn_agent_for_goal` is the primary entry point for delegating concrete
engineering work to a subordinate Copilot agent session. It replaces the
old `plan_objective` + `execute_plan` pair from the removed
`src/engineer_plan` module.

**Module**: `simard::engineer_plan` (re-exported as
`simard::engineer_loop::spawn_agent_for_goal`)

---

## Signature

```rust
pub fn spawn_agent_for_goal(
    objective: &str,
    inspection: &RepoInspection,
    workspace_path: &Path,
) -> SimardResult<String>
```

### Parameters

| Parameter        | Type             | Description                                                                                     |
|------------------|------------------|-------------------------------------------------------------------------------------------------|
| `objective`      | `&str`           | Natural-language description of the work to be done. Forwarded verbatim to the agent prompt.   |
| `inspection`     | `&RepoInspection`| Workspace snapshot from `inspect_workspace()`. Used to build the context section of the prompt.|
| `workspace_path` | `&Path`          | Absolute path to the git worktree root where the agent should operate.                         |

### Return value

Returns `Ok(execution_summary)` where `execution_summary` is the agent's
final output text — typically a short paragraph describing what was done,
what tests were run, and whether the work succeeded. This string is stored
in `ExecutedEngineerAction.selected.plan_summary` and appears verbatim in
the cycle report.

Returns `Err(SimardError::ActionExecutionFailed { reason })` on:
- Agent session failure (non-zero exit from the Copilot SDK)
- Session timeout (wall time exceeds 3600 s)
- SDK initialisation error

### Panics

Never panics. All errors are returned as `SimardResult`.

---

## Prompt construction

`spawn_agent_for_goal` builds the agent prompt by combining:

1. **System context** — identity and constraints drawn from
   `prompt_assets/simard/engineer_planning.md`
2. **Workspace snapshot** — branch name, dirty/clean status, list of changed
   files, and active goal titles from `inspection`
3. **Objective** — the raw `objective` string, appended last

```
<system context from engineer_planning.md>

Objective: <objective>
Branch: <inspection.branch>
Worktree: dirty | clean
Changed files: <comma-separated list, or "none">
Active goals: <semicolon-separated titles, or "none">
```

The agent receives this prompt as its first user turn and then operates
autonomously, calling whatever tools it needs to complete the work.

---

## Timeout behaviour

The call blocks the calling thread for up to **3600 seconds**. Internally it
uses `std::thread::spawn` + `std::sync::mpsc::channel::recv_timeout` so the
calling thread is not permanently blocked if the agent hangs:

```rust
let (tx, rx) = std::sync::mpsc::channel();
std::thread::spawn(move || {
    let result = session.run_turn(turn_input); // blocking Copilot SDK call
    let _ = tx.send(result);
});
match rx.recv_timeout(Duration::from_secs(3600)) {
    Ok(result) => result.map(|r| r.execution_summary),
    Err(_) => Err(SimardError::ActionExecutionFailed {
        reason: "agent session timed out after 3600s".to_string(),
    }),
}
```

The 3600-second limit matches `CARGO_COMMAND_TIMEOUT_SECS` defined in
`src/engineer_loop/mod.rs`.

---

## Example usage

### From the engineer loop

```rust
use simard::engineer_loop::{inspect_workspace, spawn_agent_for_goal};
use std::path::Path;

let workspace = Path::new("/home/azureuser/src/Simard/worktrees/main");
let state_root = Path::new("/tmp/simard-state");

let inspection = inspect_workspace(workspace, state_root)?;
let summary = spawn_agent_for_goal(
    "Fix the off-by-one error in src/ooda_loop/orient.rs line 42",
    &inspection,
    workspace,
)?;
println!("Agent completed: {summary}");
```

### From self_improve_executor

```rust
use simard::engineer_loop::spawn_agent_for_goal;

// Previously: plan_objective(&proposal, &inspection)?  +  execute_plan(…)
// Now:
let summary = spawn_agent_for_goal(&proposal.description, &inspection, workspace_path)?;
patch.outcome_summary = summary;
```

---

## Replaced APIs

The following APIs from `src/engineer_plan/mod.rs` are **removed** and should
not be used:

| Removed symbol          | Replacement                        |
|-------------------------|------------------------------------|
| `Plan`                  | No equivalent — not needed         |
| `PlanStep`              | No equivalent — not needed         |
| `PlanStepResult`        | No equivalent — not needed         |
| `PlanExecutionResult`   | No equivalent — not needed         |
| `plan_objective()`      | `spawn_agent_for_goal()`           |
| `execute_plan()`        | `spawn_agent_for_goal()`           |
| `parse_plan_response()` | Removed — no structured plan output|
| `extract_json_array()`  | Removed — no JSON parsing needed   |

---

## Error variants

| `SimardError` variant          | When raised                                          |
|--------------------------------|------------------------------------------------------|
| `ActionExecutionFailed`        | Agent returned non-zero exit, or timeout elapsed     |
| `PlanningUnavailable`          | Copilot SDK unavailable / session failed to start    |
| `ActionExecutionFailed`        | `workspace_path` does not exist or is not a git repo |
