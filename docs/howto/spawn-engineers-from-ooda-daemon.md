---
title: How OODA spawns engineer agents
description: Describes how the OODA daemon's advance-goal action parses the LLM's structured response and dispatches subordinate engineer agents.
last_updated: 2026-04-18
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./run-ooda-daemon.md
  - ../reference/simard-cli.md
  - https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md
---

# How OODA spawns engineer agents

When the OODA daemon advances a goal, it consults an LLM "goal session" that
returns a **structured JSON action**. The dispatcher parses that action and
either (a) spawns a subordinate engineer agent to execute the work,
(b) records a no-op with a reason, or (c) records an assessment-only update
of the goal's progress.

This replaces the previous behaviour where `dispatch_advance_goal` ran the
session purely for free-form assessment text and never actually spawned any
worker.

## The action contract

The LLM is instructed (via `prompt_assets/simard/goal_session_objective.md`)
to emit **exactly one JSON object** as its entire response, matching one of
the following three shapes:

```json
{"action": "spawn_engineer", "task": "<concrete task>", "files": ["path/to/file", "..."]}
```

```json
{"action": "noop", "reason": "<why no action is needed right now>"}
```

```json
{"action": "assess_only", "assessment": "<short status>", "progress_pct": 42}
```

Field rules:

| Field          | Variant         | Required | Notes                                      |
| -------------- | --------------- | -------- | ------------------------------------------ |
| `action`       | all             | yes      | Discriminator. Must be one of three values. |
| `task`         | spawn_engineer  | yes      | Non-empty, ≤ 8 KiB.                        |
| `files`        | spawn_engineer  | no       | Defaults to `[]`. Accepted but ignored; reserved for a future PR. Path validation will be added when wired — do not rely on any sandboxing today. |
| `reason`       | noop            | yes      | ≤ 4 KiB.                                   |
| `assessment`   | assess_only     | yes      | ≤ 4 KiB.                                   |
| `progress_pct` | assess_only     | yes      | Integer in `0..=100`. Parsed as `u8`; negative values, decimals, and out-of-range integers are rejected (action falls back to parse-failure path). |

The prompt instructs the LLM to emit JSON only (no prose, no Markdown
fences). When uncertain it should prefer `assess_only`.

## How the dispatcher consumes the action

`src/operator_commands_ooda/goal_session.rs` exposes a private
`parse_goal_action(response: &str) -> Option<GoalAction>` that:

1. Trims whitespace and tries `serde_json::from_str` directly on the response.
2. On failure, scans for the first balanced `{ ... }` block (string- and
   escape-aware, depth capped at 256, input capped at 64 KiB) and re-parses
   that substring.
3. Returns `None` (with a `warn!` log) if neither attempt yields a valid
   `GoalAction`.

`advance_goal_with_session` returns a `GoalSessionResult` carrying the parsed
action (if any), the raw response, and a descriptive outcome detail string.

`dispatch_advance_goal` in `src/operator_commands_ooda/advance_goal.rs` then
branches:

- **`SpawnEngineer { task, files }`**
  1. Re-checks `goal.assigned_to.is_none()` under the state lock to prevent
     double-spawn races.
  2. Reads `SIMARD_SUBORDINATE_DEPTH` (default `0`) and refuses to spawn if
     it is already at or above `SIMARD_MAX_SUBORDINATE_DEPTH` (default
     `u32::MAX`, i.e. unlimited unless an operator opts in). **The dispatcher
     is the sole hard gate** — see the recursion-and-safety section below.
  3. Builds a `SubordinateConfig` (`AgentRole` from
     `crate::identity_composition::AgentRole`):
     ```rust
     use crate::identity_composition::AgentRole;
     // Goal IDs may be re-dispatched after release/crash, so suffix the
     // agent_name with an epoch nonce to keep CompositeIdentity and
     // CognitiveMemory keys unique.
     let nonce = std::time::SystemTime::now()
         .duration_since(std::time::UNIX_EPOCH)
         .map(|d| d.as_secs())
         .unwrap_or(0);
     SubordinateConfig {
         agent_name: format!("engineer-{}-{}", goal.id, nonce),
         goal: task.clone(),
         role: AgentRole::Engineer,
         // NOTE: dispatch_advance_goal runs inside the daemon process whose
         // CWD is the supervisor's worktree, not a per-goal worktree. This
         // PR ships single-worktree spawn; per-goal worktrees are tracked
         // separately.
         worktree_path: std::env::current_dir()?,
         current_depth: env_depth(),
     }
     ```
  4. Calls `agent_supervisor::lifecycle::spawn_subordinate(&config)`.
  5. On success, releases the session borrow and mutates
     `state.active_goals.active.iter_mut().find(|g| g.id == goal_id)` to set
     `assigned_to = Some(agent_name)`.
  6. Records an outcome:
     `"spawn_engineer dispatched: agent='engineer-<id>-<nonce>', task='<truncated to 256>'"`.
  7. On `Err`, logs at `error!` and records:
     `"spawn_engineer failed: <error>"`.

- **`Noop { reason }`** — records `"noop: <reason truncated to 256>"` and
  leaves the goal unchanged.

- **`AssessOnly { assessment, progress_pct }`** — records
  `"assess_only: <assessment truncated to 256> (progress=<N>%)"` and applies
  the assessment via the existing progress-update path.

- **Parse failed (`None`)** — records
  `"goal-action parse failed; fell back to assessment"` and falls through to
  the legacy `assess_progress_from_outcome` + `verify_claimed_actions` path
  for backward compatibility with free-form responses.

