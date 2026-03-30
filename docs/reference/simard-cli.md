---
title: Simard CLI reference
description: Reference for the shipped `simard` command tree, the `engineer read` audit companion, and the legacy compatibility binaries that still expose selected older runtime behaviors.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./runtime-contracts.md
  - ../howto/inspect-meeting-records.md
  - ../howto/inspect-durable-goal-register.md
  - ../howto/inspect-improvement-curation-state.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../tutorials/run-your-first-local-session.md
---

# Simard CLI reference

`simard` is the canonical operator-facing CLI.

The legacy `simard_operator_probe` and `simard-gym` binaries still ship for compatibility, but new operator workflows should use `simard ...`.

This page documents the shipped operator-facing command tree. When a compatibility surface is listed as `none`, the command is canonical-only.

## Command tree

```text
simard
|- engineer
|  |- run <topology> <workspace-root> <objective> [state-root]
|  |- terminal <topology> <objective> [state-root]
|  `- read <topology> [state-root]
|- meeting
|  |- run <base-type> <topology> <structured-objective> [state-root]
|  `- read <base-type> <topology> [state-root]
|- goal-curation
|  |- run <base-type> <topology> <structured-objective> [state-root]
|  `- read <base-type> <topology> [state-root]
|- improvement-curation
|  |- run <base-type> <topology> <structured-objective> [state-root]
|  `- read <base-type> <topology> [state-root]
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

Bare `simard` prints this operator surface directly.

## Compatibility mapping

| Canonical command | Compatibility surface |
| --- | --- |
| `simard engineer run ...` | `simard_operator_probe engineer-loop-run ...` |
| `simard engineer terminal ...` | `simard_operator_probe terminal-run ...` |
| `simard engineer terminal-file ...` | `simard_operator_probe terminal-run-file ...` |
| `simard engineer terminal-read ...` | `simard_operator_probe terminal-read ...` |
| `simard engineer read ...` | `simard_operator_probe engineer-read ...` |
| `simard meeting run ...` | `simard_operator_probe meeting-run ...` |
| `simard meeting read ...` | `simard_operator_probe meeting-read ...` |
| `simard goal-curation run ...` | `simard_operator_probe goal-curation-run ...` |
| `simard goal-curation read ...` | none |
| `simard improvement-curation run ...` | `simard_operator_probe improvement-curation-run ...` |
| `simard improvement-curation read ...` | `simard_operator_probe improvement-curation-read ...` |
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

### `simard engineer read <topology> [state-root]`

This is the read-only audit companion to `simard engineer run`. It inspects the latest persisted engineer state without resuming execution, repairing artifacts, or re-running the engineer loop.

Behavior:

- reuses the same canonical default durable root as `engineer run` when `[state-root]` is omitted
- validates `topology` before deriving that default root, so the default still follows the shipped engineer runtime pairing
- requires any explicit `state-root` to already exist as a directory
- requires `latest_handoff.json`, `memory_records.json`, and `evidence_records.json` to already exist as readable regular files; symlinked artifacts are rejected
- treats `latest_handoff.json` as authoritative for session identity, selected base type, topology, session phase, redacted objective metadata, and the exported memory/evidence snapshot tied to the latest engineer run
- requires the persisted handoff session objective to already be trusted `objective-metadata(chars=<n>, words=<n>, lines=<n>)`; malformed or tampered metadata fails instead of being replayed
- uses the standalone `memory_records.json` and `evidence_records.json` files as durability checks and supporting evidence counts; if they disagree with the handoff snapshot, the handoff-derived values win
- renders only redacted objective metadata such as `objective-metadata(chars=150, words=21, lines=1)`, never the raw engineer objective text
- requires carried meeting state to remain valid persisted meeting records; malformed carried-meeting data fails instead of being downgraded to raw strings
- strips terminal control sequences and secret-shaped values from every displayed string before printing it
- prints a stable operator-visible order: runtime header, handoff session summary, repo grounding, carried context, selected action summary, verification summary, durable record counts
- fails explicitly for invalid `state-root` values and for missing, unreadable, or malformed persisted engineer state

When `[state-root]` is omitted, the command reuses the same canonical durable root that `engineer run` already writes:

```text
target/operator-probe-state/engineer-loop-run/simard-engineer/terminal-shell/<topology>
```

Example:

```bash
simard engineer read single-process "$STATE_ROOT"
```

Output shape:

```text
Probe mode: engineer-read
Identity: simard-engineer
Selected base type: terminal-shell
Topology: single-process
State root: /tmp/simard-engineer.XXXXXX
Session phase: complete
Objective metadata: objective-metadata(chars=150, words=21, lines=1)
Repo root: /path/to/repo
Repo branch: main
Repo head: 4b6cb7de0179e9adb480dfdea1cb2aee4a5d5e18
Worktree dirty: false
Changed files: <none>
Active goals count: 1
Active goal 1: p1 [active] Preserve meeting handoff
Carried meeting decisions: 1
Carried meeting decision 1: preserve meeting-to-engineer continuity
Selected action: cargo-metadata-scan
Action plan: Inspect the repo, query Cargo metadata without mutating files, and verify repo grounding stayed stable.
Verification steps: confirm cargo metadata returns valid workspace JSON || confirm repo root, branch, HEAD, and worktree state stayed stable || confirm carried meeting decisions and active goals stayed stable
Action status: success
Changed files after action: <none>
Verification status: verified
Verification summary: Verified local-only engineer action 'cargo-metadata-scan' against stable repo grounding, unchanged worktree state, and explicit repo-native action checks.
Memory records: 3
Evidence records: 19
```

### `simard engineer terminal <topology> <objective> [state-root]`

Runs the terminal-backed engineer substrate on the canonical CLI instead of
requiring the legacy probe binary.

Key behavior:

- selects the `terminal-shell` base type explicitly
- accepts bounded terminal objectives with `command:`/`input:` lines plus `wait-for:` or `expect:` checkpoints so a run can pause for expected output before sending the next line
- preserves truthful adapter reflection and now renders the terminal audit trail directly on the run surface, including ordered terminal steps, satisfied checkpoints, the last visible output line, and a sanitized transcript preview
- fails visibly for unsupported topology and invalid state-root inputs
- fails explicitly if a requested wait checkpoint never appears instead of pretending the terminal interaction succeeded
- keeps `simard_operator_probe terminal-run ...` available for compatibility

Example:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-terminal.XXXXXX)"

simard engineer terminal single-process $'working-directory: .
command: printf "terminal-foundation-ready\n"
wait-for: terminal-foundation-ready
command: printf "terminal-foundation-ok\n"' "$STATE_ROOT"
```

