---
title: "Tutorial: Run your first agent-orchestrated engineer session"
description: Step-by-step walkthrough of triggering a single engineer loop run under the agent-orchestration architecture — from workspace setup through reading the cycle report.
last_updated: 2026-05-07
owner: simard
doc_type: tutorial
related:
  - ../howto/use-agent-orchestration-engineer-loop.md
  - ../architecture/engineer-agent-orchestration.md
  - ../reference/simard-engineer-step.md
  - ../reference/engineer-loop-configuration.md
---

# Tutorial: Run your first agent-orchestrated engineer session

This tutorial walks you through a single end-to-end engineer loop run under
Simard's agent-orchestration architecture. By the end you will have:

- Inspected a workspace and understood the `RepoInspection` output
- Spawned a subordinate Copilot agent to complete an objective
- Read the resulting cycle report and understood the phase trace

**Time:** 15–20 minutes  
**Prerequisites:**

- Simard binaries built (`cargo build --release`)
- Copilot CLI authenticated (`gh auth login` or equivalent)
- A git repository to operate on (this tutorial uses a scratch repo)
- `SIMARD_LLM_PROVIDER=copilot` set in your shell

---

## Step 1 — Create a scratch workspace

```bash
mkdir -p /tmp/simard-tutorial/repo
cd /tmp/simard-tutorial/repo
git init
echo 'fn main() { println!("hello"); }' > main.rs
git add main.rs
git commit -m "initial commit"
```

```bash
mkdir -p /tmp/simard-tutorial/state
```

The state directory (`/tmp/simard-tutorial/state`) is where Simard writes
goal records and cycle reports.

---

## Step 2 — Inspect the workspace

The `inspect` subcommand snapshots the workspace state:

```bash
simard-engineer-step inspect \
  --workspace /tmp/simard-tutorial/repo \
  --state-root /tmp/simard-tutorial/state \
  | tee /tmp/simard-tutorial/inspection.json
```

You should see JSON similar to:

```json
{
  "workspace_root": "/tmp/simard-tutorial/repo",
  "repo_root": "/tmp/simard-tutorial/repo",
  "branch": "main",
  "head": "a1b2c3d4...",
  "worktree_dirty": false,
  "changed_files": [],
  "active_goals": [],
  "carried_meeting_decisions": [],
  "architecture_gap_summary": ""
}
```

Notice `head` — this SHA will be used later to diff all changes the agent
makes during its session.

---

## Step 3 — Spawn the agent

The `agent-spawn` subcommand builds the natural-language prompt from your
inspection and objective, then opens a subordinate Copilot agent session to
complete the work autonomously:

```bash
OBJECTIVE="Add a hello_world function to main.rs that returns the string \
'Hello, world!' and write a test for it."

simard-engineer-step agent-spawn \
  --inspection-json "$(cat /tmp/simard-tutorial/inspection.json)" \
  --objective "$OBJECTIVE" \
  --workspace /tmp/simard-tutorial/repo \
  --state-root /tmp/simard-tutorial/state \
  | tee /tmp/simard-tutorial/action.json
```

This call **blocks** until the agent completes or the 3600-second timeout
elapses. While it runs, the Copilot agent will:

1. Read `main.rs`
2. Add the `hello_world` function and test
3. Run `cargo test` (or equivalent)
4. Commit the changes

On success you'll see `ExecutedEngineerAction` JSON with `exit_code: 0`:

```json
{
  "selected": {
    "label": "agent-session",
    "rationale": "Spawned autonomous agent session for: Add a hello_world...",
    "argv": [],
    "plan_summary": "Add a hello_world function to main.rs that returns the string 'Hello, world!' and write a test for it.",
    "verification_steps": [],
    "expected_changed_files": [],
    "kind": { "agent_session": { "outcome_summary": "Added hello_world() returning 'Hello, world!' and a #[test] fn test_hello_world. cargo test passes." } }
  },
  "exit_code": 0,
  "stdout": "Added hello_world() returning 'Hello, world!' and a #[test] fn test_hello_world. cargo test passes.",
  "stderr": "",
  "changed_files": ["main.rs"]
}
```

> **Note:** `plan_summary` echoes the original objective. The agent's execution
> summary is in `stdout` and `selected.kind.agent_session.outcome_summary`.

---

## Step 4 — Run the optional review

```bash
simard-engineer-step review \
  --inspection-json "$(cat /tmp/simard-tutorial/inspection.json)" \
  --action-json "$(cat /tmp/simard-tutorial/action.json)"
```

The review diffs `inspection.head` (the SHA from Step 2) against `HEAD` to
capture all commits the agent made. If the diff is clean the command exits
silently. If the reviewer finds high-severity issues it exits non-zero with
a summary.

---

## Step 5 — Persist the cycle report

```bash
VERIFICATION='{"status":"agent-completed","summary":"agent verified","checks":[]}'

simard-engineer-step persist \
  --state-root /tmp/simard-tutorial/state \
  --topology "local-only" \
  --objective "$OBJECTIVE" \
  --inspection-json "$(cat /tmp/simard-tutorial/inspection.json)" \
  --action-json "$(cat /tmp/simard-tutorial/action.json)" \
  --verification-json "$VERIFICATION"
```

---

## Step 6 — Read the cycle report

```bash
ls /tmp/simard-tutorial/state/cycle-reports/
```

Open the most recent report and inspect the phase traces:

```bash
jq '.phase_traces[] | {name, outcome}' \
  /tmp/simard-tutorial/state/cycle-reports/*.json
```

Expected output:

```json
{ "name": "inspect",            "outcome": { "Success": null } }
{ "name": "load-bridge-context","outcome": { "Success": null } }
{ "name": "agent-prompt-build", "outcome": { "Success": null } }
{ "name": "agent-spawn",        "outcome": { "Success": null } }
{ "name": "agent-wait",         "outcome": { "Success": null } }
{ "name": "review",             "outcome": { "Success": null } }
{ "name": "persist",            "outcome": { "Success": null } }
```

The `agent-spawn` phase accounts for almost all the wall time. The agent's
execution summary is in `action.stdout` (and mirrored in
`action.selected.kind.agent_session.outcome_summary`):

```bash
jq '.action.stdout' \
  /tmp/simard-tutorial/state/cycle-reports/*.json
```

> **Note:** `action.selected.plan_summary` echoes the original objective string,
> not the agent's output.

---

## What you've learned

| Phase | What happened |
|-------|---------------|
| `inspect` | Workspace snapshot captured branch, HEAD SHA, and dirty files |
| `agent-prompt-build` | Objective + inspection assembled into a natural-language prompt |
| `agent-spawn` | Copilot agent ran autonomously — no JSON plan, no dispatch table |
| `agent-wait` | Engineer loop blocked until agent returned execution summary |
| `review` | Diff from pre-agent HEAD to current HEAD reviewed by LLM |
| `persist` | `EngineerLoopRun` written as cycle report |

The key difference from the old plan-parse-execute architecture is that the
agent handles **all** of steps 2–4 autonomously. The engineer loop provides
context and collects the result — it never parses intermediate reasoning or
micro-manages the agent's tool calls.

---

## Next steps

- [How the engineer loop orchestrates autonomous agents](../howto/use-agent-orchestration-engineer-loop.md) — deeper explanation of each phase
- [spawn_agent_for_goal API reference](../reference/spawn-agent-for-goal.md) — call the Rust function directly
- [Engineer loop configuration](../reference/engineer-loop-configuration.md) — env vars and timeouts
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md) — how the daemon wires this together
