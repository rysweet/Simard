---
title: "Overseer Cadence Watchdog (self-heal a hung tick that stalls the cadence)"
description: >
  How Simard keeps the acting Overseer's Observe→Orient→Decide→Act cadence
  advancing even when a single tick hangs indefinitely on an external call.
  The daemon runs one Overseer tick per cadence behind an
  `overseer_tick_running` overlap guard; if a tick thread hangs forever, the
  guard is never cleared, no further tick is ever scheduled, and `simard status`
  flips the Overseer line to `(stale)`. The cadence watchdog is a loop-side,
  additive self-heal: it records when each tick was spawned and, if the guard is
  still held past a bounded multiple of the cadence (default 3×, floor 2×),
  force-clears ONLY the overlap guard so the next `due()` can spawn a fresh tick.
  It never touches the cadence marker, never shortens the 15-minute default, and
  emits a WARN span plus a re-arm counter on every intervention so a masked hang
  is always observable.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./stale-engineer-claim-reaper.md
  - ../reference/overseer-cadence-watchdog-api.md
  - ../reference/overseer-tick-details.md
  - ../reference/overseer-tick-self-healing.md
  - ../operations/overseer-cadence-watchdog-kill-switch.md
  - ../design/overseer.md
  - ../howto/watch-overseer-activity.md
---

# Overseer Cadence Watchdog

> **Status: implemented.** This page describes shipped behaviour in the present
> tense. The watchdog decision function
> (`watchdog_should_rearm`) is a pure helper unit-tested inline; it is wired
> into the daemon loop in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs)
> alongside the existing `overseer_tick_running` overlap guard, and its
> multiplier knob lives in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs).
> The change is purely **additive** to the post-cadence baseline and leaves the
> `DEFAULT_OVERSEER_INTERVAL_SECS = 900` (15-minute) cadence semantics
> unchanged. Documentation and implementation land in the **same pull request**.

## The problem it solves

The acting Overseer runs its own OODA loop once per cadence. The daemon loop
schedules each tick behind an overlap guard so that two ticks never run at once:

```text
if overseer_cadence.due(now_secs)
    && overseer_tick_running
        .compare_exchange(false, true, SeqCst, SeqCst)
        .is_ok()
{
    // spawn "overseer-tick" thread; guard cleared by ClearOnDrop on exit/panic
}
```

The overlap guard is an `AtomicBool` that a spawned `overseer-tick` thread
clears through a `ClearOnDrop` guard when it returns **or panics**. That covers
the common failure modes. It does **not** cover a tick that never returns at
all — a thread blocked *indefinitely* inside an external call (a `gh` subprocess
or upstream read that hangs without timing out). In that case:

1. `ClearOnDrop` never runs because the thread never unwinds.
2. `overseer_tick_running` stays `true` forever.
3. Every later `due()` window is skipped because the `compare_exchange` from
   `false → true` can never succeed.
4. `OverseerCadence.last_tick_secs` stops advancing, so
   [`simard status`](../howto/simard-status.md) marks the Overseer line
   `(stale)` — e.g. `Overseer: enabled, 304 interventions (stale)`.

A single hung tick therefore silently **halts the entire cadence**. Nothing
crashes; the daemon just stops advancing the Overseer OODA loop, and the only
outward symptom is the `(stale)` marker growing more overdue.

## The self-heal

The cadence watchdog restores forward progress **without** restructuring the
spawn / `ClearOnDrop` path and **without** ever shortening the cadence. It is a
loop-side elapsed check that reuses primitives already in the daemon loop — the
`overseer_tick_running` `AtomicBool` + `ClearOnDrop` overlap guard and the
elapsed/counter bookkeeping style of the existing transient-backoff rung:

- When a tick is spawned, the daemon records the spawn instant
  (`overseer_tick_spawned_at: Option<Instant>`).
