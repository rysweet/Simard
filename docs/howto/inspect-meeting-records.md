---
title: How to inspect meeting records
description: Use `simard meeting read` to inspect the latest durable meeting record without mutating stored planning state.
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

# How to inspect meeting records

This guide shows how to use `simard meeting read <base-type> <topology> [state-root]` as the read-only audit companion to `meeting run`.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally
- [ ] You are willing to use one explicit state root for `meeting run` and `meeting read`

## 1. Pick one explicit state root

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-meeting-read.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Persist a real meeting record first

```bash
MEETING_OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard workstream
update: durable memory foundation merged in PR 29
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
EOF
)"

cargo run --quiet -- \
  meeting run local-harness single-process \
  "$MEETING_OBJECTIVE" \
  "$STATE_ROOT"
```

Look for output like:

```text
Identity: simard-meeting
Decision records: 1
Active goals count: 1
Active goal 1: p1 [active] Preserve meeting handoff
```

## 3. Read the latest durable meeting record

```bash
cargo run --quiet -- \
  meeting read local-harness single-process "$STATE_ROOT"
```

Expected output shape:

```text
Probe mode: meeting-read
Identity: simard-meeting
Selected base type: local-harness
Topology: single-process
State root: /tmp/simard-meeting-read.XXXXXX
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

The important contract is structural:

- the command is read-only
- it reads the latest persisted meeting decision record from the selected validated `state-root`
- sections always appear in this order: agenda, updates, decisions, risks, next steps, open questions, goal updates, latest raw meeting record
- empty sections render explicit `0` and `<none>` lines instead of disappearing
- persisted meeting text is sanitized before printing so stored terminal control sequences are not replayed
- when `[state-root]` is omitted, the command reuses the same canonical durable root that `meeting run` writes for the validated runtime pairing

## 4. Configuration rules that matter

- pass the exact same explicit `state-root` you used for `meeting run` when you want to inspect that stored planning record later
- if you omit `[state-root]`, keep the same shipped `base-type` and `topology` pairing you used for `meeting run` so Simard resolves the same canonical durable root
- use `meeting run` when you want to capture new durable planning state
- use `meeting read` when you want to inspect the latest durable meeting state without mutating it
- expect invalid `state-root` values, missing memory state, and malformed persisted meeting records to fail explicitly rather than silently rendering a synthetic summary

## 5. Troubleshoot the common failure shapes

### The command fails because no persisted meeting record exists

That usually means one of these is true:

- you passed a different `STATE_ROOT` than the one used for `meeting run`
- the selected state root is valid but no meeting record was persisted there yet
- you expected Simard to synthesize meeting state from current goals alone

Run `meeting run` first against the same root, then use `meeting read`.

### The command fails before any record is printed

That is the intended contract for invalid or unreadable operator state. Fix the selected `state-root` or the underlying storage rather than assuming Simard should guess.

## Related reading

- For the exact command tree and example syntax, see [Simard CLI reference](../reference/simard-cli.md).
- For the executable contract behind the read and run paths, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For the wider learning flow that also exercises engineer, goal-curation, review, and improvement-curation modes, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
- For human-readable markdown exports (as opposed to JSON transcripts), see [How to export meeting markdown](./export-meeting-markdown.md).
