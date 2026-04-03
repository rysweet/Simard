---
title: Simard CLI reference
description: Reference for the shipped `simard` command tree, the shared-state-root bridge between terminal sessions and the repo-grounded engineer loop, the `engineer read` audit companion, the shipped bounded `engineer copilot-submit` contract, and the legacy compatibility binaries that still expose selected older runtime behaviors.
last_updated: 2026-04-03
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
  - ../howto/move-from-terminal-recipes-into-engineer-runs.md
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
|  |- read <topology> [state-root]
|  |- terminal <topology> <objective> [state-root]
|  |- terminal-file <topology> <objective-file> [state-root]
|  |- terminal-recipe-list
|  |- terminal-recipe-show <recipe-name>
|  |- terminal-recipe <topology> <recipe-name> [state-root]
|  |- copilot-submit <topology> [state-root] [--json]
|  `- terminal-read <topology> [state-root]
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
|  `- run <identity> <base-type> <topology> <objective> [state-root]
|- ooda
|  `- run [--cycles=N] [state-root]
|- act-on-decisions
|- spawn <agent-name> <goal> <worktree-path>
|- handover [--canary-dir=PATH]
|- update
`- install
```

Bare `simard` prints this operator surface directly.

## Self-management commands

### `simard update`

Self-update the binary to the latest GitHub release. Downloads the release asset matching the current platform and replaces the running binary.

### `simard install`

Install the Simard binary to `~/.simard/bin`. Used by the npx wrapper (`npx github:rysweet/Simard install`) to persist the binary for direct CLI use.

## Compatibility mapping

| Canonical command | Compatibility surface |
| --- | --- |
| `simard engineer run ...` | `simard_operator_probe engineer-loop-run ...` |
| `simard engineer terminal ...` | `simard_operator_probe terminal-run ...` |
| `simard engineer terminal-file ...` | `simard_operator_probe terminal-run-file ...` |
| `simard engineer terminal-recipe ...` | `simard_operator_probe terminal-recipe-run ...` |
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

Shipped terminal surface: `simard engineer copilot-submit <topology> [state-root] [--json]`. It has no compatibility surface and is documented later on this page as the bounded one-shot local Copilot submission contract.

## Shared state-root contract

When a command accepts `[state-root]`, Simard validates it before any persistence write or read that depends on durable operator state.

Rejected inputs include:

- any path containing `..`
- an existing path that is not a directory
- a symlink root

Safe state roots are canonicalized once and then reused for the rest of the command.

## Terminal-to-engineer bridge

The `simard engineer ...` namespace now exposes two distinct shipped operator-visible surfaces:

- `engineer terminal`, `engineer terminal-file`, `engineer terminal-recipe`, and `engineer terminal-read` are bounded local terminal session surfaces
- `engineer run` and `engineer read` are the repo-grounded engineer loop and its read-only audit companion

`engineer copilot-submit` now sits on the terminal-session side of that boundary as a stricter one-shot local Copilot submission surface.

The bridge between them is explicit and local-only:

- reuse the same explicit `state-root`
- inspect the persisted terminal summary through `terminal-read`
- invoke `engineer run` explicitly with a real `workspace-root` and engineer objective

Simard writes mode-scoped handoffs under the shared root:

- `latest_terminal_handoff.json`
- `latest_engineer_handoff.json`
- `latest_handoff.json` as the compatibility fallback

Readback is fail-closed:

- `terminal-read` prefers `latest_terminal_handoff.json`
- `engineer read` prefers `latest_engineer_handoff.json`
- fallback to `latest_handoff.json` happens only when the mode-scoped file is absent
- if a mode-scoped handoff exists but is malformed, the command fails instead of silently replaying older data

The bridge is descriptive continuity only. It does not auto-resume, auto-launch engineer mode, infer a repo path, or replace the engineer loop's inspect -> plan -> act -> verify contract.

## Mode reference

### `simard engineer run <topology> <workspace-root> <objective> [state-root]`

Runs the repo-grounded bounded engineer loop against the selected repository.

Key behavior:

- inspects the selected repo before acting
- prints the chosen bounded action and explicit verification steps
- may render a separate terminal continuity section when the same `state-root` already contains a valid terminal-scoped handoff
- persists memory, evidence, `latest_engineer_handoff.json`, and compatibility `latest_handoff.json` under `state-root`
- surfaces active goals and carried meeting decisions from the same durable state
- keeps terminal continuity descriptive only; it does not override `workspace-root`, the engineer objective, planning, or verification

Example:

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-engineer.XXXXXX)"
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

To continue from a terminal recipe or terminal session, reuse the same `STATE_ROOT`. The terminal bridge stays local and explicit; Simard does not infer the engineer objective for you.

### `simard engineer read <topology> [state-root]`

This is the read-only audit companion to `simard engineer run`. It inspects the latest persisted engineer state without resuming execution, repairing artifacts, or re-running the engineer loop.

Behavior:

- reuses the same canonical default durable root as `engineer run` when `[state-root]` is omitted
- validates `topology` before deriving that default root, so the default still follows the shipped engineer runtime pairing
- requires any explicit `state-root` to already exist as a directory
- requires `memory_records.json` and `evidence_records.json` to already exist as readable regular files; symlinked artifacts are rejected
- prefers `latest_engineer_handoff.json` as the authoritative engineer readback artifact and falls back to `latest_handoff.json` only when the engineer-scoped handoff is absent
- prints which handoff artifact was used so mode-scoped readback stays operator-visible
- requires the persisted handoff session objective to already be trusted `objective-metadata(chars=<n>, words=<n>, lines=<n>)`; malformed or tampered metadata fails instead of being replayed
- uses the standalone `memory_records.json` and `evidence_records.json` files as durability checks and supporting evidence counts; if they disagree with the handoff snapshot, the handoff-derived values win
- renders only redacted objective metadata such as `objective-metadata(chars=150, words=21, lines=1)`, never the raw engineer objective text
- requires carried meeting state to remain valid persisted meeting records; malformed carried-meeting data fails instead of being downgraded to raw strings
- when the same state root contains a valid terminal-scoped handoff, renders a separate terminal continuity section with sanitized terminal summary fields
- strips terminal control sequences and secret-shaped values from every displayed string before printing it
- prints a stable operator-visible order: runtime header, handoff session summary, repo grounding, carried context, terminal continuity, selected action summary, verification summary, durable record counts
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
Engineer handoff source: latest_engineer_handoff.json
Identity: simard-engineer
Selected base type: terminal-shell
Topology: single-process
State root: /tmp/simard-engineer.XXXXXX
Session phase: complete
Objective metadata: objective-metadata(chars=150, words=21, lines=1)
Mode boundary: engineer
Repo root: /path/to/repo
Repo branch: main
Repo head: 4b6cb7de0179e9adb480dfdea1cb2aee4a5d5e18
Worktree dirty: false
Changed files: <none>
Active goals count: 1
Active goal 1: p1 [active] Preserve meeting handoff
Carried meeting decisions: 1
Carried meeting decision 1: preserve meeting-to-engineer continuity
Terminal continuity available: yes
Terminal continuity source: latest_terminal_handoff.json
Terminal recipe source: foundation-check
Terminal working directory: .
Terminal last output line: terminal-recipe-ok
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

Runs one bounded local terminal session on the canonical CLI instead of requiring the legacy probe binary.

This is not the repo-grounded engineer loop. It is the honest terminal-session surface that operators can use before deciding to launch `engineer run`.

Key behavior:

- selects the `terminal-shell` base type explicitly
- accepts bounded terminal objectives with `command:`/`input:` lines plus `wait-for:` or `expect:` checkpoints so a run can pause for expected output before sending the next line
- preserves truthful adapter reflection and now renders the terminal audit trail directly on the run surface, including ordered terminal steps, observed checkpoints, the last visible output line, and a sanitized transcript preview
- prints explicit next-step guidance for continuing into `engineer run` with the same `state-root`
- persists `latest_terminal_handoff.json` and compatibility `latest_handoff.json` under the shared root
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
- preserves the same structured terminal audit trail, mode-scoped terminal handoff, and engineer-next-step guidance as `engineer terminal`
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

### `simard engineer terminal-recipe-list`

Lists the built-in named terminal session recipes shipped under `prompt_assets/simard/terminal_recipes/`.

### `simard engineer terminal-recipe-show <recipe-name>`

Prints the selected built-in terminal recipe asset and its bounded session contents before execution.

### `simard engineer terminal-recipe <topology> <recipe-name> [state-root]`

Runs one of the built-in named terminal session recipes through the same bounded PTY-backed substrate as `engineer terminal` and `engineer terminal-file`.

Behavior:

- loads a named recipe from `prompt_assets/simard/terminal_recipes/*.simard-terminal`
- currently ships `foundation-check` for the minimal bounded PTY sanity path, `copilot-status-check` for a bounded local Copilot wrapper availability probe, and `copilot-prompt-check` for a bounded real interactive prompt-start-and-exit path; the stricter one-shot task submission slice is the dedicated `engineer copilot-submit` command documented below
- preserves the same structured terminal audit trail, mode-scoped terminal handoff, and engineer-next-step guidance as the other terminal session surfaces
- fails explicitly when the requested recipe name is unknown or invalid
- `copilot-status-check` is intentionally narrow: it only runs the fixed local argv `amplihack copilot -- --version`
- `copilot-status-check` does not inspect GitHub auth state, does not open an interactive Copilot session, and fails closed when `amplihack` is missing or the expected version signal is absent
- `copilot-prompt-check` is the first truthful interactive Copilot slice: it starts `amplihack copilot`, waits for the real prompt guidance text, sends `/exit`, and waits for the resume hint before succeeding
- `copilot-prompt-check` still does not submit a task, inspect auth state, or claim general Copilot orchestration beyond prompt reachability and clean exit
- keeps `simard_operator_probe terminal-recipe-run ...` available for compatibility

Example:

```bash
simard engineer terminal-recipe-list
simard engineer terminal-recipe-show foundation-check
simard engineer terminal-recipe-show copilot-prompt-check
simard engineer terminal-recipe-show copilot-status-check
simard engineer terminal-recipe single-process foundation-check "$STATE_ROOT"
```

### `simard engineer copilot-submit <topology> [state-root] [--json]`

This command ships as one bounded truthful local Copilot task-submission attempt that reuses the same `terminal-shell` PTY substrate as the other terminal surfaces.

Behavior:

- launch the real local argv `amplihack copilot` in the current repository context only
- use the checked-in flow contract at `prompt_assets/simard/terminal_recipes/copilot-submit.json`
- accept no `workspace-root`, no free-form objective, and no arbitrary task text; the submitted payload must stay fixed and built in
- restore workflow-only `.claude/context/PROJECT.md` and `.claude/context/PROJECT.md.bak` to their pre-launch contents when the Copilot wrapper rewrites them, so truthful terminal probing does not leave repo dirt behind
- validate `topology` and `[state-root]` with the same rules as the other terminal session surfaces; the first implementation only needs `single-process`
- require the exact ordered visible startup checkpoints from the flow asset, including `Describe a task to get started.` and the guidance line `Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts`
- submit the fixed payload once after startup checkpoints are satisfied, then observe the visible submit hint from the checked-in flow contract
- declare `success` only when the live PTY transcript satisfies a supportable checked-in post-submit contract after terminal control sequences are stripped from the visible text; the current shipped flow intentionally fails closed before claiming that because the real UI exposes `ctrl+s run command` rather than a truthful newline submission path
- return `unsupported` when the Copilot process launched but the visible prompt flow exited early, drifted, stalled after partial visible startup evidence, required folder trust confirmation, required the visible submit hotkey Simard cannot drive through this line-input PTY surface, or surfaced wrapper-specific launch errors
- reserve `runtime-failure` for Simard-side command failures such as invalid inputs, local launch failures, or persistence/readback failures before a trustworthy submit result can be claimed
- classify startup timeouts after partial visible startup evidence as `unsupported` with `missing-startup-banner`, `missing-guidance-checkpoint`, `workflow-wrapper-noise`, or `unexpected-startup-text`; only zero-evidence startup timeouts stay `runtime-failure`
- persist the same terminal-scoped handoff artifacts as the other terminal surfaces on `success` and on any `unsupported` result that captured truthful terminal evidence; a `runtime-failure` may leave partial audit data but must not invent a complete submit summary
- reserve `reason_code` for `unsupported`; `success` carries none, and `runtime-failure` remains an explicit CLI error unless the implementation later publishes a separate failure-code contract
- keep `--json` as a formatting choice only; it must not broaden capability or relax checkpoint matching
- keep `copilot-status-check` and `copilot-prompt-check` unchanged as the narrower probe surfaces
- avoid GitHub auth inspection, arbitrary slash-command support, worktree creation or reuse, and any claim of general Copilot orchestration beyond this one checked-in flow

Explicit unsupported reason codes:

- `process-exited-early`
- `unexpected-startup-text`
- `missing-startup-banner`
- `missing-guidance-checkpoint`
- `trust-confirmation-required`
- `submit-hotkey-required`
- `copilot-wrapper-error`
- `workflow-wrapper-noise`

Invocation:

```bash
simard engineer copilot-submit single-process "$STATE_ROOT"
simard engineer copilot-submit single-process "$STATE_ROOT" --json
```

The eventual operator-visible and `--json` outputs should make these facts explicit without inventing broader capability:

- the mode boundary is terminal
- the selected base type is `terminal-shell`
- the checked-in flow asset and fixed payload identifier are visible
- the final outcome is `success`, `unsupported`, or `runtime-failure`, but the current shipped flow is expected to return `unsupported` until Simard can truthfully drive the observed submit gesture
- `ordered_steps` records only the launch, waits, and fixed payload step the PTY path actually reached before the flow stopped; startup drift must not pretend the payload step ran
- any `unsupported` result carries one of the explicit reason codes above
- the ordered steps, observed checkpoints, last meaningful output line, and transcript preview remain auditable
- later `terminal-read` and `engineer run` surfaces can point back to the same terminal-scoped handoff artifact when truthful continuity exists

If you only need local wrapper availability or prompt reachability, keep using `copilot-status-check` or `copilot-prompt-check` instead.

### `simard engineer terminal-read <topology> [state-root]`

This is the read-only audit companion to `simard engineer terminal`. It inspects the latest persisted terminal session state without replaying commands or resuming the PTY session.

Behavior:

- reuses the same canonical default durable root as `engineer terminal` when `[state-root]` is omitted
- requires any explicit `state-root` to already exist as a directory
- requires `memory_records.json` and `evidence_records.json` to already exist as readable regular files; symlinked artifacts are rejected
- prefers `latest_terminal_handoff.json` as the authoritative terminal readback artifact and falls back to `latest_handoff.json` only when the terminal-scoped handoff is absent
- prints which handoff artifact was used so terminal readback stays operator-visible
- renders mode boundary, terminal shell, working directory, command count, wait count, ordered terminal steps, satisfied wait checkpoints, last output line, transcript preview, and engineer-next-step guidance in stable operator-visible order
- when `engineer copilot-submit` persisted truthful audit data, the same readback exposes the Copilot flow asset, submit outcome, fixed payload identifier, and any explicit unsupported reason code
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

Output shape:

```text
Probe mode: terminal-read
Terminal handoff source: latest_terminal_handoff.json
Mode boundary: terminal
Selected base type: terminal-shell
Topology: single-process
Terminal working directory: .
Terminal recipe source: foundation-check
Terminal steps count: 2
Terminal last output line: terminal-recipe-ok
Terminal transcript preview:
Engineer next step: simard engineer run <topology> <workspace-root> <objective> <same-state-root>
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

## OODA daemon

### `simard ooda run [--cycles=N] [state-root]`

Runs the continuous OODA (Observe-Orient-Decide-Act) daemon loop for autonomous operation. Simard observes her goal board, orients by ranking priorities, decides which actions to take, and acts by dispatching bounded work — then sleeps and repeats.

Key behavior:

- launches memory, knowledge, and gym bridges
- loads the goal board from cognitive memory
- runs OODA cycles in a loop with 60-second sleep between cycles
- `--cycles=N` limits the daemon to N cycles; `--cycles=0` or omitting the flag runs indefinitely
- logs cycle summaries to stderr: observation counts, priorities, actions dispatched, outcomes
- errors in individual actions do not abort the cycle; the daemon continues to the next phase
- state root defaults to `$SIMARD_STATE_ROOT` or `/tmp/simard-ooda` when omitted

Environment variables:

- `SIMARD_STATE_ROOT` — override the state root directory
- `SIMARD_AGENT_NAME` — override the agent name (default: `simard-ooda`)

Example:

```bash
# Run 5 cycles then exit
simard ooda run --cycles=5 "$PWD/target/simard-ooda"

# Run indefinitely as a daemon
SIMARD_STATE_ROOT="$PWD/target/simard-state" simard ooda run
```

Each cycle follows the four OODA phases:

1. **Observe** — gather goal statuses, gym health, memory statistics; degrades honestly if a bridge is unavailable (Pillar 11)
2. **Orient** — rank goals by urgency (blocked 1.0 > not-started 0.8 > in-progress scaled by remaining %). Also injects synthetic priorities: memory consolidation (urgency 0.5) when episodic count exceeds 100, and improvement cycles (urgency 0.7) when gym score drops below 70%
3. **Decide** — select up to `max_concurrent_actions` (default 3) actions from the priority list; completed goals (urgency 0) are skipped
4. **Act** — dispatch actions independently; each action produces its own outcome

Action kinds: `AdvanceGoal`, `RunImprovement`, `ConsolidateMemory`, `ResearchQuery`, `RunGymEval`, `BuildSkill`.

## Meeting handoff commands

### `simard act-on-decisions`

Reads the latest meeting handoff artifact and creates GitHub issues for each decision and action item.

Key behavior:

- loads the handoff from `target/meeting_handoffs/meeting_handoff.json`
- if no handoff exists, prints a message and exits successfully
- if the handoff is already marked as processed, prints a message and exits
- for each `MeetingDecision`: creates a GitHub issue titled `Decision: <description>` with rationale and participants in the body
- for each `ActionItem`: creates a GitHub issue titled `Action: <description>` with owner, priority, and due date in the body
- prints open questions to stdout (not filed as issues)
- marks the handoff as processed after creating all issues
- individual `gh issue create` failures are warnings; the command continues

Requires:

- `gh` CLI installed and authenticated
- a prior `simard meeting run` that produced a handoff artifact

Example:

```bash
# After closing a meeting session
simard act-on-decisions
```

Example output:

```text
Processing meeting handoff: align next workstream (closed 2026-04-02T14:30:00Z)
  Created issue for decision: adopt session builder → https://github.com/rysweet/Simard/issues/155
  Created issue for action: wire OODA to RustyClawd → https://github.com/rysweet/Simard/issues/156

Open questions (not filed as issues):
  - how aggressively should Simard reprioritize?

Done. Created 2 issue(s). Handoff marked as processed.
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
- planned Copilot submit contract should report `unsupported` with `process-exited-early`, `unexpected-startup-text`, `missing-startup-banner`, `missing-guidance-checkpoint`, `trust-confirmation-required`, `submit-hotkey-required`, `copilot-wrapper-error`, or `workflow-wrapper-noise` when the visible prompt flow drifts after launch
- structured edit requested on a dirty repo
