---
title: Configure and observe Overseer goal-board health
description: >
  Operator guide for the Overseer's goal-board health handling — enabling/disabling the
  self-heal + escalate paths with SIMARD_OVERSEER_GOAL_HEALTH, understanding how a
  false-parked perpetual goal is auto-unblocked and a "needs human review" block is
  escalated on both channels, reading the overseer::goal_health traces, the extended
  OverseerTickReport counters and the operator-activity feed, confirming the dedup,
  fail-closed-identity and opt-out guarantees, and verifying the feature end-to-end with
  injected fakes.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/overseer-goal-board-health.md
  - ../reference/overseer-goal-board-health-api.md
  - ../design/overseer.md
  - ./watch-overseer-activity.md
  - ./unblock-stuck-ooda-goals.md
  - ./set-up-the-signal-channel.md
  - ./configure-the-simard-whisperer.md
---

# Configure and observe Overseer goal-board health

> **Status: implemented** (issue
> [#2609](https://github.com/rysweet/Simard/issues/2609), merged in
> [#2616](https://github.com/rysweet/Simard/pull/2616)). The
> `SIMARD_OVERSEER_GOAL_HEALTH` flag, the `overseer::goal_health` traces, the
> `goal-blocked` operator notifications, and the `cargo test` targets below all
> ship in the acting Overseer.

The acting **Overseer** watches Simard's goal board for **`Blocked`** goals and
acts on them as its own signal (see
[Overseer goal-board health](../concepts/overseer-goal-board-health.md)):

- a standing/**perpetual** goal false-parked by the OODA no-progress safeguard is
  **self-healed** — auto-unblocked + reactivated (the exact `simard goal unblock`
  operation);
- any goal carrying a **"needs human review"** marker is **escalated** to the
  operator on both channels (email + Signal) with the goal id and reason.

This guide shows how to enable, observe, and verify that handling. For the API
contract see the
[goal-board health API reference](../reference/overseer-goal-board-health-api.md).

!!! note "Activation requires a redeploy"
    The flag is read from the environment at daemon start. Set it **before**
    launching (or relaunching) the daemon; the running daemon does not pick up a
    new value until it restarts.

## When to use this

Use this guide when:

- You want to turn goal-board health handling on or off, independently of the
  rest of the Overseer.
- The daemon emitted `overseer::goal_health` traces, or you received a
  `goal-blocked` operator notification, and you want to understand it.
- A standing goal was auto-unblocked and you want to confirm the Overseer (not a
  human) did it.
- You need to confirm the dedup / fail-closed / opt-out guarantees.

## Enable and disable

Goal-board health is gated by one environment variable and follows the
Overseer's **opt-out** convention.

| Knob | Env var | Default | What it controls |
| --- | --- | ---: | --- |
| On/off | `SIMARD_OVERSEER_GOAL_HEALTH` | enabled with the Overseer | Master gate for the self-heal + escalate paths. Opt-**out**: an explicit falsey value disables it. |

```bash
# Disable goal-board health (Overseer still runs; it only observes/reports
# blocked goals, never self-heals or escalates)
export SIMARD_OVERSEER_GOAL_HEALTH=off      # or: 0 | false | no

# Explicitly enable (the default when the Overseer is enabled)
export SIMARD_OVERSEER_GOAL_HEALTH=on       # or: 1 | true | yes

# Restore the default (enabled-with-Overseer) — just unset it
unset SIMARD_OVERSEER_GOAL_HEALTH
```

Recognised **falsey** values (case-insensitive, trimmed): `0`, `false`, `no`,
`off`. Everything else — unset, empty, `1`, `true`, `yes`, `on`, or an
unrecognised string — leaves handling **enabled**, provided the Overseer itself
is enabled (`SIMARD_OVERSEER_ENABLED` is not falsey). **If the Overseer is
disabled, goal-board health is disabled too**, regardless of this flag.

When disabled, a blocked goal is still **observed** and **reported** (it shows up
in the tick report's problem count), but no self-heal or escalation is taken; the
planned action is held with the note
`held: goal-board health disabled (SIMARD_OVERSEER_GOAL_HEALTH)`.

There are no separate window / cap / threshold knobs: the dedup gate uses a fixed
15-minute window and a per-hour cap of 20 (enough for many distinct goals without
flooding).

## How a blocked goal is handled

You do not trigger these actions manually — the Overseer composes and takes them
each tick. The path is:

1. **Observe.** The Overseer reads the live board and projects every
   `Blocked` active goal into a `BlockedGoal` carrying `perpetual`,
   `needs_review`, and `consecutive_no_action`. A board-read failure degrades to
   "no blocked goals" (never aborts the tick).
2. **Decide by shape.**
   - A **perpetual** goal whose block carries the **no-progress** marker is a
     **false park** ⇒ **self-heal** (unblock + reactivate). It is *not* escalated.
   - Any other goal carrying a **"needs human review"** marker ⇒ **escalate** to
     the operator.
   - A plain operator-set / dependency block ⇒ **report only**, left untouched.
3. **Gate.** The dedup gate suppresses a repeat of the same action for the same
   goal within the window / cap; `RecursionGuard` refuses if the steward identity
   is unconfigured.
4. **Act.**
   - **Self-heal:** the goal's status is set back to `NotStarted` (the exact
     `simard goal unblock` mutation, under the board write-lock), so the next
     OODA cycle re-enters the spawn path. Optionally, a best-effort *carve one
     bounded sub-goal* whisper is sent to Simard (only if the
     [Whisperer](./configure-the-simard-whisperer.md) is enabled and wired).
   - **Escalate:** an `OperatorNotification` of kind `goal-blocked` is sent on
     both channels via the mandatory dual-channel notifier.

## Observe goal-board health

### Structured tracing

Each action emits one `tracing` event on target `overseer::goal_health`:

```
INFO overseer::goal_health goal_id="continuously-research-…" reason="🔒 [OODA-SAFEGUARD] … needs human review" action="unblock" overseer self-healed a false-parked perpetual goal: auto-unblocked + reactivated
INFO overseer::goal_health goal_id="fix-flaky-…" reason="🔒 [OODA-SAFEGUARD] … needs human review" action="escalate" dispatched=true all_sent=true overseer escalated a genuinely-blocked needs-human-review goal to the operator
```

Key fields:

- `goal_id` — the affected goal.
- `reason` — the verbatim `Blocked` reason (carries the marker).
- `action` — `unblock` or `escalate`.
- `dispatched` / `all_sent` — (escalate only) whether the notifier dispatched to
  any / all channels.

A **suppressed** action emits a `DEBUG` event on the same target with
`reason="duplicate"` or `reason="cap_reached"`.

### Tick report and activity feed

The per-tick `OverseerTickReport` carries `goals_unblocked`, `goals_escalated`,
and `goals_health_suppressed`; the daemon's tick summary log line includes
`goals_unblocked=` and `goals_escalated=`. These roll into the durable
[Overseer activity feed](../reference/overseer-activity-feed.md), so the same
numbers appear on the dashboard **Overseer** tab, the TUI **Overseer** pane, and
`simard status`. The honest one-liner reads, e.g.:

```
… self-healed 1 blocked goal, escalated 2 blocked goals for human review …
```

See [Watch Overseer activity](./watch-overseer-activity.md) for the surfaces.

### Operator notifications

Each **escalation** is surfaced to the operator through the mandatory dual-channel
notifier as an `OperatorNotification` of kind `"goal-blocked"`:

- **Headline:** `goal <id> needs human review`
- **Body:** `Goal `<id>` is blocked and needs human review.` plus the reason.

It rides the **same** email + Signal channels the merge path uses. If Signal is
not yet set up, see [Set up the Signal channel](./set-up-the-signal-channel.md);
the notifier never drops — it queues/logs on a channel failure.

Self-heals are traced and counted but **not** operator-notified (a false park is
routine stewardship, not something a human must act on).

## Confirm the guarantees

### A false park was self-healed, not escalated

After a standing goal is auto-unblocked you will see an `action="unblock"` trace
and `goals_unblocked` increment, and **no** `goal-blocked` operator notification
for that goal. On the board, the goal moves from
`[blocked: 🔒 [OODA-SAFEGUARD] … needs human review]` back to `[not-started]` and
is re-selectable next cycle — no `simard goal unblock` typed by a human.

### Dedup — one action per goal per window

A persistent blocked goal is handled **once per 15-minute window**, not every
tick. A repeat within the window increments `goals_health_suppressed` and emits a
`reason="duplicate"` debug trace; there is no second unblock or escalation.

### Distinct identity, fail-closed

Both actions require the Overseer's distinct steward identity. If it is
unconfigured, the action is **refused** (an `OverseerError::Recursion` counted as
a tick `error`) and nothing is mutated or sent — no unblock, no escalation, no
self-whisper. Configure it with:

```bash
export SIMARD_OVERSEER_AUTHOR_LOGIN="simard-overseer[bot]"
```

### Opt-out holds both actions

With `SIMARD_OVERSEER_GOAL_HEALTH` falsey, a blocked goal is still observed and
reported, but the planned self-heal / escalation is **held** with the note
`held: goal-board health disabled (SIMARD_OVERSEER_GOAL_HEALTH)`.

## Verify

Goal-board health is fully covered by tests that inject fakes for the goal store,
the notifier, the whisper sink, the clock, and the identity — **no network**. Run
them with:

```bash
cargo test -p simard overseer::tests_goal_health
```

The suite (`src/overseer/tests_goal_health.rs`) proves, at minimum:

- **Sensor projection.** A perpetual no-progress block and a normal needs-review
  block are both surfaced as `BlockedGoal`s with the right `perpetual` /
  `needs_review` / `consecutive_no_action`
  (`blocked_goals_projection_surfaces_perpetual_and_needs_review_goals`).
- **Observe populates the signal.** `run_cycle` fills
  `ObservedState.blocked_goals` and emits `Signal::GoalBlocked`
  (`run_cycle_populates_observed_blocked_goals_and_emits_signals`).
- **Signal → problem.** `GoalBlocked` maps to `ProblemKind::GoalHygiene`
  (`goal_blocked_signal_maps_to_a_goal_hygiene_problem`).
- **Self-heal, once, not escalated.** A blocked perpetual goal is unblocked +
  reactivated exactly once (deduped) and never escalated
  (`perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`).
- **Escalate on both channels.** A needs-review goal fires an operator
  notification on email **and** Signal with the id + reason
  (`needs_review_goal_escalates_to_operator_on_both_channels`).
- **Fail-closed identity.** With the steward identity unset, neither self-heal
  nor escalation happens
  (`self_heal_and_escalate_fail_closed_without_a_distinct_identity`).
- **Opt-out.** A disabled flag holds both actions
  (`disabled_goal_health_holds_both_actions`); the flag is opt-out and off when
  the Overseer is off
  (`goal_health_enable_flag_is_opt_out_and_off_when_overseer_off`).
- **Isolation.** A failing unblock is isolated and the tick survives
  (`a_failing_unblock_is_isolated_and_the_tick_survives`).
- **Routing + risk.** `decide` routes a blocked goal by shape
  (`decide_routes_a_blocked_goal_by_shape`); the actions are `Routine` and
  admitted by the default gate
  (`goal_health_interventions_are_routine_and_admitted_by_default_gate`).

## Troubleshoot

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Blocked goals never self-heal/escalate | `SIMARD_OVERSEER_GOAL_HEALTH` falsey, or the Overseer disabled (`SIMARD_OVERSEER_ENABLED` falsey) | Unset the flag or set a truthy value; ensure the Overseer is enabled; redeploy. |
| Trace shows the tick logging a recursion `error` on a blocked goal | Steward identity unconfigured (fail-closed) | Set `SIMARD_OVERSEER_AUTHOR_LOGIN`; redeploy. |
| A standing goal keeps re-parking each cycle | The in-loop exemption (#2609) is not active in the running build | Redeploy the current binary; the Overseer self-heal is the steward-side backstop, not a substitute for the exemption. See [perpetual-goal exemption](../concepts/perpetual-goal-no-progress-exemption.md). |
| Escalation traced `dispatched=true` but you got no message | A channel (e.g. Signal) is not configured | Set up the channel; the notifier queues/logs on failure and never drops. See [Set up the Signal channel](./set-up-the-signal-channel.md). |
| A perpetual goal was escalated instead of self-healed | Its block did not carry the **no-progress** marker (e.g. a brain-failure block) | Expected: only a perpetual + no-progress false-park self-heals; other needs-review blocks escalate by design. |
| A deliberately-held goal got unblocked | Should be impossible — only a perpetual + no-progress block self-heals | File a bug: inspect the goal's `Blocked` reason; a plain hold must route to `Report`. |

## See also

- Concept: [Overseer goal-board health](../concepts/overseer-goal-board-health.md)
- API reference: [Overseer goal-board health API](../reference/overseer-goal-board-health-api.md)
- Design: [Overseer — operator/observer co-process](../design/overseer.md)
- Related: [Watch Overseer activity](./watch-overseer-activity.md),
  [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md),
  [Configure the Simard Whisperer](./configure-the-simard-whisperer.md)
