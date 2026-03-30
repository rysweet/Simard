---
title: Simard CLI reference
description: Reference for the planned unified `simard` command tree and the current runnable command mappings that expose the same runtime behaviors today.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./runtime-contracts.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../tutorials/run-your-first-local-session.md
---

# [PLANNED - Implementation Pending] Simard CLI reference

## Status

This document describes the unified operator-facing CLI Simard is intended to ship.

That full command tree is **not** implemented in `src/main.rs` yet.

Today:

- `simard` is a thin bootstrap entrypoint that reads configuration from environment variables
- `simard_operator_probe` is the current multi-mode compatibility binary
- `simard-gym` is the current benchmark binary

Use the current command mappings in this document when you need runnable commands today. Remove the planned markers once `src/main.rs` dispatches the full tree and dedicated CLI tests cover it.

## Current executables

| Executable | Status today | Purpose |
| --- | --- | --- |
| `simard` | shipped | Bootstrap-configured local session entrypoint |
| `simard_operator_probe` | shipped | Current engineer, meeting, goal-curation, improvement-curation, review, and compatibility bootstrap commands |
| `simard-gym` | shipped | Current benchmark CLI |

## Running from source

From the repository root, use these Cargo forms:

- `cargo run --quiet -- ...` for `simard`
- `cargo run --quiet --bin simard_operator_probe -- ...` for `simard_operator_probe`
- `cargo run --quiet --bin simard-gym -- ...` for `simard-gym`

## [PLANNED] Command tree

```text
simard
|- engineer
|  |- run <topology> <workspace-root> <objective> [state-root]
|  `- terminal <topology> <structured-objective>
|- meeting
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- goal-curation
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- improvement-curation
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- gym
|  |- list
|  |- run <scenario-id>
|  `- run-suite <suite-id>
|- review
|  |- run <base-type> <topology> <objective> [state-root]
|  `- read <base-type> <topology> [state-root]
`- bootstrap
   `- run <identity> <base-type> <topology> <objective> [state-root]
```

## Mode summary

| Planned namespace | Current runnable surface | Purpose |
| --- | --- | --- |
| `engineer run` | `simard_operator_probe engineer-loop-run ...` | Inspect, plan, act, and verify on a local repo through a bounded engineering loop |
| `engineer terminal` | `simard_operator_probe terminal-run ...` | Drive the terminal-backed engineer substrate directly |
| `meeting run` | `simard_operator_probe meeting-run ...` | Capture decisions, risks, next steps, and optional goal updates without editing code |
| `goal-curation run` | `simard_operator_probe goal-curation-run ...` | Maintain durable backlog records and the active top 5 goals |
| `improvement-curation run` | `simard_operator_probe improvement-curation-run ...` | Promote approved review proposals into durable priorities |
| `gym ...` | `simard-gym ...` | Run benchmark scenarios and suites |
| `review ...` | `simard_operator_probe review-run ...` and `review-read ...` | Persist or read the latest review artifact tied to durable state |
| `bootstrap run` | `simard` with `SIMARD_*` env vars, or `simard_operator_probe bootstrap-run ...` | Assemble an explicit runtime selection and print the reflected startup summary |

## `engineer`

### [PLANNED] `simard engineer run <topology> <workspace-root> <objective> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe engineer-loop-run <topology> <workspace-root> <objective> [state-root]`.

**Parameters**:

| Parameter | Required | Description |
| --- | --- | --- |
| `topology` | Yes | Runtime topology for this engineer run. The bounded local-first path is expected to use an honestly supported topology such as `single-process`. |
| `workspace-root` | Yes | Path to the repository or workspace Simard should inspect. The path must exist and remain inside the selected workspace boundary. |
| `objective` | Yes | Human task text for the bounded run. |
| `state-root` | No | Durable state directory for goals, evidence, review artifacts, and carried meeting records. |

**Behavior**:

- inspects the selected repo before acting
- prints a short action plan and explicit verification steps
- chooses one bounded local action
- verifies the result explicitly
- persists durable evidence and memory under `state-root` when provided
- surfaces active goals and up to the three most recent carried meeting records from the same state root

**Structured edit contract**:

The default engineer path is a read-only local scan. To request the narrow structured edit path, the objective must contain all of these lines:

- `edit-file: <repo-relative path>`
- `replace: <existing text>`
- `with: <replacement text>`
- `verify-contains: <required post-edit text>`

That edit path is intentionally narrow:

- the repo must start clean
- the target path must stay inside the selected repo
- only one expected changed file is allowed
- verification must prove both file content and git-visible change state

**Current runnable example**:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-engineer.XXXXXX)"
ENGINEER_OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

**Planned unified equivalent**:

```bash
simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

