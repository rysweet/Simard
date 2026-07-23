---
title: "Reference: Overseer Cadence Watchdog API"
description: >
  The API contract for the Overseer cadence watchdog: the pure
  watchdog_should_rearm(guard_held, elapsed, cadence_secs) decision function and
  its exact predicate, the daemon-loop wiring around overseer_tick_running and
  overseer_tick_spawned_at, the guard-only re-arm invariant (never writes
  OverseerCadence.last_tick_secs), the SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER
  resolver (default 3, floor 2, clamp+WARN), the overseer_tick_watchdog_rearm_total
  counter and WARN span, fail-safe / fail-visible semantics, and the regression
  test list.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/overseer-cadence-watchdog.md
  - ./overseer-tick-details.md
  - ./overseer-tick-self-healing.md
  - ./telemetry-metrics.md
  - ../operations/overseer-cadence-watchdog-kill-switch.md
---

# Reference: Overseer Cadence Watchdog API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary sources:
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs)
> (loop wiring + `watchdog_should_rearm` helper) and
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs)
> (multiplier resolver). Conceptual overview:
> [Overseer Cadence Watchdog](../concepts/overseer-cadence-watchdog.md).

## Overview

The cadence watchdog is a loop-side self-heal for the acting Overseer's per-tick
overlap guard. It is a **pure decision function** plus a small amount of daemon
wiring. It force-clears a stuck overlap guard so the cadence can resume, and does
nothing else — no thread killing, no cadence mutation, no external action.

## The decision function

### watchdog_should_rearm

```rust
/// Decide whether the cadence watchdog should force-clear the Overseer
/// tick overlap guard.
///
/// Returns `true` iff the overlap guard is currently held AND the in-flight
/// tick has been outstanding for strictly longer than `multiplier × cadence`,
/// where `multiplier` is resolved from configuration and clamped to a floor of
/// `WATCHDOG_MULTIPLIER_FLOOR` (2).
///
/// Pure and deterministic: no clock reads, no I/O, no globals. All timing is
/// passed in so the predicate is unit-testable.
pub fn watchdog_should_rearm(
    guard_held: bool,
    elapsed: std::time::Duration,
    cadence_secs: u64,
    multiplier: u32,
) -> bool
```

### Predicate

Let `m = max(multiplier, WATCHDOG_MULTIPLIER_FLOOR)` and
`threshold = m * cadence_secs` seconds. Then:

```text
watchdog_should_rearm = guard_held AND elapsed.as_secs() > threshold
```

Notes:

- The comparison is **strictly greater-than**, so a tick that has run exactly
  `m × cadence` is not yet re-armed.
- `guard_held == false` short-circuits to `false`: if no tick is in flight there
  is nothing to re-arm.
- `cadence_secs` is the effective Overseer interval
  ([`overseer_interval_secs()`](./overseer-tick-details.md), default
  `DEFAULT_OVERSEER_INTERVAL_SECS = 900`). Callers pass the live cadence so the
  threshold tracks any operator-set interval.

### Constants

| Constant | Value | Meaning |
| --- | --- | --- |
| `WATCHDOG_MULTIPLIER_FLOOR` | `2` | Minimum effective multiplier. Guarantees a slow-but-healthy tick (under 2× cadence) is never re-armed. |
| `DEFAULT_OVERSEER_TICK_WATCHDOG_MULTIPLIER` | `3` | Default multiplier when the env knob is unset/empty/unparseable/out-of-range. |

## Daemon wiring

The daemon loop already owns the overlap guard:

```rust
let overseer_tick_running = Arc::new(AtomicBool::new(false));
```

The watchdog adds one piece of state and one loop-side check, both additive:

```rust
// Spawn instant of the in-flight overseer tick (None when idle).
let mut overseer_tick_spawned_at: Option<std::time::Instant> = None;
```

1. **On spawn.** Immediately after the tick thread is spawned (right after the
   successful `compare_exchange`), record
   `overseer_tick_spawned_at = Some(Instant::now())`.
2. **On each iteration.** Before evaluating `due()`, if the guard is held and a
   spawn instant is recorded, evaluate the watchdog:

   ```rust
   if let Some(spawned) = overseer_tick_spawned_at {
       let held = overseer_tick_running.load(Ordering::SeqCst);
       if watchdog_should_rearm(held, spawned.elapsed(), cadence_secs, multiplier) {
           // Force-clear ONLY the overlap guard. NEVER touch last_tick_secs.
           overseer_tick_running.store(false, Ordering::SeqCst);
           overseer_tick_spawned_at = None;
           tracing::warn!(
               target: "overseer.cadence_watchdog",
               elapsed_secs = spawned.elapsed().as_secs(),
               cadence_secs,
               multiplier,
               "overseer tick guard held past watchdog threshold; re-arming cadence"
           );
           crate::telemetry::counter_add(
               names::OVERSEER_TICK_WATCHDOG_REARM, 1, &[],
           );
       }
   }
   ```

3. **On healthy completion.** When the guard is observed cleared (the normal
   `ClearOnDrop` path), reset `overseer_tick_spawned_at = None` so the next tick
   starts a fresh timer.

