---
title: Overseer tick self-healing reference
description: >
  Reference for the transient-failure self-healing rung added to the Overseer
  meta-thread's per-tick health derivation. A tick whose OODA cycle fails for a
  transient, externally-caused reason (upstream 5xx, timeout, connection reset,
  rate-limit) now routes the `overseer` meta-thread to a self-clearing
  `"backoff"` state for exactly one cadence instead of pinning it in
  `"erroring"`, while genuine (fatal / unknown / panicking) cycle failures still
  surface as `"erroring"`. Covers the additive `transient_cycle_failure` field,
  the fail-closed `is_transient` classifier, the bounded consecutive-transient
  escalation ceiling, the `overseer_meta` health mapping, configuration, and the
  safety invariants.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./overseer-activity-feed.md
  - ./overseer-tick-details.md
  - ./overseer-self-observation-stability.md
  - ../design/overseer.md
  - ../howto/watch-overseer-activity.md
  - ../../src/overseer/wiring.rs
  - ../../src/overseer/activity.rs
  - ../../src/operator_commands_ooda/daemon/mod.rs
---

# Overseer tick self-healing reference

> **Status: implemented.** This reference is the **binding contract** for the
> feature: every field, function, and health-mapping row below is shipped in the
> codebase. Documentation and implementation landed in the **same pull request**.
> See [`../design/overseer.md`](../design/overseer.md) for the design context.
>
> The change is purely additive to the post-#4080 baseline. It adds the
> `transient_cycle_failure` field and the `is_transient` classifier to
> [`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs);
> extends the `overseer_meta` health mapping in
> [`src/overseer/activity.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/activity.rs);
> adds the `OVERSEER_TRANSIENT_BACKOFF_CEILING` accessor to
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs);
> and threads the transient signal plus a consecutive-transient counter through
> the daemon caller in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs).

The acting **Overseer** runs its own Observe → Orient → Decide → Act loop once
per cadence. The daemon derives the synthetic `overseer` meta-thread's health
from the just-completed [`OverseerTickReport`](./overseer-activity-feed.md).
Before this change, any cycle failure — including a purely transient upstream
outage such as a GitHub `503` during the Observe read — set `cycle_failed` and
drove the meta-thread's health to `"erroring"`, where it stayed until a fully
clean tick landed. A single flaky upstream response could therefore leave the
meta-thread looking permanently broken even though the very next tick would
have recovered on its own.

This reference specifies the **self-healing rung**: a transient, externally
caused cycle failure now routes the meta-thread to a **self-clearing
`"backoff"`** state for one cadence rather than `"erroring"`. Genuine failures
— panics, unknown errors, or a transient condition that will not clear — still
surface as `"erroring"`, loudly and without suppression.

