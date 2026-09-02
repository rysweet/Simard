---
title: Standing/perpetual goals are exempt from the no-progress hard-block
description: Why a standing/perpetual goal is inherently bursty, why the OODA no-progress safeguard must never park it for human review, and how the runtime exemption plus load-time self-heal keep the research/CI-stewardship standing goals continuous and self-sustaining without operator unblocking (#2589).
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./steerable-ooda-daemon.md
  - ./ooda-loop-self-detection.md
  - ./overseer-goal-board-health.md
  - ./research-goal-never-idle.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/research-goal-never-idle-rail-api.md
  - ../reference/completion-evidence-gate-api.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# Standing/perpetual goals are exempt from the no-progress hard-block

> **Status: implemented.** A standing/perpetual goal — one whose description
> durably marks it standing (`is_perpetual()`, issue #2580) — is never parked by
> the OODA **no-progress breaker**. The runtime exemption lives in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> and the load-time self-heal in
> [`src/goal_board_store/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs).
> See the [no-progress breaker API reference](../reference/no-progress-breaker-api.md)
> for the exact types and functions.

> **Superseded for the research goal (#4399).** This page describes the benign
> exemption that treats an idle cycle as *normal* for a bursty standing goal. That
> is still correct for **non-research** standing goals (e.g. a CI-stewardship
> perpetual goal). It is **no longer** correct for the standing *research* goal:
> under [#4399](./research-goal-never-idle.md) an idle cycle for that goal is a
> **fault** — the breaker records it in `research_idle_faults` and **re-orients**
> the goal (still fail-closed: never blocked), rather than granting the benign
> `perpetual_idled` exemption. The split is made by the shared
> `classify_standing_idle` classifier keyed on `is_standing_research_goal()`. See
> [The standing research goal never idles](./research-goal-never-idle.md) and the
> [never-idle rail API](../reference/research-goal-never-idle-rail-api.md).

## The defect this fixes (#2589)

The OODA daemon carries a deterministic **no-progress safeguard**: if a single
goal produces `NO_PROGRESS_BREAKER_THRESHOLD` (3) consecutive *no-action* cycles
and the done-gate cannot certify it complete or obsolete, the breaker hard-blocks
it — setting `GoalProgress::Blocked` with the sentinel reason

```
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive no-action cycles; needs human review
```

and filing a GitHub issue so a human can look. For a *bounded* goal that is the
right behaviour: three cycles of doing nothing usually means a livelock.

In production this same safeguard **parked the standing research goal**
`continuously-research-and-improve-your-own-cogn-*` (priority 5, described
"STANDING PERPETUAL goal — never mark complete"). A standing goal is
**inherently bursty**: it ships a durable improvement, then legitimately *idles*
for a few cycles while there is nothing new to ship, then ships again. To the
breaker those idle cycles are indistinguishable from a livelock, so it blocked
the goal and demanded a manual `simard goal unblock`.

That directly violates the operator's firm, repeated requirement: **the
research/standing goals must be continuous and self-sustaining — they must never
require a human to unblock them.**

## This is a *different* gate from the completion-evidence gate

Two separate gates touch perpetual goals; conflating them is the trap:

| Gate | What it does | Perpetual behaviour |
| --- | --- | --- |
| **Completion gate** ([#2580](https://github.com/rysweet/Simard/issues/2580)/#2589) | Refuses to mark a standing goal `Completed` / archive it; rolls it to a new cycle instead. | Already handled — a standing goal is *non-completable*. |
| **No-progress breaker** (this page, #2589) | Hard-blocks any goal that idles for 3 cycles, pending human review. | **Was still parking standing goals.** Now exempt. |

Making a goal non-completable (the earlier fix) does **not** stop the
no-progress breaker from blocking it. The breaker is an orthogonal safeguard and
had to be taught the same exemption. Both gates now key off the **same single
flag** — `ActiveGoal::is_perpetual()` — so there is exactly one notion of
"standing/perpetual," never a second.

## The fix: two orthogonal, additive concerns

### 1. Runtime exemption — a standing goal never reaches the block

Inside the breaker's per-outcome loop, once a cycle is confirmed to be a
no-action outcome, Simard checks `goal.is_perpetual()` **before** the counter is
resolved into a block. For a standing goal she:

1. **resets** that goal's consecutive no-action counter (so it never climbs
   toward the threshold),
2. records the goal id in the report's `perpetual_idled` list, and
3. emits a structured `tracing::info!` note that a standing goal idled this
   cycle — framed explicitly as *normal for a bursty goal, not a fault* —

then **continues** to the next outcome. The escalation/`Blocked` path is never
entered, so the goal stays active and is re-selectable next cycle. Its status is
left untouched.

Non-perpetual goals fall straight through this check and hit the existing
safeguard behaviour byte-for-byte — no regression.

### 2. Load-time self-heal — un-park a goal a prior build already blocked

A goal may already carry a stale safeguard marker on disk because an *older*
daemon build (before this fix) parked it. The daemon reloads the authoritative
goal board from `goal_board.json` **at startup and again at the top of every
cycle**, and at each of those points — after tombstoned goals are filtered
out — Simard runs a pure `heal_stale_no_progress_blocks` pass over the in-memory
board: for every **perpetual** goal whose status is `Blocked(reason)` where
`is_no_progress_marker(reason)` holds, she restores the status to `NotStarted`
(the canonical re-dispatchable state used by `roll_to_new_cycle`). The goal
self-heals to active on the very next cycle — no `simard goal unblock`, no
operator action.

The per-cycle pass is the load-bearing one: because the daemon re-reads the
board from disk each cycle, a heal applied *only* at startup would be overwritten
by the first per-cycle reload (disk still says `Blocked`), and a `Blocked` goal
is never dispatched — so it would never produce the outcome that lets the runtime
exemption fire, and it would stay parked forever. Running the heal at the
per-cycle re-sync (before the memory-cache overwrite that `run_ooda_cycle` reads)
closes that loop.

The heal is **in-memory only**; it is persisted naturally by the next
`commit_cycle`, so hydration never writes to `~/.simard`.

## Why this is safe

- **Marker specificity.** Both the runtime exemption and the self-heal key
  *only* on the no-progress sentinel (`is_no_progress_marker`). Blocks authored
  by any *other* path — operator-set holds, scope blocks, dependency blocks, the
  brain-failure safeguard — are left exactly as they are. Simard never overrides
  an intentional human hold.
- **No tombstone resurrection.** The self-heal runs *after* `filter_tombstoned`,
  so a tombstoned goal can never be brought back.
- **Single source of truth.** Exemption keys off `is_perpetual()` — the same
  flag the completion gate uses (#2580/#2589). There is no second "is-standing"
  notion to drift.
- **Observability preserved.** A genuinely livelocked *perpetual* goal is now
  never hard-blocked (accepted per the operator mandate that standing goals are
  self-sustaining), but every idle cycle is still visible: it appears in the
  report's `perpetual_idled` list and in a per-goal `tracing::info!` line, so a
  real livelock surfaces in logs and metrics rather than as a parked goal. Note
  the cycle-summary line (`log_line()`) is emitted only when the breaker takes a
  *disruptive* action (`fired()`), and a perpetual idle deliberately does not
  count as firing — so the always-on signal for an idle is the per-goal
  `tracing::info!`, not the summary line.

## What an operator sees

Nothing to do. A standing goal that used to appear as

```
p5 [blocked: 🔒 [OODA-SAFEGUARD] … needs human review] continuously research and improve your own cognition. STANDING PERPETUAL goal.
```

now stays

```
p5 [not-started] continuously research and improve your own cognition. STANDING PERPETUAL goal.
```

across idle cycles, self-heals on the next daemon start — and on the next cycle
even without a restart — if it was parked by an older build, and continues
shipping durable improvements on its own schedule.
See the [unblock-stuck-OODA-goals runbook](../howto/unblock-stuck-ooda-goals.md)
for the (now rare) cases that still need a human.

## Related

- [The standing research goal never idles — an idle cycle is a fault](./research-goal-never-idle.md)
  — #4399 supersedes this benign exemption for the standing *research* goal (idle →
  fault → re-orient) while preserving it for non-research standing goals.
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md)
- [Overseer goal-board health](./overseer-goal-board-health.md) — the steward-side
  defense-in-depth complement (#2616): if a standing goal is parked anyway, the
  acting Overseer observes it and self-heals it (or escalates a genuine block).
- [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md) — where the
  no-progress breaker was introduced.
- [Completion-evidence gate API](../reference/completion-evidence-gate-api.md) —
  the sibling gate that makes standing goals non-completable.
- [Unblock OODA goals stuck after a safeguard lockout](../howto/unblock-stuck-ooda-goals.md)
