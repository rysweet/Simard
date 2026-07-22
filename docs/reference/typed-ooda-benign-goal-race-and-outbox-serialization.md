---
title: Typed-OODA benign goal-race no-op and outbox write serialization
description: Reference for the two additive resilience behaviours in the typed-OODA decide→act effect-dispatch and outcome-persistence pipeline — (a) a goal legitimately completed or removed between effect *prepare* and *dispatch* is recorded as a benign, structured, counted no-op outcome instead of a DownstreamFailed cycle failure, and (b) the outbox/outcome SQLite writes are serialized with a bounded busy/locked retry-with-backoff so startup recovery and concurrent cycles stop colliding on "database is locked". Covers the EffectExecutionError::benign_no_op constructor and no_op flag, the execute_claimed no-op dispatch arm, the reclassified vs. still-permanent goal-not-found sites, the is_busy_locked classifier and retry_on_busy wrapper, the structured tracing events and counters, and the journal signatures the fix eliminates.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../architecture/typed-ooda-loop.md
  - ./no-bridge-naming-guard.md
  - ../operations/cognitive-memory-durability.md
  - ../../src/typed_ooda/executor.rs
  - ../../src/typed_ooda/ledger.rs
  - ../../src/ooda_actions/advance_goal/typed_goal_session.rs
---

# Typed-OODA benign goal-race no-op and outbox write serialization