For the base health ladder and the feed schema see the
[Overseer activity feed reference](./overseer-activity-feed.md); for the
`cycle_failed` discriminator that this builds on see the same reference (fields
`panicked` / `cycle_failed`, added in #4080).

## Contents

- [The problem this closes](#the-problem-this-closes)
- [The `transient_cycle_failure` field](#the-transient_cycle_failure-field)
- [Transient classification (`is_transient`)](#transient-classification-is_transient)
- [Health mapping](#health-mapping)
- [Bounded self-healing (the escalation ceiling)](#bounded-self-healing-the-escalation-ceiling)
- [How a tick flows through the states](#how-a-tick-flows-through-the-states)
- [What operators see](#what-operators-see)
- [Configuration](#configuration)
- [Safety invariants](#safety-invariants)
- [What is unchanged](#what-is-unchanged)
- [Tests](#tests)

## The problem this closes

The `overseer` meta-thread's health is derived on every tick by
[`OverseerThreadStatus::overseer_meta`](https://github.com/rysweet/Simard/blob/main/src/overseer/activity.rs)
from a single boolean the daemon feeds it. Post-#4080 that boolean is
`last_success = !panicked && !cycle_failed`, and `derive_health` maps a
`false` into `"erroring"` (via `consecutive_errors > 0`).

`cycle_failed` is set whenever `run_cycle()` returns `Err`. But not all cycle
errors are equal:

- A **panic**, a logic bug, or an unrecognized error is a genuine defect that
  an operator must see and act on. `"erroring"` is correct.
- A **transient upstream failure** — the GitHub Actions REST API returning
  `503`, a socket timeout, a connection reset, a rate-limit response — is not a
  Simard defect. The next tick, one cadence later, will almost always succeed.
  Pinning the meta-thread in `"erroring"` for such a blip produces a false
  "steward is down" signal and drowns real failures in noise.

The self-healing rung distinguishes these two classes at the tick boundary and
routes the transient class to a **bounded, self-clearing `"backoff"`** — the
same rung the scheduler already uses for cognitive threads that are
deliberately waiting out a retry window.

## The `transient_cycle_failure` field

A single additive boolean is added to
[`OverseerTickReport`](./overseer-activity-feed.md):

```rust
/// Additive. `true` only when `cycle_failed` is set AND the underlying
/// `run_cycle()` error was classified transient/externally-caused by
/// `is_transient` (upstream 5xx, timeout, connection reset, rate-limit).
///
/// A transient cycle failure routes the `overseer` meta-thread to a
/// self-clearing `"backoff"` for one cadence instead of `"erroring"`.
/// A panic, an unknown error, or any non-transient failure leaves this
/// `false`, so the meta-thread still surfaces as `"erroring"`.
///
/// Covered by the struct-level `#[serde(default)]`: legacy feed records
/// written before this field existed deserialize with
/// `transient_cycle_failure = false` (the conservative, fatal default), and
/// the feed `SCHEMA_VERSION` does not change.
pub transient_cycle_failure: bool,
```

Key properties:

- **Never set alone.** `transient_cycle_failure = true` implies
  `cycle_failed = true` and `panicked = false`. A panic is always fatal and
  keeps `transient_cycle_failure = false`.
- **Cheap and `Clone`.** `OverseerTickReport` derives
  `Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize` (it is **not**
  `Copy` — it already carries `Vec<String>` detail fields). Adding one `bool`
  keeps every existing derive intact and adds no new bound.
- **Serde-default-safe.** The default is `false` (fatal). A truncated or
  tampered feed record can only ever downgrade *toward* `"erroring"`, never
  toward a masked transient. See [Safety invariants](#safety-invariants).
- **Surfaced, not silent.** The field is emitted on the structured `tracing`
  event for the tick and included in the daemon's human `daemon_log` line
  alongside `panicked` and `cycle_failed`.

## Transient classification (`is_transient`)

A pure, fail-closed helper in `wiring.rs` decides whether a `run_cycle()` error
is transient:

```rust
/// Fail-closed transient classifier. Returns `true` ONLY for an
/// `OverseerError::Capability` whose `detail` matches a known-retryable
/// marker (upstream 5xx / 502 / 503, timeout, connection reset, rate-limit).
/// Every other error — and any unknown/unmatched detail — returns `false`
/// (treated as fatal, so the meta-thread stays `"erroring"`).
fn is_transient(err: &OverseerError) -> bool
```

**Fail-closed allowlist.** Only errors that *explicitly* match a
known-transient marker are treated as transient. The wildcard arm returns
`false`. This means:

- An error we recognize as an external blip → `backoff` (self-heal).
- An error we do **not** recognize → `erroring` (surfaced, never masked).

The classifier matches only the `OverseerError::Capability` variant, which
carries `{ what: &'static str, detail: String }`. Classification is a
case-insensitive substring match against `detail` (the human message from the
underlying capability call); `what` is a stable static category and may be used
to scope the match. `OverseerError` derives `Clone, Debug, PartialEq` (not
`Eq` — the `Budget` variant carries `f64`), so the classifier takes it by
reference and never moves or clones it. The recognized `detail` markers
include:

| Marker class      | Example `detail` fragments                         |
| ----------------- | -------------------------------------------------- |
| HTTP 5xx          | `500`, `502`, `503`, `504`, `bad gateway`, `service unavailable` |
| Timeout           | `timed out`, `timeout`, `deadline exceeded`        |
| Connection reset  | `connection reset`, `connection refused`, `broken pipe`, `eof` |
| Rate-limit        | `rate limit`, `too many requests`, `429`, `secondary rate limit` |

Errors that are **not** transient by construction (and therefore stay
`"erroring"`): validation failures, gate refusals, parse errors, `4xx` other
than `429`, missing-capability (`None` capability) errors, and anything whose
detail does not match a marker.

The `run_cycle` `Err(e)` arm in `overseer_tick` sets both fields:

```rust
Err(e) => {
    report.errors += 1;
    report.cycle_failed = true;
    report.transient_cycle_failure = is_transient(&e);
    tracing::warn!(
        target: "overseer::tick",
        error = %e,
        transient = report.transient_cycle_failure,
        "overseer run_cycle failed — isolated, no actions taken"
    );
}
```

The `warn!` is preserved (log-and-recover). No `print!`/`println!` is used —
structured `tracing` + OpenTelemetry only.

## Health mapping

`overseer_meta` accepts the transient signal in addition to `last_success`.
The mapping, evaluated per tick (first match wins):

| Tick outcome                                              | `last_success` | `transient_cycle_failure` | Meta-thread health |
| -------------------------------------------------------- | :------------: | :-----------------------: | ------------------ |
| Cycle completed (isolated `act()` errors allowed)        | `true`         | `false`                   | `"ok"`             |
| Cycle failed, transient, within the ceiling              | `false`        | `true`                    | `"backoff"`        |
| Cycle failed, transient, **ceiling exceeded**            | `false`        | `true`                    | `"erroring"`       |
| Cycle failed, non-transient / unknown                    | `false`        | `false`                   | `"erroring"`       |
| Tick panicked                                            | `false`        | `false`                   | `"erroring"`       |

`"backoff"` is produced by setting `backoff_until = now + cadence` with
`consecutive_errors = 0`, so the existing
[`derive_health`](./overseer-activity-feed.md) ladder (`backoff_until.is_some()`
→ `"backoff"`, which is checked *before* `consecutive_errors > 0`) yields
`"backoff"` without any new label. The state **self-clears**: one cadence later,
the next tick recomputes health from scratch. A clean tick → `"ok"`; a repeated
transient → `"backoff"` again (until the ceiling); a fatal error → `"erroring"`.

`derive_health` itself is unchanged in shape — the ladder already distinguishes
a self-healing `"backoff"` from a stuck `"erroring"`. Only `overseer_meta`'s
input handling and docstring change.

## Bounded self-healing (the escalation ceiling)

Unbounded self-healing is a hazard: if an upstream dependency is *actually*
down, every tick will classify transient and the meta-thread would sit in
`"backoff"` forever, masking a real, sustained outage.

The self-healing rung is therefore **bounded** by a consecutive-transient
counter with a hard ceiling.

> **Where the counter lives.** `overseer_meta` is a *stateless* per-tick
> constructor — it recomputes the health label from scratch every cadence and
> keeps no memory between ticks (this is what makes a `"backoff"` self-clear).
> A consecutive-transient counter therefore **cannot** live inside
> `overseer_meta`; it must be owned by the one component with durable
> cross-tick state: the **daemon loop** in
> [`operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs).
> The loop holds a `consecutive_transient: u32` alongside its other per-iteration
> state, updates it after each tick, and passes the *current* value (plus the
> transient signal) into the extended `overseer_meta` call. `overseer_meta`
> stays pure: given `(cadence, last_success, transient_cycle_failure, counter,
> ceiling)` it deterministically returns the row. The label is recomputed each
> tick; only the counter persists.

The counter rules:

- Each consecutive tick that fails transiently increments the daemon's counter.
- While the counter is **at or below** `OVERSEER_TRANSIENT_BACKOFF_CEILING`
  (default **3**), the health is `"backoff"`.
- Once the counter **exceeds** the ceiling, the transient failure is escalated
  to `"erroring"` — a dependency that has been "transiently" failing for
  several cadences in a row is treated as genuinely down and surfaced.
- **Any** successful tick (`last_success = true`) resets the counter to `0`.
- A **non-transient / panic** failure does not increment the transient counter
  (it is already `"erroring"` on its own rung).

This guarantees that a persistent outage cannot hide behind the transient rung
for more than `ceiling` cadences.

## How a tick flows through the states

```
tick completes (no cycle failure)         → last_success=true             → "ok"      (counter reset to 0)
run_cycle Err, is_transient=true, n≤3      → backoff_until=now+cadence      → "backoff" (counter = n)
run_cycle Err, is_transient=true, n>3      → consecutive_errors=1           → "erroring"
run_cycle Err, is_transient=false          → consecutive_errors=1           → "erroring"
tick panicked                              → consecutive_errors=1           → "erroring"
```

Example timeline with the default ceiling of 3 (cadence = 15 min):

| Tick | run_cycle result                    | transient | counter | health     |
| ---- | ----------------------------------- | :-------: | :-----: | ---------- |
| 1    | Ok                                  | –         | 0       | `ok`       |
| 2    | Err: GitHub `503`                   | yes       | 1       | `backoff`  |
| 3    | Err: GitHub `503`                   | yes       | 2       | `backoff`  |
| 4    | Ok                                  | –         | 0       | `ok`       |
| 5    | Err: timeout                        | yes       | 1       | `backoff`  |
| 6    | Err: timeout                        | yes       | 2       | `backoff`  |
| 7    | Err: timeout                        | yes       | 3       | `backoff`  |
| 8    | Err: timeout                        | yes       | 4       | `erroring` |
| 9    | Err: parse failure (non-transient)  | no        | 5       | `erroring` |

## What operators see

The change is transparent to every read-only surface — the dashboard
**Overseer** tab, the TUI **Overseer** pane, and `simard status OVERSEER` all
render the derived `health` string. `"backoff"` is an **existing** label those
surfaces already display for scheduler threads, so no consumer code changes.

`simard status` prints one line per thread — `thread <id> <on|off> · last
<run> · next <due> · <health>` (see
[`src/status/render.rs`](https://github.com/rysweet/Simard/blob/main/src/status/render.rs)).
During a transient blip the `overseer` row therefore reads:

```
  thread overseer          on  ·  last 2026-07-16T22:52:09Z  ·  next 2026-07-16T23:07:09Z  ·  backoff
```

A genuine failure shows the same row shape with `erroring` in the health slot:

```
  thread overseer          on  ·  last 2026-07-16T22:52:09Z  ·  next 2026-07-16T23:07:09Z  ·  erroring
```

The transient-vs-fatal *reason* is not on this row (the renderer emits only the
one-word `health`). The distinguishing detail — the classified error and its
`transient` flag — is carried on the structured `tracing`/OTel event for the
tick and in the daemon's `daemon_log` line, not in `simard status`. Surfacing a
human "self-healing (upstream 503)" hint in `status` would require a *separate*
change to `render.rs` and is intentionally **out of scope** here.

## Configuration

| Variable                              | Meaning                                                                                                   | Default |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------- | :-----: |
| `OVERSEER_TRANSIENT_BACKOFF_CEILING`  | Number of consecutive transient cycle failures tolerated as `"backoff"` before escalating to `"erroring"`. Set to `0` to disable the self-healing rung entirely (every cycle failure is then `"erroring"`, the pre-change behavior). | `3`     |

The backoff window itself is the meta-thread **cadence** (the same
`feed_cadence_secs` used to compute `next_due`); there is no separate
backoff-duration knob.

**Config plumbing.** Overseer tunables follow a two-function convention in
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs):
a testable `*_from(lookup: impl Fn(&str) -> Option<String>)` that takes an
injected lookup closure, plus a thin production wrapper `*()` that calls it with
`|k| std::env::var(k).ok()` (as `overseer_enabled` / `overseer_acting_enabled` /
`gap_scan_every_n` already do). Add
`overseer_transient_backoff_ceiling_from(lookup) -> u32` (default `3`,
`0` = disabled) and its `overseer_transient_backoff_ceiling()` wrapper — mirror
the existing numeric accessor `gap_scan_every_n_from` — so the knob stays
unit-testable with an injected closure and consistent with the existing overseer
config surface. Do **not** read it via an ad-hoc `std::env::var` at the tick or
daemon site.

## Safety invariants

- **SR-1 Fail-closed classification.** `is_transient` returns `false` for the
  wildcard arm; only an explicit known-transient marker routes to `"backoff"`.
  An unrecognized error is always fatal. A dedicated unit test pins the
  "wildcard-is-fatal" invariant.
- **SR-2 Bounded self-healing.** The consecutive-transient ceiling escalates a
  never-clearing transient to `"erroring"`, so an infinite `always transient`
  state cannot mask a hard-down dependency. Reset on any successful tick.
- **SR-3 No sensitive data in error surfaces.** Classification and logging use
  the error *kind/category* via structured `tracing` fields, never raw upstream
  response bodies or credential-bearing headers. `{:?}` is not used on error
  types that may carry auth context.
- **SR-4 Preserve observability.** `report.errors`, `report.memory_errors`, and
  the transient signal remain surfaced in the feed and on the `tracing`/OTel
  event. A recovered/transient tick is `"backoff"`, but the underlying error is
  still counted and logged — log-and-recover, never silence.
- **SR-5 Serde-default safety.** `transient_cycle_failure` is
  `#[serde(default)] = false`, the conservative (fatal) default. A tampered or
  truncated report cannot downgrade a real failure into a masked transient. A
  regression test asserts the default is `false`.

## What is unchanged

- **`derive_health` shape and label set.** No new health label; `"backoff"`
  already existed. The ladder order (`disabled` → `backoff` → `erroring` →
  `idle` → `ok`) is unchanged.
- **`cycle_failed` semantics (#4080).** A cycle failure still sets
  `cycle_failed`. The new field only *refines* how a `cycle_failed` tick maps
  to health.
- **Isolated `act()` errors.** An isolated per-intervention capability error
  still increments `errors`, still leaves `cycle_failed = false`, and still
  yields `"ok"`. This rung concerns only whole-cycle (`run_cycle` `Err`)
  failures.
- **Feed `SCHEMA_VERSION`.** Unchanged; the new field is `#[serde(default)]`.
- **PRD and tick semantics.** Additive and non-breaking. No `Bridge` naming; no
  `print!`/`println!` — structured `tracing` + OpenTelemetry only.

## Tests

Regression coverage will live in the `#[cfg(test)]` modules of
`src/overseer/wiring.rs` (classification) and `src/overseer/activity.rs`
(health mapping):

- `is_transient` returns `true` for each recognized marker class (5xx,
  timeout, connection reset, rate-limit) and `false` for the wildcard arm
  (**SR-1**).
- A transient `run_cycle` `Err` within the ceiling → meta-thread health
  `"backoff"` with `backoff_until` one cadence out.
- Repeated transient failures beyond `OVERSEER_TRANSIENT_BACKOFF_CEILING` →
  `"erroring"` (**SR-2**); a subsequent successful tick resets to `"ok"`.
- A non-transient `run_cycle` `Err` → `"erroring"` (unchanged).
- A panicking tick → `"erroring"` with `transient_cycle_failure = false`.
- A legacy feed record without `transient_cycle_failure` deserializes to
  `false` (**SR-5**), and the feed `SCHEMA_VERSION` string is unchanged.

Validate locally with `cargo test overseer`; rely on PR-level
`statusCheckRollup` for CI (the default-branch Actions REST API returned `503`
at the time of writing — itself an example of the transient class this rung
handles).
