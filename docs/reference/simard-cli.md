---
title: Simard CLI reference
description: Reference for the shipped `simard` command tree and the legacy compatibility binaries that still expose the same runtime behaviors.
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

# Simard CLI reference

`simard` is the canonical operator-facing CLI.

The legacy `simard_operator_probe` and `simard-gym` binaries still ship for compatibility, but new operator workflows should use `simard ...`.

## Command tree

```text
simard
|- engineer
|  `- run <topology> <workspace-root> <objective> [state-root]
|- meeting
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- goal-curation
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- improvement-curation
|  `- run <base-type> <topology> <structured-objective> [state-root]
|- gym
|  |- list
|  |- run <scenario-id>
|  |- compare <scenario-id>
|  `- run-suite <suite-id>
|- review
|  |- run <base-type> <topology> <objective> [state-root]
|  `- read <base-type> <topology> [state-root]
`- bootstrap
   `- run <identity> <base-type> <topology> <objective> [state-root]
```

Bare `simard` prints this unified help surface.

## Compatibility mapping

| Canonical command | Compatibility surface |
| --- | --- |
| `simard engineer run ...` | `simard_operator_probe engineer-loop-run ...` |
| `simard meeting run ...` | `simard_operator_probe meeting-run ...` |
| `simard goal-curation run ...` | `simard_operator_probe goal-curation-run ...` |
| `simard improvement-curation run ...` | `simard_operator_probe improvement-curation-run ...` |
| `simard review run ...` | `simard_operator_probe review-run ...` |
| `simard review read ...` | `simard_operator_probe review-read ...` |
| `simard bootstrap run ...` | `simard_operator_probe bootstrap-run ...` |
| `simard gym ...` | `simard-gym ...` |

## Shared state-root contract

When a command accepts `[state-root]`, Simard validates it before any persistence write or read that depends on durable operator state.

Rejected inputs include:

- any path containing `..`
- an existing path that is not a directory
- a symlink root

Safe state roots are canonicalized once and then reused for the rest of the command.

## Mode reference

### `simard engineer run <topology> <workspace-root> <objective> [state-root]`

Runs the bounded local engineer loop against the selected repository.

Key behavior:

- inspects the selected repo before acting
- prints the chosen bounded action and explicit verification steps
- persists memory, evidence, and latest handoff under `state-root`
- surfaces active goals and carried meeting decisions from the same durable state

Example:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-engineer.XXXXXX)"
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

### `simard meeting run <base-type> <topology> <structured-objective> [state-root]`

Captures decisions, risks, next steps, open questions, and optional goal updates without editing code.

Supported structured lines include:

- `agenda: ...`
- `update: ...`
- `decision: ...`
- `risk: ...`
- `next-step: ...`
- `open-question: ...`
- `goal: title | priority=1 | status=active | rationale=...`

Example:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-meeting.XXXXXX)"
MEETING_OBJECTIVE="$(cat <<'EOF2'
agenda: align the next Simard workstream
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
EOF2
)"

simard meeting run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

### `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`

Maintains durable backlog records and the active top five goals.

Supported structured lines include:

- `goal: title | priority=1 | status=active|proposed|paused|completed | rationale=...`

Example:

```bash
simard goal-curation run local-harness single-process   "goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is a shipped product responsibility"   "$STATE_ROOT"
```

### `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`

Promotes approved review proposals into durable priorities.

Supported structured lines include:

- `approve: proposal title | priority=1 | status=active|proposed | rationale=...`
- `defer: proposal title | rationale=...`

Example:

```bash
simard improvement-curation run local-harness single-process   "approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now"   "$STATE_ROOT"
```

### `simard gym list`

Lists the shipped benchmark scenarios.

### `simard gym run <scenario-id>`

Runs one benchmark scenario and prints the generated artifact paths.

### `simard gym compare <scenario-id>`

Compares the latest two completed runs for the selected scenario and prints both source report paths plus a persisted comparison artifact.

The comparison contract is intentionally explicit:

- it fails visibly if fewer than two completed runs exist for the scenario
- it classifies the latest run as `improved`, `unchanged`, or `regressed`
- it writes JSON and text comparison artifacts under `target/simard-gym/comparisons/<scenario-id>/`

### `simard gym run-suite <suite-id>`

Runs a benchmark suite.

Artifacts are written under `target/simard-gym/`.

### `simard review run <base-type> <topology> <objective> [state-root]`

Builds and persists the latest review artifact tied to the selected durable state.

### `simard review read <base-type> <topology> [state-root]`

Reads back the latest persisted review artifact from the selected durable state.

Example:

```bash
simard review run local-harness single-process   "inspect the current Simard review surface and preserve concrete proposals"   "$STATE_ROOT"

simard review read local-harness single-process "$STATE_ROOT"
```

### `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Bootstraps an explicit runtime selection from positional CLI arguments. This is the only supported bootstrap entrypoint on the canonical CLI surface; the old zero-argument environment-only fallback is gone.

Example:

```bash
simard bootstrap run simard-engineer local-harness single-process   "verify current reflection metadata"   "$PWD/target/simard-state"
```

## Running from source

From the repository root, use these Cargo forms:

- `cargo run --quiet -- ...` for `simard`
- `cargo run --quiet --bin simard_operator_probe -- ...` for `simard_operator_probe`
- `cargo run --quiet --bin simard-gym -- ...` for `simard-gym`

## Base types and topology constraints

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

## Operator-visible errors

Simard fails explicitly for these common operator-facing cases:

- unsupported top-level command
- missing required positional argument, reported as `expected <arg>`
- invalid `state-root`
- unsupported base type for the selected identity
- unsupported topology for the selected base type
- missing or invalid workspace root
- nested-worktree or repo-root drift detected during engineer-mode execution
- structured edit requested on a dirty repo