> **Status: implemented (issue #4468).** Two additive behaviours in the
> typed-OODA `decide → act` effect-dispatch and outcome-persistence pipeline:
>
> 1. A prepared goal-session effect whose goal was **legitimately completed or
>    removed** between *prepare* and *dispatch* is now recorded as a **benign,
>    structured, counted no-op** outcome — a finished outbox row and a
>    `tracing` event — instead of raising
>    `EffectExecutionError::permanent("goal disappeared before effect dispatch")`
>    and mapping to `CycleErrorCode::DownstreamFailed`.
> 2. The outbox/outcome SQLite writes are **serialized with a bounded
>    busy/locked retry-with-backoff** so startup recovery and concurrent cycles
>    stop colliding on `database is locked`.
>
> Both changes are additive. The outcome schema, `EffectResult`, and every
> happy-path effect semantic are unchanged; there is **no** happy-path behaviour
> change. No `Bridge` naming is introduced (see
> [No-Bridge naming guard](./no-bridge-naming-guard.md)); observability is
> structured `tracing` + OTel only, with no stray `print!`/`println!`/`eprintln!`
> and no silent fallbacks — every swallowed race emits a structured, counted
> outcome.

The typed-OODA executor prepares an effect (claims an outbox row, resolves the
goal's authenticated repository) and then dispatches it. Between those two
steps another cycle — or the goal's own successful completion — can remove the
goal record. That is a **legitimate, benign race**, not a failure: the work the
effect represents is already moot. Before this fix the dispatch site treated the
missing goal as a permanent downstream failure, so seven distinct goals in a
single six-hour window each burned an OODA cycle on the same shared signature.
In the same window the outbox/outcome store took ten `database is locked` hits,
nine of them during **startup recovery** racing live cycles for the single
writer connection.

This reference describes the finished behaviour: what is reclassified as a
no-op (and, importantly, what is **not**), how the no-op outcome is shaped and
counted, and how the SQLite writes are serialized. For the surrounding pipeline
see [Typed-OODA architecture](../architecture/typed-ooda-loop.md).

## Contents

- [The two journal signatures this eliminates](#the-two-journal-signatures-this-eliminates)
- [Benign goal-race no-op](#benign-goal-race-no-op)
  - [Which sites are reclassified](#which-sites-are-reclassified)
  - [Which sites stay permanent](#which-sites-stay-permanent)
  - [The no-op outcome shape](#the-no-op-outcome-shape)
  - [`EffectExecutionError` API](#effectexecutionerror-api)
  - [`execute_claimed` dispatch arm](#execute_claimed-dispatch-arm)
- [Outbox / outcome write serialization](#outbox--outcome-write-serialization)
  - [`is_busy_locked` classifier](#is_busy_locked-classifier)
  - [`retry_on_busy` wrapper](#retry_on_busy-wrapper)
  - [Connection PRAGMAs](#connection-pragmas)
- [Observability](#observability)
- [Configuration](#configuration)
- [Verification](#verification)
- [Security model](#security-model)
- [Examples](#examples)
- [When the no-op does *not* fire](#when-the-no-op-does-not-fire)

## The two journal signatures this eliminates

Both defects surface in the OODA journal:

```bash
journalctl --user -u simard-ooda --since '-6h'
```

| Signature (before fix) | Count/6h | Root cause | Now |
| --- | --- | --- | --- |
| `typed goal-session effect incomplete (DownstreamFailed): goal disappeared before effect dispatch` | 7, across 7 distinct goals | goal completed/removed between prepare and dispatch | benign counted no-op; cycle succeeds |
| `typed outcome persistence failed: database is locked` | 10 (9 at `typed OODA outbox startup recovery incomplete`, 1 as `typed goal-session cycle failed (ToolFailed)`) | SQLite lock contention on the outbox/outcome store under concurrent writers + startup recovery | bounded retry-with-backoff; write completes |

One shared signature spanning many goals is the tell that this is **systemic**,
not per-goal — the fix is in the shared dispatch and persistence path, not in
any single goal handler.

## Benign goal-race no-op

The named defect lives at
[`src/ooda_actions/advance_goal/typed_goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs)
in `require_goal_repository`, which resolves the goal's authenticated repository
just before the effect is dispatched. If the goal is gone, the work is moot —
there is nothing to advance — so the effect becomes a no-op rather than a
failure.

### Which sites are reclassified

Exactly the two **pre-side-effect** goal-not-found misses return
`EffectExecutionError::benign_no_op(...)`:

| Site | Location | Why benign |
| --- | --- | --- |
| `require_goal_repository` goal-not-found | `typed_goal_session.rs` — the `.ok_or_else(...)` on the active-goals lookup ("goal disappeared before effect dispatch") | The named defect. No process has been spawned; nothing to undo. |
| spawn goal-not-found ("before spawn") | `typed_goal_session.rs` — the pre-spawn active-goals lookup | Same pre-side-effect race, same file. Reclassified for consistency. |

Both are races the system is *designed* to tolerate: the goal reached a terminal
state (completed/removed) between prepare and dispatch.

### Which sites stay permanent

Reclassification is deliberately narrow. These sites keep returning
`EffectExecutionError::permanent(...)` and continue mapping to
`CycleErrorCode::DownstreamFailed`, because each is either a genuine error or a
site where a side effect has already happened:

| Site | Why it stays permanent |
| --- | --- |
| **Post-spawn goal-not-found** ("after engineer spawn") | An engineer process was **already spawned**. Swallowing this would orphan a live process — the outbox row must record a real failure so recovery can act. |
| **Repository mismatch** (effect repo ≠ authenticated goal repo) | A genuine repo/tenant-isolation error. Treating it as benign would be an authorization-suppression primitive — an effect bound to a *different* repository must never be silently closed. |
| **`goal_repository()` resolution error** | The goal exists but its repository metadata is malformed — a real error, not a race. |
| **Goal already assigned** (`already_assigned` → "goal already has an assigned engineer") | The goal exists and already has a live engineer. This is a duplicate-spawn guard, not a race — the effect must not proceed, and it is not a benign disappearance. |

The negative regression test `repo_mismatch_still_downstream_failed` locks the
repository-mismatch boundary in: a mismatch must still produce
`DownstreamFailed`. The `already_assigned` and post-spawn sites are covered by
their own duplicate-spawn / orphan-reconciliation tests and are unchanged by
this work — they are listed here only to make the reclassification boundary
exhaustive.

### The no-op outcome shape

A benign no-op is a **completed** outbox row, not a skipped one. `execute_claimed`
calls `finish_effect` with a succeeded result carrying **empty evidence**:

```rust
EffectResult::Succeeded { evidence: vec![] }
```

This closes the outbox row exactly as any completed effect would (no
re-dispatch, no retry), never maps to `DownstreamFailed`, and adds nothing to
the outcome schema — empty-evidence `Succeeded` reuses the existing
`EffectResult::Succeeded` variant and serde round-trips `vec![]` unchanged. The
finish uses a distinct `noop` operation suffix in its idempotency key
(`effect_mutation_request_id(&job, "noop")` → `"{effect_id}:{lease_generation}:noop"`),
partitioning it from the `complete` / `failed` / `retry` request-id space so a
no-op can never collide with, or double-close, a real completion.

### `EffectExecutionError` API

`EffectExecutionError` (in
[`src/typed_ooda/executor.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/executor.rs))
gains one additive, internal, **non-serialized** field and one constructor:

```rust
// src/typed_ooda/executor.rs

pub struct EffectExecutionError {
    message: String,
    /// Terminal (non-retryable). Both `permanent` and `benign_no_op` set this.
    permanent: bool,
    /// Legitimately-removed goal between prepare and dispatch: record a benign,
    /// counted no-op outcome instead of a DownstreamFailed cycle failure.
    /// Internal, in-memory only — never read from a persisted outbox row or
    /// any external message.
    no_op: bool,
}

impl EffectExecutionError {
    /// Terminal failure. `no_op = false`.
    pub fn permanent(message: impl Into<String>) -> Self;

    /// Transient failure; the effect is released for retry. `no_op = false`.
    pub fn retryable(message: impl Into<String>) -> Self;

    /// A goal legitimately completed/removed between prepare and dispatch.
    /// Sets `permanent = true` (defense-in-depth: even if the no-op arm were
    /// bypassed it is still terminal, never retried) AND `no_op = true`.
    pub fn benign_no_op(message: impl Into<String>) -> Self;
}
```

`benign_no_op` sets **both** `permanent` and `no_op`. The `no_op` flag is a
pure in-memory discriminator: it is never serialized into an outbox row and
never trusted from external input, so it cannot be forged to suppress a real
failure.

### `execute_claimed` dispatch arm

The dispatch match in `execute_claimed` gains a **first** `Err` arm, evaluated
**before** the existing `if !error.permanent` (retryable) and permanent
branches:

```rust
let result = match self.effects.execute(&job) {
    Ok(result) => result,
    Err(error) if error.no_op => {
        // Benign race: the goal was legitimately completed/removed between
        // prepare and dispatch. Emit a structured, counted no-op outcome.
        tracing::warn!(
            effect_id = %job.effect_id,
            goal_id = %job.goal_id,
            reason = "goal-removed-before-dispatch",
            "typed goal-session effect no-op: goal completed or removed between prepare and dispatch",
        );
        let _ = crate::self_metrics::record_metric(
            "typed_ooda_effect_benign_no_op",
            1.0,
            &job.effect_id,
        );
        self.handler
            .finish_effect(
                &job,
                &effect_mutation_request_id(&job, "noop"),
                SystemTime::now(),
                &EffectResult::Succeeded { evidence: vec![] },
            )
            .map_err(|failure| {
                CycleError::new(CycleErrorCode::PersistenceFailed, failure.to_string())
            })?;
        return Ok(());
    }
    Err(error) => {
        // ... existing retryable (!permanent) and permanent branches, unchanged
    }
};
```

Ordering matters: because `benign_no_op` also sets `permanent = true`, the arm
guard is `error.no_op` and it is placed **first**, so a benign race is handled
as a no-op and never falls through to the retry or `DownstreamFailed` branches.
A persistence failure while closing the no-op row still surfaces as
`PersistenceFailed` — the no-op path never masks a real write error.

## Outbox / outcome write serialization

The outbox/outcome store is one SQLite database file, but it is **not** reached
through a single connection. Each `CapabilityHandler` in
[`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
holds its own `Mutex<Connection>`, and `CapabilityHandler::open(&ledger_path, …)`
is invoked from **many** call sites against the same file — per goal-session
cycle (`typed_goal_session.rs`), the subordinate path, overseer wiring, and the
operator CLI (a **separate process**). The per-handler `Mutex<Connection>`
serializes writes **within one handler only**; it does nothing to order writes
issued by a *different* handler or a *different* process. Those cross-connection
writes contend at SQLite's own file-lock layer.

WAL + a 5 s `busy_timeout` already absorb most of that contention, but under
concurrent live cycles **plus** startup recovery replaying expired effects (and,
occasionally, an operator-CLI write), a writer can still exhaust the
`busy_timeout` and surface `SQLITE_BUSY` / `SQLITE_LOCKED`. The fix wraps the
commit paths in a **bounded retry-with-backoff** keyed on a **typed** error-code
classifier — retry, not a wider mutex, is the right tool because the contention
is *between* connections/processes, which no single in-process lock can serialize.

The wrapper is applied to the three outbox/outcome commit paths:

| Method | Role |
| --- | --- |
| `finish_effect` | Closes an outbox row with its `EffectResult` (including the benign no-op). |
| `release_effect_for_retry` | Returns a transiently-failed effect to the queue. |
| `recover_expired_effects` | Startup/periodic recovery that re-queues effects whose lease expired — the dominant source of the 9 startup collisions. |

### `is_busy_locked` classifier

Mirrors the existing `is_constraint` classifier and matches **only** the two
contention codes — never on error-message text, so no log/error string can steer
retry behaviour:

```rust
// src/typed_ooda/ledger.rs

fn is_busy_locked(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::DatabaseBusy
                || inner.code == rusqlite::ErrorCode::DatabaseLocked
    )
}
```

Constraint violations, logic errors, and every other `rusqlite::Error` are **not**
retried — they surface immediately.

Because a busy/locked error is mapped into a `CapabilityError` by `persistence`
before it reaches the retry loop (a write transaction calls many
`CapabilityResult`-returning helpers, not raw `rusqlite` calls), the typed
signal is preserved via a **marker stamped solely from the typed error code** —
never from arbitrary or injected message text. `persistence` prepends
`BUSY_PERSISTENCE_MARKER` iff `is_busy_locked` is true, and
`capability_error_is_busy` is the retry loop's predicate:

```rust
// src/typed_ooda/ledger.rs

const BUSY_PERSISTENCE_MARKER: &str = "[sqlite-busy] ";

fn persistence(error: rusqlite::Error) -> CapabilityError {
    // Marker derived purely from the typed rusqlite::ErrorCode, not message text.
    let marker = if is_busy_locked(&error) { BUSY_PERSISTENCE_MARKER } else { "" };
    CapabilityError::new(
        CapabilityErrorCode::PersistenceFailed,
        format!("typed outcome persistence failed: {marker}{error}"),
    )
}

fn capability_error_is_busy(error: &CapabilityError) -> bool {
    error.code() == CapabilityErrorCode::PersistenceFailed
        && error.to_string().contains(BUSY_PERSISTENCE_MARKER)
}
```

### `retry_on_busy` wrapper

A small, bounded helper that re-runs a commit closure on `DatabaseBusy`/
`DatabaseLocked` only. Each attempt runs the whole `begin → execute → commit`
closure, so a retry always uses a **fresh `IMMEDIATE` transaction** — never a
rolled-back or poisoned one. The closure returns `CapabilityResult<T>` (the
write body's natural result type), and busy is recognised via
`capability_error_is_busy` above:

```rust
// src/typed_ooda/ledger.rs

/// Run `op` under bounded retry-with-backoff on SQLite busy/locked contention.
/// Any non-busy error, and busy/locked after `MAX_ATTEMPTS`, is returned as-is.
///
/// - MAX_ATTEMPTS: 6 (hard cap; anti-DoS on the single Mutex<Connection>).
/// - Backoff:      exponential from ~10ms, capped at 400ms per sleep.
/// - Exhaustion:   the final busy/locked error surfaces as PersistenceFailed —
///                 never masked, never retried unbounded.
///
/// INVARIANT: `op` must acquire (and drop) the `Mutex<Connection>` guard
/// *itself*, once per invocation. The backoff `sleep` below runs strictly
/// between `op` calls, so the connection mutex is **released while sleeping** —
/// a retrying writer never blocks same-handler callers during its backoff.
fn retry_on_busy<T>(mut op: impl FnMut() -> CapabilityResult<T>) -> CapabilityResult<T> {
    const MAX_ATTEMPTS: u32 = 6;
    const MAX_BACKOFF: Duration = Duration::from_millis(400);
    let mut attempt = 0u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) if capability_error_is_busy(&error) && attempt + 1 < MAX_ATTEMPTS => {
                let backoff = (Duration::from_millis(10) * (1u32 << attempt)).min(MAX_BACKOFF);
                tracing::warn!(
                    target: "typed_ooda.outbox_write",
                    attempt = attempt + 1,
                    max_attempts = MAX_ATTEMPTS,
                    backoff_ms = backoff.as_millis() as u64,
                    "typed OODA outbox write contended (busy/locked); retrying",
                );
                // Guard is already dropped here: `op` acquired and released the
                // connection mutex within its own body, so this sleep holds no lock.
                std::thread::sleep(backoff);
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
```

**Guard-release invariant.** Every commit method currently opens with
`let mut connection = self.lock()?;` and holds that `MutexGuard<Connection>`
for its whole body. The retry rewrite must move that acquisition *inside* the
`op` closure so the guard is created and dropped **per attempt**. Concretely,
each wrapped method becomes:

```rust
// Representative: finish_effect (release_effect_for_retry and
// recover_expired_effects follow the same shape). The closure returns
// CapabilityResult<T>, and every rusqlite error is mapped through `persistence`
// (which stamps the typed busy marker), so `retry_on_busy` sees busy contention
// regardless of which helper inside the transaction raised it.
retry_on_busy(|| {
    // lock acquired at the START of each attempt ...
    let mut connection = self.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(persistence)?;
    // ... replay-dedup, write the outbox row / result ...
    transaction.commit().map_err(persistence)
    // ... guard dropped at the END of each attempt (Ok or Err), BEFORE any sleep.
})
// retry_on_busy already returns CapabilityResult; exhaustion surfaces the busy
// error as CapabilityErrorCode::PersistenceFailed. No extra map_err is needed.
```

Because the guard lifetime is scoped to a single `op` call, `thread::sleep`
in `retry_on_busy` always runs with the connection mutex released. If the lock
were instead held across the loop, the backoff would block every other
same-handler caller for the full retry window — self-inflicted contention that
would defeat the fix. Lock poisoning is surfaced as `PersistenceFailed` (via
`self.lock()`'s existing mapping) and, being non-busy, is **never** retried.

On exhaustion the caller maps the surfaced error to
`CapabilityErrorCode::PersistenceFailed` exactly as before — a genuinely stuck
writer is still reported, never swallowed. Retry counts are **not** part of any
outcome contract; tests assert on the **absence** of `database is locked`, not
on an exact attempt count, so they stay timing-robust.

> **Latency interaction with `busy_timeout`.** Because every connection is
> opened with `busy_timeout(5s)`, each *individual* attempt can already block up
> to 5 s inside SQLite before it returns `SQLITE_BUSY`. The ≤400 ms backoff
> therefore does **not** bound total latency — the retry loop layers *on top of*
> the busy_timeout wait, so a pathologically contended write can take on the
> order of `MAX_ATTEMPTS × busy_timeout` in the worst case. This is acceptable
> for the outbox commit path (a stuck writer surfacing `PersistenceFailed` after
> several seconds is strictly better than the previous immediate failure), but it
> is the reason the retry bound is small and the backoff is capped: the goal is
> to ride out a *brief* cross-connection overlap, not to sit on a genuinely
> wedged database.

### Connection PRAGMAs

Every connection — including the recovery worker — opens through
`CapabilityHandler::open`, which applies `busy_timeout(5s)` and initializes the
schema, which sets `PRAGMA journal_mode = WAL`. WAL persists at the database-file
level; `busy_timeout` is re-applied on every open. No schema migration and no new
PRAGMA are introduced — the retry wrapper layers on top of the existing WAL +
busy_timeout baseline.

## Observability

All signals are structured `tracing` + OTel; there are no `print!`/`println!`/
`eprintln!` calls in the touched paths. As part of this change the one remaining
`eprintln!` at the startup-recovery site in `typed_goal_session.rs` is converted
to a structured `tracing::warn!` (static message, `error = %error` field, no
payloads).

| Event | Level | Key fields | Fires when |
| --- | --- | --- | --- |
| `typed goal-session effect no-op: goal completed or removed between prepare and dispatch` | `warn` | `effect_id`, `goal_id`, `reason="goal-removed-before-dispatch"` | A benign goal-race no-op is recorded at the dispatch site. |
| `typed OODA outbox write contended (busy/locked); retrying` | `warn` | `attempt`, `max_attempts`, `backoff_ms` | A SQLite write is retried after busy/locked contention. |
| Startup-recovery warning (converted from `eprintln!`) | `warn` | `error` | Startup outbox recovery reports a non-fatal error. |

The benign no-op is also counted via `record_metric`:

| Metric | Value | `context` | Meaning |
| --- | --- | --- | --- |
| `typed_ooda_effect_benign_no_op` | `1.0` per occurrence | the effect id | Number of goal-race no-ops absorbed instead of failing a cycle. |

`record_metric` is best-effort (`let _ = …`): the **authoritative** audit signal
is the `tracing` event plus the finished outbox row, not the counter. The counter
lands in `<state_root>/metrics/metrics.jsonl` — `<state_root>` is
`SIMARD_STATE_ROOT` when set, else `$HOME/.simard` — as a standard `MetricEntry`
(JSON key `metric_name`); see [Metrics hygiene](./distill-raw-capture-on-parse-failure.md#metrics-hygiene)
for the envelope.

## Configuration

There is **nothing to configure**. Both behaviours are always-on, additive, and
self-contained:

- The benign no-op reclassification has no toggle — a legitimately-removed goal
  is always a no-op, never a failure.
- The retry wrapper's bounds are compile-time constants (`MAX_ATTEMPTS = 6`,
  `MAX_BACKOFF = 400ms`). The backoff is capped well below the 5 s `busy_timeout`
  so a retry re-probes the lock quickly once the timeout elapses; note the
  attempts stack *on top of* the busy_timeout wait (see the latency note above),
  so the bound is deliberately small. They are intentionally not env-tunable: an
  operator knob here would be a foot-gun (unbounded retry re-introduces the DoS
  risk the cap exists to prevent).

The only operator-visible surface is the journal and the metrics file.

## Verification

Confirm both signatures are gone from a fresh window:

```bash
# Should return nothing after the fix.
journalctl --user -u simard-ooda --since '-6h' \
  | grep -E 'goal disappeared before effect dispatch|typed outcome persistence failed: database is locked'
```

Confirm no-ops are being absorbed (rather than the goals failing):

```bash
# Benign no-op events (structured tracing).
journalctl --user -u simard-ooda --since '-6h' \
  | grep 'typed goal-session effect no-op'

# Benign no-op counter (path honors SIMARD_STATE_ROOT; default shown).
grep '"metric_name":"typed_ooda_effect_benign_no_op"' \
  "${SIMARD_STATE_ROOT:-$HOME/.simard}/metrics/metrics.jsonl" | tail
```

Regression tests (must FAIL on `main` before the fix, PASS after):

| Test | Asserts |
| --- | --- |
| `dispatch_after_goal_removed_is_benign_no_op` | Goal removed between prepare and dispatch → cycle returns `Ok`, a `Succeeded { evidence: vec![] }` outbox row is written, the no-op tracing event is emitted, and **no** `DownstreamFailed` is produced. |
| `repo_mismatch_still_downstream_failed` | A repository mismatch still maps to `DownstreamFailed` — the reclassification did not widen. |
| `concurrent_outbox_write_under_lock_recovers` | Concurrent/startup-recovery writers against a shared temp DB all complete with **no** `database is locked` surfaced. Uses a barrier to force overlap; asserts on absence of the lock error, not on retry counts. |

Run the targeted suites:

```bash
cargo test -p simard typed_ooda:: -- --nocapture
cargo test -p simard advance_goal::typed_goal_session
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Security model

- **Narrow reclassification.** `benign_no_op` covers **only** the two
  pre-side-effect goal-not-found sites. It must **never** cover the
  repository-mismatch, metadata, or post-spawn sites — doing so would create an
  authorization-suppression primitive that could silently close an effect bound
  to a different repository or orphan a live engineer process. The
  `repo_mismatch_still_downstream_failed` negative test guards this boundary.
- **No_op is in-memory only.** The `no_op` flag is never serialized into an
  outbox row and never read from persisted state or an external message, so it
  cannot be forged to suppress a real failure.
- **Typed retry classification.** `is_busy_locked` matches on
  `rusqlite::ErrorCode`, never on error-message text. Because the write bodies
  return `CapabilityResult`, the busy signal crosses the `CapabilityError`
  boundary via `BUSY_PERSISTENCE_MARKER`, which `persistence` stamps **solely**
  from that typed code (checked by `capability_error_is_busy`). An injected
  "database is locked" string in unrelated error text can never steer retry
  behaviour.
- **Bounded, non-recursive retry.** The hard cap (6 attempts) and capped
  backoff (≤400 ms) bound contention on the single `Mutex<Connection>` — an
  anti-DoS property — and exhaustion is surfaced as `PersistenceFailed`, never
  swallowed. The backoff sleep runs with the connection mutex **released**
  (the guard is scoped per attempt inside the `op` closure), so a retrying
  writer cannot self-inflict a lock-hold stall on other same-handler callers.
- **Log hygiene.** New `tracing` events (including the `eprintln!` conversion)
  log only `effect_id`, `goal_id`, a static reason, and numeric retry fields —
  no payloads, no credential-bearing URLs, no tokens, no `{:?}` of opaque
  structs. Evidence is kept empty (`vec![]`), so no attacker-influenced goal
  metadata is persisted.
- **Best-effort metrics.** The counter is advisory (`let _ = …`); the durable
  audit record is the finished outbox row plus the tracing event.
- **No silent fallbacks.** Every swallowed race produces a structured, counted
  outcome; every retry exhaustion surfaces `PersistenceFailed`.

## Examples

### A goal completing mid-cycle no longer fails the cycle

```text
# Before (DownstreamFailed — one wasted cycle per goal):
typed goal-session effect incomplete (DownstreamFailed): goal disappeared before effect dispatch

# After (benign, counted no-op — the cycle succeeds):
WARN typed goal-session effect no-op: goal completed or removed between prepare and dispatch
     effect_id=eff-9f8e7d goal_id=move-the-governed-repo-roster reason=goal-removed-before-dispatch
```

### Startup recovery no longer collides with live cycles

```text
# Before (9 startup collisions in 6h):
typed OODA outbox startup recovery incomplete: typed outcome persistence failed: database is locked

# After (bounded retry absorbs the contention, write completes):
WARN typed OODA outbox write contended (busy/locked); retrying attempt=1 max_attempts=6 backoff_ms=10
# (no "database is locked" surfaced; recovery completes)
```

## When the no-op does *not* fire

The no-op is intentionally narrow. The dispatch produces a real
`DownstreamFailed` (not a no-op) when:

- The goal exists but the effect's repository **does not match** the
  authenticated goal repository (repo/tenant isolation error).
- The goal's repository **metadata fails to resolve** (`goal_repository()`
  error).
- The goal **already has an assigned engineer** (`already_assigned` →
  "goal already has an assigned engineer") — a duplicate-spawn guard, not a
  benign disappearance.
- The goal disappears **after** an engineer process was already spawned
  (post-spawn miss) — a real failure must be recorded so recovery can reconcile
  the orphaned process.
- The effect fails for any reason **other** than a legitimately-removed goal
  (retryable failures still release for retry; other permanent failures still
  map to `DownstreamFailed`).

This keeps the no-op scoped to exactly the benign prepare→dispatch race it
exists to absorb.

## Related

- [Typed-OODA architecture](../architecture/typed-ooda-loop.md) — the surrounding
  `decide → act` pipeline.
- [No-Bridge naming guard](./no-bridge-naming-guard.md) — the naming policy this
  change complies with.
- [Cognitive-memory durability](../operations/cognitive-memory-durability.md) —
  the sibling SQLite-durability posture this mirrors.
- [Distill raw-capture — metrics hygiene](./distill-raw-capture-on-parse-failure.md#metrics-hygiene)
  — the `metrics.jsonl` / `MetricEntry` envelope the benign-no-op counter uses.
