---
title: Typed-OODA persistence concurrency API
description: How the typed-OODA outcome ledger stays writer-safe under concurrent per-goal cycles and startup outbox recovery — WAL journaling, per-connection pragmas, and a process-wide per-file writer lock that make "database is locked" unreachable.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-capability-api.md
  - ../architecture/typed-ooda-loop.md
  - ../howto/diagnose-typed-ooda-database-locked.md
---

# Typed-OODA persistence concurrency API

The typed-OODA outcome ledger (`src/typed_ooda/ledger.rs`, `src/typed_ooda/schema.rs`)
is the single SQLite-backed store for terminal outcomes, progress records,
engineer claims, the idempotency registry, and the effect outbox. In live
operation the `simard-ooda` daemon runs **one thread per active goal** inside a
single process (`std::thread::scope`), and each thread opens its **own**
`CapabilityHandler` against the **same** ledger file. Startup also runs an
outbox recovery pass that writes to the same file.

This page documents the concurrency contract that keeps those writers from
colliding. It is the reference for issue #4483, which eliminated the systemic
`typed outcome persistence failed: database is locked` crash-loop.

## Guarantee

> Concurrent per-goal cycle writes and startup outbox recovery against one
> ledger file never surface `SQLITE_BUSY` / `database is locked`. A single slow
> writer cannot fail another goal's cycle.

The guarantee holds for all writers **inside one process**. Cross-process
contention (two `simard-ooda` processes on the same file) is out of scope; WAL
plus `busy_timeout` still let SQLite's own file locking degrade gracefully
rather than corrupt.

## How it works

Two independent, reinforcing layers deliver the guarantee.

### Layer 1 — correct SQLite configuration on every open

`schema::configure_connection` applies the durability and contention pragmas on
**every** connection handle, unconditionally, before schema initialization:

| Pragma | Value | Why |
| --- | --- | --- |
| `busy_timeout` | `5000` ms | A writer waits up to 5 s for a competing writer instead of failing instantly. |
| `journal_mode` | `WAL` | Write-ahead logging lets readers and one writer proceed concurrently; no whole-file exclusive lock for the common path. |
| `synchronous` | `NORMAL` | Safe with WAL; avoids an `fsync` per commit while preserving crash consistency. |
| `foreign_keys` | `ON` | Referential integrity across outcome / effect / claim tables. |

`configure_connection` is idempotent — re-asserting WAL on an already-WAL
database is a cheap no-op — so it is safe to run on the first open and on every
subsequent open.

!!! note "Why every open, not just first-init"
    Schema initialization early-returns once `user_version == SCHEMA_VERSION`,
    and before #4483 the `journal_mode = WAL` pragma lived *inside* that gated
    init path. `journal_mode=WAL` is a persistent, file-stored property, so a
    database first created by current code stays WAL across every reopen — the
    setting is not lost when the connection closes. The real gaps were narrower:
    (a) a **legacy ledger created by a build that predated the WAL line** never
    gets upgraded, because the version gate short-circuits before the pragma
    runs; and (b) `synchronous` (a per-connection setting) was never applied at
    all. Moving the pragmas into `configure_connection` — called
    unconditionally, outside the version gate — makes WAL authoritative on
    **every** handle regardless of when the file was created, and applies
    `synchronous=NORMAL` to every connection. WAL is necessary but not
    sufficient: it still permits only one writer at a time, so it does not by
    itself prevent multi-connection `SQLITE_BUSY` — which is exactly why Layer 2
    exists.

`journal_mode` is a persistent, file-stored property, so it is verifiable from
any connection — including an external `sqlite3` inspection:

```sql
PRAGMA journal_mode;   -- => wal
```

`synchronous`, by contrast, is a **per-connection** setting that is not stored
in the database file. An external `sqlite3` process opens its own connection and
reports its own default (`2`/FULL), *not* the daemon's `NORMAL`. It is therefore
observable only from within the daemon's own connection (see the regression test
below), never via external inspection.

### Layer 2 — process-wide per-file writer lock

WAL still permits only **one** writer at a time. Under a synchronized burst
(seven goals plus startup recovery), several handles can still contend and,
once `busy_timeout` is exhausted, one would fail. Layer 2 removes that race at
the source.

A process-global registry maps each **canonicalized** ledger path to a shared
writer lock:

```text
WRITER_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>
```

Every `CapabilityHandler::open` resolves the canonical path and registers (or
reuses) the `Arc<Mutex<()>>` for that path. All handlers pointing at the same
file therefore share **one** writer lock, even though each has its own
`Connection`. Path canonicalization means a relative handle and an absolute
handle to the same file share the same lock.

Every write acquires the writer lock **before** the per-connection mutex:

```text
lock():
    1. writer_lock.lock()      # process-wide, per file  (ordered first)
    2. connection.lock()       # per handler
    -> LedgerWriteGuard { _writer, connection }
```

