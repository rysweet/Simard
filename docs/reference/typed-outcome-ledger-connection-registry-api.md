---
title: "Reference: Typed-Outcome Ledger Connection Registry API"
description: >
  The process-global, path-keyed connection registry that makes every
  CapabilityHandler opened against the same typed-outcome ledger file
  (typed-ooda/outcomes.sqlite3) share a single serialized SQLite connection.
  Covers the registry data structure, apply_pragmas (WAL + busy_timeout +
  foreign_keys), the with_busy_retry / is_sqlite_busy bounded backoff, the
  unchanged CapabilityHandler::open surface, lock ordering, error semantics,
  security notes, and the concurrency regression contract that closes issue
  #4483 (systemic typed-outcome PersistenceFailed under concurrent writers).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ../concepts/typed-outcome-ledger-shared-connection.md
  - ../howto/diagnose-typed-outcome-database-is-locked.md
  - ooda-capability-api.md
  - engineer-claim-release-api.md
  - typed-ooda-goal-session-rails.md
---

# Reference: Typed-Outcome Ledger Connection Registry API

> **Spec-first (retcon) document.** This reference describes the *target* design
> for issue **#4483**. At the time of writing the fix is **not yet landed**:
> `CapabilityHandler` still holds a private `Mutex<Connection>` and each `open`
> builds an independent connection. This document is the implementation
> specification. **The documentation and the implementation land in the same
> pull request** — when that PR merges, flip `status:` to `implemented`. Until
> then, treat present-tense statements below as the contract to build against,
> not as shipped behaviour.
>
> **Snippet disclaimer.** The Rust snippets below illustrate the *contract*
> (types, ordering, error mapping). They are not copied verbatim from a
> not-yet-written source; the final `ledger.rs` may differ in naming and layout
> as long as the observable contract in this document holds.

## Problem this API closes

The typed-outcome ledger is a single SQLite file:

```
<state-root>/typed-ooda/outcomes.sqlite3
```

(`typed_ooda::LEDGER_RELATIVE_PATH`, resolved by `typed_ooda::ledger_path`.)

Multiple `CapabilityHandler` instances are opened against that **same file**
within one process during a single goal session — at minimum:

- the goal-session **startup outbox worker** that drains pending effects
  (`OutboxWorker::drain_pending`, wired in
  `src/ooda_actions/advance_goal/typed_goal_session.rs`), and
- the **route executor** that records terminal outcomes and enqueues effects
  (`typed_ooda::route` / `typed_ooda::executor`).

Before this fix each `open` created its **own** `Connection` and wrapped it in a
**per-handler** `Mutex`. That mutex serializes access *within one handler* but
does nothing across handlers: two handlers hold two independent connections to
the same file. Concurrent write transactions from those distinct connections
collide at the SQLite file-lock layer and one returns `SQLITE_BUSY`
("database is locked"). `open` sets a `busy_timeout`, but under a burst of
simultaneous writers the timeout can still be exhausted. The busy error is
mapped through `persistence(..)` to `CapabilityErrorCode::PersistenceFailed`,
which aborts the record and **drops a durable terminal outcome from the audit
trail** — the systemic failure reported in issue #4483.

The fix collapses "one connection per handler" into **one connection per ledger
file per process**, so the existing inner `Mutex` becomes a process-wide
serialization point and the cross-connection `SQLITE_BUSY` race is eliminated by
construction.

## Design decisions

| ID  | Decision |
| --- | -------- |
| D1  | A process-global, **path-keyed** connection registry: `OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>>`. One shared connection per canonical ledger path. |
| D2  | `CapabilityHandler.connection` changes type from `Mutex<Connection>` to a shared `Arc<Mutex<Connection>>` (`SharedConn`). All handlers for the same file clone the **same** `Arc`. |
| D3  | `apply_pragmas` runs **once per newly created connection**, before `schema::initialize`: `journal_mode = WAL`, `busy_timeout = 5000ms`, `foreign_keys = ON`. |
| D4  | `with_busy_retry` wraps write transactions in a **bounded** retry that classifies `SQLITE_BUSY` / `SQLITE_LOCKED` via `is_sqlite_busy` and retries with short backoff before surfacing `PersistenceFailed`. Fail-visible: exhausted retries still return the error. |
| D5  | The startup outbox drain and the route executor now share one connection, so startup recovery no longer contends with the executor for the same file. |
| D6  | **First-init serialization**: the first opener of a path initializes the schema while holding the registry mutex; concurrent openers of the same path block, then clone the ready `Arc`. |
| D7  | **Lock ordering**: hold the registry mutex only to look up / insert and clone the `Arc`, then **release it before** locking the inner connection mutex. No nested acquisition → no deadlock. |
| D8  | The registry is **path-keyed**, not a single global connection. Distinct ledger files (e.g. per-test temp dirs) get distinct connections, preserving test isolation and multi-tenant separation. |

