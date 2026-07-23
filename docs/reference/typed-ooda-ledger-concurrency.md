---
title: "Reference: Typed-OODA ledger concurrency hardening"
description: >
  The concurrency contract for the typed-OODA SQLite ledger: unconditional
  WAL journal mode and a 30s busy_timeout applied at every connection open,
  Immediate write transactions with minimized hold time, and fail-visible
  propagation of `database is locked` faults through CapabilityResult with
  bounded retry (never swallowed). Also documents the decide->act
  outcome-persistence fix and the reaper lease-ownership guard that eliminates
  false stale-engineer reaps and leaked claims.
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
  - ../../src/overseer/claim_reaper.rs
---

# Reference: Typed-OODA ledger concurrency hardening

> **Status: implemented (issues #4483, #4468, #4467, #4464, #4462, #4500).**
> Present-tense description of shipped behaviour. Primary sources:
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs),
> [`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs),
> and
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs).
>
> This change eliminates the `typed outcome persistence failed: database is
> locked` crash-loop (#4483) that was failing OODA cycles across many goals,
> and the associated decide→act effect-dispatch (#4468) and claim-reaper
> lifecycle (#4467/#4464/#4462/#4500) concurrency defects.

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
3. Fail-visible lock-error propagation with bounded retry.

Plus a fourth, lifecycle part: a reaper lease-ownership guard.

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

`database is locked` is treated as a **first-class, retryable** condition, not a
terminal failure and never a silently-discarded one:

- Persistence faults surface through `CapabilityResult` (the
  [`persistence`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
  error mapper), preserving the fail-visible posture of the capability layer.
- The Act-loop persistence path retries a locked write with **bounded
  backoff**. Because `busy_timeout` already absorbs sub-second contention, a
  retry escalation only fires on sustained contention, and after the bounded
  retries are exhausted the error is returned (visible in the daemon log and
  metrics) rather than looping forever.

> **Security requirement.** A swallowed lock error on a claim-release or
> outcome-persist would leak a privileged `engineer_claims` row or drop a
> terminal outcome. Lock errors are therefore **always** propagated — dropping
> them is prohibited by the regression tests below.

---

## 4. Reaper lease-ownership guard

The claim reaper (#4467/#4464/#4462/#4500) reaps an `effect_jobs` /
`engineer_claims` lease **only** when all of the following hold under a
consistent, monotonic clock:

1. `lease_owner` matches the reaping actor's identity, **and**
2. `lease_generation` matches the generation the reaper observed, **and**
3. `lease_expires_at` is genuinely in the past.

A lease owned by a *different* actor, or whose generation has advanced (a live
renewal), is never reaped — this eliminates the false stale-engineer reaps and
leaked claims. Expiry is evaluated against a monotonic time source so wall-clock
skew cannot trigger a premature or cross-owner reap. The decide→act
outcome-persistence defect (#4468) is fixed in the same effect-dispatch path so
a decided effect is persisted exactly once before it is dispatched.

See the [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md) for the full
reaper contract; this section documents only the ownership/expiry guard added
by the concurrency hardening.

---

## Regression tests

| Test surface | Guarantee |
|---|---|
| `ledger.rs` — concurrent-writer test | Multiple writers persisting outcomes concurrently all succeed; no `database is locked` terminal error. |
| `ledger.rs` — lock-error-propagation test | A forced lock returns a `CapabilityResult` error; it is never swallowed or converted to a silent no-op. |
| `engineer_worktree/tests_reaping_safety.rs` — cross-owner no-reap | A lease owned by another actor (or with an advanced generation) is not reaped. |
| `engineer_worktree/tests_reaping_safety.rs` — reaper race | Concurrent renew + reap never double-releases or leaks a claim. |

---

## Operator guidance

If you observe `typed outcome persistence failed: database is locked` in the
daemon log, follow
[Diagnose a typed-OODA "database is locked" crash-loop](../howto/diagnose-typed-ooda-database-locked.md).
On a correctly-hardened daemon this message should not recur; a single
transient occurrence followed by a successful retry is expected and benign.
