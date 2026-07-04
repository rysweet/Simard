---
title: amplihack freshness gate reference
description: Authoritative contract for the pre-spawn `amplihack update` gate — the two integration points, the three config variables, the flock lockfile and durable last-success state file, the lock/TTL ordering algorithm, the `amplihack_update_failure` metric, the four traced outcomes and their structured fields, the idle/liveness bound, and the failure-mode matrix.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/amplihack-freshness-gate.md
  - ../howto/configure-amplihack-freshness-gate.md
  - ./state-root-resolution.md
  - ./concurrent-engineer-dispatch.md
  - ./telemetry-metrics.md
  - ../safe-self-update.md
---

# amplihack freshness gate reference

The freshness gate runs `amplihack update` immediately before each engineer
subprocess is launched (and once at daemon startup), so every engineer runs on a
freshly-updated `amplihack-rs` install. This page is the authoritative contract.
For the rationale and the #439 incident narrative see the
[concept page](../concepts/amplihack-freshness-gate.md); for operator tasks see
the [how-to](../howto/configure-amplihack-freshness-gate.md).

> **Modules:** the per-spawn gate runs in
> `src/ooda_actions/advance_goal/spawn.rs::dispatch_spawn_engineer`, immediately
> before the `spawn_subordinate(&config)` call; the startup gate runs in
> `src/operator_commands_ooda/daemon/mod.rs::run_ooda_daemon`, after the
> runtime-dependency ensure. Both use the same lockfile and last-success state
> file under the resolved state root, and the failure metric is emitted through
> `self_metrics::record_metric`.

## Design in one paragraph

The gate acquires a cross-process advisory `flock(2)` over
`<state_root>/amplihack-update.lock`, re-reads a durable last-success timestamp
from `<state_root>/amplihack-update-state.json` **while holding the lock**, and
skips the update if the last success is within
`SIMARD_AMPLIHACK_UPDATE_TTL_SECS`; otherwise it runs `amplihack update` and, on
success, writes a new timestamp before releasing the lock. A failed update is
never silent: it logs at warn/error and records an `amplihack_update_failure`
metric, then by default proceeds to spawn on the last-known-good install, or —
under `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` — refuses the spawn with an explicit
error. Every decision is traced with an outcome and durations.

## Configuration

| Variable | Type | Default | Effect |
|---|---|---|---|
| `SIMARD_ENGINEER_AMPLIHACK_UPDATE` | bool (`0` disables) | **on** | Master switch for the gate. Default ON per operator directive. Set to `0` to disable the gate entirely — engineers then run on whatever `amplihack` is already installed, with no lock, no update, no TTL. |
| `SIMARD_AMPLIHACK_UPDATE_TTL_SECS` | integer seconds | **300** | Dedup window. If a **successful** `amplihack update` completed within this many seconds, the gate skips re-running (outcome `skipped-fresh`). |
| `SIMARD_REQUIRE_FRESH_AMPLIHACK` | bool (`1` enables) | **off** | Strict mode. When set to `1`, a failed update **hard-blocks** the spawn (outcome `blocked`) instead of proceeding on the last-known-good install. |

