---
title: How to inspect the durable goal register
description: Use `simard goal-curation read` to inspect the stored durable goal register across `active`, `proposed`, `paused`, and `completed` under one state root.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../tutorials/run-your-first-local-session.md
  - ./carry-meeting-decisions-into-engineer-sessions.md
---

# How to inspect the durable goal register

Use this guide to inspect the full stored goal ledger, not just the active top-five summary that `simard goal-curation run ...` prints after a curation update.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally
- [ ] You have a state root you want to inspect, or you are willing to create one for this walkthrough

## 1. Pick one explicit state root

The read workflow only sees durable state stored under the selected root.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-goal-register.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Populate realistic goal state through the public CLI

If you already have a durable state root, reuse it. If not, create one through `goal-curation run` with a realistic mix of goal statuses.

```bash
GOAL_OBJECTIVE="$(cat <<'EOF'
goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is a shipped product responsibility
goal: Preserve meeting-to-engineer continuity | priority=2 | status=active | rationale=meeting outputs should shape later engineer sessions
goal: Promote benchmark drift alerts | priority=3 | status=proposed | rationale=operators should see drift candidates before they become active work
goal: Expand multi-process orchestration carefully | priority=4 | status=paused | rationale=the architecture matters, but current local reliability is still more urgent
goal: Ship the canonical bootstrap contract | priority=5 | status=completed | rationale=bootstrap no longer depends on a hidden environment-only bootstrap
EOF
)"

cargo run --quiet --   goal-curation run local-harness single-process   "$GOAL_OBJECTIVE"   "$STATE_ROOT"
```

Look for output like this:

```text
Identity: simard-goal-curator
Active goals count: 2
Active goal 1: p1 [active] Keep Simard's top 5 goals current
Active goal 2: p2 [active] Preserve meeting-to-engineer continuity
```

That mutation path still focuses on curation plus the active top-five summary. It does not replace the dedicated read workflow below.

## 3. Read the durable goal register

```bash
cargo run --quiet --   goal-curation read local-harness single-process "$STATE_ROOT"
```

The output shape is:

```text
Goal register: durable
Selected base type: local-harness
Topology: single-process
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

The important contract is structural:

- the command is read-only
- sections always appear in `active`, `proposed`, `paused`, `completed` order
- the output is grouped for operator inspection rather than raw file browsing
- when `[state-root]` is omitted, the command reuses the same canonical durable root that `goal-curation run` writes for the validated runtime pairing
- persisted goal text is sanitized at render time so terminal escape/control sequences are not replayed
- the stored goal ledger is read from the same validated `state-root` used by other durable Simard flows

## 4. Read an empty register honestly

```bash
EMPTY_STATE_ROOT="$(mktemp -d /tmp/simard-goal-register-empty.XXXXXX)"

cargo run --quiet --   goal-curation read local-harness single-process "$EMPTY_STATE_ROOT"
```

The empty-state output is:

```text
Goal register: durable
Selected base type: local-harness
Topology: single-process
State root: /tmp/simard-goal-register-empty.XXXXXX
Active goals count: 0
Active goals: <none>
Proposed goals count: 0
Proposed goals: <none>
Paused goals count: 0
Paused goals: <none>
Completed goals count: 0
Completed goals: <none>
```

This is the honest zero-state contract. Empty durable state is visible as empty durable state, not inferred success.

## 5. Configuration rules that matter

For predictable future goal-register inspection, keep these rules in mind:

- pass the exact same explicit `state-root` you used for earlier `meeting`, `goal-curation`, or `improvement-curation` activity when you want to inspect that stored ledger
- if you omit `[state-root]`, keep the same shipped `base-type` and `topology` pairing you used for `goal-curation run` so Simard resolves the same canonical durable root
- keep using a supported operator runtime pairing such as `local-harness single-process` unless your environment intentionally exercises another shipped base type
- use `goal-curation run` when you want to curate or reprioritize durable goal state
- use `goal-curation read` when you want to inspect durable goal state without mutating it
- expect invalid `state-root` values, unreadable storage, and malformed durable goal data to fail explicitly rather than silently rendering an empty register

## 6. Troubleshoot the common failure shapes

### You only see active goals

That usually means the stored register only contains active records so far. `goal-curation read` is still working as designed if it prints the other sections with `0` and `<none>`.

### The output is empty, but you expected stored goals

Usually one of these is true:

- you passed a different `STATE_ROOT` than the one used to curate or carry goals earlier
- the selected state root is valid but has no persisted goal ledger yet
- earlier durable state came from another workspace or another temporary directory

### The command fails instead of printing a register

That is the intended contract for invalid or unreadable operator state. Fix the selected `state-root` or the underlying storage rather than assuming Simard should guess.

## Related reading

- For the exact command tree and example syntax, see [Simard CLI reference](../reference/simard-cli.md).
- For the executable contract behind the read and run paths, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For a broader learning flow that also exercises engineer, meeting, review, and benchmark modes, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
