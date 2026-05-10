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
engineering work to an autonomous agent session. It is a **thin subprocess
wrapper around `amplihack RustyClawd --auto`** — Simard does not implement
its own LLM loop, tool selection, or reflection. The subprocess IS the
engineer.

This replaces the earlier in-process `SessionBuilder` integration which
duplicated agent-loop logic in custom Rust. See ADR-0024 in
`Specs/ARCHITECTURAL_DECISIONS.md` (issue #1648) for the rationale.

**Module**: `simard::engineer_loop::agent_spawn` (re-exported as
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
- Subprocess spawn failure (e.g., `amplihack` not on PATH)
- `amplihack RustyClawd` exited non-zero (full stderr tail in `reason`)
- Subprocess wall time exceeds 3600 s (`SimardError::CommandTimeout`)

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
spawns the `amplihack RustyClawd --auto` subprocess and polls it on a
background thread; the calling thread receives the result over an
`mpsc::channel`, so it is never permanently blocked if the subprocess
hangs:

```rust
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

let (tx, rx) = mpsc::channel();
std::thread::spawn(move || {
    // Spawns: amplihack RustyClawd --auto --subprocess-safe
    //                  --no-reflection --max-turns 30 -- -p <prompt>
    let result = run_rustyclawd_subprocess(&prompt, &workspace);
    let _ = tx.send(result);
});
match rx.recv_timeout(Duration::from_secs(3600 + 30)) {
    Ok(result) => result,
    Err(_) => Err(SimardError::ActionExecutionFailed { /* timeout */ }),
}
```

The 3600-second limit matches `CARGO_COMMAND_TIMEOUT_SECS` defined in
`src/engineer_loop/mod.rs`.

The amplihack binary path is resolved from `PATH` by default; override
via the `SIMARD_AMPLIHACK_BIN` environment variable for tests or
non-standard installs.

### Choosing the agent kind

The subprocess defaults to `amplihack RustyClawd --auto`, but the agent
kind is configurable via the `SIMARD_ENGINEER_AGENT` environment
variable. Recognised values (case-insensitive):

| Value         | Subcommand invoked            | Argv shape (after the binary)                                                     |
|---------------|-------------------------------|-----------------------------------------------------------------------------------|
| `rustyclawd`  | `amplihack RustyClawd`        | `RustyClawd --auto --subprocess-safe --no-reflection --max-turns 30 -- -p <prompt>` |
| `copilot`     | `amplihack copilot`           | `copilot --allow-all-paths -p <prompt>`                                           |

Unknown values fall back to `rustyclawd` with a stderr warning so a typo
does not silently change the engineer behaviour. Adding more agent kinds
is a matter of extending `AgentKind` in
`src/engineer_loop/agent_spawn.rs` and providing the per-kind argv in
`engineer_argv()`.

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
| `ActionExecutionFailed`        | Subprocess spawn failed, `amplihack RustyClawd` returned non-zero exit, or `workspace_path` does not exist / is not a git repo |
| `CommandTimeout`               | Subprocess wall time exceeded `AGENT_SESSION_TIMEOUT_SECS` (3600 s); subprocess was killed |
