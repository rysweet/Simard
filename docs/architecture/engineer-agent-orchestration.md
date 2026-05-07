---
title: Engineer Loop — Agent Orchestration Architecture
description: How Simard's engineer loop delegates work to a subordinate autonomous agent instead of parsing LLM-generated JSON plans.
last_updated: 2026-05-06
owner: simard
doc_type: concept
related:
  - ./overview.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../reference/simard-engineer-step.md
---

# Engineer Loop — Agent Orchestration Architecture

Simard's engineer loop delegates concrete coding work to a **subordinate
autonomous agent** (a nested Copilot CLI session). The engineer loop itself
acts as an **orchestrator**: it inspects the workspace, builds a natural
language goal prompt, spawns the agent, waits for it to finish, and then
records the result. The agent is fully autonomous — it calls its own tools,
writes its own code, runs tests, and commits. The engineer loop never parses
the agent's reasoning or micro-manages its steps.

This replaces the previous plan-parse-execute architecture where the loop
asked the LLM to produce a JSON array of `PlanStep` objects and then
mechanically dispatched each step via Rust match arms.

---

## Why the plan-parser was removed

The old `src/engineer_plan/mod.rs` module sent a prompt asking the LLM to
emit a JSON plan array. The engineer loop then iterated over the array and
executed each action through a fixed Rust dispatch table. This created three
compounding problems:

1. **Brittle serialisation boundary.** Any variation in the LLM's JSON
   output — extra prose, different casing, a missing field — caused a parse
   failure and aborted the whole loop. The `extract_json_array` heuristic was
   a symptom, not a solution.

2. **Capability ceiling.** The action enum (`EngineerActionKind`) constrained
   what the engineer could do. Adding a new capability meant a Rust PR, not a
   prompt edit.

3. **Artificial decomposition.** Forcing the LLM to commit to a multi-step
   plan up front, before it had seen the results of each step, produced
   brittle plans that failed in the middle and left the workspace dirty.

The agentic model eliminates all three problems: the agent sees its own tool
results, adapts mid-run, and can use any tool the Copilot CLI exposes.

---

## High-level data flow

```
OODA daemon
    │  advance_goal → task description
    ▼
Engineer loop (run_local_engineer_loop)
    │
    ├── Phase 1: inspect_workspace()           ← unchanged
    │       → RepoInspection (branch, dirty files, active goals, …)
    │
    ├── Phase 2: build agent prompt            ← new
    │       combines objective + RepoInspection into a natural-language prompt
    │
    ├── Phase 3: spawn_agent_for_goal()        ← new (replaces select+execute+verify)
    │       spawns copilot agent session
    │       waits up to 3600 s for completion
    │       returns execution_summary string
    │
    ├── Phase 4: run_optional_review()         ← unchanged
    │       diff-based review if agent mutated files
    │
    └── Phase 5: persist_engineer_loop_artifacts()  ← unchanged
            writes EngineerLoopRun → cycle report
```

The three old phases (select, execute, verify) are collapsed into a single
`spawn_agent_for_goal()` call. Selection, execution, and verification are all
handled autonomously by the agent.

---

## EngineerActionKind::AgentSession

The `EngineerActionKind` enum has one new variant:

```rust
pub enum EngineerActionKind {
    // …existing variants…
    AgentSession { outcome_summary: String },   // subordinate Copilot agent managed the work
}
```

`AgentSession` is treated as **mutating** by `run_optional_review` — it is
included in the `is_mutating` `matches!` arm in `review_persist.rs`.

For `compute_diff_for_review`, `AgentSession` requires a dedicated match arm
that diffs against the SHA captured **before** the agent was spawned (stored
as `inspection.head`). The wildcard arm runs `git diff` (uncommitted
working-tree only) — insufficient when the agent commits before returning:

```rust
EngineerActionKind::AgentSession { .. } => {
    &["git", "diff", inspection.head.as_str(), "HEAD"]
}
```

`inspection.head` already exists in `RepoInspection` and holds the HEAD SHA
captured at the time of workspace inspection — before the agent is spawned.
Without this dedicated arm, an agent run that commits its work produces an
empty diff and the review silently skips.

An `ExecutedEngineerAction` whose `selected.kind` is `AgentSession` has:

| Field              | Content                                              |
|--------------------|------------------------------------------------------|
| `label`            | `"agent-session"`                                    |
| `rationale`        | The natural-language prompt sent to the agent        |
| `argv`             | `[]` (agent sessions have no argv; the prompt is sent via session API) |
| `plan_summary`     | The `execution_summary` returned by the agent        |
| `exit_code`        | `0` on success, non-zero on agent failure or timeout |
| `stdout`           | Agent's final output text                            |
| `stderr`           | Copilot SDK stderr / timeout message                 |
| `changed_files`    | Files touched (from `git diff --name-only`)          |

---

## Timeout

`spawn_agent_for_goal()` enforces a **3600-second** wall-clock timeout
(`AGENT_SESSION_TIMEOUT_SECS` in `agent_spawn.rs`). If the agent session does not return
within that window the call returns:

```
Err(SimardError::ActionExecutionFailed {
    reason: "agent session timed out after 3600s"
})
```

The engineer loop records this as a `PhaseOutcome::Failed` and proceeds to
persist the (incomplete) cycle report so the OODA daemon can re-schedule the
goal.

---

## PhaseTrace names

The three agent-related phases recorded in `EngineerLoopRun.phase_traces` are:

| Phase name            | What it covers                                      |
|-----------------------|-----------------------------------------------------|
| `agent-prompt-build`  | Formatting objective + inspection into prompt text  |
| `agent-spawn`         | `run_turn()` call — full agent session wall time    |
| `agent-wait`          | Any post-session polling / result extraction        |

Downstream consumers (dashboards, cycle-report parsers) should treat any
`phase_traces` entry whose `name` starts with `agent-` as belonging to the
consolidated execution phase.
