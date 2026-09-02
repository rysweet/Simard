---
title: OODA coverage parallelism ceiling reference
description: >
  The per-OODA-cycle goal-coverage parallelism ceiling — its default of 24,
  the SIMARD_OODA_MAX_CONCURRENT environment override (fail-closed, range
  1..=64), how it seeds the AIMD scaler's base and ceiling, and why 24 is a
  ceiling rather than a guarantee — the resource-admission and
  overlap/dependency gates still bound actual engineer spawns.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./maximum-safe-parallelism.md
  - ./goal-coverage-allocation.md
  - ./adaptive-scaling-api.md
  - ./resource-admission-api.md
  - ../concepts/adaptive-scaling.md
  - ../concepts/resource-aware-engineer-admission.md
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ../howto/configure-adaptive-scaling.md
---

# OODA coverage parallelism ceiling reference

> **Issue [#2935](https://github.com/rysweet/Simard/issues/2935).** The
> per-OODA-cycle goal-coverage parallelism ceiling now defaults to **24**
> (previously ~6 early in a run, under an AIMD ceiling of 20) and is
> **environment-configurable** via
> `SIMARD_OODA_MAX_CONCURRENT`. This lets Simard cover up to 24 genuinely
> independent incomplete goals in a single cycle when the host has headroom,
> while the resource-admission and overlap/dependency gates continue to bound
> the number of engineers actually spawned.

Modules: `simard::ooda_loop::types` (`OodaConfig`, `env_u32_bounded`),
`simard::ooda_loop::adaptive_scaling` (`AdaptiveScaler`),
`simard::ooda_loop::coverage` (`ensure_goal_coverage`),
`simard::ooda_loop::cycle` (per-cycle `coverage_cap`).

## The problem

Each OODA cycle derives a **coverage cap** — the maximum number of distinct
`AdvanceGoal` actions [`ensure_goal_coverage`](./goal-coverage-allocation.md)
may emit — from the AIMD scaler's `current_max()` (or the static
`OodaConfig.max_concurrent_actions` when `SIMARD_SCALING` is off):

```rust
let coverage_cap = config
    .scaler
    .as_ref()
    .map(|s| s.current_max() as usize)
    .unwrap_or(config.max_concurrent_actions as usize);
```

Historically `max_concurrent_actions` defaulted to **5**, and with
`SIMARD_SCALING=auto` the AIMD ceiling was `base × 4 = 20`. So the *ceiling*
was 20 — but the scaler *started at the base (5)* and only added `+1` per
low-pressure cycle, so in practice the effective cap started near **6** and
rarely climbed far toward 20 before the run ended or pressure reset it:

```
[simard] OODA cycle: coverage — covered 6/12 incomplete goals, deferred 6 due to cap (cap 6)
```

That early-run cap of ~6 was an **arbitrary low starting point**, not a
resource limit. On a host with spare disk, memory, and CPU — and a board full
of independent, non-overlapping goals — Simard left work uncovered every cycle
for no safety reason.

## What changed

Two coupled changes raise the ceiling to 24 and make it configurable, while
leaving every downstream safety gate intact:

1. **Base default raised 5 → 24.** `OodaConfig.max_concurrent_actions` now
   defaults to **24**. Because the coverage cap reads `current_max()`, and the
   scaler's initial value *is* the base, raising only the AIMD ceiling would
   have left the cap stuck near 6 — the **base** had to rise too.
2. **AIMD ceiling = configured max.** With `SIMARD_SCALING=auto`, the scaler is
   constructed as `AdaptiveScaler::new(base, 1, base)` — floor `1`, ceiling
   equal to the configured base (24 by default). 24 is therefore the *true*
   per-cycle ceiling, and the AIMD back-off/recover behaviour operates inside
   `[1, 24]`.

