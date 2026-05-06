---
title: simard-engineer-step — CLI Reference
description: Reference for the simard-engineer-step helper binary used by recipe-driven engineer loops.
last_updated: 2026-05-06
owner: simard
doc_type: reference
related:
  - ../architecture/engineer-agent-orchestration.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# `simard-engineer-step` — CLI Reference

`simard-engineer-step` is a helper binary used by the recipe-driven engineer
loop. Each subcommand corresponds to one phase of the loop and reads/writes
JSON over stdin/stdout for IPC between recipe steps.

---

## Subcommands

### `inspect`

Snapshot the workspace state.

```
simard-engineer-step inspect \
    --workspace <path> \
    --state-root <path>
```

| Flag           | Required | Description                                       |
|----------------|----------|---------------------------------------------------|
| `--workspace`  | yes      | Absolute path to the git worktree root            |
| `--state-root` | yes      | Path to Simard's persistent state directory       |

**Output**: `RepoInspection` JSON object.

---

### `agent-spawn`

Build an agent prompt and spawn a subordinate Copilot agent session to
complete the work autonomously. This is the single replacement for the
former `select`, `execute`, and `verify` subcommands.

```
simard-engineer-step agent-spawn \
    --inspection-json <json> \
    --objective <text> \
    --workspace <path> \
    --state-root <path>
```

| Flag                | Required | Description                                               |
|---------------------|----------|-----------------------------------------------------------|
| `--inspection-json` | yes      | JSON-encoded `RepoInspection` (output of `inspect`)       |
| `--objective`       | yes      | Natural-language goal description                         |
| `--workspace`       | yes      | Absolute path to the git worktree root                    |
| `--state-root`      | yes      | Path to Simard's persistent state directory               |

**Output**: `ExecutedEngineerAction` JSON object with `kind = "agent_session"`.

**Timeout**: The agent session is allowed up to **3600 seconds**. If the
session does not return within that window `simard-engineer-step` exits with
code `2` and prints a timeout message to stderr.

**Exit codes**:

| Code | Meaning                                        |
|------|------------------------------------------------|
| `0`  | Agent session completed successfully           |
| `2`  | Flag parsing error, agent failure, or timeout  |

---

### `review`

Run the optional diff-based review after the agent session.

```
simard-engineer-step review \
    --inspection-json <json> \
    --action-json <json>
```

| Flag                | Required | Description                                           |
|---------------------|----------|-------------------------------------------------------|
| `--inspection-json` | yes      | JSON-encoded `RepoInspection`                         |
| `--action-json`     | yes      | JSON-encoded `ExecutedEngineerAction`                 |

**Output**: Review result JSON or an empty object when review is skipped.

---

### `persist`

Persist the full engineer loop run as a cycle report.

```
simard-engineer-step persist \
    --state-root <path> \
    --topology <kebab> \
    --objective <text> \
    --inspection-json <json> \
    --action-json <json> \
    --verification-json <json> \
    [--terminal-bridge-json <json>]
```

| Flag                     | Required | Description                                          |
|--------------------------|----------|------------------------------------------------------|
| `--state-root`           | yes      | Path to Simard's persistent state directory          |
| `--topology`             | yes      | Runtime topology string (e.g. `local-only`)          |
| `--objective`            | yes      | Original goal text                                   |
| `--inspection-json`      | yes      | JSON-encoded `RepoInspection`                        |
| `--action-json`          | yes      | JSON-encoded `ExecutedEngineerAction`                |
| `--verification-json`    | yes      | JSON-encoded `VerificationReport`                    |
| `--terminal-bridge-json` | no       | JSON-encoded `TerminalBridgeContext` (if applicable) |

**Output**: Confirmation JSON with the path to the written report.

---

## Removed subcommands

The following subcommands existed in earlier versions and have been removed.
Recipes and scripts that referenced them must be updated to use `agent-spawn`.

| Removed subcommand | Replacement     |
|--------------------|-----------------|
| `select`           | `agent-spawn`   |
| `execute`          | `agent-spawn`   |
| `verify`           | `agent-spawn`   |

The `inspect`, `review`, and `persist` subcommands are unchanged.

---

## JSON schemas

### `RepoInspection`

```json
{
  "branch": "feat/my-feature",
  "worktree_dirty": true,
  "changed_files": ["src/foo.rs", "src/bar.rs"],
  "active_goals": [
    { "id": "goal-uuid", "title": "Implement foo" }
  ]
}
```

### `ExecutedEngineerAction` (agent-spawn output)

```json
{
  "selected": {
    "label": "agent-session",
    "rationale": "Implement the frobnicator as described in objective",
    "argv": ["copilot", "agent", "--goal", "<objective>"],
    "plan_summary": "Added frobnicator module, all tests pass.",
    "verification_steps": [],
    "expected_changed_files": [],
    "kind": "agent_session"
  },
  "exit_code": 0,
  "stdout": "Agent session completed successfully.\nAdded frobnicator…",
  "stderr": "",
  "changed_files": ["src/frobnicator/mod.rs", "src/frobnicator/tests.rs"]
}
```

---

## Full recipe example

```bash
# Phase 1: inspect
INSPECTION=$(simard-engineer-step inspect \
  --workspace "$WORKSPACE" \
  --state-root "$STATE_ROOT")

# Phase 2+3: build prompt + spawn agent (replaces select/execute/verify)
ACTION=$(simard-engineer-step agent-spawn \
  --inspection-json "$INSPECTION" \
  --objective "Fix the off-by-one error in src/ooda_loop/orient.rs" \
  --workspace "$WORKSPACE" \
  --state-root "$STATE_ROOT")

# Phase 4: review
simard-engineer-step review \
  --inspection-json "$INSPECTION" \
  --action-json "$ACTION" > /dev/null

# Phase 5: persist
simard-engineer-step persist \
  --state-root "$STATE_ROOT" \
  --topology "local-only" \
  --objective "Fix the off-by-one error in src/ooda_loop/orient.rs" \
  --inspection-json "$INSPECTION" \
  --action-json "$ACTION" \
  --verification-json '{"status":"passed","summary":"agent verified","checks":[]}'
```