### [PLANNED] `simard engineer terminal <topology> <structured-objective>`

Planned unified entrypoint. Today, use `simard_operator_probe terminal-run <topology> <structured-objective>`.

**Current runnable example**:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  terminal-run single-process \
  $'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'
```

**Planned unified equivalent**:

```bash
simard engineer terminal single-process \
  $'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'
```

## `meeting`

### [PLANNED] `simard meeting run <base-type> <topology> <structured-objective> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe meeting-run <base-type> <topology> <structured-objective> [state-root]`.

Supported structured lines include:

- `agenda: ...`
- `update: ...`
- `decision: ...`
- `risk: ...`
- `next-step: ...`
- `open-question: ...`
- `goal: title | priority=1 | status=active | rationale=...`

A concise meeting record is persisted when the structured objective contains persistable outputs such as updates, decisions, risks, next steps, open questions, or structured goals.

**Current runnable example**:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-meeting.XXXXXX)"
MEETING_OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard workstream
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
EOF
)"

cargo run --quiet --bin simard_operator_probe -- \
  meeting-run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

**Planned unified equivalent**:

```bash
simard meeting run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

## `goal-curation`

### [PLANNED] `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe goal-curation-run <base-type> <topology> <structured-objective> [state-root]`.

Supported structured lines include:

- `goal: title | priority=1 | status=active|proposed|paused|completed | rationale=...`

**Current runnable example**:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  goal-curation-run local-harness single-process \
  "$(cat <<'EOF'
goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is a shipped product responsibility
goal: Preserve meeting-to-engineer continuity | priority=2 | status=active | rationale=meeting outputs should shape later engineer sessions
EOF
)" \
  "$STATE_ROOT"
```

**Planned unified equivalent**:

```bash
simard goal-curation run local-harness single-process "...structured objective..." "$STATE_ROOT"
```

## `improvement-curation`

### [PLANNED] `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe improvement-curation-run <base-type> <topology> <structured-objective> [state-root]`.

Supported structured lines include:

- `approve: proposal title | priority=1 | status=active|proposed | rationale=...`
- `defer: proposal title | rationale=...`

**Current runnable example**:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  improvement-curation-run local-harness single-process \
  "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
defer: Add autonomous cross-repo execution | rationale=that would overrun the current bounded product contract
EOF
)" \
  "$STATE_ROOT"
```

**Planned unified equivalent**:

```bash
simard improvement-curation run local-harness single-process "...structured objective..." "$STATE_ROOT"
```

## `gym`

### [PLANNED] `simard gym list`

Planned unified entrypoint. Today, use `simard-gym list`.

### [PLANNED] `simard gym run <scenario-id>`

Planned unified entrypoint. Today, use `simard-gym run <scenario-id>`.

### [PLANNED] `simard gym run-suite <suite-id>`

Planned unified entrypoint. Today, use `simard-gym run-suite <suite-id>`.

**Current runnable example**:

```bash
cargo run --quiet --bin simard-gym -- run-suite starter
```

**Planned unified equivalent**:

```bash
simard gym run-suite starter
```

Artifacts are written under `target/simard-gym/`.

## `review`

### [PLANNED] `simard review run <base-type> <topology> <objective> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe review-run <base-type> <topology> <objective> [state-root]`.

### [PLANNED] `simard review read <base-type> <topology> [state-root]`

Planned unified entrypoint. Today, use `simard_operator_probe review-read <base-type> <topology> [state-root]`.

**Current runnable example**:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  review-run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"

cargo run --quiet --bin simard_operator_probe -- \
  review-read local-harness single-process "$STATE_ROOT"
```

**Planned unified equivalent**:

```bash
simard review run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"

simard review read local-harness single-process "$STATE_ROOT"
```

## `bootstrap`

### [PLANNED] `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Planned unified entrypoint. Today, the real `simard` binary bootstraps directly from `SIMARD_*` environment variables. `simard_operator_probe bootstrap-run ...` exists as a compatibility helper when you want future-style positional arguments.

**Parameters**:

| Parameter | Required | Description |
| --- | --- | --- |
| `identity` | Yes | Identity to load, such as `simard-engineer`, `simard-meeting`, `simard-goal-curator`, `simard-improvement-curator`, `simard-gym`, or `simard-composite-engineer`. |
| `base-type` | Yes | Base type to request. Unsupported or unregistered values fail explicitly. |
| `topology` | Yes | Runtime topology to request. Unsupported topology or base-type pairings fail explicitly. |
| `objective` | Yes | Objective text passed into the bootstrapped run. |
| `state-root` | No | Durable state directory for the bootstrapped run. |