## The connection registry

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use rusqlite::Connection;

/// One serialized connection shared by every handler for a given ledger file.
type SharedConn = Arc<Mutex<Connection>>;

/// Process-global registry keyed by canonical ledger path.
static LEDGER_CONNECTIONS: OnceLock<Mutex<HashMap<PathBuf, SharedConn>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<PathBuf, SharedConn>> {
    LEDGER_CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}
```

Keying is by a **canonicalized** path so that `./x/outcomes.sqlite3` and an
absolute form of the same file resolve to one entry. If canonicalization fails
(the file does not exist yet), the registry falls back to the path as given —
the directory is created by the caller before `open`, and the first successful
`open` canonicalizes for subsequent lookups.

## `apply_pragmas`

```rust
fn apply_pragmas(connection: &Connection) -> CapabilityResult<()> {
    // Readers never block the single writer; the single writer is already
    // serialized by SharedConn, so WAL contention cannot burst across
    // connections.
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(persistence)?;
    // Defense-in-depth for any *external* contention (e.g. another OS process
    // touching the file). Equivalent to `PRAGMA busy_timeout = 5000`.
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(persistence)?;
    // Enforce the effect_jobs -> terminal_outcomes foreign key at write time.
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(persistence)?;
    Ok(())
}
```

`apply_pragmas` runs exactly once, on the freshly opened `Connection`, **before**
`super::schema::initialize`, and while the registry mutex is held (D6). Handlers
that clone an already-registered `Arc` never re-run it.

## `with_busy_retry` / `is_sqlite_busy`

```rust
fn is_sqlite_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::DatabaseBusy
                || e.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// Retry a write closure a bounded number of times on SQLITE_BUSY/LOCKED,
/// with short backoff, then surface the error unchanged.
fn with_busy_retry<T>(
    mut op: impl FnMut() -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    const MAX_ATTEMPTS: u32 = 5; // ~low-hundreds-of-ms worst case
    let mut attempt = 0;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) if is_sqlite_busy(&error) && attempt + 1 < MAX_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(backoff(attempt));
            }
            Err(error) => return Err(error), // fail-visible
        }
    }
}
```

`with_busy_retry` is defense-in-depth. With `SharedConn` in place the *in-process*
writer is already single-threaded, so busy errors should not originate from this
process; the retry absorbs transient contention from an external process and
guarantees that a genuinely stuck write still ends as `PersistenceFailed` rather
than hanging. The constant (`MAX_ATTEMPTS ≈ 5`) and backoff schedule are
illustrative — reconcile the doc with the final constants once implemented.

## `CapabilityHandler::open` — unchanged surface

The public signature does **not** change:

```rust
impl CapabilityHandler {
    pub fn open(path: impl AsRef<Path>, policy: CapabilityPolicy) -> CapabilityResult<Self>;
    pub fn with_engineer_liveness(self, liveness: Box<dyn EngineerLiveness>) -> Self;
}
```

Only the body changes — it consults the registry instead of building a private
connection:

```rust
pub fn open(path: impl AsRef<Path>, policy: CapabilityPolicy) -> CapabilityResult<Self> {
    let key = canonical_key(path.as_ref());
    let mut table = registry()
        .lock()
        .map_err(|_| persistence_message("outcome ledger registry lock is poisoned"))?;

    let shared = match table.get(&key) {
        Some(existing) => Arc::clone(existing),               // D2/D7: clone + reuse
        None => {
            let mut connection = Connection::open(path.as_ref()).map_err(persistence)?;
            apply_pragmas(&connection)?;                      // D3, before schema
            super::schema::initialize(&mut connection, now_millis()).map_err(persistence)?; // D6
            let shared = Arc::new(Mutex::new(connection));
            table.insert(key, Arc::clone(&shared));
            shared
        }
    };
    drop(table); // D7: release registry mutex before any inner lock

    Ok(Self { connection: shared, policy, engineer_liveness: None })
}
```

`lock()` — the single accessor of the connection field — is unchanged in
behaviour and still maps a poisoned mutex to a fail-visible `PersistenceFailed`:

```rust
fn lock(&self) -> CapabilityResult<MutexGuard<'_, Connection>> {
    self.connection
        .lock()
        .map_err(|_| persistence_message("outcome ledger lock is poisoned"))
}
```

Because `connection` is now `Arc<Mutex<Connection>>`, `self.connection.lock()`
still yields `MutexGuard<'_, Connection>` and every existing call site
(`release_engineer_claim`, `record_action`, `claim_next_effect`,
`recover_expired_effects`, …) compiles unchanged.

## Shared startup-recovery path (D5)

`typed_goal_session.rs` opens the handler once and builds the startup
`OutboxWorker` from it:

```rust
let handler = CapabilityHandler::open(&ledger_path, policy)?
    .with_engineer_liveness(Box::new(WorktreeEngineerLiveness { .. }));

let startup_worker = OutboxWorker::new(
    &handler, &effects, "goal-session-startup-worker", Duration::from_secs(300),
);
if let Err(error) = startup_worker.drain_pending(32) {
    eprintln!("[simard] typed OODA outbox startup recovery incomplete: {error}");
}
```

The subsequent `route.execute(.., &handler, ..)` uses the **same** handler and
therefore the same `SharedConn`. Even if a second handler were opened against the
same `ledger_path`, D1–D2 guarantee it clones the same `Arc` — the startup drain
and the executor can never hold two competing connections to the file.

## Error semantics

| Situation | Result |
| --- | --- |
| Registry mutex poisoned | `PersistenceFailed` — `"outcome ledger registry lock is poisoned"` |
| Connection mutex poisoned | `PersistenceFailed` — `"outcome ledger lock is poisoned"` (unchanged) |
| `Connection::open` / pragma / `schema::initialize` fails | `PersistenceFailed` via `persistence(..)` |
| Write hits `SQLITE_BUSY`/`SQLITE_LOCKED`, retries exhausted | `PersistenceFailed` (fail-visible; never swallowed) |
| Duplicate terminal outcome | Existing `UNIQUE(outcome_id)` / `UNIQUE(session_id, cycle_id)` conflict handling is unchanged; the shared connection does not relax it |

All ledger errors continue to funnel through `persistence` /
`persistence_message` into `CapabilityErrorCode::PersistenceFailed`. This fix
adds **no** new error code and preserves the fail-visible contract: an outcome is
either durably recorded or the caller sees an explicit error.

## Security notes

- **S — least exposure.** Registry keys are canonical ledger **file paths** only.
  No record payloads, credentials, session identities, or actor scopes are held
  in the registry. Its blast radius is a `PathBuf → Arc<Mutex<Connection>>` map.
- **S — audit-trail integrity.** The durable-terminal uniqueness invariants
  (`UNIQUE(outcome_id)`, `UNIQUE(session_id, cycle_id)`) are enforced by the
  schema, not by connection multiplicity. Sharing one connection only *serializes*
  writers; it cannot cause a double-insert, and `with_busy_retry` re-runs the same
  guarded transaction, so a retried insert of an already-committed outcome still
  conflicts deterministically rather than duplicating.
- **S — referential integrity.** `foreign_keys = ON` (D3) keeps the
  `effect_jobs.outcome_id → terminal_outcomes(outcome_id)` foreign key enforced,
  so an effect can never be enqueued for a non-existent outcome.
- **S — fail-visible, never fail-open.** Poisoned locks and exhausted retries map
  to `PersistenceFailed`. The ledger never silently degrades to "recorded" when a
  write did not commit.
- **S — fail-closed liveness preserved.** The engineer-liveness reclaim gate
  (`with_engineer_liveness`, `EngineerLiveness`) is untouched; with no provider a
  claim is still treated as live and a duplicate spawn stays rejected.
- **S — no new external surface.** The registry is in-process only
  (`static OnceLock`). It exposes no IPC, no network, and no filesystem paths
  beyond the ledger file the process already owns.
- **S — test / tenant isolation.** Because keys are per-path (D8), distinct
  ledgers never share a connection; one test or tenant cannot serialize against,
  observe, or corrupt another's file.

## Concurrency regression contract

The fix is guarded by a regression test that reproduces issue #4483's burst:

- Open **N** `CapabilityHandler`s against **one** ledger path (mirroring the
  goal-session startup-worker + executor + concurrent sessions).
- Drive concurrent terminal-outcome / effect writes across those handlers.
- Assert **zero** `PersistenceFailed` results attributable to
  `SQLITE_BUSY`/`database is locked`, and that every outcome that reported success
  is durably present (row counts match, uniqueness intact).

The test must **fail** against the pre-fix "connection per handler" code and
**pass** once the registry, pragmas, and retry land — that is the acceptance
signal for issue #4483.

## What did not change

- `CapabilityHandler::open` / `with_engineer_liveness` signatures.
- The `lock()` accessor contract and its poisoned-lock error message.
- The `persistence` / `persistence_message` mapping to `PersistenceFailed`.
- The schema (`terminal_outcomes`, `effect_jobs`, `engineer_claims`, …) and its
  uniqueness / foreign-key constraints.
- The `OutboxWorker` API (`new`, `drain_pending`, `recover_startup`).
- Any authorization, replay, or effect-lease semantics from
  [OODA capability API](./ooda-capability-api.md).

## See also

- Concept: [Typed-outcome ledger shared connection](../concepts/typed-outcome-ledger-shared-connection.md) — *why* one connection per file.
- Runbook: [Diagnose "typed outcome persistence failed: database is locked"](../howto/diagnose-typed-outcome-database-is-locked.md).
- [OODA capability API](./ooda-capability-api.md).
- [Engineer-Claim Release & Reclaim API](./engineer-claim-release-api.md).