The `amplihack update` subprocess is guarded by an idle/liveness bound, not a
configurable wall-clock deadline — see [Subprocess liveness bound](#subprocess-liveness-bound).
It exposes no operator env var.

`state_root` is resolved as `SIMARD_STATE_ROOT` when set, else `$HOME/.simard`,
matching the engineer-worktree tree — see
[state-root resolution](./state-root-resolution.md).

## On-disk files

Both files live directly under the resolved state root.

| Path | Kind | Purpose |
|---|---|---|
| `<state_root>/amplihack-update.lock` | `flock(2)` advisory lockfile | Serializes `amplihack update` across processes. Advisory OS lock (`LOCK_EX`), **not** a bare existence check. Held only for the acquire → TTL-read → run/skip → timestamp-write critical section. |
| `<state_root>/amplihack-update-state.json` | JSON | Durable record of the last successful update, so the TTL survives across spawns and process restarts. |

### `amplihack-update-state.json` schema

```json
{ "last_success_epoch_secs": 1751645000 }
```

| Field | Type | Meaning |
|---|---|---|
| `last_success_epoch_secs` | `i64` | UNIX epoch seconds at which `amplihack update` last completed **successfully**. Written only on success, only while the lock is held. |

This single-field shape is the on-disk contract: operator tooling (see the
[how-to](../howto/configure-amplihack-freshness-gate.md)) reads
`last_success_epoch_secs` directly, so the implementation writes exactly this key.

Default absolute paths (no `SIMARD_STATE_ROOT`):

```
/home/<user>/.simard/amplihack-update.lock
/home/<user>/.simard/amplihack-update-state.json
```

## Lock / TTL ordering algorithm

The TTL is checked **under the lock** so a burst of spawners cannot each decide
"stale" and all run. In order:

1. **Gate enabled?** If `SIMARD_ENGINEER_AMPLIHACK_UPDATE=0`, skip everything and
   spawn on the current install — no lock, no update. The gate emits a single
   `disabled` trace (see [Tracing events](#tracing-events)) so the off state is
   observable; no per-spawn outcome from the table below is recorded.
2. **Acquire the lock.** `flock(LOCK_EX)` over `<state_root>/amplihack-update.lock`.
   Concurrent spawners block here — this is the serialization point. The advisory
   lock is released by the OS if the holding process dies, so a crash cannot
   strand it; a waiting spawner may block for up to the in-flight update's
   duration.
3. **Re-read the timestamp.** Read `last_success_epoch_secs` from
   `<state_root>/amplihack-update-state.json` (absent/unparseable ⇒ treat as "no
   prior success").
4. **Within TTL?** Compute `age_secs = now - last_success_epoch_secs`. If
   `age_secs <= SIMARD_AMPLIHACK_UPDATE_TTL_SECS`, **release the lock** and record
   outcome `skipped-fresh`. With no prior success the update **always runs** —
   there is nothing fresh to skip on.
5. **Otherwise run** `amplihack update` (the exact operator command), bounded
   only by the idle/liveness bound below.
6. **On success:** write the new `last_success_epoch_secs`, release the lock,
   record outcome `ran`.
7. **On failure:** release the lock, log at warn/error, record the
   `amplihack_update_failure` metric, then branch on
   `SIMARD_REQUIRE_FRESH_AMPLIHACK`:
   - unset/`0` ⇒ outcome `failed`, **proceed** to spawn on last-known-good;
   - `1` ⇒ outcome `blocked`, **refuse** the spawn with an explicit error.

The lock is always released — on the skip path, the success path, and every
failure/branch path — so it never strands a subsequent spawn.

**Infrastructure failures count as update failures.** If the gate cannot acquire
the lock or cannot write `amplihack-update-state.json` (for example, an
unwritable state root), it does **not** proceed silently: the condition is
treated as an update failure — logged at warn/error and recorded to the
`amplihack_update_failure` metric — then resolved through the same
`SIMARD_REQUIRE_FRESH_AMPLIHACK` branch as step 7 (`failed` and proceed by
default, `blocked` under strict mode).

**Startup gate.** The once-at-startup evaluation in `run_ooda_daemon` follows the
same algorithm and the same failure branch: by default a failed startup update is
surfaced (`failed`) and the daemon continues on the last-known-good install;
under `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` the failure is `blocked` and surfaced,
and no engineer is spawned until a fresh update succeeds. Strict mode gates
engineer spawns, not daemon boot itself.

## The `amplihack_update_failure` metric

On any update failure the gate calls:

```rust
self_metrics::record_metric("amplihack_update_failure", 1.0, context);
```

`record_metric(metric_name: &str, value: f64, context: &str)` appends one JSONL
record to `~/.simard/metrics/metrics.jsonl` (see the
[telemetry metrics reference](./telemetry-metrics.md) for the wider metric
surface).

| Element | Value | Meaning |
|---|---|---|
| `metric_name` | `amplihack_update_failure` | One update failure occurred at the gate. |
| `value` | `1.0` | One failure occurrence (count-style marker; sum over a window = failures in that window). |
| `context` | short string | Names the failure cause and the resulting decision — whether the engineer will run on the **last-known-good** install (`failed`) or the spawn was **blocked** under strict mode (`blocked`). |

A `MetricEntry` row therefore looks like:

```json
{"timestamp":"2026-07-04T16:05:12.481Z","metric_name":"amplihack_update_failure","value":1.0,"context":"amplihack update failed (build error); proceeding on last-known-good install"}
```

The metric is **never** the only signal — it always accompanies a warn/error log
line. There is no code path where a failure is recorded to neither.

## Tracing events

Every decision is emitted through the `tracing` crate — **never** `println!` or
`eprintln!` — on target `simard::amplihack_update`. The target and the outcome
tokens below are part of the contract: the implementation emits them verbatim and
the operator recipes in the
[how-to](../howto/configure-amplihack-freshness-gate.md) grep for them literally.
Exactly one of these outcomes is recorded per gate evaluation:

| `outcome` | Level | When |
|---|---|---|
| `ran` | info | update executed and succeeded; fresh timestamp written |
| `skipped-fresh` | info/debug | a successful update is within the TTL; update skipped |
| `failed` | warn/error | update ran but failed; default proceeds on last-known-good |
| `blocked` | error | update failed **and** `SIMARD_REQUIRE_FRESH_AMPLIHACK=1`; spawn refused |

One additional non-outcome event exists: when the gate is disabled
(`SIMARD_ENGINEER_AMPLIHACK_UPDATE=0`) it emits a single `disabled` trace (once
at startup) so an operator can confirm freshness is off. It is not one of the
four per-spawn outcomes and carries no update/lock fields.

Structured fields carried on the event:

| Field | Type | Meaning |
|---|---|---|
| `outcome` | enum string | one of `ran` / `skipped-fresh` / `failed` / `blocked` |
| `ttl_secs` | integer | the effective `SIMARD_AMPLIHACK_UPDATE_TTL_SECS` |
| `age_secs` | integer | `now - last_success_epoch_secs` (absent when there is no prior success) |
| `update_duration_ms` | integer | wall-clock time spent in the `amplihack update` subprocess (present for `ran` / `failed` / `blocked`) |
| `gate_duration_ms` | integer | wall-clock time for the whole gate decision, including lock wait |
| `require_fresh` | bool | the effective `SIMARD_REQUIRE_FRESH_AMPLIHACK` |
| `error` | string | the failure detail (present for `failed` / `blocked`) |

## Subprocess liveness bound

If the `amplihack update` subprocess is bounded at all, the bound is an
**idle/liveness** bound — it fires only when the subprocess stops making
progress, never as a fixed total-runtime deadline. A build or network fetch that
is still making progress is **never** aborted mid-work. If the bound is ever hit
(the update has genuinely stalled), its expiry is surfaced **explicitly** as a
`failed` outcome (warn/error log + `amplihack_update_failure` metric), identical
to any other update failure — never a silent kill. The concrete mechanism and any
tunable is left to the implementation; no fixed wall-clock default is promised
here.

## Failure-mode matrix

| Situation | Decision outcome | Log + metric | Spawn proceeds? |
|---|---|---|---|
| Update succeeds (or a fresh success is within TTL) | `ran` / `skipped-fresh` | info trace; no failure metric | **yes**, on a fresh install |
| Update fails, default mode | `failed` | warn/error log **+** `amplihack_update_failure` | **yes**, on last-known-good install (surfaced, not silent) |
| Update fails, `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` | `blocked` | error log **+** `amplihack_update_failure` | **no** — explicit error outcome |
| Gate disabled (`SIMARD_ENGINEER_AMPLIHACK_UPDATE=0`) | `disabled` (not a per-spawn outcome) | single `disabled` trace; no failure metric | **yes**, on whatever is already installed |

## See also

- [The amplihack freshness gate](../concepts/amplihack-freshness-gate.md) — why
  the gate runs before every spawn and how honest degradation differs from a
  silent fallback.
- [Configure the amplihack freshness gate](../howto/configure-amplihack-freshness-gate.md)
  — operator tasks and diagnostics.
- [State-root resolution](./state-root-resolution.md) — how `<state_root>`
  resolves for the lockfile and state file.
- [Concurrent engineer dispatch](./concurrent-engineer-dispatch.md) — the
  spawn-burst path the lock serializes.
- [Telemetry metrics reference](./telemetry-metrics.md) — the wider metric
  surface `metrics.jsonl` participates in.