### `simard engineer terminal-file <topology> <objective-file> [state-root]`

Runs the same bounded terminal-backed engineer substrate, but loads the session recipe from a reusable UTF-8 text file instead of requiring the whole objective inline on the command line.

Behavior:

- reuses the same `terminal-shell` base type and bounded wait/send terminal semantics as `engineer terminal`
- requires `<objective-file>` to exist as a readable regular file; symlinks and non-files fail explicitly
- preserves the same structured terminal audit trail on the run surface and through `terminal-read`
- keeps `simard_operator_probe terminal-run-file ...` available for compatibility

Example:

```bash
cat > /tmp/simard-terminal.recipe <<'EOF'
working-directory: .
command: printf "terminal-file-ready\n"
wait-for: terminal-file-ready
input: printf "terminal-file-ok\n"
EOF

simard engineer terminal-file single-process /tmp/simard-terminal.recipe "$STATE_ROOT"
```

### `simard engineer terminal-read <topology> [state-root]`

This is the read-only audit companion to `simard engineer terminal`. It inspects the latest persisted terminal session state without replaying commands or resuming the PTY session.

Behavior:

- reuses the same canonical default durable root as `engineer terminal` when `[state-root]` is omitted
- requires any explicit `state-root` to already exist as a directory
- requires `latest_handoff.json`, `memory_records.json`, and `evidence_records.json` to already exist as readable regular files; symlinked artifacts are rejected
- treats `latest_handoff.json` as authoritative for session identity, selected base type, topology, session phase, redacted objective metadata, and the persisted terminal evidence summary
- renders terminal shell, working directory, command count, wait count, ordered terminal steps, satisfied wait checkpoints, last output line, and transcript preview in stable operator-visible order
- strips terminal control sequences and secret-shaped values from displayed output before printing it
- fails explicitly for invalid `state-root` values and for missing, unreadable, or malformed persisted terminal state

