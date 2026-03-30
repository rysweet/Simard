---
title: "How to carry meeting decisions into engineer sessions"
description: Persist concise meeting records under a shared state root and verify that later engineer sessions carry them forward, using today's binaries and the planned unified CLI mapping.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ./configure-bootstrap-and-inspect-reflection.md
  - ../tutorials/run-your-first-local-session.md
---

# How to carry meeting decisions into engineer sessions

Use this guide when you want to prove one specific product seam: a meeting run captures durable meeting memory, and a later engineer run against the same state root carries that planning context forward without pretending code changed during the meeting.

## Status

The handoff behavior is real today through `simard_operator_probe`.

The future `simard meeting run ...` and `simard engineer run ...` entrypoints are planned, not shipped yet.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet --bin simard_operator_probe -- ...` works locally
- [ ] You want a local file-backed handoff, not a network service or remote orchestrator

## 1. Pick one explicit state root for both commands

The handoff only works when both runs point at the same durable state root.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-meeting-handoff.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Capture a structured meeting record through today's operator surface

Run `meeting-run` with a real structured objective. A carried meeting record is persisted when the objective includes any persistable structured output such as `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`, or structured `goal:` lines. This example uses all of them.

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
  meeting-run local-harness single-process \
  "$MEETING_OBJECTIVE" \
  "$STATE_ROOT"
```

Look for output like this:

```text
Mode: meeting
Identity: simard-meeting
Decision records: 1
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
```

Planned unified equivalent:

```bash
simard meeting run local-harness single-process "$MEETING_OBJECTIVE" "$STATE_ROOT"
```

This run writes one concise meeting record and goal state under `STATE_ROOT`. It does not run code or mutate the repository.

## 3. Reuse the same state root in a later engineer run

Now point the engineer loop at the same repository and the same `STATE_ROOT`.

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output like this:

```text
Mode: engineer
Repo root: /path/to/repo
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
Active goal 2: p2 [active] Keep outside-in verification strong
Carried meeting decisions: 1
Carried meeting decision 1: agenda=align the next Simard workstream; updates=[]; decisions=[preserve meeting-to-engineer continuity]; risks=[workflow routing is still unreliable]; next_steps=[keep durable priorities visible]; open_questions=[how aggressively should Simard reprioritize?]; goals=[p1:active:Preserve meeting handoff:meeting decisions must shape later work | p2:active:Keep outside-in verification strong:operator confidence depends on real product exercise]
Verification status: verified
```

Planned unified equivalent:

```bash
simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

The important contract is additive:

- `Active goals count` and `Active goal N` still describe the durable goal register
- `Carried meeting decisions` describes separate meeting-decision memory
- the engineer mode currently surfaces at most the three most recent persisted meeting records from that state root
- the engineer loop keeps that decision memory visible while choosing one bounded local action
- `Verification status: verified` still proves the engineer loop performed its normal inspect -> act -> verify cycle

## 4. Configuration rules that matter

For predictable handoff behavior, keep these rules in mind:

- pass the same explicit `state-root` argument to both `meeting-run` and `engineer-loop-run`
- keep `meeting-run` on a supported facilitator pairing such as `local-harness single-process`
- keep `engineer-loop-run` pointed at a real repository path for `workspace-root`
- expect the engineer loop to surface at most the three most recent carried meeting records, not an unbounded history dump
- treat carried meeting decisions as advisory context only; they do not auto-edit code or silently rewrite goals

## 5. Troubleshoot the common failure shapes

### `Carried meeting decisions: 0`

Usually one of these is true:

- the prior meeting run used a different `STATE_ROOT`
- the meeting objective only contained agenda text and no persistable structured output such as `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`, or structured `goal:` lines
- you are looking at a clean state root with no earlier meeting data

### Goals show up, but no carried decision does

That usually means the goal state came from `goal-curation` or another earlier flow rather than the shared `meeting` run, or the engineer run is reading a different state root. A meeting run that persisted structured output writes both the goal updates and a concise meeting record.

### The engineer loop ran, but nothing was verified

That is outside this feature's contract. The handoff is only complete when the later engineer run still reports `Verification status: verified`.

## Related reading

- For the broader current-to-planned command mapping, see [Simard CLI reference](../reference/simard-cli.md).
- For the broader runtime contract, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For a longer end-to-end walk through the current binaries, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
