---
title: "Reference: Typed-OODA ledger concurrency hardening"
description: >
  The concurrency contract for the typed-OODA SQLite ledger: unconditional
  WAL journal mode and a 30s busy_timeout applied at every connection open,
  Immediate write transactions with minimized hold time, and fail-visible
  propagation of `database is locked` faults through CapabilityResult
  (never swallowed).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./claim-reaper-api.md
  - ./typed-ooda-goal-session-rails.md
  - ./ooda-capability-api.md
  - ./engineer-claim-release-api.md
  - ../operations/cognitive-memory-durability.md
  - ../howto/diagnose-typed-ooda-database-locked.md
  - ../../src/typed_ooda/ledger.rs
  - ../../src/typed_ooda/schema.rs
---

# Reference: Typed-OODA ledger concurrency hardening

> **Status: implemented (issue #4483).** Present-tense description of shipped
> behaviour. Primary sources:
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
> and
> [`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs).
>
> This change eliminates the `typed outcome persistence failed: database is
> locked` crash-loop (#4483) that was failing OODA cycles across many goals, by
> correcting the ledger's SQLite concurrency configuration at connection open.
> The adjacent decide→act effect-dispatch (#4468) and claim-reaper lifecycle
> (#4467/#4464/#4462/#4500) concurrency defects are tracked and delivered
> **separately** — see [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md).

---

## Why this exists

The typed-OODA ledger is a single SQLite database
([`CapabilityHandler::open`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs))
that multiple in-process writers touch concurrently: the OODA Act loop
persisting terminal outcomes and progress records, the effect dispatcher
leasing and completing `effect_jobs`, and the overseer claim reaper releasing
`engineer_claims`. Under load these writers contended on a single rollback-mode
connection and SQLite returned `SQLITE_BUSY` (`database is locked`), which the
persistence path surfaced as a terminal error and re-entered on the next cycle —
a crash-loop that produced no forward progress.

The fix has three parts, all **additive / non-breaking** and requiring **no
`SCHEMA_VERSION` bump** (the changes are runtime connection settings, not schema
migrations):

1. Correct SQLite concurrency configuration at every open.
2. Immediate, short-lived write transactions.
3. Fail-visible lock-error propagation.

---

## 1. Connection configuration (WAL + busy_timeout)

`CapabilityHandler::open` applies concurrency-critical pragmas
**unconditionally on every connection open**, independent of whether the
database is being migrated:

| Setting | Value | Rationale |
|---|---|---|
| `PRAGMA journal_mode` | `WAL` | Write-ahead logging lets one writer proceed concurrently with readers, removing the dominant `SQLITE_BUSY` source. |
| `busy_timeout` | `30s` | A generous timeout lets a blocked writer wait out a peer's short transaction instead of failing immediately. |
| `PRAGMA foreign_keys` | `ON` | Preserved from prior behaviour. |

> **Design note — WAL moved out of the migration branch.** Previously
> `journal_mode = WAL` was set only inside the schema-migration branch of
> [`schema::initialize`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs),
> so a database already at `SCHEMA_VERSION` re-opened in **rollback** journal
> mode and never got WAL. WAL is now applied at open time for **every** ledger,
> including pre-existing v1 databases, so concurrency settings are correct
> regardless of migration state.

### Sidecar files and permissions

Opening in WAL creates `-wal` and `-shm` sidecar files next to the ledger
database. These inherit the ledger directory's permissions. The ledger path is
derived internally (never from an environment variable or CLI argument), so the
sidecars are never created in a world-writable or temp location. Operators
performing a cold backup of the ledger must copy the `-wal` and `-shm` files
alongside the main database, or checkpoint first — see
[Cognitive Memory Durability](../operations/cognitive-memory-durability.md) for
the equivalent WAL/checkpoint discipline on the cognitive store.

---

## 2. Write transactions are Immediate and short

Every method that writes uses an **Immediate** transaction:

```rust
let transaction = connection
    .transaction_with_behavior(TransactionBehavior::Immediate)?;
// ... bound-parameter writes only ...
transaction.commit()?;
```

`TransactionBehavior::Immediate` acquires the write lock at `BEGIN` rather than
at first write, so two writers serialize deterministically at the start of their
transactions (waiting out the `busy_timeout`) instead of racing to upgrade a
deferred transaction and one of them failing late with `SQLITE_BUSY`.

Transaction bodies are kept minimal: no network calls, no agent I/O, and no
long computation happen while the write lock is held. All SQL is
**parameter-bound** (`params![…]` / positional `?n`); no write statement is
assembled with `format!`, preserving the injection-safe contract of the
capability layer.

Writer methods covered include (non-exhaustive):
[`record_action`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs),
`record_progress`, `record_completed`, `record_blocked`, `record_no_action`,
`execute_process`, `claim_next_effect`, `claim_effect_for_outcome`,
`recover_expired_effects`, `register_actor_session`, `issue_privileged_approval`,
and `release_engineer_claim`.

---

## 3. Lock errors are propagated, never swallowed

`database is locked` is treated as a **first-class** condition — never a
silently-discarded one:

- The 30s `busy_timeout` (part 1) absorbs contention at the SQLite layer: a
  writer that cannot immediately acquire the write lock waits and retries
  internally for up to 30s before SQLite surfaces `SQLITE_BUSY`.
- If a write still cannot acquire the lock within that window, the fault
  surfaces through `CapabilityResult` (the
  [`persistence`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
  error mapper) and is returned to the caller — visible in the daemon log and
  metrics — rather than being swallowed or converted to a silent no-op. No
  application-level retry loop wraps the persistence call; the busy-timeout wait
  is the sole retry mechanism.

> **Security requirement.** A swallowed lock error on a claim-release or
> outcome-persist would leak a privileged `engineer_claims` row or drop a
> terminal outcome. Lock errors are therefore **always** propagated: the
> [`persistence`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
> mapper returns an `Err` on the `CapabilityResult` path and never converts a
> lock failure into a silent no-op.

---

## 4. Related: reaper lifecycle and decide→act persistence

The claim-reaper lease-ownership guard (#4467/#4464/#4462/#4500) and the
decide→act outcome-persistence fix (#4468) address adjacent concurrency defects
in the same subsystem, but are tracked and delivered **separately** from this
ledger-open hardening — they are not part of the change documented here. See the
[Stale-Engineer-Claim Reaper API](./claim-reaper-api.md) for the reaper
contract.

---

## Regression tests

| Test surface | Guarantee |
|---|---|
| `ledger.rs` — `open_applies_wal_journal_mode_on_preexisting_v1_db` | Re-opening a pre-existing v1 ledger whose journal was reverted to rollback mode leaves it in WAL mode. |
| `ledger.rs` — `open_sets_busy_timeout_at_least_30s` | A `busy_timeout` of at least 30s is configured at every open. |
| `ledger.rs` — `concurrent_cross_connection_writers_never_hit_database_locked` | Multiple writers on separate connections to the same file all succeed; no `database is locked` error surfaces. |

---

## Operator guidance

If you observe `typed outcome persistence failed: database is locked` in the
daemon log, follow
[Diagnose a typed-OODA "database is locked" crash-loop](../howto/diagnose-typed-ooda-database-locked.md).
On a correctly-hardened daemon this message should not recur; a single
transient occurrence followed by a successful retry is expected and benign.
