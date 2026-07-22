---
title: "Diagnosable canary-red + red-canary loop-halt (self-deploy)"
description: Reference for the two additive rails that make an autonomous self-deploy canary refusal DIAGNOSABLE (the canary's own failure detail — including the named failing gate when the canary reports one — reaches operator notifications, the Capability error, and OTel) and STOP the silent crash-loop drift growth (a bounded consecutive-red-canary halt that suppresses re-issuing a persistently-red SHA and raises a one-shot operator "stuck" escalation). Covers enrich_refusal, the RedCanaryStreak process-global counter recorded at the Act/canary site and read in observe_deploy_drift, the SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT env var, the deploy_drift.stuck tracing target, and the operator playbook.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ./self-deploy-api.md
  - ./overseer-operator-notifications.md
  - ../safe-self-update.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/overseer/deploy.rs
  - ../../src/overseer/deploy_trigger.rs
  - ../../src/overseer/mod.rs
---

# Diagnosable canary-red + red-canary loop-halt

> **Status: implemented.** Both rails ship in the Overseer self-deploy
> subsystem: the diagnosability enrichment and the point where each
> canary result is **recorded** live in
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)
> (the Act/canary site, where `canary.passed` is known), the
> consecutive-red counter (`RedCanaryStreak`) in
> [`src/overseer/deploy_trigger.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_trigger.rs),
> and the loop-halt **read/suppress/escalate** wiring in `observe_deploy_drift` in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> Both are **additive and non-breaking**: default behaviour is unchanged for a
> green canary, and every existing gate (`origin_strict`, the
> `autonomous_deploy_enabled` opt-out, no-op / rollback / crash-loop refusals,
> and the min-interval anti-thrash guard) keeps its original authority and
> ordering.

## Why

The autonomous self-deploy loop
([reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md))
detects the daemon drifting behind merged `origin/main`, canary-builds the
target SHA, and swaps the binary only if every gate passes. When the canary
goes **red**, the deploy is correctly refused — but two failure modes made a
persistently-red canary *actively harmful* instead of merely stalled:

1. **The refusal was not diagnosable.** The operator notice and the
   `Capability` error surfaced only the bare aggregate
   `deploy_gate: red canary (one or more gates failed)`. An operator could see
   *that* the canary was red, but not *which* gate failed or *why* — so a
   stuck deploy required manual log spelunking on the canary build host.

2. **The loop re-attempted the same red SHA every tick, forever.** Nothing
   stopped the OODA daemon from re-observing the same drift, re-building the
   same failing canary, and re-refusing it on every cycle while
   `DeployDrift` silently grew (e.g. `1 → 6` commits behind main over hours).
   The running binary went progressively more stale with no operator-visible
   escalation — a silent crash-loop.

These two rails address exactly those failures, and nothing more. They do
**not** try to make a red canary pass, and they do **not** reimplement the
durable per-SHA backoff ledger that ships separately in the deploy-throttle
work — they compose with it (see [Relationship to the durable
deploy-throttle ledger](#relationship-to-the-durable-deploy-throttle-ledger)).

---

## Rail A — Diagnosable canary-red

### What changed

The `RedCanary` refusal path now carries the canary's own `detail` string all
the way to the operator. Where the deploy gate previously discarded
`CanaryResult::detail` (see the [Self-deploy API
reference](./self-deploy-api.md)) and surfaced only
`DeployRefusal::RedCanary`'s bare `Display`, the refusal notice and the
`OverseerError::Capability { detail }` now report **whatever the canary itself
recorded about the failure** — which names the failing gate and its reason
*when the canary produced a gate-level failure*.

`CanaryResult::detail` encodes the failure at its source
(`src/overseer/deploy.rs`, `ProdCanaryRunner::run_canary`), and has **three
shapes**:

| Branch (`run_canary`)              | `detail` shape                               | Names the gate? |
| ---------------------------------- | -------------------------------------------- | --------------- |
| `Err(GateFailed { gate, detail })` | `target canary gate <gate> failed: <reason>` | **Yes**         |
| `Err(BuildFailed { detail })`      | `target canary build failed: <reason>`       | No (build-level)|
| `Ok(report)` (red aggregate)       | `<passed>/<total> gates`                     | No (count only) |

> **Diagnosability is conditional on the canary's own granularity.** Rail A
> faithfully forwards whatever `detail` the canary produced; it does **not**
> synthesize a gate name. When `run_canary` returns the `Ok(report)` red
> aggregate, the enriched reason is only `red canary (…): <passed>/<total>
> gates` — still an improvement over the bare aggregate, but it does **not**
> name the failing gate. The named-gate outcome requires the canary to surface
> a `GateFailed` result. Widening the `Ok(report)` count path to also carry
> per-gate names is a **canary-side** enhancement tracked separately and is out
> of scope for this rail.

Rail A routes the `detail` string into the refusal so it reaches the operator
and OTel instead of being dropped.

### Before / after

Before (aggregate only — not diagnosable):

```text
deploy 4be8c6f955df failed — deploy_gate: red canary (one or more gates failed)
```

After (`GateFailed` — named gate + reason, fully diagnosable):

```text
deploy 4be8c6f955df failed — deploy_gate: red canary (one or more gates failed): target canary gate clippy failed: 2 warnings denied
```

After (`Ok(report)` red aggregate — improved but gate not named):

```text
deploy 4be8c6f955df failed — deploy_gate: red canary (one or more gates failed): 3/5 gates
```

The enrichment is `{refusal}: {canary.detail}`. For every **non-`RedCanary`**
refusal (`NoOp`, `Rollback`, `CrashLoop`) the message is **unchanged** — those
refusals have no canary detail to add, so their `Display` is passed through
verbatim.

### Structured telemetry

The enriched reason is emitted as a **structured tracing field**, never spliced
into a message template:

```rust
tracing::warn!(
    target: "deploy",
    sha = %target,
    running = %running,
    reason = %reason,          // the enriched, named-gate string
    "self-deploy refused",
);
```

Because `reason` is subprocess-derived (it originates from the canary build
output), it is:

- carried only as a `%reason` field value, so a malicious/garbage canary
  message can never inject a fake log line or notification template, and
- **truncated to a fixed cap** (`REASON_DETAIL_CAP`, 512 bytes, on a UTF-8
  char boundary) before it is attached to the operator notice or the
  `Capability` error, bounding notification and log size.

### Where it surfaces

The same enriched string reaches **all three** operator-facing sinks:

1. `OperatorNotification::deploy_refused(target, running, repo, reason)` —
   the dual-channel (email + Signal) operator notice.
2. `OverseerError::Capability { what: "deploy_gate", detail: reason }` — the
   error returned up the ACT path.
3. The `target: "deploy"` structured tracing span → OTel.

---

## Rail B — Red-canary loop-halt escalation

### What changed

A persistently-red canary for the *same* target SHA no longer re-signals drift
silently forever. The mechanism is split across the two OODA phases where the
relevant facts are actually known:

- **Record (Act path, `deploy.rs`).** The canary is only *built* during a
  deploy attempt, so `canary.passed` is known only at the Act/canary site — the
  same site Rail A enriches. Immediately after the canary result is available,
  that site calls `record_red_canary_result(target_sha, is_red)` to update the
  streak. This is the **only** place a result is recorded.
- **Read + suppress + escalate (Observe path, `observe_deploy_drift`).** The
  next Observe tick *reads* the streak for the probed drift SHA via
  `red_canary_streak_for(sha)`. It never records; it only decides, based on the
  streak already accumulated by prior Act ticks, whether to suppress and
  escalate.

After a bounded number of **consecutive** recorded red-canary results for the
same SHA, the Observe path:

1. **suppresses re-issuing that SHA's deploy signal** — so `DeployDrift` stops
   silently growing on a SHA that will never deploy, and
2. **raises a one-shot operator `stuck` escalation** — a single dual-channel
   `OperatorNotification` plus a structured
   `tracing::warn!(target: "deploy_drift.stuck", …)`.

The suppression is **narrow**: only the specific stuck SHA's *deploy signal* is
suppressed. The **operator escalation is never suppressed** — a stuck SHA is
always made operator-visible. Below-threshold drift, and drift for any *other*
SHA, continue to signal and deploy normally.

> **Why the split matters.** An earlier design recorded the result inside
> `observe_deploy_drift`. That is incorrect: Observe runs *before* the canary
> is built this tick, so it would key off a **stale or previous** canary
> result. Recording strictly at the Act/canary site — where the fresh
> `canary.passed` exists — and reading in Observe keeps the streak honest.

### The consecutive-red counter

Loop-halt state is a process-global, poison-tolerant per-SHA consecutive-red
counter, `RedCanaryStreak`, mirroring the existing
`global_deploy_throttle_allow` static. It is process-global (a `static`)
because the daemon rebuilds the acting `Overseer` every tick, so per-instance
state could never accumulate a streak.

- A **red** recorded result for SHA `X` (at the Act/canary site) increments
  `X`'s streak.
- A **green** recorded result, or recording against a **different** target SHA,
  **resets** the streak (and re-arms the one-shot escalation). At most one SHA
  has an active streak entry in the steady state, so the map does not grow
  unbounded.

### The one-shot latch

The `stuck` escalation fires **exactly once per stuck SHA**. It re-arms only
when the streak resets — i.e. when the canary goes green or the target SHA
changes. This prevents alert flooding: a SHA that stays red for 200 ticks
produces **one** operator alert, not 200.

### Ordering: where recording and reading happen

Recording and reading live in **different** OODA phases:

**Act path (`deploy.rs`, canary/refusal site) — RECORD only:**

```text
deploy attempt for target SHA
  … build canary → canary.passed known …
  ├─ Rail A: enrich the RedCanary refusal with canary.detail
  └─ Rail B: record_red_canary_result(target_sha, is_red)   ← the ONLY writer
```

**Observe path (`observe_deploy_drift`) — READ + suppress + escalate only:**

Rail B slots in **after** the existing layer-1 anti-thrash
`global_deploy_throttle_allow` guard and the live crash-loop-churn guard, so
it never weakens any authoritative safety gate. The gate ordering is preserved:

```text
observe_deploy_drift
  1. autonomous_deploy_enabled  — operator opt-out pins the daemon
  2. observer wired?            — inert until the sensor exists
  3. global_deploy_throttle_allow — min-interval anti-thrash
  4. crash-loop-churn guard     — never deploy into a restart storm
  5. probe drift (fail-safe: None on any git error)
     └─ Rail B (READ ONLY): red_canary_streak_for(drift.sha); if it has already
        reached the threshold (from prior Act ticks), SUPPRESS this SHA's signal
        + fire the one-shot `stuck` escalation. Never records here.
```

Rail B can only ever **suppress** a deploy signal and **add** an operator
escalation. It can never convert a suppression into a deploy, and it never
suppresses the escalation itself. The observer stays fail-safe end to end: any
git/source error still maps to "no drift" → no signal → never a blind deploy.

### The `deploy_drift.stuck` tracing target

The escalation emits a dedicated structured tracing target so operators and
dashboards can alert on it directly:

```rust
tracing::warn!(
    target: "deploy_drift.stuck",
    sha = %target_sha,
    streak,                    // consecutive red-canary count
    threshold,                 // the configured halt threshold
    behind_commits,            // current DeployDrift depth
    "self-deploy halted: canary persistently red for the same SHA",
);
```

The `deploy_drift.stuck` target is **distinct** from any target used by the
durable deploy-throttle work, so the two never collide (see below).

---

## Configuration

### `SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT`

Optional environment variable setting the number of **consecutive** red-canary
observations of the same SHA that trip the loop-halt. The name uses the
established `SIMARD_OVERSEER_DEPLOY_*` namespace shared by the sibling
self-deploy controls below.

| Property        | Value                                                        |
| --------------- | ----------------------------------------------------------- |
| Env var         | `SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT`                    |
| Default         | `3`                                                         |
| Floor           | `2` (values below the floor are clamped up via `.max(2)`)  |
| Invalid / `0`   | Falls back to the default (`3`)                            |
| Applies to      | Consecutive reds for the **same** target SHA               |

Parsing is defensive: a garbage value, an empty value, `0`, or a value below
the floor can never disable the guard or set it to trip on a single red. The
effective threshold is always `≥ 2`.

```bash
# Trip the halt after 5 consecutive red-canary observations of the same SHA.
export SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT=5

# Garbage / 0 / below-floor all resolve safely:
export SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT=0        # → default 3
export SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT=1        # → clamped to floor 2
export SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT=banana   # → default 3
```

This variable is additive and backward-compatible. It sits alongside the
existing self-deploy environment controls:

| Env var                                  | Purpose                                            |
| ---------------------------------------- | -------------------------------------------------- |
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY`      | Opt-OUT: `0`/`false`/`off`/`no` pins the daemon.   |
| `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` | Min interval between deploy attempts (anti-thrash). |
| `SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT` | Consecutive reds before loop-halt (this rail).     |

---

## Operator playbook

When you receive a `deploy_drift.stuck` escalation:

1. **Read the enriched reason.** The `deploy_refused` notice and the
   `deploy_drift.stuck` log now carry the canary's own failure detail — e.g.
   `target canary gate clippy failed: 2 warnings denied` when the canary
   reported a gate-level failure. (If the canary only reported an aggregate
   count, e.g. `3/5 gates`, the gate name is not present — see [Rail A](#rail-a-diagnosable-canary-red).)
   In the gate-level case you no longer need to SSH to the canary host to learn
   *which* gate is red.
2. **Fix forward.** The stuck SHA will never deploy while its canary is red.
   Land a fix on `origin/main`; the new HEAD is a *different* SHA, which resets
   the streak and re-arms the escalation, so the daemon resumes normal
   drift-triggered deploys automatically.
3. **Or pin the daemon** while you investigate:
   `export SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0`.

You will get **one** `stuck` alert per stuck SHA, not one per tick.

### Alerting

Alert on the `deploy_drift.stuck` tracing target (via OTel/your log pipeline).
Its presence means: *"a merged self-change is stuck behind a red canary and the
daemon has stopped silently accumulating drift on it."* Pair it with a
`DeployDrift` depth gauge to see how far behind main the daemon is.

---

## API surface

### `enrich_refusal` (Rail A)

```rust
// src/overseer/deploy.rs (private)
fn enrich_refusal(refusal: &DeployRefusal, canary: &CanaryResult) -> String;

/// Max bytes of subprocess-derived canary detail forwarded to operators.
const REASON_DETAIL_CAP: usize = 512;
```

- For `DeployRefusal::RedCanary`, returns `"{refusal}: {canary.detail}"`,
  truncated to `REASON_DETAIL_CAP` on a UTF-8 char boundary. The forwarded
  `detail` is verbatim from the canary — it names the failing gate only when
  the canary itself reported a `GateFailed` result (see [Rail A](#rail-a-diagnosable-canary-red)).
- For every other refusal variant, returns `refusal.to_string()` unchanged.
- Pure and side-effect-free; the caller routes the result into the
  `deploy_refused` notice, the `Capability { detail }` error, and the
  `%reason` tracing field.

### `RedCanaryStreak` API (Rail B)

```rust
// src/overseer/deploy_trigger.rs
/// Record a canary result for a target SHA. `is_red == true` increments the
/// SHA's consecutive-red streak; `false` (green) resets it. A new SHA also
/// supersedes the prior one (at most one active stuck entry).
///
/// CALL SITE: the Act/canary site in `deploy.rs`, immediately after
/// `canary.passed` is known. This is the ONLY writer of streak state.
pub(crate) fn record_red_canary_result(sha: &str, is_red: bool);

/// Current consecutive-red streak for `sha` (0 if none / just reset).
///
/// CALL SITE: `observe_deploy_drift` (read-only) to decide suppress + escalate.
pub(crate) fn red_canary_streak_for(sha: &str) -> u32;

/// Effective halt threshold from `SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT`
/// (default 3, floor 2, invalid/0 → default).
pub(crate) fn red_canary_halt_threshold() -> u32;

/// Latch a stuck SHA as escalated: returns `true` only the FIRST time (so the
/// caller escalates once) and `false` on subsequent ticks for the same SHA.
///
/// CALL SITE: `escalate_deploy_stuck_once` (one-shot operator escalation).
pub(crate) fn mark_deploy_stuck_escalated(sha: &str) -> bool;

/// Env var name for the halt threshold.
pub const RED_CANARY_HALT_ENV: &str = "SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT";

/// Default consecutive-red threshold before the loop-halt trips.
pub const DEFAULT_RED_CANARY_HALT: u32 = 3;

/// Hard floor so a mis-set env can never trip on a single red.
pub const RED_CANARY_HALT_FLOOR: u32 = 2;

#[cfg(test)]
/// Test-only: clear all streak + escalation-latch state.
pub(crate) fn reset_red_canary_streak();
```

The counter is in-memory only. **Durable, cross-restart per-SHA backoff is out
of scope here** — it is owned by the deploy-throttle ledger work.

---

## Relationship to the durable deploy-throttle ledger

This feature is intentionally scoped to be **orthogonal** to the durable
per-SHA backoff ledger (`DeployAttemptLedger` / `deploy_throttle.rs`) developed
separately. To guarantee the two compose without collision or regression, Rail
B **by construction**:

- defines **no** `deploy_throttle.rs` module and **no** `DeployAttemptLedger`
  type,
- uses a **distinct** tracing namespace (`deploy_drift.stuck`) and distinct
  symbol names (`RedCanaryStreak`, `record_red_canary_result`, …),
- keeps its state **in-memory** (process-global static), leaving durable
  cross-restart backoff entirely to the ledger.

The result: this rail is an in-process, single-lifetime **diagnosability +
escalation** guard; the ledger is a **durable backoff** guard. They stack —
this rail surfaces the *reason* and escalates a *stuck* SHA within a process
lifetime; the ledger enforces *durable* backoff across restarts.

## Security & safety invariants

- **Root-of-trust preserved.** `origin_strict` (never deploy local HEAD) and
  the `autonomous_deploy_enabled` opt-out remain authoritative and ordered
  ahead of both rails.
- **Fail-safe preserved.** `observe()` maps any error → `None` → no signal;
  Rail B only ever suppresses a signal or adds an escalation — never a deploy.
- **No log/notification injection.** The subprocess-derived canary detail is
  carried only as a structured `%reason` field and truncated to `REASON_DETAIL_CAP`.
- **No alert flooding.** The `stuck` escalation is one-shot per SHA.
- **Defensive config.** `SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT` clamps to a
  floor of 2 and falls back to the default on any invalid value.
- **No `print!`/`println!`.** All new emission is structured `tracing` + OTel.
