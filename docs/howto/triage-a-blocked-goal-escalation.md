---
title: How to triage a blocked-goal escalation (before it reaches you)
description: >
  Operator playbook for the escalation-triage behaviour (#4276 / #4904): what the
  plain-English Signal updates look like, how to confirm the Overseer
  course-corrected a stale block itself (rewrote a done-gate, completed an
  already-delivered goal, or asked you ONE specific question) instead of dumping
  raw markers, how to verify a completed goal left the board and was tombstoned,
  how to confirm no internal marker leaked, and what to do when you DO get a
  single plain-English question.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/escalation-triage-before-human.md
  - ../reference/escalation-triage-api.md
  - ./reinvestigate-bare-blocked-goals.md
  - ./unblock-stuck-ooda-goals.md
  - ./configure-overseer-signal-rpc-notifications.md
  - ./inspect-durable-goal-register.md
---

# How to triage a blocked-goal escalation (before it reaches you)

> **Status: implemented.** Present-tense operator guide. The behaviour lives in
> [`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md)
> and runs on the Overseer tick behind a thin rail. See the
> [concept](../concepts/escalation-triage-before-human.md) and the
> [API reference](../reference/escalation-triage-api.md).

When the Overseer decides a goal is blocked, it does **not** page you with a raw
machine marker. It first restates the block in plain English, tries to fix it
itself, and only asks you something when a decision is genuinely yours. This page
shows how to observe that loop and confirm it did the right thing.

## 1. Read the plain-English Signal updates

You receive a short thread on Signal (and email), one message per reasoning step.
They are jargon-free by contract. A completed-delivered-goal triage looks like:

```
Action needed — a goal is blocked in rysweet/Simard.

Problem:
  Simard's work to lift automated test coverage above 70% was recorded as
  stuck and then left alone, so it made no further progress and nobody was told.
```

```
The coverage work this goal describes has already been delivered by merged
changes — there is nothing left to build.
```

```
Done — I closed the goal. It has left the active list and can't be reopened by
accident. Nothing is needed from you.
```

You should **never** see tokens like `OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`,
`why=`, `evidence=[…]`, `blocked-terminal`, or 🔒. If you do, that is a bug —
see step 5.

## 2. Identify which course-correction was taken

The final update tells you which of three things happened:

| You see… | The Overseer decided… | Your action |
| --- | --- | --- |
| "I rewrote its finish condition to: …" | `rewrite-done-gate` — the goal's done-gate couldn't be checked automatically, so it re-scoped it to something machine-checkable. | None — the goal can now certify itself. |
| "I closed the goal — the work already shipped." | `complete-delivered-goal` — a merged PR already delivered the work. | None — verify with step 3 if you like. |
| A single, specific question. | `ask-operator-one-question` — a genuine call is yours. | Answer the one question (step 4). |

## 3. Verify a completed goal left the board and was tombstoned

For a `complete-delivered-goal` outcome, confirm the goal is gone from the active
board:

```bash
simard goal list | grep audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
```

No match means it was removed. Confirm the durable tombstone was written so
cycle-reconcile can't resurrect it (see
[inspect the durable goal register](./inspect-durable-goal-register.md)):

```bash
grep audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a \
  "${SIMARD_STATE_ROOT:-$HOME/.simard}/goal_tombstones.json"
```

And confirm the stable log line (drop `--user` for a system install):

```bash
journalctl --user -u simard-ooda -n 500 | grep 'goal complete:'
# [simard] goal complete: 'audit-…-4d27c91a' marked done, removed from board, and tombstoned
```

**Idempotent re-run.** Running `simard goal complete <id>` again on an
already-completed goal is safe — it re-records the tombstone and logs:

```
[simard] goal complete: 'audit-…-4d27c91a' not on board; recorded tombstone (idempotent)
```

> **Standing goals are never completed away.** If the id names a perpetual /
> standing goal, `complete` refuses to terminate it and rolls it to a fresh
> cycle instead (`… is a standing goal — refused to terminate; reopened it …`).
> That is expected, not an error.

## 4. If you got a single question, answer it

`ask-operator-one-question` is the only outcome that needs you. By contract it is
**exactly one** crisp, plain-English question — never a wall of jargon. Reply on
the same Signal thread. The Overseer takes your answer and applies the
corresponding course-correction on the next tick.

If you ever receive *more than one* question, or a raw diagnosis instead of a
question, that violates the contract — file it as a bug (step 5).

## 5. Confirm no internal marker leaked (and what to do if one did)

Every operator-facing string is run through a marker-scrub gate before it is
sent. To spot-check a message, scan for any forbidden token:

```bash
# none of these should ever appear in an operator message:
grep -Ei 'OODA-SAFEGUARD|UNCLEAR-CRITERIA|GENUINELY-STUCK|blocked-terminal|why=|evidence=\[|🔒'
```

If a real operator message leaks any of these, the scrub gate failed — capture
the message verbatim and open an issue referencing #4276 (the escalation must
never surface raw markers).

## 6. Confirm the escalation was real, not zero

The behaviour also fixes the "sat stuck and told no one" failure: a blocked goal
that is never escalated. After a triage tick, confirm at least one operator
notification actually went out (the health-review pass flags `escalations=0` over
24h as the trigger condition):

```bash
journalctl --user -u simard-ooda -n 500 | grep -E 'notify|goal-blocked|escalat'
```

You should see the dual-channel dispatch for the `goal-blocked` updates. Because
`goal-blocked` is a suppressible kind, identical repeats for the *same*
still-blocked goal are deduped — but the three distinct triage updates all
dispatch.

## Related

- [Concept: triage & course-correct a blocked goal](../concepts/escalation-triage-before-human.md)
- [Escalation-triage API & output contract](../reference/escalation-triage-api.md)
- [Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md)
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)
- [Configure Overseer Signal notifications](./configure-overseer-signal-rpc-notifications.md)
- [Inspect the durable goal register](./inspect-durable-goal-register.md)