**Current runnable example**:

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_IDENTITY=simard-engineer \
SIMARD_BASE_TYPE=local-harness \
SIMARD_RUNTIME_TOPOLOGY=single-process \
SIMARD_OBJECTIVE="verify current reflection metadata" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
cargo run --quiet --
```

**Compatibility helper**:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-engineer local-harness single-process \
  "verify current reflection metadata"
```

**Planned unified equivalent**:

```bash
simard bootstrap run simard-engineer local-harness single-process \
  "verify current reflection metadata" \
  "$PWD/target/simard-state"
```

## Configuration

### Environment variables

| Variable | Used by | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | current `simard` bootstrap entrypoint | Yes for explicit bootstrap from source | none | Root directory for prompt assets. |
| `SIMARD_BOOTSTRAP_MODE` | current `simard` bootstrap entrypoint | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_STATE_ROOT` | current `simard` bootstrap entrypoint and runtime internals | No when builtin defaults are explicitly selected | none in `explicit-config`; `target/simard-state` in `builtin-defaults` | Durable root for memory, goals, evidence, handoff snapshots, and review artifacts. |
| `SIMARD_IDENTITY` | current `simard` bootstrap entrypoint | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults` | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Requested identity. |
| `SIMARD_BASE_TYPE` | current `simard` bootstrap entrypoint | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults` | none in `explicit-config`; `local-harness` in `builtin-defaults` | Requested base type. |
| `SIMARD_RUNTIME_TOPOLOGY` | current `simard` bootstrap entrypoint | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults` | none in `explicit-config`; `single-process` in `builtin-defaults` | Requested topology. |
| `SIMARD_OBJECTIVE` | current `simard` bootstrap entrypoint | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults` | none in `explicit-config`; `bootstrap the Simard engineer loop` in `builtin-defaults` | Objective text for the bootstrapped run. |

### Base types and topology constraints

| Base type selection | Current backend identity | Supported topologies in this scaffold |
| --- | --- | --- |
| `local-harness` | `local-harness` | `single-process` |
| `terminal-shell` | `terminal-shell::local-pty` | `single-process` |
| `rusty-clawd` | `rusty-clawd::session-backend` | `single-process`, `multi-process` |
| `copilot-sdk` | `local-harness` | `single-process` |

Notes:

- `terminal-shell` is an engineer-only local terminal path
- unsupported topology and base-type pairs fail explicitly instead of degrading silently
- `copilot-sdk` remains an explicit alias of the local harness implementation in this scaffold

## Current-to-planned command mappings

| Planned unified command | Current runnable command |
| --- | --- |
| `simard engineer run <topology> <workspace-root> <objective> [state-root]` | `simard_operator_probe engineer-loop-run <topology> <workspace-root> <objective> [state-root]` |
| `simard engineer terminal <topology> <structured-objective>` | `simard_operator_probe terminal-run <topology> <structured-objective>` |
| `simard meeting run <base-type> <topology> <structured-objective> [state-root]` | `simard_operator_probe meeting-run <base-type> <topology> <structured-objective> [state-root]` |
| `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]` | `simard_operator_probe goal-curation-run <base-type> <topology> <structured-objective> [state-root]` |
| `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]` | `simard_operator_probe improvement-curation-run <base-type> <topology> <structured-objective> [state-root]` |
| `simard review run <base-type> <topology> <objective> [state-root]` | `simard_operator_probe review-run <base-type> <topology> <objective> [state-root]` |
| `simard review read <base-type> <topology> [state-root]` | `simard_operator_probe review-read <base-type> <topology> [state-root]` |
| `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]` | `SIMARD_PROMPT_ROOT=... SIMARD_IDENTITY=... SIMARD_BASE_TYPE=... SIMARD_RUNTIME_TOPOLOGY=... SIMARD_OBJECTIVE=... [SIMARD_STATE_ROOT=...] simard` |
| `simard gym list` | `simard-gym list` |
| `simard gym run <scenario-id>` | `simard-gym run <scenario-id>` |
| `simard gym run-suite <suite-id>` | `simard-gym run-suite <suite-id>` |

## Operator-visible errors

Simard fails explicitly for these common operator-facing cases:

- unsupported top-level command on the current compatibility binaries
- missing required positional argument, reported as `expected <arg>`
- unsupported base type for the selected identity
- unsupported topology for the selected base type
- missing or invalid workspace root
- nested-worktree or repo-root drift detected during engineer-mode execution
- structured edit requested on a dirty repo
- state root outside the allowed filesystem boundary
- missing latest review artifact for `improvement-curation` or `review read`
- missing required bootstrap environment for the current `simard` entrypoint

The current binaries do not silently fall back to another mode, another base type, another topology, or another executable. The planned unified CLI should preserve that same failure model.

## See also

- [Runtime contracts reference](./runtime-contracts.md)
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [How to carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)
