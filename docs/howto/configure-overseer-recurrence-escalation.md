---
title: Configure and verify Overseer recurrence escalation
description: >
  Operator guide for the Overseer's recurrence dead-band escalation — how a
  perpetual, no-progress goal whose root cause has re-parked twice (recalled from
  cognitive memory) is now escalated once to the operator with the missing
  dependency named, instead of being silently re-unblocked and re-parked every
  cycle. Covers the goal-scoped recurrence counter and its two escalation floors,
  what governs the acting path, how to read the escalation in logs and the
  operator notification, how the per-goal dedup gate prevents notification spam,
  and how to reproduce and verify loop termination with injected fakes.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
status: design — not yet implemented
related:
  - ../concepts/overseer-root-cause-why.md
  - ../reference/overseer-recurrence-dead-band-escalation-api.md
  - ../reference/overseer-operator-notifications.md
  - ./configure-overseer-root-cause-why.md
  - ./configure-overseer-goal-board-health.md
  - ./configure-overseer-memory-recall.md
  - ./unblock-stuck-ooda-goals.md
---

# Configure and verify Overseer recurrence escalation

> **Status: design — not yet implemented** (issue
> [#4124](https://github.com/rysweet/Simard/issues/4124)). This runbook describes
> the behavior the implementing PR will add: the Overseer will escalate a
> perpetual, no-progress goal whose root cause has already re-parked twice,
> instead of leaving it in the `[2, 3)` recurrence dead-band where it is
> re-unblocked and re-parked forever. The implementation and this documentation
> land in the same pull request.

## What changes, in one sentence

A perpetual, no-progress goal whose root cause has been recalled **twice** from
cognitive memory — such as
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`
— will be **escalated once** to the operator with the missing dependency named,
rather than being silently re-unblocked and re-emitting the identical signature
on the next cycle.

## Background: one recurrence counter, two escalation floors

The Overseer tracks how often a blocked goal's **root cause** re-occurs with a
single, **goal-scoped** counter, `RootCause.recurrence`, recalled from cognitive
memory. You do not configure it — this section is context for reading the logs.

- **How it climbs.** Each time the Overseer acts on a blocked goal it records an
  occurrence keyed on `goal:blocked:{goal_id}`. On the next re-park, recall of
  those occurrences raises `recurrence`. So a perpetual goal that keeps re-parking
  reliably reaches `recurrence == 2` and beyond.
- **General escalation floor.** At `RECURRENCE_ESCALATION_THRESHOLD = 3`, **any**
  recurring cause escalates on its own.
- **Dead-band floor (new, #4124).** At
  `PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD = 2`, a **perpetual, no-progress**
  goal — the re-park loop specifically — escalates instead of being self-healed
  again.

**The dead-band.** Before this fix, a perpetual no-progress goal at
`recurrence == 2` sat in the `[2, 3)` gap: below the general floor of 3, so it
kept falling into the "self-heal" (re-unblock) path — re-unblocked, re-parked,
and re-emitted unchanged, a silent loop. The new lower floor of 2 escalates that
re-park loop once, closing the gap.

## There is nothing new to enable

This feature adds **no environment variable and no flag**. It is a routing change
inside the existing goal-health acting path, governed by the gates you already
know:

| Governs | Variable | Default |
| ------- | -------- | ------- |
| The whole Overseer | `SIMARD_OVERSEER_ENABLED` | on |
| The goal-health acting paths (self-heal / escalate) | `SIMARD_OVERSEER_GOAL_HEALTH` | on unless set falsey |
| Cognitive-memory recall (feeds the recurrence counter) | see [Configure memory recall](./configure-overseer-memory-recall.md) | recommended |

```bash
# The Overseer and its goal-health acting path run by default.
export SIMARD_OVERSEER_ENABLED=1
# Disabling only the ACTIONS still logs the WHY but suppresses the escalation act:
# export SIMARD_OVERSEER_GOAL_HEALTH=0
```

The two escalation floors are compile-time constants, intentionally **not**
env-tunable to avoid config sprawl:

- `PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD = 2` (`root_cause.rs`) — the
  dead-band floor for a perpetual, no-progress re-park loop.
- `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs`) — the general fast-path
  floor for any recurring cause.

## Cognitive memory is what feeds the recurrence counter

The recurrence counter derives from recall. If no `CognitiveMemoryOps` handle is
attached, `recurrence` folds to `0`, escalation does **not** fire, and the goal
is handled exactly as before (self-heal / report). To get the dead-band closure
in production, wire memory as described in
[Configure memory recall](./configure-overseer-memory-recall.md); the daemon does
this automatically via `build_overseer` (`Overseer::with_memory(mem)`).

Because the counter is **goal-scoped** (write and read both key on
`goal:blocked:{goal_id}`), it reliably reaches `2` for a re-parking perpetual
goal — this is the "seen 2 in cognitive memory" corroboration issue #4124
describes, and it is what the escalation keys on.

## Read the escalation

When the dead-band branch fires you will see a single goal-health escalation:

```
INFO overseer::goal_health goal_id=fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed
     reason="blocked: waiting on agent-kgpacks-rs issue-17 WS2 int8/PQ embed"
     action=escalate dispatched=true all_sent=true
     "overseer escalated a genuinely-blocked needs-human-review goal to the operator"
```

The operator notification is built with
`OperatorNotification::goal_blocked_with_why(goal_id, reason, why)`, so the body
carries the one-line root-cause **WHY**, which names the missing dependency. See
[Operator notifications reference](../reference/overseer-operator-notifications.md)
for delivery channels.

### No notification spam

The escalation deduplicates on the per-goal signature `escalate:{goal_id}` through
the existing `blocked_goal_gate`. Repeated escalation of the same goal within the
window (`WhisperGate::new(900, 20)` — 900 s, burst 20) collapses to **one**
notification. The newly reachable escalation path therefore cannot amplify
notifications or SMTP.

## Verify it end-to-end

The behavior is covered by hermetic tests — no network, no `~/.simard`.

```bash
# Dead-band closure + the surrounding decision table:
cargo test -p simard overseer::tests_goal_health

# Named-dependency escalation body + no secret-shaped substrings:
cargo test -p simard overseer::tests_root_cause
```

What the tests assert:

| Test | recurrence | perpetual + no-progress | Expected |
| ---- | ---------- | ----------------------- | -------- |
| T1 | 2 | yes | `EscalateBlockedGoal` (dead-band closed) |
| T2 | 1 | yes | `UnblockGoal` (below dead-band floor; self-heal) |
| T3 | 0 | yes | `UnblockGoal` (first park; self-heal) |
| T4 | 3 | — | `EscalateBlockedGoal` (general fast path) |
| T5 | repeat same goal | — | one notification (deduped) |
| T6 | escalating | — | `why` names the dependency; no secrets |

## Worked example: the kgpacks-rs issue-17 loop

The recurring signature that motivated this fix,
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`,
tracks a goal blocked on out-of-tree work (`agent-kgpacks-rs` issue-17 WS2, int8/PQ
embed). Simard cannot functionally complete that embedding work, so the goal is
genuinely blocked on an external dependency.

- **Before:** each re-park self-healed (re-unblocked) the perpetual no-progress
  goal, it re-parked, and the identical signature was re-emitted — observed 2×
  (and more) in cognitive memory with no operator ever notified, until the general
  floor of 3 was finally reached.
- **After:** the goal self-heals at `recurrence == 0` and `1`, but the third
  re-park at `recurrence == 2` escalates once. The operator receives a blocker
  report naming `agent-kgpacks-rs issue-17 WS2 int8/PQ embed` as the missing
  dependency, the goal stops re-emitting the un-escalated park, and the loop
  terminates one cycle earlier than before.

The other goals sharing the blocked board (kgpacks-rs #12/#18/#23/#25, the
test-coverage-to-70 goal, the coin-benchmark harness, the `simard-identity-*`
personas) are unaffected in identity — they are simply no longer starved by the
shared re-park loop.

## Troubleshooting

| Symptom | Likely cause | Action |
| ------- | ------------ | ------ |
| Goal still re-parks, never escalates | No cognitive memory attached → `recurrence` stays `0` | Wire memory ([recall howto](./configure-overseer-memory-recall.md)). |
| WHY logged but no notification | `SIMARD_OVERSEER_GOAL_HEALTH=0` suppresses the act | Unset it to enable escalation actions. |
| Escalated once then silent on re-block | Working as designed — `blocked_goal_gate` dedup | Expected; the single escalation is the surfaced blocker. |
| Escalation fires at `recurrence = 1` | Not possible via the dead-band path (`>= 2` required) | Check for an unrelated `needs_review` marker driving the needs-review branch. |