Lock ordering is fixed at the single `lock()` site (writer → connection), so no
deadlock is possible. The guard is held for the entire write operation and
released when it drops.

`LedgerWriteGuard` derefs to `Connection`, so all existing call sites
(`let mut connection = self.lock()?;`) are unchanged — serialization is fully
transparent to callers, including the startup outbox recovery path
(`drain_pending`). This is why one goal's slow write can no longer time out and
fail a different goal's cycle: the writes are serialized, not racing.

## Failure visibility

Persistence is fail-visible; nothing is swallowed.

| Condition | Result |
| --- | --- |
| SQLite busy/locked | Retried within `busy_timeout`; serialized by the writer lock so it does not surface in normal operation. |
| Real SQL / serialization error | Returned as `CapabilityError::PersistenceFailed` (cycle path: `PersistenceFailed`), never a silent fallback. |
| Poisoned **connection** mutex | Surfaced as an `Err` — a poisoned connection is a real fault. |
| Poisoned **writer** `()` mutex | Recovered (`into_inner`) — the `()` guard protects no data, so one panic must not cascade into a crash loop across every goal. |
| `canonicalize` fails (path removed mid-run) | Falls back to the raw path and emits `tracing::warn!` — never panics, never silent. |

All new diagnostics use structured `tracing` / OpenTelemetry only. No
`print!` / `println!` is introduced. See the
[OODA capability API errors table](./ooda-capability-api.md#errors) for the full
`PersistenceFailed` semantics.

## Configuration

There are **no new configuration knobs, environment variables, or CLI flags**.
The fix is additive and non-breaking:

- `SCHEMA_VERSION` stays `1`; no schema-shape change and no migration.
- No public-API change; `CapabilityHandler::open`, `record_*`, and the read
  API (`simard ooda outcomes ...`) keep their existing signatures.
- The busy timeout is a named constant (`schema::BUSY_TIMEOUT = 5000 ms`),
  applied uniformly on every open. It is intentionally not operator-tunable —
  WAL plus the writer lock remove the contention that a longer timeout would
  otherwise paper over.

Existing ledger paths need no action: the first write after upgrade runs
`configure_connection`, which promotes the database to WAL in place. WAL adds
`-wal` and `-shm` sidecar files next to the ledger; both are managed by SQLite
and require no operator handling.

## Verifying the fix

Inspect a live ledger (read-only) to confirm the persistent journal mode:

```bash
sqlite3 "$SIMARD_STATE_ROOT/typed_ooda/ledger.db" 'PRAGMA journal_mode;'
# journal_mode -> wal
```

`journal_mode` is stored in the database file, so this external reading is
authoritative. Do **not** try to verify `synchronous` this way: it is a
per-connection setting, so the `sqlite3` CLI reports its own connection's default
(`2`/FULL), not the daemon's `NORMAL`. `synchronous=NORMAL` is asserted on the
daemon's own connection and is checked by the in-process regression test below.

The presence of `ledger.db-wal` next to `ledger.db` is a further at-a-glance
confirmation that WAL is active.

The concurrency contract is exercised by a regression test in
`src/typed_ooda/ledger.rs` (`#[cfg(test)]`): it opens multiple
`CapabilityHandler`s on one temp-dir ledger, drives burst writes plus a
startup-recovery-style writer from several threads, and asserts that

1. no operation returns `PersistenceFailed` with `database is locked`,
2. `PRAGMA journal_mode == wal` and `PRAGMA synchronous == 1`, and
3. the final row count is consistent across handlers (serialization held).

The test reads `PRAGMA synchronous` from a `CapabilityHandler`'s **own**
connection — the only place `NORMAL` is observable — not from an external
process.

Without the fix the test flakes under `SQLITE_BUSY`; with WAL plus the writer
lock it passes deterministically.

!!! warning "Maintainer note — keep this page in lockstep with the code"
    This reference is the implementation spec for #4483 and names the artifacts
    the fix must ship: `schema::configure_connection`, `schema::BUSY_TIMEOUT`,
    the process-global `WRITER_LOCKS` registry, `LedgerWriteGuard`, and the
    `writer_lock` field on the handler. After the fix lands, run a doc
    verification pass: `grep` those symbol names under `src/typed_ooda/` and
    confirm the regression test asserts `PRAGMA journal_mode == wal`. If any
    symbol is renamed or the timeout stops being a named constant, update this
    page so the contract stays accurate.

## Related

- [OODA capability API](./ooda-capability-api.md) — terminal schemas, the
  effect outbox, and the `PersistenceFailed` error contract this layer upholds.
- [Typed-capability OODA architecture](../architecture/typed-ooda-loop.md) —
  where the ledger sits in the goal-session path.
- [Diagnose the typed-OODA "database is locked" crash-loop](../howto/diagnose-typed-ooda-database-locked.md)
  — operator runbook for the #4483 signature.