When `[state-root]` is omitted, the command reuses the same canonical durable root that `engineer terminal` already writes:

```text
target/operator-probe-state/terminal-run/simard-engineer/terminal-shell/<topology>
```

Example:

```bash
simard engineer terminal-read single-process "$STATE_ROOT"
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

### `simard meeting read <base-type> <topology> [state-root]`

Reads the latest durable meeting record without mutating it.

Key behavior:

- loads the latest persisted meeting decision record from the validated `state-root`
- reuses the same canonical default durable root as `meeting run` when `[state-root]` is omitted
- validates `base-type` and `topology` before deriving that default root
- requires explicit read-layout inputs before probing: the state root itself must already exist as a directory and `memory_records.json` must already be present
- prints sections in this fixed order: latest agenda, updates, decisions, risks, next steps, open questions, goal updates, latest meeting record
- includes explicit zero-state lines for empty update, decision, risk, next-step, open-question, and goal-update sections
- strips terminal control sequences from persisted meeting text before printing it
- preserves `meeting run` as the only meeting-state mutation workflow
- fails explicitly for invalid `state-root` values and for missing, unreadable, or malformed persisted meeting state

Example:

```bash
simard meeting read local-harness single-process "$STATE_ROOT"
```

Output shape:

```text
Probe mode: meeting-read
Identity: simard-meeting
Selected base type: local-harness
Topology: single-process
State root: /tmp/simard-meeting.XXXXXX
Meeting records: 1
Latest agenda: align the next Simard workstream
Updates count: 1
Update 1: durable memory foundation merged in PR 29
Decisions count: 1
Decision 1: preserve meeting-to-engineer continuity
Risks count: 1
Risk 1: workflow routing is still unreliable
Next steps count: 1
Next step 1: keep durable priorities visible
Open questions count: 1
Open question 1: how aggressively should Simard reprioritize?
Goal updates count: 1
Goal update 1: p1 [active] Preserve meeting handoff
Latest meeting record: agenda=align the next Simard workstream; ...
```

### `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`

Maintains durable backlog records and the active top five goals.

Supported structured lines include:

- `goal: title | priority=1 | status=active|proposed|paused|completed | rationale=...`

Example:

```bash
simard goal-curation run local-harness single-process   "goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is a shipped product responsibility"   "$STATE_ROOT"
```

`goal-curation run` is the mutation path. It curates durable goal state and still surfaces the active top-five summary for quick operator feedback.

When `[state-root]` is omitted, `goal-curation run` writes under the canonical durable root for the selected shipped runtime pairing:

```text
target/operator-probe-state/goal-curation-run/simard-goal-curator/<base-type>/<topology>
```

### `simard goal-curation read <base-type> <topology> [state-root]`

Reads the stored durable goal register without mutating it.

Key behavior:

- loads the stored goal register from the validated `state-root`
- reuses the same canonical default durable root as `goal-curation run` when `[state-root]` is omitted
- validates `base-type` and `topology` before deriving that default root
- prints sections in this fixed order: `active`, `proposed`, `paused`, `completed`
- includes explicit zero-state lines for empty sections
- strips terminal control sequences from persisted goal text before printing it
- preserves `goal-curation run` as the only curation workflow
- fails explicitly for invalid `state-root` values and unreadable or malformed durable goal state

Example:

```bash
simard goal-curation read local-harness single-process "$STATE_ROOT"
```

You should see output shaped like:

```text
Goal register: durable
State root: /tmp/simard-goal-register.XXXXXX
Active goals count: 2
Active goal 1: p1 [active] Keep Simard's top 5 goals current
Active goal 2: p2 [active] Preserve meeting-to-engineer continuity
Proposed goals count: 1
Proposed goal 1: p3 [proposed] Promote benchmark drift alerts
Paused goals count: 1
Paused goal 1: p4 [paused] Expand multi-process orchestration carefully
Completed goals count: 1
Completed goal 1: p5 [completed] Ship the canonical bootstrap contract
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

