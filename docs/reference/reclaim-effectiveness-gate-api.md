---
title: Reclaim effectiveness gate — API reference
description: >
  The typed surface of the disk-reclaim effectiveness gate (#4809 / #4825 /
  #4810): the `ReclaimEffectivenessGate` suppress-only cooldown primitive and
  its `EffectivenessDecision` enum in `src/disk_reclaim/effectiveness.rs`, the
  cross-cycle canonicalized skip-memory in `src/disk_reclaim/executor.rs`, the
  `SIMARD_DISK_RECLAIM_COOLDOWN_*` / `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT`
  configuration accessors, the new additive `emit_reclaim_telemetry` attributes,
  and how the gate is wired into the OODA daemon disk-reclaim trigger.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/reclaim-effectiveness-backoff.md
  - ./disk-reclaim-api.md
  - ./disk-reclaim-telemetry.md
  - ./overseer-backoff-gate-api.md
  - ../operations/reclaim-effectiveness-kill-switch.md
  - ../howto/configure-reclaim-effectiveness.md
---

# Reclaim effectiveness gate — API reference

> **Status: implemented (#4809 / #4825 / #4810).** The
> `ReclaimEffectivenessGate` and `EffectivenessDecision` types live in
> [`src/disk_reclaim/effectiveness.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_reclaim/effectiveness.rs);
> the cross-cycle skip-memory in
> [`src/disk_reclaim/executor.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_reclaim/executor.rs);
> the config accessors alongside the existing reclaim knobs in
> [`src/disk_reclaim/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_reclaim/mod.rs);
> the telemetry attributes in
> [`src/disk_reclaim/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_reclaim/mod.rs)
> (`emit_reclaim_telemetry`) with names in
> [`src/telemetry/names.rs`](https://github.com/rysweet/Simard/blob/main/src/telemetry/names.rs);
> and the daemon wiring in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs).
> For the rationale see [reclaim effectiveness backoff](../concepts/reclaim-effectiveness-backoff.md).

## `EffectivenessDecision`

```rust
/// The gate's verdict for the current daemon cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectivenessDecision {
    /// Run reclamation this cycle (unseen key, cooldown elapsed, effective last
    /// time, or hard-ceiling bypass).
    Run,
    /// Skip reclamation this cycle — a streak of no-op runs is in cooldown.
    Suppress,
}
```

## `ReclaimEffectivenessGate`

A suppress-only wrapper around the same bounded-exponential-backoff semantics as
[`BackoffGate`](./overseer-backoff-gate-api.md). It tracks a per-key no-op streak
and cooldown window and, crucially, **cannot alter the reclaim run's destructive
posture** — it only decides *whether* a run happens, never *how* it runs.

```rust
pub struct ReclaimEffectivenessGate { /* private */ }

impl ReclaimEffectivenessGate {
    /// base cooldown / growth multiplier / cap, plus the hard %-used ceiling
    /// above which suppression is always bypassed.
    pub fn new(
        base_window_secs: i64,
        multiplier: i64,
        max_window_secs: i64,
        hard_ceiling_pct: u8,
    ) -> Self;

    /// Decide WITHOUT recording. `used_pct` is a FRESH local df sample (never
    /// telemetry): at/above `hard_ceiling_pct` this always returns `Run`
    /// (bypassing any cooldown). Otherwise an unseen key, an elapsed cooldown,
    /// or a backwards clock jump returns `Run`; a re-hit strictly inside the
    /// current cooldown returns `Suppress`.
    pub fn peek(&self, key: &str, used_pct: u8, now_secs: i64) -> EffectivenessDecision;

    /// Record the OUTCOME of a run that actually happened. `effective == true`
    /// (bytes were freed or used_pct dropped) RESETS the streak and cooldown;
    /// `effective == false` grows the no-op streak and the cooldown window
    /// (× multiplier, saturating, capped). A silence ≥ 2× the current window
    /// since the last record also resets to the base window.
    pub fn record(&mut self, key: &str, effective: bool, now_secs: i64);
}
```

### Semantics

| Situation | `peek` result | Effect |
| --------- | ------------- | ------ |
| Key never seen | `Run` | first attempt always runs |
| `used_pct ≥ hard_ceiling_pct` | `Run` | **bypass** — genuine fill-up always reclaims |
| Cooldown window elapsed | `Run` | retry after backoff |
| Backwards clock jump | `Run` | fail toward surfacing; never suppress on an untrusted clock |
| Re-hit inside cooldown, below ceiling | `Suppress` | skip cycle |

| `record` input | Effect on streak / window |
| -------------- | ------------------------- |
| `effective = true` | reset streak → 0, cooldown → base |
| `effective = false` (first) | arm base cooldown |
| `effective = false` (subsequent) | window `× multiplier`, saturating, capped |
| silence ≥ 2× window | reset to base window |

The daemon **peeks then records** — it records the outcome only *after* a run
completes, so a run that is skipped (suppressed) never advances the streak, and
a run that errors is not counted as an effective reclaim.

### The dedup key

The daemon keys the gate on the reclamation target partition:

```rust
let key = format!("disk-reclaim:{}", state_root_partition_id);
```

so distinct partitions back off independently.

## Cross-cycle skip memory

`exec_reclaim` (`src/disk_reclaim/executor.rs`) now carries a set of
**canonicalized** candidate paths that a rail rejected on a previous cycle and
declines to re-propose them:

- Every candidate is `canonicalize`d **before** both the guard check and the
  skip-memory lookup, so a symlink or `..` alias cannot smuggle a
  previously-rejected (or protected) path past the guard.
- A candidate whose canonical path is in skip-memory is dropped without
  re-vetting; the corresponding `candidates_skipped` telemetry is still emitted
  so the human-review list stays complete.
- Any canonicalization failure treats the path as **not authorized to delete**
  (skip), never as authorized.

Skip-memory is bounded and evicts on TTL / effective-run reset so it cannot grow
unbounded on a long-running daemon.

## Configuration

All accessors fail safe (invalid input → the documented default) and are pure
functions of an injected `lookup: impl Fn(&str) -> Option<String>` for testing,
mirroring the existing reclaim/overseer config style.

| Env var | Default | Meaning |
| ------- | ------- | ------- |
| `SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE` | `on` | Master kill switch. `off` (case-insensitive) reverts to fire-every-over-threshold-cycle. See the [kill switch](../operations/reclaim-effectiveness-kill-switch.md). |
| `SIMARD_DISK_RECLAIM_COOLDOWN_BASE_SECS` | `900` | Base cooldown window after the first no-op run. |
| `SIMARD_DISK_RECLAIM_COOLDOWN_MULTIPLIER` | `2` | Growth factor per additional no-op run (`≥ 2`; lower values clamp to `2`). |
| `SIMARD_DISK_RECLAIM_COOLDOWN_MAX_SECS` | `14400` | Hard cap on the cooldown window (4 h). |
| `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT` | `97` | Locally-observed `%-used` at/above which suppression is bypassed and reclamation always runs. |

The pre-existing reclaim knobs are unchanged:
`SIMARD_DISK_RECLAIM_PCT` (trigger threshold, default `85`) and
`SIMARD_DISK_RECLAIM_DAEMON_APPLY` (the sole apply opt-in).

## Telemetry (additive)

`emit_reclaim_telemetry` gains three additive attributes on the existing
`simard.disk.reclaim.*` series plus one new counter. Existing metrics keep their
names and shapes — see [disk-reclaim telemetry](./disk-reclaim-telemetry.md) for
the full catalog.

| Name | Type | Attributes | Meaning |
| ---- | ---- | ---------- | ------- |
| `simard.disk.reclaim.suppressed_cycles` | counter | `source` | daemon cycles the effectiveness gate skipped (would-have-run-but-held). |
| *(existing series)* | — | `+ noop_streak`, `+ suppressed_cycles`, `+ effective` | current no-op streak, cumulative suppressed count, and whether the just-completed run freed space. Low-cardinality; **no raw paths, env, or secrets** are ever emitted as attributes. |

## Daemon wiring

In the Tier-3 disk-reclaim block of
`src/operator_commands_ooda/daemon/mod.rs`, the trigger now:

1. samples fresh `used_pct` via the existing `df` probe;
2. if `used_pct ≥ SIMARD_DISK_RECLAIM_PCT`, calls
   `gate.peek(key, used_pct, now)`;
3. on `Suppress`, logs a `WARN` line, increments
   `simard.disk.reclaim.suppressed_cycles`, and skips the run;
4. on `Run`, invokes `run_disk_reclaim(..)` as before, then calls
   `gate.record(key, report.was_effective(), now)` where `was_effective()` is
   `bytes_freed > 0 || used_pct_after < used_pct_before`.

The gate is a suppress-only pre-filter in front of the *unchanged*
propose/dispose path; every existing safety rail (protected paths, live
processes, uncommitted/unpushed, active worktree, allow-root, dry-run default)
runs exactly as before.

## Invariants (asserted by unit tests)

- **Suppress-only:** the gate can never turn a dry-run into an apply.
- **Ceiling bypass:** `used_pct ≥ hard_ceiling_pct` always returns `Run`, even
  deep inside a cooldown.
- **Effective reset:** an effective run immediately re-admits the next cycle.
- **Saturating counters:** streak/exponent never overflow.
- **Canonicalized skip-memory:** a symlink/`..` alias of a rejected path is
  still rejected.
- **Least-data telemetry:** new attributes carry only counts/booleans — no paths
  or env values.

## See also

- [Reclaim effectiveness backoff](../concepts/reclaim-effectiveness-backoff.md) — the rationale.
- [Disk reclaim API](./disk-reclaim-api.md) — the propose/dispose contract this pre-filters.
- [Overseer BackoffGate reference](./overseer-backoff-gate-api.md) — the shared backoff primitive.
- [Reclaim-effectiveness kill switch](../operations/reclaim-effectiveness-kill-switch.md) — how to disable it.