### Invariants

| # | Invariant | Enforced by |
| --- | --- | --- |
| I1 | The watchdog writes **only** `overseer_tick_running`; it never writes `OverseerCadence.last_tick_secs`. | Wiring has no reference to the cadence marker in the re-arm branch. |
| I2 | The effective multiplier is always `>= 2`. | `watchdog_should_rearm` clamps to `WATCHDOG_MULTIPLIER_FLOOR`; the resolver clamps too. |
| I3 | Re-arm is idempotent with the abandoned thread's `ClearOnDrop`. | Both only ever `store(false)`; a double-clear is a no-op. |
| I4 | The re-arm grants no external authority. | The branch performs no intervention, no `gh` call, no goal/PR mutation. |
| I5 | Every re-arm is observable. | Mandatory `WARN` span + `overseer_tick_watchdog_rearm_total` increment. |

## Configuration

Resolved in [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs)
following the established `*_from(lookup)` pattern (matching the
`SIMARD_CLAIM_REAP_*` resolvers and the transient-backoff rung's config
resolvers):

```rust
pub const SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER_ENV: &str =
    "SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER";
pub const DEFAULT_OVERSEER_TICK_WATCHDOG_MULTIPLIER: u32 = 3;
pub const OVERSEER_TICK_WATCHDOG_MULTIPLIER_FLOOR: u32 = 2;

/// Resolve the watchdog multiplier from the environment.
///
/// Unset / empty / unparseable / below-floor ⇒ clamped to a safe value
/// (default `3`, never below `2`) with a `WARN`. A bad value can therefore
/// never *disable* the self-heal, only tune it.
pub fn overseer_tick_watchdog_multiplier_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> u32;

pub fn overseer_tick_watchdog_multiplier() -> u32 {
    overseer_tick_watchdog_multiplier_from(|k| std::env::var(k).ok())
}
```

| Variable | Default | Floor | Effect |
| --- | --- | --- | --- |
| `SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER` | `3` | `2` | Number of cadence periods a held guard may remain outstanding before the watchdog re-arms. |

Resolution rules (fail-safe):

- Unset / empty / non-numeric ⇒ `DEFAULT_OVERSEER_TICK_WATCHDOG_MULTIPLIER` (3).
- Parsed value `< 2` ⇒ clamped up to `2` with a `WARN`.
- There is **no** value that disables the watchdog; the self-heal is always on.

## Telemetry

Metric names follow the house **dotted OTel** convention (as in `names.rs`, e.g.
`simard.daemon.cycle`). The internal constant *value* is the dotted name; the
`_total` suffix is the **Prometheus exporter's rendering**, not the internal
name.

| Internal name (`names::`) | Constant value | Prometheus-exporter view |
| --- | --- | --- |
| `OVERSEER_TICK_WATCHDOG_REARM` | `simard.overseer.tick_watchdog_rearm` | `simard_overseer_tick_watchdog_rearm_total` |

Incremented once per force-clear of the overlap guard, with no attributes
(`counter_add(names::OVERSEER_TICK_WATCHDOG_REARM, 1, &[])`). A rising value
indicates a dependency is routinely hanging the Overseer tick.

Plus a `WARN` `tracing` span (`target: "overseer.cadence_watchdog"`) carrying
`elapsed_secs`, `cadence_secs`, and `multiplier` on every re-arm. Emitted through
the unified [telemetry facade](./telemetry-metrics.md) — no `print!` family
calls.

## Fail-safe / fail-visible semantics

- **Fail-safe:** any misconfiguration clamps to a safe multiplier; the watchdog
  cannot be turned off by a bad env value.
- **Fail-visible:** a re-arm is never silent — it always emits the WARN span and
  the counter, so a masked hang surfaces to operators.
- **Non-destructive:** the watchdog only flips an in-process boolean; it never
  interacts with `gh`, goals, PRs, or worktrees.

## Regression tests

In [`src/overseer/tests_self_healing.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_self_healing.rs)
(extended) and inline `#[cfg(test)]` tests next to `watchdog_should_rearm`:

| Test | Asserts |
| --- | --- |
| `watchdog_rearms_when_guard_held_past_multiplier` | `guard_held=true`, `elapsed > m×cadence` ⇒ `true`. |
| `watchdog_does_not_rearm_slow_but_healthy_tick` | `guard_held=true`, `cadence < elapsed < 2×cadence` ⇒ `false`. |
| `watchdog_does_not_rearm_when_guard_free` | `guard_held=false` ⇒ `false` regardless of elapsed. |
| `watchdog_multiplier_floors_at_two` | A configured multiplier `< 2` is clamped to `2`. |
| `watchdog_rearm_is_idempotent_with_clear_on_drop` | A re-arm followed by a late `ClearOnDrop` leaves the guard `false` (no spurious extra tick). |
| `watchdog_never_writes_last_tick_secs` | `OverseerCadence.last_tick_secs` is unchanged across a re-arm. |
| `watchdog_multiplier_resolver_defaults_and_clamps` | Resolver returns 3 when unset, clamps `<2` to 2, and never yields a disabling value. |