When `[state-root]` is omitted, `improvement-curation run` reuses the same canonical durable root that `review run` uses for the validated runtime pairing:

```text
target/operator-probe-state/review-run/simard-engineer/<base-type>/<topology>
```

### `simard improvement-curation read <base-type> <topology> [state-root]`

Reads the latest durable improvement-curation state without mutating it.

Key behavior:

- loads the latest persisted review artifact from the validated `state-root`, where "latest" means the review artifact with the highest `reviewed_at_unix_ms`
- loads the latest persisted improvement-curation decision record from the same root, where "latest" means the last decision memory record whose key ends with `improvement-curation-record`
- reuses the same canonical default durable root as `review run` and `improvement-curation run` when `[state-root]` is omitted
- validates `base-type` and `topology` before deriving that default root
- requires explicit read-layout inputs before probing: the state root itself must already exist as a directory, `review-artifacts/` must exist, and both `memory_records.json` and `goal_records.json` must already be present
- prints sections in this fixed order: latest review metadata, approved proposals, deferred proposals, active goals, proposed goals, latest improvement record
- includes explicit zero-state lines for empty approved, deferred, active-goal, and proposed-goal sections
- strips terminal control sequences from persisted proposal titles, rationales, goal text, review metadata, and decision records before printing them
- preserves `improvement-curation run` as the only curation workflow
- fails explicitly for invalid `state-root` values and for missing, unreadable, or malformed persisted review or improvement state

Example:

```bash
simard review run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"

simard improvement-curation run local-harness single-process \
  "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
defer: Promote this pattern into a repeatable benchmark | rationale=hold this until the next benchmark planning pass
EOF
)" \
  "$STATE_ROOT"

simard improvement-curation read local-harness single-process "$STATE_ROOT"
```

Output shape:

```text
Probe mode: improvement-curation-read
Identity: simard-improvement-curator
Selected base type: local-harness
Topology: single-process
State root: /tmp/simard-improvement-curation.XXXXXX
Latest review artifact: /tmp/simard-improvement-curation.XXXXXX/review_artifacts/review-....json
Review id: review-...
Review target: operator-review
Review proposals: 2
Approved proposals: 1
Approved proposal 1: p1 [active] Capture denser execution evidence
Deferred proposals: 1
Deferred proposal 1: Promote this pattern into a repeatable benchmark (hold this until the next benchmark planning pass)
Active goals count: 1
Active goal 1: p1 [active] Capture denser execution evidence
Proposed goals count: 0
Proposed goals: <none>
Latest improvement record: review=review-... target=operator-review approvals=[p1 [active] Capture denser execution evidence] deferred=[Promote this pattern into a repeatable benchmark (hold this until the next benchmark planning pass)]
```

### `simard gym list`

Lists the shipped benchmark scenarios.

### `simard gym run <scenario-id>`

Runs one benchmark scenario and prints the operator-facing text report for that run.

Key behavior today:

- persists `report.json`, `report.txt`, and `review.json` under `target/simard-gym/<scenario-id>/<session-id>/`
- preserves exact operator-visible output parity with `simard-gym run <scenario-id>`
- requires no extra configuration beyond the selected scenario id

The current counting boundary is:

- `unnecessary_action_count`: benchmark-runner-observed benchmark-controlled action boundaries beyond the single scenario execution path required by the current v1 harness
- `retry_count`: benchmark-runner-observed re-attempts of the same scenario work inside one benchmark run

