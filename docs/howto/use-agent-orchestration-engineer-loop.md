---
title: How the engineer loop orchestrates autonomous agents
description: Explains how Simard's engineer loop spawns a subordinate Copilot agent to do engineering work, replacing the old plan-parse-execute cycle.
last_updated: 2026-05-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../architecture/engineer-agent-orchestration.md
  - ../reference/spawn-agent-for-goal.md
  - ../reference/simard-engineer-step.md
  - ./spawn-engineers-from-ooda-daemon.md
---

# How the engineer loop orchestrates autonomous agents

Simard's engineer loop no longer produces or parses a multi-step JSON plan.
Instead it hands the entire objective to a **subordinate Copilot agent
session** and waits for that agent to complete the work autonomously. This
guide explains the new flow, what the loop does and does not control, and
how to interpret cycle reports produced under the new architecture.

---

## What happens during an engineer run

1. **Workspace inspection** — `inspect_workspace()` snapshots the branch,
   changed files, and active goals into a `RepoInspection` struct. This is
   unchanged.

2. **Prompt assembly** — The loop formats the objective and inspection into a
   natural-language prompt using the template in
   `prompt_assets/simard/engineer_planning.md`. No JSON schema is imposed;
   the agent is free to interpret and execute the objective however it sees
   fit.

3. **Agent spawn** — `spawn_agent_for_goal()` opens a Copilot agent session
   with the assembled prompt. The agent runs autonomously: it reads files,
   edits code, runs `cargo test`, commits, and opens issues — whatever the
   objective requires. The engineer loop blocks until the session returns or
   the 3600-second timeout elapses.

4. **Review** — `run_optional_review()` runs `git diff <pre-agent-sha>..HEAD`
   to capture all workspace mutations committed by the agent. The
   `EngineerActionKind::AgentSession` variant requires a dedicated
   `compute_diff_for_review` arm that diffs against `inspection.head`
   (the commit SHA recorded before the agent was spawned). Without this, an
   agent that commits its work before returning produces an empty diff and
   the review silently skips.

5. **Persist** — `persist_engineer_loop_artifacts()` writes the
   `EngineerLoopRun` to the state directory as a cycle report. The
   `plan_summary` field in the cycle report contains the agent's own
   description of what it did.

---

## What the engineer loop controls

| Controlled by loop | Controlled by agent              |
|--------------------|----------------------------------|
| Workspace inspection | File reads and edits           |
| Prompt assembly    | Tool selection                   |
| Session timeout    | Commit authoring                 |
| Review (diff)      | Test execution                   |
| Cycle report write | Verification strategy            |
| OODA result report | Sub-goal decomposition           |

The loop does **not** micro-manage the agent's steps. It provides context and
gets a result.

---

## Reading cycle reports

Cycle reports from agent-orchestrated runs have a `phase_traces` array like:

```json
[
  { "name": "inspect",            "duration": 1200,  "outcome": { "Success": null } },
  { "name": "agent-prompt-build", "duration": 12,    "outcome": { "Success": null } },
  { "name": "agent-spawn",        "duration": 487000, "outcome": { "Success": null } },
  { "name": "agent-wait",         "duration": 0,     "outcome": { "Success": null } },
  { "name": "review",             "duration": 340,   "outcome": { "Success": null } },
  { "name": "persist",            "duration": 45,    "outcome": { "Success": null } }
]
```

The bulk of the wall time lives in `agent-spawn`. The `plan_summary` field of
the embedded `ExecutedEngineerAction` contains the agent's own summary of what
it did.

There are no `select`, `execute`, or `verify` phase entries in new runs.
Dashboard queries that filter on those phase names will return empty results
for runs after the refactor.

---

## Handling agent failures

If the agent session fails or times out the engineer loop records a
`PhaseOutcome::Failed` in `agent-spawn` and propagates the error to the OODA
daemon as a goal failure. The OODA daemon will re-queue the goal for the next
advance cycle.

To investigate a failed agent run:

```bash
# Find the most recent cycle report
ls -lt ~/.simard/state/cycle-reports/ | head -5

# Read the phase traces and plan_summary
jq '.phase_traces, .action.selected.plan_summary' \
  ~/.simard/state/cycle-reports/<timestamp>.json
```

If `plan_summary` is empty and `agent-spawn` shows `Failed`, the agent either
timed out or the Copilot SDK returned a non-zero exit code. Check the
operator logs for the raw stderr:

```bash
simard logs --tail 100 | grep "agent session"
```

---

## Migrating custom scripts from the old plan API

Scripts that previously called the `select`, `execute`, or `verify`
subcommands of `simard-engineer-step` should be updated to call `agent-spawn`
instead:

**Before (old plan-parse-execute):**

```bash
SELECTED=$(simard-engineer-step select \
  --inspection-json "$INSPECTION" \
  --objective "$OBJECTIVE")

ACTION=$(simard-engineer-step execute \
  --repo-root "$WORKSPACE" \
  --selected-json "$SELECTED")

simard-engineer-step verify \
  --inspection-json "$INSPECTION" \
  --action-json "$ACTION" \
  --state-root "$STATE_ROOT"
```

**After (agent orchestration):**

```bash
ACTION=$(simard-engineer-step agent-spawn \
  --inspection-json "$INSPECTION" \
  --objective "$OBJECTIVE" \
  --workspace "$WORKSPACE" \
  --state-root "$STATE_ROOT")
```

The `agent-spawn` output is an `ExecutedEngineerAction` JSON object with the
same schema as before, so the `review` and `persist` steps are unchanged.

---

## Updating self_improve_executor integrations

If your code calls `plan_objective` and `execute_plan` directly (e.g. in a
custom improvement executor), replace both calls with a single
`spawn_agent_for_goal` call and store the returned summary in
`ImprovementPatch.outcome_summary`:

```rust
// Before
let plan = plan_objective(&proposal.description, &inspection)?;
let exec_result = execute_plan(&plan, workspace_path);
patch.plan = plan;

// After
let summary = spawn_agent_for_goal(&proposal.description, &inspection, workspace_path)?;
patch.outcome_summary = summary;
```

The `ImprovementPatch.plan: Plan` field no longer exists. Any serialised
patches from before the refactor that contain a `plan` key will have that
key ignored on deserialisation.
