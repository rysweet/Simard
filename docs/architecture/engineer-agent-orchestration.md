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

## Subprocess delegation (architectural pivot — issue #1648)

The agent session is **not** an in-process LLM SDK call. It is a subprocess
invocation of the upstream `amplihack RustyClawd --auto` autonomous engineer.
Simard's role is to act as a PM architect orchestrating fleets of coding
agents — not to reimplement the agent loop in custom Rust.

The exact subprocess invocation is:

```
amplihack RustyClawd --auto --subprocess-safe --no-reflection \
                     --max-turns 30 -- -p "<prompt>"
```

| Flag                | Why                                                                    |
|---------------------|------------------------------------------------------------------------|
| `--auto`            | enables the autonomous agentic loop (clarify → plan → execute → eval)  |
| `--subprocess-safe` | skip staging mutations from a child invocation                         |
| `--no-reflection`   | Simard owns reflection separately via `review_pipeline`                |
| `--max-turns 30`    | upper bound on autonomous turns; matches amplihack complex-task guide  |

The amplihack binary is resolved from `PATH` by default; override with
`SIMARD_AMPLIHACK_BIN` for tests / non-standard installs.

### Why a subprocess and not an in-process SDK call?

The previous in-process `SessionBuilder` integration kept hitting two failure
modes that the subprocess model cleanly resolves:

1. **SIGTERM ignored mid-call.** The in-process LLM SDK held the engineer
   thread inside a single blocking `run_turn()` call. SIGTERM to Simard
   could not interrupt the call, so the daemon would start NEW autonomous
   turns after a stop request.
2. **Capability duplication.** Tool selection, retry policy, prompt
   construction, and reflection were all duplicated between Simard and the
   upstream `amplihack` agent loop — every upgrade to one had to be
   re-implemented in the other.

With the subprocess model, `kill(simard_pid)` orphans the child to init
(reaped naturally), and the agent-loop logic lives in exactly one place.

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
