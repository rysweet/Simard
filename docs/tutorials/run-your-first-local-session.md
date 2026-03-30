---
title: "Tutorial: Run your first local session"
description: Exercise the shipped local-session flows through the canonical `simard` CLI and understand where the legacy compatibility binaries still fit.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../howto/inspect-meeting-records.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
---

# Tutorial: Run your first local session

This tutorial exercises the shipped local-session flows through the canonical `simard` CLI.

## Status

Today:

- `simard` is the primary operator-facing CLI
- `simard_operator_probe` remains available for older compatibility scripts
- `simard-gym` remains available for compatibility with legacy benchmark scripts, although `simard gym ...` is the canonical benchmark surface

## What you'll learn

- how to run the bounded engineer loop against a local repo
- how meeting mode carries durable decision context into later engineer runs
- how goal curation, review, and improvement curation reuse explicit durable state roots
- how bootstrap and benchmark flows fit into the same operator-facing CLI story

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

## Step 2: Run engineer mode through the canonical CLI

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

cargo run --quiet --   engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Repo root: /path/to/repo
Active goals count: 0
Execution scope: local-only
Action plan: Inspect the repo ...
Selected action: cargo-metadata-scan
Verification status: verified
```

**Checkpoint**: Simard inspected the repo, chose one bounded action, and verified the result.

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

## Step 4: Re-run engineer mode and confirm carryover

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

## Step 5: Curate durable goals directly

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

## Step 6: Generate a review artifact, curate one approval and one deferral, then read the stored improvement state

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

## Step 7: Exercise bootstrap and the terminal-backed engineer substrate

Bootstrap and benchmark execution both live on the canonical CLI:

```bash
cargo run --quiet --   bootstrap run simard-engineer local-harness single-process   "bootstrap the Simard engineer loop"   "$STATE_ROOT"

cargo run --quiet -- gym list
```

The terminal-backed engineer substrate now lives on the canonical CLI too:

```bash
cargo run --quiet --   engineer terminal single-process   $'working-directory: .
command: pwd
command: printf "terminal-foundation-ok\n"'   "$STATE_ROOT"
```

Look for:

- `Adapter implementation: terminal-shell::local-pty`
- `Terminal steps count: 2`
- `Terminal step 1: input: pwd`
- `Terminal last output line: terminal-foundation-ok`
- `Terminal transcript preview:`

If you want to keep a reusable interactive session recipe on disk instead of packing it into one CLI string, use the file-backed entrypoint:

```bash
cat > /tmp/simard-terminal.recipe <<'EOF'
working-directory: .
command: printf "terminal-file-ready\n"
wait-for: terminal-file-ready
input: printf "terminal-file-ok\n"
EOF

cargo run --quiet -- \
  engineer terminal-file single-process /tmp/simard-terminal.recipe "$STATE_ROOT"
```

Simard also ships named built-in terminal recipes when you want reusable sessions without managing your own temp file:

```bash
cargo run --quiet -- engineer terminal-recipe-list
cargo run --quiet -- engineer terminal-recipe-show foundation-check
cargo run --quiet -- engineer terminal-recipe single-process foundation-check "$STATE_ROOT"
```

## Summary

You now know how to:

- run the shipped engineer flow through `simard`
- carry meeting decisions into later engineer runs
- curate durable goals directly
- turn review findings into durable improvement priorities
- keep compatibility binaries reserved for older scripts or exact legacy output

## Next steps

- Use [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md) when you need the bootstrap contract in more detail.
- Use [How to carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md) when you need a narrower handoff-focused workflow.
- Use [How to inspect meeting records](../howto/inspect-meeting-records.md) when you need the read-only meeting audit flow.
- Use [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md) when you need the read-only review-to-priority audit flow.
- Use [Simard CLI reference](../reference/simard-cli.md) when you need the exact command tree and compatibility mapping.
