---
title: Engineer Loop — Configuration Reference
description: All environment variables, constants, and runtime configuration options that govern the agent-orchestration engineer loop.
last_updated: 2026-05-07
owner: simard
doc_type: reference
related:
  - ../architecture/engineer-agent-orchestration.md
  - ./spawn-agent-for-goal.md
  - ./simard-engineer-step.md
---

# Engineer Loop — Configuration Reference

This page lists every environment variable, compile-time constant, and
runtime setting that affects how the engineer loop spawns and manages
subordinate Copilot agent sessions.

---

## Environment variables

### `SIMARD_LLM_PROVIDER`

Selects the LLM provider used to open the subordinate agent session.

| Value         | Meaning                                                      |
|---------------|--------------------------------------------------------------|
| `copilot`     | Copilot CLI (`amplihack copilot -p`). **Default when set.** |
| `rustyclawd`  | RustyClawd direct API (not supported for engineer sessions — returns an explicit error) |

Resolution order for `LlmProvider::resolve()`:

1. `SIMARD_LLM_PROVIDER` environment variable
2. `llm_provider` field in `~/.simard/config.toml`
3. Error — no provider configured

If neither the env var nor the config file provides a value,
`spawn_agent_for_goal` returns `Err(SimardError::ActionExecutionFailed)` with
a message indicating the provider is not configured.

**Example:**

```bash
export SIMARD_LLM_PROVIDER=copilot
```

---

### `SIMARD_STATE_ROOT`

Absolute path to Simard's persistent state directory. Used by the engineer
loop to read active goals and write cycle reports.

Defaults to `~/.simard/state` when unset.

**Example:**

```bash
export SIMARD_STATE_ROOT=/var/lib/simard/state
```

---

### `SIMARD_SUBORDINATE_DEPTH`

Integer tracking the current recursion depth of subordinate engineer sessions.
Set automatically by the OODA daemon when it spawns an engineer. Engineer
sessions read this to prevent infinite spawning chains.

Default: `0` (top-level session; no depth limit applies).

Controlled by `SIMARD_MAX_SUBORDINATE_DEPTH` (see below).

---

### `SIMARD_MAX_SUBORDINATE_DEPTH`

Maximum nesting depth for subordinate engineer sessions. The OODA daemon
refuses to spawn a new engineer session if
`SIMARD_SUBORDINATE_DEPTH >= SIMARD_MAX_SUBORDINATE_DEPTH`.

Default: `2`.

---

## Compile-time constants

These constants are defined in `src/engineer_loop/agent_spawn.rs` and
`src/engineer_loop/mod.rs`. They cannot be changed at runtime without
recompiling.

| Constant                    | Value | Location                              | Purpose                                          |
|-----------------------------|-------|---------------------------------------|--------------------------------------------------|
| `AGENT_SESSION_TIMEOUT_SECS`| `3600`| `src/engineer_loop/agent_spawn.rs`    | Wall-clock timeout for a single agent session    |
| `GIT_COMMAND_TIMEOUT_SECS`  | `60`  | `src/engineer_loop/mod.rs`            | Timeout for each git subprocess (inspect phase)  |
| `CARGO_COMMAND_TIMEOUT_SECS`| `120` | `src/engineer_loop/mod.rs`            | Timeout for each cargo subprocess                |
| `MAX_CARRIED_MEETING_DECISIONS` | `3` | `src/engineer_loop/mod.rs`          | Maximum meeting decisions injected into context  |

---

## Configuration file — `~/.simard/config.toml`

The runtime config file can set the LLM provider and other defaults. Relevant
fields for the engineer loop:

```toml
[llm]
provider = "copilot"   # or "rustyclawd"
```

The config file is loaded once at process start. Environment variables always
take precedence over file-based configuration.

---

## Agent session identity

The engineer agent session is opened with the following fixed identity
parameters (see `src/engineer_loop/agent_spawn.rs`):

| Parameter     | Value                    |
|---------------|--------------------------|
| `node_id`     | `"engineer-agent"`       |
| `address`     | `"engineer-agent://local"` |
| `adapter_tag` | `"engineer-agent"`       |
| `mode`        | `OperatingMode::Engineer`|

These values appear in session logs and cycle reports to distinguish engineer
sub-sessions from the top-level OODA daemon session.

---

## Review gate configuration

`run_optional_review()` runs only when all of the following are true:

1. The action is mutating (`EngineerActionKind` is one of
   `StructuredTextReplace`, `CreateFile`, `AppendToFile`, `GitCommit`,
   or `AgentSession`).
2. `crate::review_pipeline::ReviewSession::open()` succeeds — i.e. an LLM
   session is available and an API key / Copilot token is present.
3. `compute_diff_for_review` produces a non-empty diff.

The review gate uses the diff text from:
- **`AgentSession`**: `git diff <pre-spawn-head> HEAD` — covers all commits
  made by the agent during its session.
- **`GitCommit`**: `git diff HEAD~1 HEAD`.
- All other mutating actions: `git diff` (uncommitted working-tree changes).

If `ReviewSession::open()` returns `ReviewUnavailable`, review is silently
skipped — it is an optional safety net, not a hard gate.

---

## Cleared git environment variables

To prevent git subprocess contamination when engineer sessions run inside
existing git worktrees, the following git environment variables are cleared
before each git command:

```
GIT_DIR
GIT_WORK_TREE
GIT_INDEX_FILE
GIT_OBJECT_DIRECTORY
GIT_ALTERNATE_OBJECT_DIRECTORIES
GIT_COMMON_DIR
GIT_PREFIX
```

These are listed in `CLEARED_GIT_ENV_VARS` in `src/engineer_loop/mod.rs`.