- On **each** daemon iteration, while the guard is still held, the watchdog asks
  the pure decision function
  [`watchdog_should_rearm(guard_held, elapsed, cadence_secs)`](../reference/overseer-cadence-watchdog-api.md#watchdog_should_rearm):
  is the guard held **and** has more than `multiplier × cadence` elapsed since
  the spawn?
- If yes, the watchdog **force-clears only the `overseer_tick_running`
  `AtomicBool`**. It never writes `OverseerCadence.last_tick_secs`.

Clearing only the overlap guard is the whole trick. `due()` already advanced
`OverseerCadence.last_tick_secs` to the tick's **spawn instant** when it
scheduled the hung tick; the hung tick *body* never advances the cadence beyond
that spawn instant. By the time the watchdog fires (at least 2× cadence later),
more than one full cadence has elapsed since that spawn instant, so on the next
loop iteration `overseer_cadence.due(now_secs)` is already `true` again. The
`compare_exchange` from `false → true` now succeeds, and a **fresh** tick is
spawned. The cadence resumes at its normal rhythm; the `(stale)` marker clears
on the next healthy tick.

```mermaid
flowchart TD
    A[Tick spawned<br/>record spawned_at] --> B{Guard held<br/>on next iteration?}
    B -- no --> A
    B -- yes --> C{elapsed &gt; multiplier × cadence?}
    C -- no --> B
    C -- yes --> D[force-clear ONLY the AtomicBool guard<br/>WARN span + overseer_tick_watchdog_rearm_total++]
    D --> E[next due() spawns a fresh tick<br/>cadence resumes]
```

### Why it cannot double-fire

The obvious risk (raised in review as **R-A2**) is that re-arming spawns a
*second* tick that races the still-running hung one. Two design choices prevent
it:

1. **Guard-only re-arm.** The watchdog re-arms *only* the overlap
   `AtomicBool` and never `OverseerCadence.last_tick_secs`. Because the cadence
   marker is untouched, the re-armed tick is simply the *normal* next tick that
   `due()` was already owed — not an extra one injected out of band.
2. **Floor of 2× cadence.** The multiplier is clamped to a floor of **2**, so
   the watchdog can only fire after a tick has been outstanding for at least
   *two full cadence periods*. A merely **slow-but-healthy** tick (one that runs
   longer than a cadence but well under 2×) is never re-armed. By the time the
   watchdog acts, the original tick is overwhelmingly likely to be genuinely
   stuck; even if the abandoned thread later wakes and unwinds, its
   `ClearOnDrop` simply stores `false` into a guard that has already moved on —
   an idempotent no-op.

Because the re-arm targets an in-process boolean only, it grants **no external
authority**: it triggers no intervention, no `gh` call, and no PR/goal action.

## Observability (never a silent mask)

A watchdog that quietly papers over hangs would hide a real defect. Every re-arm
is therefore **loud**:

- a **`WARN`** [`tracing`](../reference/telemetry-metrics.md) span records the
  elapsed time, the cadence, and the effective multiplier; and
- the counter **`overseer_tick_watchdog_rearm_total`** increments on every
  re-arm. (That is the Prometheus-exporter view; the internal OTel name follows
  the house dotted convention, `simard.overseer.tick_watchdog_rearm` — see the
  [API reference](../reference/overseer-cadence-watchdog-api.md#telemetry).)

A rising re-arm counter is an operator signal that some external dependency is
routinely hanging the Overseer tick and deserves investigation — the watchdog
keeps the cadence alive but never conceals *that* it had to.

Per the project telemetry rule, the watchdog uses structured `tracing` + OTel
counters only — **no `print!` / `println!` / `eprintln!`**.

## What it deliberately does **not** do

- It does **not** change the cadence. `DEFAULT_OVERSEER_INTERVAL_SECS` stays
  `900` (15 minutes); the watchdog can only *restore* the normal rhythm, never
  accelerate it.
- It does **not** kill the hung thread. The abandoned `overseer-tick` thread is
  left to unwind on its own; its late `ClearOnDrop` is harmless.
- It does **not** write the cadence marker, so it cannot fabricate a "fresh"
  `last_tick_secs` or hide a stall from `simard status` between the hang and the
  next healthy tick.

## Configuration

One additive environment knob tunes the watchdog; a bad or out-of-range value
clamps to a safe floor with a `WARN` and never disables the self-heal. See the
[operations kill-switch & tuning page](../operations/overseer-cadence-watchdog-kill-switch.md)
and the [API reference](../reference/overseer-cadence-watchdog-api.md#configuration).

| Variable | Default | Floor | Effect |
| --- | --- | --- | --- |
| `SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER` | `3` | `2` | Re-arm once a held guard's tick has been outstanding for more than `multiplier × cadence`. |

## Related

- [Overseer Cadence Watchdog API reference](../reference/overseer-cadence-watchdog-api.md)
- [Operations: cadence-watchdog tuning](../operations/overseer-cadence-watchdog-kill-switch.md)
- [Overseer Tick Details](../reference/overseer-tick-details.md)
- [Overseer tick self-healing (transient-failure backoff rung)](../reference/overseer-tick-self-healing.md)
- [Design — Overseer](../design/overseer.md)
