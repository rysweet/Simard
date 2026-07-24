---
title: 'How-to: diagnose a typed-OODA "database is locked" crash-loop'
description: >
  Confirm, diagnose, and clear the `typed outcome persistence failed: database
  is locked` crash-loop in the typed-OODA ledger. Covers reading the
  fail-visible tracing lines, verifying the WAL journal mode and 30s
  busy_timeout are applied at open, and checking for the `-wal`/`-shm` sidecars,
  so OODA cycles persist outcomes reliably.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/typed-ooda-ledger-concurrency.md
  - ../reference/claim-reaper-api.md
  - ../operations/cognitive-memory-durability.md
  - ./diagnose-leaked-engineer-claims.md
  - ./diagnose-and-recover-ooda-step-failures.md
---

# Diagnose a typed-OODA "database is locked" crash-loop

> **Status: implemented (issue #4483).**
> The concurrency hardening described here ships in
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
> and [`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs).
> Contract:
> [Typed-OODA ledger concurrency hardening](../reference/typed-ooda-ledger-concurrency.md).

## Symptom

The daemon log repeats a persistence failure and OODA cycles stop making
progress across many goals:

```text
typed outcome persistence failed: database is locked
```

On a hardened daemon this message should not recur. A single transient
occurrence immediately followed by a successful retry is expected and benign;
a **crash-loop** (the same message every cycle, no forward progress) means one
of the concurrency settings below is not in effect — for example a ledger that
was created before the fix and re-opened in rollback journal mode.

## 1. Confirm it is the typed-OODA ledger

The message originates in the ledger persistence path
([`persistence`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)
error mapper). Confirm the failing writes are terminal-outcome / progress /
effect-job persistence, and note whether the error clears on retry (benign) or
loops (needs action). All lock diagnostics are structured `tracing` / OTel
lines — there is no `print!`/`println!` output to grep for.

## 2. Verify WAL and busy_timeout are applied

WAL journal mode and the 30s `busy_timeout` are applied **at every connection
open**, unconditionally — not only during a schema migration. Inspect the live
ledger database:

```bash
sqlite3 <ledger.db> 'PRAGMA journal_mode;'   # expect: wal
```

If this reports `delete` (rollback mode), the ledger is running the
pre-fix configuration. Restarting the daemon on the hardened binary re-opens the
database and switches it to WAL; verify with the command above.

## 3. Check the WAL sidecar files

A WAL-mode ledger has two sidecar files next to the database:

```bash
ls -l <ledger-dir>/<ledger.db>-wal <ledger-dir>/<ledger.db>-shm
```

Both should be present and owned/permissioned like the ledger directory (not
world-writable, not in a temp path). When taking a **cold** backup of the
ledger, copy the `-wal` and `-shm` files alongside the main database, or
checkpoint first — the same discipline used for the cognitive store
([Cognitive Memory Durability](../operations/cognitive-memory-durability.md)).

## 4. Confirm write transactions serialize, not fail

Writers use `TransactionBehavior::Immediate`, so concurrent writers acquire the
write lock at `BEGIN` and wait out the `busy_timeout` instead of racing and
failing late. If you still see sustained lock errors after confirming WAL +
busy_timeout, look for a writer holding a transaction open across slow work
(network / agent I/O) — transaction bodies are meant to contain only
bound-parameter SQL. A write that cannot acquire the lock within the
`busy_timeout` surfaces the error to the log and metrics rather than looping
forever; that surfaced error is the signal to investigate the slow holder.

## 5. Rule out false reaps / leaked claims

Persistent lock contention can coincide with engineer-claim lifecycle issues.
The reaper lease-ownership guard that prevents false stale-engineer reaps is
tracked separately (#4467/#4464/#4462/#4500) and is **not** part of this
ledger-open hardening. To inspect leaked or reaped claims, follow
[Diagnose and clear leaked engineer claims](./diagnose-leaked-engineer-claims.md).

## Resolution checklist

- [ ] Ledger reports `journal_mode = wal`.
- [ ] `-wal` / `-shm` sidecars present with correct permissions.
- [ ] `database is locked` no longer recurs every cycle (transient + retry OK).
- [ ] OODA cycles persist terminal outcomes and progress again.
- [ ] No false stale-engineer reaps or leaked `engineer_claims` rows.