The cycle report's `outcomes` vec now always contains at least one
descriptive entry per advance-goal dispatch (it was previously left empty).

## Recursion and safety

- **Depth guard.** The parent supervisor sets `SIMARD_SUBORDINATE_DEPTH`
  before forking each child. The dispatcher reads it and refuses to spawn
  when it is already at or above `SIMARD_MAX_SUBORDINATE_DEPTH` (default
  unlimited, see `src/identity_composition.rs`). **The dispatcher is the
  sole hard gate**: `SubordinateConfig::validate` (in
  `src/agent_supervisor/types.rs`) only emits an `eprintln!("warning: …")`
  when the configured limit is exceeded and then spawns anyway, mirroring
  the supervisor's "external tools (Copilot, Claude, etc.) have their own
  guardrails" stance. If the dispatcher does not enforce the gate, nothing
  else will.
- **Argv-only spawn.** The task string is passed as opaque data through the
  child's argv — never interpolated into a shell.
- **Input bounds.** The parser caps the response at 64 KiB and the
  brace-balanced extractor at depth 256. Malformed or oversized responses
  fall back instead of panicking.
- **Race protection.** `assigned_to.is_none()` is rechecked under the state
  lock immediately before dispatching, so two cycles racing on the same goal
  can never spawn two engineers.

## Worked example

Suppose the goal board contains:

```text
goal_42: "Add JSON serialization tests for GoalAction"
```

A cycle invokes the LLM goal-session prompt and the model returns:

```json
{
  "action": "spawn_engineer",
  "task": "Add unit tests in src/operator_commands_ooda/goal_session.rs that exercise GoalAction serde for all three variants, including JSON-in-prose extraction.",
  "files": ["src/operator_commands_ooda/goal_session.rs"]
}
```

The dispatcher:

1. Parses the JSON into `GoalAction::SpawnEngineer { task, files }`.
2. Confirms `goal_42.assigned_to` is `None`.
3. Spawns `engineer-goal_42-1776552500` (epoch-suffixed) with the task as
   its goal.
4. Sets `goal_42.assigned_to = Some("engineer-goal_42-1776552500")`.
5. Appends to the cycle report:
   `outcomes: ["spawn_engineer dispatched: agent='engineer-goal_42-1776552500', task='Add unit tests in src/operator_commands_ooda/goal_session.rs ...'"]`.

Subsequent cycles see `assigned_to.is_some()` and will skip respawning until
the engineer terminates and the goal is released.

## Backward compatibility

If a model still returns free-form prose (or any non-JSON), the dispatcher:

1. Logs a single `warn!` line ("goal session response did not parse as JSON
   action; falling back").
2. Runs the legacy assessment path unchanged.
3. Records the parse-failure outcome string.

No existing OODA loop behaviour is removed — `noop` and `assess_only` map
1-to-1 onto previous semantics, and parse failures preserve the old code
path completely.

Operators with a stale `prompt_assets/simard/goal_session_objective.md`
(one that does not instruct the model to emit JSON) will continue to hit
the legacy assessment branch on every cycle — nothing breaks, but no
engineers will ever spawn until the prompt asset is updated. Refresh the
prompt asset to opt in.

## Configuration

| Variable                       | Default     | Purpose                                                                                                     |
| ------------------------------ | ----------- | ----------------------------------------------------------------------------------------------------------- |
| `SIMARD_SUBORDINATE_DEPTH`     | `0`         | Current recursion depth. The parent supervisor sets this before forking each child.                         |
| `SIMARD_MAX_SUBORDINATE_DEPTH` | `u32::MAX`  | Maximum allowed recursion. Default is **unlimited**; operators commonly set this to `3` or `4` in practice. The dispatcher refuses to spawn when `SIMARD_SUBORDINATE_DEPTH >= SIMARD_MAX_SUBORDINATE_DEPTH`. The supervisor's `SubordinateConfig::validate` only logs a warning at this threshold — it does not block. |

There is no new CLI flag and no systemd config change. Operators who
already run `simard ooda run` get engineer-spawning behaviour automatically
once their goal-session prompt asset emits the JSON contract above.

## Observability

Each cycle report includes:

- `outcomes[]` — one descriptive string per advance-goal action (see branch
  table above). Strings are truncated to 256 characters to prevent log
  flooding. **Format stability:** the exact outcome strings
  (e.g. `spawn_engineer dispatched: agent='…', task='…'`) are **unstable**
  in this PR and may change without notice. Do not parse them from external
  tools or tests; query the structured cycle-report fields instead. A
  versioned schema will be introduced if/when downstream consumers need it.
- `warn!` log on JSON parse failure (with raw response truncated to 256 chars).
- `error!` log on `spawn_subordinate` failure (with the underlying error).
- `info!` log on successful spawn, including agent name and goal id.

## Related

- [How to run the OODA daemon](./run-ooda-daemon.md)
- [Simard CLI reference](../reference/simard-cli.md)
- [Goal session objective prompt](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md)
- Source: `src/agent_supervisor/lifecycle.rs` (`spawn_subordinate`)
- Source: `src/agent_supervisor/types.rs` (`SubordinateConfig::validate`)
- Source: `src/identity_composition.rs` (`max_subordinate_depth`,
  `SIMARD_MAX_SUBORDINATE_DEPTH`)
- Issue: [#929 — Wire Simard OODA daemon to spawn engineer agents](https://github.com/rysweet/Simard/issues/929)
