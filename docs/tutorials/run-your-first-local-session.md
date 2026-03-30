---
title: "Tutorial: Run your first local session"
description: Exercise the current local-session flows through today's binaries and understand how they map onto the planned unified `simard` CLI.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
---

# Tutorial: Run your first local session

This tutorial exercises the local-session flows as they exist today, then maps each step to the unified `simard` CLI the product architecture intends to ship.

## Status

Today:

- `simard` is the bootstrap entrypoint only
- `simard_operator_probe` is the current way to run engineer, meeting, goal-curation, improvement-curation, and review flows
- the unified `simard engineer ...` and `simard meeting ...` forms are planned, not shipped

## What you'll learn

- how to run the bounded engineer loop against a local repo today
- how meeting mode carries durable decision context into later engineer runs
- how goal curation and improvement curation reuse the same durable state root
- how the current binaries map onto the planned unified CLI

## Prerequisites

- Rust and Cargo installed
- a shell in the repository root
- a clean working tree if you want to exercise the structured edit path later

All runnable examples below use Cargo so they match the current executable surface exactly.

## Step 1: Create one explicit durable state root

Use one state root for the whole tutorial so later steps can read the same meeting, goal, evidence, and review state.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-local-session.XXXXXX)"
```

## Step 2: Run engineer mode through today's compatibility binary

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Mode: engineer
Repo root: /path/to/repo
Active goals count: 0
Execution scope: local-only
Action plan: Inspect the repo ...
Selected action: cargo-metadata-scan
Verification status: verified
```

**Planned unified equivalent**:

```bash
simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

**Checkpoint**: Simard inspected the repo, chose one bounded action, and verified the result. The contract is already real even though the unified command name is still pending.

## Step 3: Capture a meeting record in the same state root

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

cargo run --quiet --bin simard_operator_probe -- \
  meeting-run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Mode: meeting
Identity: simard-meeting
Decision records: 1
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
```

**Planned unified equivalent**:

```bash
simard meeting run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

**Checkpoint**: the meeting run persisted one concise decision record and durable goal updates, but it did not mutate the repository.

## Step 4: Re-run engineer mode and confirm carryover

Use the same repo and the same state root again.

```bash
cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

This time, look for lines like these:

```text
Mode: engineer
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
Active goal 2: p2 [active] Keep outside-in verification strong
Carried meeting decisions: 1
Verification status: verified
```

**Planned unified equivalent**:

```bash
simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

**Checkpoint**: meeting mode and engineer mode now share durable planning context through one explicit state root.

## Step 5: Curate durable goals directly

You can also update the goal register without running a meeting first.

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

Look for:

- `Mode: goal-curation`
- `Identity: simard-goal-curator`
- `Active goals count: 2`
- `Active goal 1: p1 [active] Keep Simard's top 5 goals current`

**Planned unified equivalent**:

```bash
simard goal-curation run local-harness single-process "...structured objective..." "$STATE_ROOT"
```

**Checkpoint**: durable backlog stewardship is its own operator-visible mode, not an engineer-loop side effect.

## Step 6: Generate a review artifact and promote one approved improvement

First persist the latest review artifact:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  review-run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"
```

Then curate explicit approvals into durable priorities:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  improvement-curation-run local-harness single-process \
  "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
approve: Promote this pattern into a repeatable benchmark | priority=2 | status=proposed | rationale=carry this into the next benchmark planning pass
EOF
)" \
  "$STATE_ROOT"
```

Look for:

- `Mode: improvement-curation`
- `Identity: simard-improvement-curator`
- `Approved proposals: 2`
- `Active goal 1: p1 [active] Capture denser execution evidence`
- `Proposed goal 1: p2 [proposed] Promote this pattern into a repeatable benchmark`

**Planned unified equivalent**:

```bash
simard improvement-curation run local-harness single-process "...structured objective..." "$STATE_ROOT"
```

**Checkpoint**: reviewed evidence is now feeding durable priorities through the same runtime contract the unified CLI will expose.

## Step 7: Know where bootstrap and gym fit today

Today, bootstrap already lives on `simard`, while benchmark execution still lives on `simard-gym`.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults cargo run --quiet --
cargo run --quiet --bin simard-gym -- list
```

Planned unified equivalents:

```bash
simard bootstrap run simard-engineer local-harness single-process "bootstrap the Simard engineer loop"
simard gym list
```

## Summary

You now know how to:

- run the current engineer flow through `simard_operator_probe`
- carry meeting decisions into later engineer runs
- curate durable goals directly
- turn review findings into durable improvement priorities
- distinguish the current binaries from the unified CLI Simard is still building toward

## Next steps

- Use [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md) when you need the current bootstrap contract in more detail.
- Use [How to carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md) when you need a narrower handoff-focused workflow.
- Use [Simard CLI reference](../reference/simard-cli.md) when you need the exact current-to-planned command mapping.
