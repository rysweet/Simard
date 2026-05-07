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

**Module**: `simard::engineer_loop` (defined in
`src/engineer_loop/agent_spawn.rs`, re-exported at the crate root as
`simard::spawn_agent_for_goal`)

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
in `ExecutedEngineerAction.stdout` (and mirrored in
`ExecutedEngineerAction.selected.kind.agent_session.outcome_summary`).
`ExecutedEngineerAction.selected.plan_summary` holds the original
objective string, not the agent output.

Returns `Err(SimardError::ActionExecutionFailed { reason })` on:
- Agent session failure (non-zero exit from the Copilot SDK)
- Session timeout (wall time exceeds 3600 s)
- SDK initialisation error

### Panics

Never panics. All errors are returned as `SimardResult`.

---

## Prompt construction

`spawn_agent_for_goal` builds the agent prompt via `build_agent_prompt` in
`src/engineer_loop/agent_spawn.rs`. The prompt is a hardcoded format string
that combines:

1. **Fixed preamble** — "You are an autonomous software engineer…" instruction
   directing the agent to implement the objective and summarise what it changed
2. **Workspace snapshot** — branch name, HEAD SHA, dirty/clean status, list of
   changed files, and active goal titles from `inspection`
3. **Objective** — the raw `objective` string

```
You are an autonomous software engineer working on a git repository.
Use your tools to implement the following objective completely and correctly.
When done, summarize what you changed.

Objective: <objective>
Branch: <inspection.branch>
HEAD: <inspection.head>
Worktree: dirty | clean
Changed files: <comma-separated list, or "none">
Active goals: <semicolon-separated titles, or "none">
```

The agent receives this prompt as its first user turn and then operates
autonomously, calling whatever tools it needs to complete the work.

---

## Timeout behaviour

The call blocks the calling thread for up to **3600 seconds**. Internally
`spawn_agent_for_goal` delegates to two helpers:

1. **`start_agent_session`** — opens the LLM session, spawns a background
   thread that calls `session.run_turn(...)`, and returns an
   `mpsc::Receiver<SimardResult<String>>`.
2. **`await_agent_session`** — calls `recv_timeout` on that receiver with the
   `AGENT_SESSION_TIMEOUT_SECS` deadline:

```rust
// await_agent_session (src/engineer_loop/agent_spawn.rs)
rx.recv_timeout(Duration::from_secs(AGENT_SESSION_TIMEOUT_SECS))
    .map_err(|_| SimardError::ActionExecutionFailed {
        action: "agent-spawn".to_string(),
        reason: format!("agent session timed out after {AGENT_SESSION_TIMEOUT_SECS}s"),
    })?
    .map_err(|e| SimardError::ActionExecutionFailed {
        action: "agent-spawn".to_string(),
        reason: format!("agent session failed: {e}"),
    })
```

The 3600-second limit is set by `AGENT_SESSION_TIMEOUT_SECS` defined in
`src/engineer_loop/agent_spawn.rs`.

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

| `SimardError` variant          | When raised                                                                              |
|--------------------------------|------------------------------------------------------------------------------------------|
| `ActionExecutionFailed`        | Agent returned non-zero exit, timeout elapsed, or the Copilot SDK session failed to open |
| `MissingRequiredConfig`        | `LlmProvider::resolve()` found no provider — neither `SIMARD_LLM_PROVIDER` env var nor `llm_provider` in `~/.simard/config.toml` |
