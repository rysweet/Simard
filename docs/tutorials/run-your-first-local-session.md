---
title: "Tutorial: Run your first local session"
description: Exercise the shipped local-session flows through the canonical `simard` CLI and review where the bounded `engineer copilot-submit` slice fits before continuing into the repo-grounded engineer loop through one explicit state root.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../howto/move-from-terminal-recipes-into-engineer-runs.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../howto/inspect-meeting-records.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
---

# Tutorial: Run your first local session

This tutorial exercises the shipped local-session flows through the canonical `simard` CLI.

The first half focuses on the honest bridge from bounded terminal recipes into the repo-grounded engineer loop. It now includes the shipped bounded `engineer copilot-submit` slice so you can see where that stricter one-shot Copilot handoff fits before the later repo-grounded engineer run. The later steps show how meeting, goal, review, improvement, bootstrap, and gym surfaces still fit into the same local operator story.

Use `simard` as the canonical operator-facing CLI. `simard_operator_probe` and `simard-gym` remain compatibility surfaces for older scripts, and `engineer terminal*` plus `engineer run/read` still share one honest local state model while remaining separate operator-visible modes.

## What you'll learn

- how to run the bounded engineer loop against a local repo
- how to start from a discoverable terminal recipe, run the bounded `engineer copilot-submit` flow through the same explicit `state-root`, and then continue into the engineer loop
- how meeting mode carries durable decision context into later engineer runs
- how goal curation, review, and improvement curation reuse explicit durable state roots
- how bootstrap and benchmark flows fit into the same operator-facing CLI story

## Prerequisites

- Rust and Cargo installed
- a shell in the repository root
- `amplihack` available on `PATH` if you want to exercise the Copilot slices
- a clean working tree if you want to exercise the structured edit path later

All runnable examples below use Cargo so they match the current executable surface exactly.

## Step 1: Create one explicit durable state root

Use one state root for the whole tutorial so later steps can read the same meeting, goal, evidence, and review state.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-local-session.XXXXXX)"
```

## Step 2: Discover the shipped terminal recipe surface

Start on the bounded local terminal surface, not the engineer loop.

```bash
cargo run --quiet -- engineer terminal-recipe-list
cargo run --quiet -- engineer terminal-recipe-show copilot-prompt-check
cargo run --quiet -- engineer terminal-recipe-show copilot-status-check
```

Look for:

- `copilot-prompt-check`
- `copilot-status-check`
- a real bounded recipe asset with `working-directory:`, `command:`, and `wait-for:` lines
- `copilot-prompt-check` should show a real `amplihack copilot` launch plus a bounded `/exit`, while `copilot-status-check` remains the narrower `--version` probe
- the `copilot-submit` flow is intentionally not part of `terminal-recipe-list`; it stays a dedicated command because it submits exactly one checked-in payload and nothing else

**Checkpoint**: you can discover and inspect the shipped prompt-only Copilot probes without claiming repo-grounded planning, task submission, or verification happened yet.

## Step 3: Run the shipped bounded Copilot prompt slice

```bash
cargo run --quiet -- \
  engineer terminal-recipe single-process copilot-prompt-check "$STATE_ROOT"
```

Look for:

- `Probe mode: terminal-run`
- `Mode boundary: terminal`
- the visible prompt guidance line `Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts`
- the visible resume hint `Resume any session with copilot --resume`
- `Next step 1: run 'simard engineer run <topology> <workspace-root> <objective> <same-state-root>'`

**Checkpoint**: Simard launched the real local `amplihack copilot` session, waited for the visible prompt guidance text, exited cleanly through the bounded prompt-check recipe, persisted truthful terminal artifacts, and showed an explicit next-step path into the engineer loop without pretending task submission happened.

## Step 4: Read back the stored terminal session

```bash
cargo run --quiet -- \
  engineer terminal-read single-process "$STATE_ROOT"
```

Look for:

- `Probe mode: terminal-read`
- `Terminal handoff source: latest_terminal_handoff.json`
- `Mode boundary: terminal`
- `Terminal last output line:`
- `Terminal transcript preview:`
- `Next step 1: run 'simard engineer run <topology> <workspace-root> <objective> <same-state-root>'`

**Checkpoint**: the terminal readback stays read-only, and the bridge guidance plus prompt-check audit are coming from durable local state rather than from a hidden resume system.

## Step 5: Run the bounded `engineer copilot-submit` slice

`engineer copilot-submit` sits between the prompt-check step above and the repo-grounded engineer run below as a stricter one-shot local submit path.

```bash
cargo run --quiet -- \
  engineer copilot-submit single-process "$STATE_ROOT" --json
```

The contract is intentionally narrow:

- `success` is reserved for a future checked-in flow where Simard can truthfully observe a real post-submit checkpoint after sending the fixed payload
- today, the honest local result is usually `unsupported`: the visible Copilot UI can require folder trust confirmation, emit wrapper noise before the prompt, or surface the visible `ctrl+s run command` submit hint that this line-input PTY path cannot drive truthfully
- `runtime-failure` is reserved for Simard-side failures such as invalid inputs, local launch failures, or persistence/readback errors before a trustworthy submit summary can be claimed
- the command still submits one checked-in fixed payload only, and it must not accept arbitrary task text, inspect remote auth state, create or reuse worktrees, or claim general Copilot orchestration

## Step 6: Continue into engineer mode through the same state root

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

cargo run --quiet --   engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Mode boundary: engineer
Repo root: /path/to/repo
Terminal continuity available: yes
Terminal continuity source: latest_terminal_handoff.json
Terminal continuity last output line: <line from the prior terminal session>
Action plan: Inspect the repo ...
Selected action: cargo-metadata-scan
Verification status: verified
```