Both values are seeded from the same resolved number (see
[Configuration](#configuration)), so one environment variable moves the base
and the ceiling together.

## Configuration

| Variable | Default | Range | Precedence | Effect |
| --- | --- | --- | --- | --- |
| `SIMARD_OODA_MAX_CONCURRENT` | `24` | `1..=64` | highest | Preferred knob. Seeds both the scaler base and ceiling, and the static `max_concurrent_actions` fallback. |
| `SIMARD_MAX_CONCURRENT_ACTIONS` | `24` | `1..=64` | legacy fallback | Still honoured when `SIMARD_OODA_MAX_CONCURRENT` is unset. Retained for backward compatibility. |

**Resolution order.** The preferred variable wins whenever it is *present*.
Each step is **fail-closed independently**: an invalid value uses the default
**24** and does **not** fall through to a lower-precedence source (otherwise a
typo in the preferred variable would silently resurrect a stale legacy value).

1. `SIMARD_OODA_MAX_CONCURRENT` **present** → use it if valid, else **24**
   (warn). The legacy variable is ignored in this case.
2. else `SIMARD_MAX_CONCURRENT_ACTIONS` (legacy) **present** → use it if valid,
   else **24** (warn).
3. else the default **24**.

```rust
// The preferred var wins when present. An invalid *present* value fails
// closed to 24 — it does NOT fall through to the legacy var. The legacy
// var is consulted only when the preferred var is entirely absent.
let max_concurrent_actions = if std::env::var("SIMARD_OODA_MAX_CONCURRENT").is_ok() {
    env_u32_bounded("SIMARD_OODA_MAX_CONCURRENT", 24, 1, 64)
} else {
    env_u32_bounded("SIMARD_MAX_CONCURRENT_ACTIONS", 24, 1, 64)
};
```

### Fail-closed validation

Parsing is done by `env_u32_bounded`, a bounded, fail-closed helper. It is
**not** the silent `env_u32` used by other config fields — an invalid value
here would otherwise be an easy way to mis-size machine load.

```rust
/// Parse an unsigned integer from `key`, accepting only values in
/// `min..=max`. On an absent key, returns `default` silently. On a present
/// but non-numeric / zero / out-of-range value, logs a `tracing::warn!` and
/// returns `default` (fail-closed) rather than propagating a bad cap.
fn env_u32_bounded(key: &str, default: u32, min: u32, max: u32) -> u32;
```

| Input for `SIMARD_OODA_MAX_CONCURRENT` | Result | Logged? |
| --- | --- | --- |
| unset | fallback chain → `24` | no (absence is normal) |
| `"24"` | `24` | no |
| `"1"` … `"64"` | that value | no |
| `"0"` | fallback (`24`) | yes — `tracing::warn!` |
| `"65"` or higher | fallback (`24`) | yes — `tracing::warn!` |
| `"abc"`, empty, `"-1"`, `"3.5"` | fallback (`24`) | yes — `tracing::warn!` |

The warning is emitted on the `simard::ooda_loop::types` target — the module
where `env_u32_bounded` is defined, so `tracing`'s default target already
matches at runtime with no custom `target:` override needed (mirroring the
`simard::disk_pressure` precedent, whose target is likewise its own module).
Operators can therefore see that a bad value was ignored:

```
WARN simard::ooda_loop::types: invalid value for SIMARD_OODA_MAX_CONCURRENT; using default 24 key="SIMARD_OODA_MAX_CONCURRENT" value="99" min=1 max=64 default=24
```

There are **no** `println!` / `eprintln!` calls in the parse path — all
diagnostics go through `tracing`.

> **Why cap the range at 64?** `64` is a documented absurd-value guard: well
> above the 24 target, but below anything a real host would sustain. It exists
> to reject typos and overflow attempts, not to express a resource limit — the
> resource limit is enforced later, by the admission gates.

### Examples

```bash
# Default: cover up to 24 independent goals per cycle.
SIMARD_SCALING=auto simard ooda run

# Widen further on a large host (still bounded by admission gates).
SIMARD_OODA_MAX_CONCURRENT=48 SIMARD_SCALING=auto simard ooda run

# Narrow on a constrained host.
SIMARD_OODA_MAX_CONCURRENT=8 SIMARD_SCALING=auto simard ooda run

# Legacy variable still works when the new one is unset.
SIMARD_MAX_CONCURRENT_ACTIONS=16 SIMARD_SCALING=auto simard ooda run

# Invalid value → warn + fall back to 24 (does not crash, does not use 99).
SIMARD_OODA_MAX_CONCURRENT=99 SIMARD_SCALING=auto simard ooda run
```

## 24 is a ceiling, not a guarantee

The parallelism ceiling caps how many goals coverage may **plan** per cycle. It
does **not** override the gates that decide how many engineers actually
**spawn**. Even at cap 24, an OODA cycle may spawn far fewer engineers — or
none — when resources are tight or work overlaps.

```
                        per-cycle pipeline (Decide → Coverage → Act → spawn)

  Decide  ──►  Coverage cap (≤ 24)  ──►  Act dispatch  ──►  per-goal spawn gates
                    │                                         │
     up to 24 distinct AdvanceGoal            each spawn still passes through:
     actions planned this cycle                 • overlap/dependency dedup
                                                 • resource-admission gate
                                                   (disk / build-cache / load)
                                                 • disk-ceiling hard rail
```

The gates run **after** the count cap and can independently `Defer`:

- **Overlap / dependency gate.** Duplicate or overlapping goals are de-duplicated
  so two engineers never work the same item. A cap of 24 does not admit 24
  engineers onto 24 copies of the same work. See
  [dependency-overlap-aware scheduling](../concepts/dependency-overlap-aware-scheduling.md).
- **Resource-admission gate.** Before each spawn, Simard reasons over the host
  resource picture (disk %, build-cache/worktree footprint, load average,
  in-flight builds) and returns **admit**, **defer**, or **reclaim-first**; a
  deterministic disk-ceiling rail (`SIMARD_DISK_ADMISSION_CEILING_PCT`) blocks
  admission regardless of the reasoning. Under disk or memory pressure it
  defers *even when the cap is 24*. See
  [resource-aware engineer admission](../concepts/resource-aware-engineer-admission.md)
  and the [resource-admission API](./resource-admission-api.md).
- **AIMD back-off.** The scaler still halves `current_max()` under CPU/memory
  pressure or 429 errors and recovers `+1` per low-pressure cycle, so the
  *effective* cap drops below 24 while the host is loaded and climbs back to 24
  as it drains. See the [adaptive scaling API](./adaptive-scaling-api.md).

The invariant is: **raise the ceiling to allow up to ~24 genuinely-independent
goals to be covered per cycle when resources allow — never bypass the gates
that keep the host safe.**

## Invariants

- **Effective cap ≤ 24 by default.** `coverage_cap == scaler.current_max()`,
  and the scaler ceiling equals the configured base (24 by default), so a cycle
  never plans more than 24 coverage actions unless the operator raised the base.
- **Config-driven, not code-driven.** The 24 default and the `1..=64` range live
  in `OodaConfig::default`; there is no separate hard-coded `6`.
- **Fail-closed override.** An invalid `SIMARD_OODA_MAX_CONCURRENT` (or legacy)
  value warns and uses the default 24; it never crashes and never applies the
  bad number.
- **AIMD adaptivity preserved.** The scaler still backs off on pressure/429 and
  recovers, now inside `[1, 24]` instead of `[1, 20]`.
- **Gates are never bypassed.** The count cap runs *before* the overlap and
  resource-admission gates, which still `Defer` at cap 24.

## Related reading

- [Maximum safe parallelism](./maximum-safe-parallelism.md) — how coverage, the
  AIMD cap, and goal decomposition combine to fill spare machine capacity.
- [Goal coverage allocation API](./goal-coverage-allocation.md) — the allocator
  that consumes this cap.
- [Adaptive scaling API](./adaptive-scaling-api.md) — the AIMD scaler that
  supplies `current_max()`.
- [Resource-admission API](./resource-admission-api.md) — the disk/load gate
  that still bounds spawns at cap 24.
- [How to configure adaptive scaling](../howto/configure-adaptive-scaling.md) —
  operator guide for the ceiling and the `SIMARD_OODA_MAX_CONCURRENT` override.
