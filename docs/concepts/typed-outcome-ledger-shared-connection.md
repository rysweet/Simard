---
title: "Typed-Outcome Ledger Shared Connection (one connection per file, not per handler)"
description: >
  Why the typed-outcome ledger uses a single, process-wide serialized SQLite
  connection per ledger file instead of one connection per CapabilityHandler.
  Explains the concurrent-writer burst that produced systemic
  "typed outcome persistence failed: database is locked" errors (issue #4483),
  why a per-handler Mutex could not prevent it, how a path-keyed shared
  connection removes the race by construction, and why serializing the single
  ledger writer is an acceptable trade.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: explanation
status: design — not yet implemented
related:
  - ../reference/typed-outcome-ledger-connection-registry-api.md
  - ../howto/diagnose-typed-outcome-database-is-locked.md
  - ../reference/ooda-capability-api.md
---

# Typed-Outcome Ledger Shared Connection

> **Spec-first (retcon) document.** This explains the *target* design for issue
> **#4483**. The fix is **not yet landed** — today each `CapabilityHandler::open`
> still builds its own `Mutex<Connection>`. The documentation and implementation
> land in the **same pull request**; flip `status:` to `implemented` when that PR
> merges.

## The invariant

There is exactly **one** typed-outcome ledger file per state root:

```
<state-root>/typed-ooda/outcomes.sqlite3
```

It is the durable audit trail of terminal OODA outcomes and the effect outbox.
The invariant this design protects: **an outcome the system reported as recorded
is durably present in that file, and a write never silently fails.**

## What went wrong (issue #4483)

Within a single goal session, several `CapabilityHandler` instances open that
*same file*:

- the **startup outbox worker** that drains pending effects on session start
  (`OutboxWorker::drain_pending` in
  `src/ooda_actions/advance_goal/typed_goal_session.rs`), and
- the **route executor** that records terminal outcomes and enqueues effects.

Concurrently running goal sessions add more openers of the same file.

Before the fix, each `open` created an **independent** `Connection` and wrapped it
in a **per-handler** `Mutex`:

```rust
// pre-fix
connection: Mutex<Connection>,          // one per handler
// ...
connection: Mutex::new(connection),     // independent connection per open()
```

A `Mutex<Connection>` serializes access **inside one handler**. It does nothing
between handlers: two handlers are two independent connections to the same file.
When both attempt a write transaction at once, SQLite's file-level locking lets
only one writer proceed and the other gets `SQLITE_BUSY` — surfaced as
`"database is locked"`.

`open` sets a `busy_timeout`, which retries for a few seconds. But when many
writers converge in a **burst** — several goals reaching a terminal at the same
moment, each with its startup drain firing — the timeout is exhausted and the
busy error escapes. It is mapped through `persistence(..)` to
`CapabilityErrorCode::PersistenceFailed`, which **aborts the record**. The
terminal outcome that the loop believed it had persisted is dropped from the
audit trail. That is the "systemic typed-outcome PersistenceFailed" of #4483.

Concretely, the failure signature is several goals (say `<goal-a>` … `<goal-f>`,
six distinct goals reaching a terminal in the same window) each logging
`typed outcome persistence failed: ... database is locked` within the same
few seconds — a contention *burst*, not a steady leak.

## Why a bigger `busy_timeout` is not the fix

Raising the timeout only widens the window before the error surfaces; under a
true burst of independent connections the collision is structural, and a longer
timeout trades a lost outcome for a stalled goal session. The problem is *having
multiple independent writers to one file at all*, not how long each one waits.

## The fix: one connection per file, shared

The design collapses "one connection per handler" into **one connection per
ledger file, per process**, held behind a process-global, path-keyed registry:

```
OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>>
```

Every `CapabilityHandler` opened against a given file **clones the same**
`Arc<Mutex<Connection>>`. The `Mutex` that used to serialize one handler now
serializes **every** writer to that file across the whole process. There is only
ever one connection issuing writes, so the cross-connection `SQLITE_BUSY` race
cannot occur — it is removed by construction, not merely retried away.

Opening a new file also applies durable pragmas once — WAL journaling,
`busy_timeout = 5000`, and `foreign_keys = ON` — and a bounded
`with_busy_retry` wraps writes as defense-in-depth against *external* processes
touching the file. See the
[connection registry API reference](../reference/typed-outcome-ledger-connection-registry-api.md)
for the exact contract.

## Why serializing the writer is acceptable

The typed-outcome ledger is a **low-frequency, small-write** audit trail:
terminal outcomes and outbox effect rows, written at OODA cycle boundaries — not
a hot data-plane. Serializing its single writer costs nothing meaningful in
throughput, and it buys a hard correctness guarantee: no dropped outcomes under
contention. WAL additionally keeps **readers** from blocking the writer, so
liveness/claim reads stay responsive while a write holds the connection.

## Why path-keyed, not one global connection

A single global connection would force *every* ledger file in the process to
serialize against one lock — breaking test isolation (each test uses its own
temp-dir ledger) and any future multi-tenant separation. Keying the registry by
**canonical path** means only handlers for the *same* file share a connection;
distinct files remain fully independent. This preserves the existing test
suite's parallelism and keeps tenants' ledgers isolated.

## What this does *not* change

- The public `CapabilityHandler::open` / `with_engineer_liveness` API.
- The schema and its `UNIQUE(outcome_id)` / `UNIQUE(session_id, cycle_id)`
  invariants and foreign keys.
- The fail-visible contract: a write either commits or the caller sees
  `PersistenceFailed`. Sharing a connection changes *how many* writers exist, not
  *whether* failures are surfaced.
- Authorization, replay, and effect-lease semantics from the
  [OODA capability API](../reference/ooda-capability-api.md).

## See also

- Reference: [Typed-outcome ledger connection registry API](../reference/typed-outcome-ledger-connection-registry-api.md).
- Runbook: [Diagnose "typed outcome persistence failed: database is locked"](../howto/diagnose-typed-outcome-database-is-locked.md).