**Checkpoint**: Simard inspected the repo, preserved the v1 engineer contract, and rendered the earlier terminal continuity as descriptive context only.

## Step 7: Read back the engineer audit trail

```bash
cargo run --quiet -- \
  engineer read single-process "$STATE_ROOT"
```

Look for:

- `Probe mode: engineer-read`
- `Engineer handoff source: latest_engineer_handoff.json`
- `Mode boundary: engineer`
- `Terminal continuity available: yes`
- `Terminal continuity source: latest_terminal_handoff.json`
- `Verification status: verified`

**Checkpoint**: `engineer read` prefers the engineer-scoped handoff, keeps the terminal continuity section separate, and never replays raw objective text.

## Step 8: Capture a meeting record in the same state root

```bash
MEETING_OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard workstream
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
goal: Keep outside-in verification strong | priority=2 | status=active | rationale=operator confidence depends on real product exercise
EOF
)"

cargo run --quiet --   meeting run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Identity: simard-meeting
Decision records: 1
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
```

**Checkpoint**: the meeting run persisted one concise decision record and durable goal updates, but it did not mutate the repository.

If you want to inspect the stored meeting state directly before moving on, run:

```bash
cargo run --quiet -- \
  meeting read local-harness single-process "$STATE_ROOT"
```

Look for:

- `Probe mode: meeting-read`
- `Latest agenda: align the next Simard workstream`
- `Decision 1: preserve meeting-to-engineer continuity`
- `Goal update 1: p1 [active] Preserve meeting handoff`

## Step 9: Re-run engineer mode and confirm carryover

Use the same repo and the same state root again.

```bash
cargo run --quiet --   engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

This time, look for lines like these:

```text
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
Active goal 2: p2 [active] Keep outside-in verification strong
Carried meeting decisions: 1
Verification status: verified
```

**Checkpoint**: meeting mode and engineer mode now share durable planning context through one explicit state root.

## Step 10: Curate durable goals directly

You can also update the goal register without running a meeting first.

```bash
cargo run --quiet --   goal-curation run local-harness single-process   "$(cat <<'EOF'
goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is a shipped product responsibility
goal: Preserve meeting-to-engineer continuity | priority=2 | status=active | rationale=meeting outputs should shape later engineer sessions
EOF
)"   "$STATE_ROOT"
```

Look for:

- `Identity: simard-goal-curator`
- `Active goals count: 2`
- `Active goal 1: p1 [active] Keep Simard's top 5 goals current`

**Checkpoint**: durable backlog stewardship is its own operator-visible mode, not an engineer-loop side effect.

## Step 11: Generate a review artifact, curate one approval and one deferral, then read the stored improvement state

First persist the latest review artifact:

```bash
cargo run --quiet --   review run local-harness single-process   "inspect the current Simard review surface and preserve concrete proposals"   "$STATE_ROOT"
```

Then curate explicit approvals into durable priorities:

```bash
cargo run --quiet --   improvement-curation run local-harness single-process   "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
defer: Promote this pattern into a repeatable benchmark | rationale=hold this until the next benchmark planning pass
EOF
)"   "$STATE_ROOT"
```

Look for:

- `Identity: simard-improvement-curator`
- `Approved proposals: 1`
- `Deferred proposals: 1`
- `Active goal 1: p1 [active] Capture denser execution evidence`

**Checkpoint**: reviewed evidence is now feeding durable priorities, and deferred proposals stay in durable state instead of vanishing into session output.

Now read the durable audit state through the same public CLI:

```bash
cargo run --quiet -- \
  improvement-curation read local-harness single-process "$STATE_ROOT"
```

Look for:

- `Probe mode: improvement-curation-read`
- `Latest review artifact:`
- `Deferred proposal 1: Promote this pattern into a repeatable benchmark (hold this until the next benchmark planning pass)`
- `Latest improvement record: review=`

## Step 12: Exercise bootstrap and benchmark discovery

Bootstrap and benchmark execution both live on the canonical CLI:

```bash
cargo run --quiet --   bootstrap run simard-engineer local-harness single-process   "bootstrap the Simard engineer loop"   "$STATE_ROOT"

cargo run --quiet -- gym list
```

## Summary

You now know how to:

- discover a shipped terminal recipe on the canonical CLI
- run a bounded local terminal session and read back its truthful audit trail
- continue from that terminal surface into the repo-grounded engineer loop through the same explicit `state-root`
- run the shipped engineer flow through `simard`
- carry meeting decisions into later engineer runs
- curate durable goals directly
- turn review findings into durable improvement priorities
- keep compatibility binaries reserved for older scripts or exact legacy output

## Next steps

- Use [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md) when you need the bootstrap contract in more detail.
- Use [How to move from terminal recipes into engineer runs](../howto/move-from-terminal-recipes-into-engineer-runs.md) when you want the narrow terminal-to-engineer bridge workflow only.
- Use [How to carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md) when you need a narrower handoff-focused workflow.
- Use [How to inspect meeting records](../howto/inspect-meeting-records.md) when you need the read-only meeting audit flow.
- Use [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md) when you need the read-only review-to-priority audit flow.
- Use [Simard CLI reference](../reference/simard-cli.md) when you need the exact command tree and compatibility mapping.
