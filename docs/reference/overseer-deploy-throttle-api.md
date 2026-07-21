---
title: Overseer durable deploy anti-thrash throttle API
description: >
  The typed surface of the Overseer's restart-durable, fail-closed self-deploy
  anti-thrash throttle (#4390): the `DeployAttemptLedger` per-commit ledger and
  its `ThrottleDecision` enum in `src/overseer/deploy_throttle.rs`, the durable
  on-disk `deploy-attempt-ledger.json` state file, the
  `SIMARD_OVERSEER_DEPLOY_*` configuration accessors, and how the ledger is
  wired into the Overseer OBSERVE (`observe_deploy_drift`) and ACT
  (`Intervention::Deploy`) paths so a commit whose canary deploy-gate is red is
  not re-attempted every tick — even across an overseer restart.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/deploy-anti-thrash-throttle.md
  - ../howto/configure-overseer-deploy-throttle.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-backoff-gate-api.md
  - ./overseer-operator-notifications.md
  - ./state-root-resolution.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../safe-self-update.md
  - ../../src/overseer/deploy_throttle.rs
  - ../../src/overseer/deploy_trigger.rs
  - ../../src/overseer/mod.rs
---

# Overseer durable deploy anti-thrash throttle API

> **Status: implemented ([#4390](https://github.com/rysweet/Simard/issues/4390)).**
> The `DeployAttemptLedger` primitive and its `ThrottleDecision` enum live in
> [`src/overseer/deploy_throttle.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_throttle.rs);
> the process-global fast guard (`global_deploy_throttle_allow`) and the
> `SIMARD_OVERSEER_DEPLOY_*` accessors stay in
> [`src/overseer/deploy_trigger.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_trigger.rs);
> the OBSERVE/ACT wiring is in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> This rail closes the self-deploy thrash tracked by
> [#4390](https://github.com/rysweet/Simard/issues/4390). For the rationale see
> [the deploy anti-thrash throttle concept](../concepts/deploy-anti-thrash-throttle.md);
> for the operator knobs, the
> [configure how-to](../howto/configure-overseer-deploy-throttle.md).

## What this fixes

Before this rail the Overseer's only anti-thrash guard on the autonomous
self-deploy path was the **process-global min-interval** clock
(`global_deploy_throttle_allow`, [self-deploy API](./self-deploy-api.md)). That
guard is:

- **Commit-agnostic.** It throttles *any* attempt within the window, but it does
  not remember *which* commit failed. Once the window elapses it re-admits the
  same known-bad SHA.
- **Restart-resetting.** It is a process `static` (`AtomicU64`). A daemon
  restart — exactly what a self-deploy attempt can cause — resets it to `0`
  ("never attempted"), so the next tick re-attempts the same commit immediately.
- **Fail-open.** An unreadable/empty state defaults to "allow".

The observed symptom ([#4390](https://github.com/rysweet/Simard/issues/4390)):
commit `56b10bef5057` failed the canary `deploy_gate` ("red canary") on **five
consecutive** overseer ticks (15:10Z–17:38Z). Every tick re-observed
`DeployDrift — running binary is 1 commit behind merged main — self-deploy
required`, so the system never converged and kept re-attempting an identical
failing deploy.

The durable `DeployAttemptLedger` closes that seam. It records, **per target
SHA**, the last attempt time, the consecutive-failure count, and an exponential
`backoff_until`, persisted to disk so it **survives an overseer restart**, and
it is **fail-closed per-SHA**: when a SHA's record is present but corrupt or
ambiguous, the deploy is refused rather than re-attempted. A *missing* ledger
(first-ever run) loads empty and allows, so the guard can never deadlock the
first deploy.

## Two-layer suppression contract

The two guards compose. Both must admit a SHA before an autonomous deploy fires.

| Layer | Scope | Persistence | Keyed on | Fails toward | Source | Status |
|-------|-------|-------------|----------|--------------|--------|--------|
| **1. `global_deploy_throttle_allow`** | In-process fast per-tick min-interval | Process `static` (reset on restart) | Time only (commit-agnostic) | **Allow** (fail-open) | `deploy_trigger.rs` | **Implemented (#2590)** |
| **2. `DeployAttemptLedger`** | Per-commit backoff + known-bad memory | **Durable** (`deploy-attempt-ledger.json`, survives restart) | Target SHA | **Refuse the specific SHA** (fail-closed per-SHA) | `deploy_throttle.rs` | **Implemented (#4390)** |

Layer 1 is the cheap first gate — it caps the *rate* of any attempt. Layer 2 is
the durable memory — it stops a *specific* red-canary SHA from being retried
every tick and remembers that decision across a restart. Layer 2 is **per-SHA**:
a never-seen SHA with no prior failure still deploys, so the ledger can never
deadlock the first-ever deploy or a genuinely-new merged commit.

## `ThrottleDecision`

```rust
/// The `DeployAttemptLedger`'s verdict for one candidate target SHA. The
/// decision is a pure function of durable ledger state and `now` — it needs no
/// live "is-the-canary-red" signal, because the ledger *is* the durable memory
/// of a past red canary (`last_deploy_result = "failed"` + `backoff_until`).
/// Fails closed per-SHA: a ledger this tick cannot trust for the candidate SHA
/// (corrupt file, or a record present with no terminal result) yields
/// `FailClosed`, never `Allow`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThrottleDecision {
    /// The SHA is new (no record ⇒ never attempted) or its backoff window has
    /// elapsed — admit the deploy attempt.
    Allow,
    /// The SHA failed a recent attempt and is inside its exponential backoff
    /// window. Suppress the attempt until `retry_after_unix_secs`.
    BackingOff {
        /// The SHA being suppressed.
        target_sha: String,
        /// Consecutive canary/deploy failures recorded for this SHA.
        failure_count: u32,
        /// Epoch-seconds after which this SHA becomes eligible again.
        retry_after_unix_secs: u64,
    },
    /// The ledger could not be trusted for the candidate SHA (corrupt/unreadable
    /// file, or a record present but with no terminal result). Refuse the deploy
    /// and surface the stuck state. Reachable purely from ledger state — see the
    /// note below on why this never deadlocks the first-ever deploy.
    FailClosed {
        /// The SHA being refused.
        target_sha: String,
        /// Why the ledger declined to admit (for the surfaced warning).
        reason: FailClosedReason,
    },
}

/// Why a `FailClosed` decision was reached, surfaced on the suppressed tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailClosedReason {
    /// Ledger file present but unreadable / deserialize error (torn or corrupt
    /// state). A **missing** file is not this — it loads an empty ledger and
    /// yields `Allow` — so `Unreadable` can only occur once at least one attempt
    /// has already been persisted, and therefore never blocks the literal
    /// first-ever deploy.
    Unreadable,
    /// A record exists for the SHA (so it *was* attempted) but its
    /// `last_deploy_result` is unset — the outcome is ambiguous, so don't
    /// re-attempt it.
    Ambiguous,
}
```

`Allow` is the only variant that admits a deploy. Both `BackingOff` and
`FailClosed` suppress it; the Overseer clears `observed.deploy_drift` and
surfaces the stuck state (see [Surfacing](#surfacing-the-stuck-state)).

**Why `FailClosed` needs no live red signal.** At OBSERVE time the observer only
yields *that there is drift* and *the concrete target SHA* — it does **not** know
the canary is red (that is only learned after an ACT deploy attempt returns
`Err`). The ledger closes this gap by *being* the durable red-canary memory: a
past red canary is recorded as `last_deploy_result = "failed"` with a
`backoff_until`, so `consult` can decide `BackingOff`/`FailClosed` from the
record alone. Because there is at most one drift target SHA per tick,
`FailClosed` only ever refuses that single in-flight candidate — never "all
deploys" — and a *missing* ledger is empty ⇒ `Allow`, so a first-ever deploy is
never deadlocked by a corrupt file that cannot yet exist.

## `DeployAttemptLedger`

```rust
/// Durable, per-target-SHA anti-thrash ledger for the autonomous self-deploy
/// rail (#4390). Persisted to `~/.simard/state/deploy-attempt-ledger.json`
/// (atomic tmp+rename, 0600) so a red-canary commit is not re-attempted every
/// tick even across an overseer restart. Fail-closed per-SHA.
pub struct DeployAttemptLedger { /* ... */ }

impl DeployAttemptLedger {
    /// Load the ledger from `state_dir`. A **missing** file loads an empty
    /// ledger (`Ok`) — a first-ever run is not an error, and yields `Allow`. A
    /// present-but-corrupt file loads a ledger in a `poisoned` mode that returns
    /// `FailClosed(Unreadable)` for the candidate SHA, so a torn write never
    /// silently re-admits a commit that had already been persisted as bad.
    /// Called **once at Overseer construction**; see [Wiring](#wiring).
    pub fn load(state_dir: &Path) -> Self;

    /// Consult the ledger for `target_sha` at `now_secs`. A pure, read-only
    /// function of durable ledger state — records nothing and needs no live
    /// red-canary signal (the ledger record *is* that memory; see
    /// [`ThrottleDecision`](#throttledecision)).
    pub fn consult(&self, target_sha: &str, now_secs: u64) -> ThrottleDecision;

    /// Record a successful deploy of `target_sha`: clears the SHA's failure
    /// count and backoff (a green commit is immediately eligible again) and
    /// persists atomically. Idempotent. Returns the persist result so the caller
    /// can log it — but the ACT path consumes it **best-effort** (a durable-write
    /// failure is logged, never propagated, so it can neither mask nor flip the
    /// actual deploy outcome; see [Wiring](#act--interventiondeploy)).
    pub fn record_success(&mut self, target_sha: &str, now_secs: u64) -> io::Result<()>;

    /// Record a failed deploy of `target_sha`: increments `failure_count`,
    /// sets `last_attempt_unix_secs = now_secs`, computes the next
    /// `backoff_until_unix_secs` (exponential, capped), and persists atomically.
    /// Consumed best-effort by the ACT path (as `record_success`).
    pub fn record_failure(&mut self, target_sha: &str, now_secs: u64) -> io::Result<()>;

    /// Path of the ledger file inside `state_dir`.
    pub fn ledger_path(state_dir: &Path) -> PathBuf; // state_dir/deploy-attempt-ledger.json
}
```

### On-disk record

Each SHA maps to a durable record. The file is a small JSON object keyed by full
target SHA:

```json
{
  "version": 1,
  "entries": {
    "56b10bef5057…": {
      "failure_count": 5,
      "last_attempt_unix_secs": 1753118280,
      "backoff_until_unix_secs": 1753132680,
      "last_deploy_result": "failed"
    }
  }
}
```

| Field | Type | Meaning |
|-------|------|---------|
| `failure_count` | `u32` | Consecutive canary/deploy failures for this SHA. Reset to `0` on `record_success`. |
| `last_attempt_unix_secs` | `u64` | Epoch-seconds of the last recorded attempt. |
| `backoff_until_unix_secs` | `u64` | Epoch-seconds before which the SHA is suppressed (`BackingOff`). |
| `last_deploy_result` | `"failed" \| "succeeded"` (nullable) | Terminal result of the last attempt. **Unset ⇒ ambiguous ⇒ `FailClosed`.** |

`version` gates the schema; an unknown/greater version loads poisoned
(fail-closed), never silently migrated.

### Backoff curve

`record_failure` sets an **exponential** window keyed on `failure_count`, capped:

```text
backoff_secs = min(base * 2^(failure_count - 1), cap)
backoff_until_unix_secs = now_secs + backoff_secs
```

Defaults: `base` = the resolved deploy min-interval
([`deploy_min_interval_secs`](./self-deploy-api.md), default **900 s**), `cap` =
a fixed **6 h** (`21600 s`) constant. So a SHA's suppression grows 15 min → 30 →
60 → 120 → 240 → 360 (capped) across successive failures. A `record_success`
clears the curve, so a SHA that later goes green is immediately eligible again —
the throttle backs off churn without ever becoming a permanent hard-stop that
needs manual reset.

## Configuration

The durable ledger reuses the existing self-deploy knobs; the backoff `cap` is a
fixed 6 h constant (not an env knob), keeping the config surface minimal. All
knobs are read fail-safe (an unset/unparseable value uses the default; values
are clamped, never trusted blindly).

| Env var | Default | Floor / clamp | Effect |
|---------|---------|---------------|--------|
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY` | enabled | — | Set `0`/`false`/`off`/`no` to pin the daemon (disables autonomous deploy entirely, so the ledger is never consulted). |
| `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` | `900` | floor `60` | Layer-1 min-interval **and** the ledger's backoff `base`. |

The backoff **cap** is the compile-time constant `DEPLOY_BACKOFF_CAP_SECS =
21600` (6 h); it is not operator-tunable. Lower the effective aggressiveness via
`base` instead.

The ledger's **location** follows the shared state-root contract
([state-root resolution](./state-root-resolution.md)):
`~/.simard/state/deploy-attempt-ledger.json`, falling back to
`./.simard-state/deploy-attempt-ledger.json` only when `$HOME` is unreadable
(keeps tests hermetic). See
[`default_state_dir`](https://github.com/rysweet/Simard/blob/main/src/safe_update/state.rs).

## Wiring

### OBSERVE — `observe_deploy_drift`

The ledger is `load`ed **once when the Overseer is constructed** and held on
`self.deploy_ledger`. The per-tick-rebuilt Overseer model is fine: durability
lives on disk, so a rebuilt Overseer re-`load`s the same file and sees the same
records. Every tick threads the same `now = now_epoch_secs()` used by the
layer-1 guard.

The ledger is consulted in
[`observe_deploy_drift`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
as an **additional gate** after the existing static time throttle. Ordering:

1. Operator opt-out (`autonomous_deploy_enabled`) — pins the daemon.
2. Observer wired? (inert until the sensor exists).
3. Layer-1 fast guard (`global_deploy_throttle_allow`) — per-tick rate cap.
4. Crash-loop guard (`restart_churn ≥ CRASH_LOOP_CHURN_THRESHOLD`).
5. **Probe drift** to resolve the concrete `target_commit`.
6. **Layer-2 durable gate:** `ledger.consult(target_sha, now)`. Only `Allow`
   sets `observed.deploy_drift`; `BackingOff` / `FailClosed` suppress the drift
   and surface the stuck state.

Because the ledger is keyed on the concrete SHA, step 6 runs *after* the drift
probe (step 5) — the throttle must know **which** commit it is judging, unlike
the commit-agnostic layer-1 guard.

> **Layer-1 clock side effect.** `global_deploy_throttle_allow` (step 3)
> *advances* its process-global min-interval clock (`compare_exchange` stores
> `now`) at the moment it returns `Allow` — i.e. before step 6 runs. So a SHA
> that layer-2 subsequently suppresses still consumes that layer-1 window: the
> next tick's layer-1 guard will short-circuit at step 3 until the min-interval
> elapses. This is harmless (both layers suppress the same doomed re-attempt) but
> means the *observed* suppression `reason` on the immediately following ticks is
> the layer-1 rate cap, not the layer-2 backoff.

### ACT — `Intervention::Deploy`

The [`Intervention::Deploy { commit }`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
handler records the terminal result so the OBSERVE gate remembers it next tick.
The ledger write is **best-effort**: its `io::Result` is logged on failure but
never propagated with `?`, so a disk-write failure can neither turn a successful
deploy into an `Err` nor mask a real deploy error — the actual deploy outcome is
always the value returned:

```rust
Intervention::Deploy { commit } => {
    let result = self.caps.deployer.deploy(commit);
    match &result {
        // Best-effort durable record; a persist failure is logged, never
        // propagated, so it cannot flip or mask the deploy outcome above.
        Ok(_) => {
            if let Err(e) = self.deploy_ledger.record_success(commit, now) {
                tracing::warn!(target_sha = %commit, error = %e,
                    "deploy_ledger.persist_failed record=success");
            }
        }
        Err(_) => {
            if let Err(e) = self.deploy_ledger.record_failure(commit, now) {
                tracing::warn!(target_sha = %commit, error = %e,
                    "deploy_ledger.persist_failed record=failure");
            }
        }
    }
    result.map(ActOutcome::Deployed)
}
```

A red canary surfaces as `Err` from `deploy` (the `GuardedDeployer` refuses the
swap on a red canary / rollback / crash-loop), so `record_failure` is the path
that grows the backoff for `56b10bef5057`-style thrash. (`now` and
`self.deploy_ledger` are threaded through the tick as described in
[OBSERVE](#observe--observe_deploy_drift) above; the snippet is illustrative of
the wiring, not the exact current `act` body.)

## Surfacing the stuck state

On every suppressed tick (`BackingOff` or `FailClosed`) the Overseer emits a
structured `tracing::warn!` plus an OTel span attribute — no `print!`/`println!`,
reusing the sanctioned observability path:

```text
WARN deploy_throttle.stuck=true target_sha=56b10bef5057…
     failure_count=5 backoff_until=1753132680 reason=backing_off
```

| Attribute | Meaning |
|-----------|---------|
| `deploy_throttle.stuck` | `true` on any suppressed tick. |
| `target_sha` | The commit being suppressed. |
| `failure_count` | Consecutive failures for the SHA. |
| `backoff_until` | Epoch-seconds the SHA becomes eligible again (`BackingOff`). |
| `reason` | `backing_off`, `unreadable`, or `ambiguous`. |

This is the requested "surface the stuck state instead of silently looping": the
operator dashboard and `simard` telemetry see a single, throttled, explained
warning per tick instead of a stream of identical red-canary deploy attempts.

## Security & robustness

- **Atomic durable writes.** `record_*` write a sibling tmp file and `rename(2)`
  into place (mirroring [`safe_update`](./self-deploy-source-prep.md)), so a
  crash mid-write never leaves a torn ledger.
- **Restrictive perms.** The state dir is created `0700`, the ledger `0600`; the
  writer refuses to follow a symlink at the target path.
- **No panics on IO/parse.** `load` degrades a corrupt file to a poisoned,
  fail-closed ledger; it never `unwrap`s on IO or deserialize.
- **Bounds validation.** On read the schema `version`, SHA shape (full 40/64-char
  lowercase hex — the same [`is_full_hex_sha`](./self-deploy-api.md) trust shape),
  and numeric fields are validated; out-of-range values are rejected.
- **Fail-closed is per-SHA, never global.** `consult` decides for the single
  candidate SHA in flight this tick; a corrupt/ambiguous ledger refuses only that
  SHA, never "all deploys". A **missing** file loads empty ⇒ `Allow`, so a
  first-ever deploy (no file yet) is never deadlocked — `Unreadable` can only
  arise once a real attempt has already been persisted.
- **Bounded growth.** The entry map is capped; the oldest terminal records are
  evicted first so the ledger cannot grow without bound.
- **No secrets logged.** The surfaced warning carries only SHA + counters.

## Test surface

The behavior is covered by unit tests in
[`src/overseer/deploy_throttle.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_throttle.rs)
and integration tests in
[`src/overseer/tests_deploy_drift.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_deploy_drift.rs):

- **Restart survival:** record a failure, drop the ledger, `load` from the same
  `state_dir`, and assert the red-canary SHA is `BackingOff` (not re-deployed).
- **Fail-closed:** a corrupt (`Unreadable`) or record-present-but-result-unset
  (`Ambiguous`) ledger for the candidate SHA yields `FailClosed`, and the OBSERVE
  gate refuses the deploy; a **missing** file yields `Allow` (never deadlocks a
  first deploy).
- **Backoff window:** a failed SHA is suppressed within its window and `Allow`ed
  after it; the window grows exponentially and is capped.
- **New-SHA no-deadlock:** a never-seen SHA (no record) is `Allow`ed.
- **Success clears:** `record_success` resets the SHA's count/backoff so a
  now-green commit deploys immediately.
- **End-to-end OBSERVE gate:** `tests_deploy_drift.rs` drives the same
  OBSERVE→guarded-ACT rail through the durable ledger and asserts a red-canary
  SHA is not re-attempted on the next tick, stays suppressed across a simulated
  overseer restart, and a corrupt ledger fails closed rather than re-deploying.

## See also

- [Deploy anti-thrash throttle (concept)](../concepts/deploy-anti-thrash-throttle.md)
- [Configure the Overseer deploy throttle (how-to)](../howto/configure-overseer-deploy-throttle.md)
- [Self-Deploy API](./self-deploy-api.md)
- [Overseer BackoffGate & gap-scan dedup](./overseer-backoff-gate-api.md)
- [reconcile-and-self-deploy (concept)](../concepts/reconcile-and-self-deploy.md)
- [State-Root Resolution](./state-root-resolution.md)