Fresh runs now persist values derived from those benchmark-controlled facts under `scorecard.unnecessary_action_count` and `scorecard.retry_count`, surface them through the CLI, and stop emitting fresh review proposals, `human_review_notes`, or `measurement_notes` that claim those fields are "not measured". Older or incomplete artifacts should surface `unmeasured` instead of fabricated zeroes.

Example:

```bash
cargo run --quiet -- gym run repo-exploration-local
```

You should see output shaped like:

```text
Scenario: repo-exploration-local
Suite: starter
Session: session-...
Passed: true
Checks passed: 8/8
Unnecessary actions: 0
Retry count: 0
Artifact report: target/simard-gym/repo-exploration-local/.../report.json
Artifact summary: target/simard-gym/repo-exploration-local/.../report.txt
Review artifact: target/simard-gym/repo-exploration-local/.../review.json
```

The detailed per-run text artifact at `Artifact summary:` also includes the identity, base type, topology, plan, execution summary, reflection summary, and the same metric lines.

### `simard gym compare <scenario-id>`

Compares the latest two completed runs for the selected scenario and prints both source report paths plus a persisted comparison artifact.

The comparison contract is intentionally explicit:

- it fails visibly if fewer than two completed runs exist for the scenario
- it classifies the latest run as `improved`, `unchanged`, or `regressed`
- it writes JSON and text comparison artifacts under `target/simard-gym/comparisons/<scenario-id>/`
- it preserves exact operator-visible output parity with `simard-gym compare <scenario-id>`
- it reports current, previous, and delta values for `unnecessary_action_count` and `retry_count`
- it validates the scenario id against the shipped benchmark registry before reading any scenario directory
- those metric lines render `unmeasured` explicitly when either compared artifact predates the new measurements instead of fabricating `0`

Example:

```bash
cargo run --quiet -- gym compare repo-exploration-local
```

You should see output shaped like:

```text
Scenario: repo-exploration-local
Comparison status: unchanged
Comparison summary: latest run matched session '...' on pass/fail status and checks, with unnecessary-action delta +0, retry delta +0, memory delta +0, and evidence delta +0
Current session: ...
Current passed: true
Current checks passed: 8/8
Current report: target/simard-gym/repo-exploration-local/.../report.json
Current unnecessary actions: 0
Current retry count: 0
Previous session: ...
Previous passed: true
Previous checks passed: 8/8
Previous report: target/simard-gym/repo-exploration-local/.../report.json
Previous unnecessary actions: 0
Previous retry count: 0
Delta correctness checks passed: +0
Delta unnecessary actions: +0
Delta retry count: +0
Delta exported memory records: +0
Delta exported evidence records: +0
Comparison artifact report: target/simard-gym/comparisons/repo-exploration-local/.../report.json
Comparison artifact summary: target/simard-gym/comparisons/repo-exploration-local/.../report.txt
```

Only comparisons that involve older artifacts should show `unmeasured` for those metric lines.

### `simard gym run-suite <suite-id>`

Runs a benchmark suite.

Artifacts are written under `target/simard-gym/`.

Each scenario run within the suite emits the same scorecard fields as `simard gym run <scenario-id>`, so single-run reports and suite-generated reports remain directly comparable.

## Benchmark gym configuration

The benchmark metric reporting surface does not require feature flags or environment variables.

The public operator contract is:

- pass a scenario id to `simard gym run <scenario-id>` or `simard gym compare <scenario-id>`
- pass a suite id to `simard gym run-suite <suite-id>`
- read artifacts from the default output root `target/simard-gym/`
- expect current reports to preserve exact parity with `simard-gym` today
- expect fresh reports to include `scorecard.unnecessary_action_count` and `scorecard.retry_count`
- expect comparisons against legacy reports to remain readable through explicit `unmeasured` output

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
- missing persisted review state for `review read`
- nested-worktree or repo-root drift detected during engineer-mode execution
- structured edit requested on a dirty repo
