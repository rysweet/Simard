---
title: Diagnose "typed outcome persistence failed: database is locked"
description: >
  Operator runbook for the systemic typed-outcome PersistenceFailed burst fixed
  under issue #4483. Recognise the concurrent-writer "database is locked"
  signature in the typed-ooda/outcomes.sqlite3 ledger, confirm the shared
  path-keyed connection registry, WAL/busy_timeout/foreign_keys pragmas, and
  bounded busy-retry are in effect, run the concurrency regression test, and
  localise a recurrence to an external writer or a bypassed open() path.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: how-to
status: design — not yet implemented
related:
  - ../concepts/typed-outcome-ledger-shared-connection.md
  - ../reference/typed-outcome-ledger-connection-registry-api.md
  - diagnose-and-recover-ooda-step-failures.md
---

# Diagnose "typed outcome persistence failed: database is locked"

> **Spec-first (retcon) runbook.** This documents operating the fix for issue
> **#4483**, which is **not yet landed**. The documentation and implementation
> ship in the **same pull request**; flip `status:` to `implemented` on merge.
> Until then, "in effect" checks below describe the state you are verifying once
> the fix is present.

## Symptom

One or more terminal outcomes fail to persist, and the logs show:

```
typed outcome persistence failed: ... database is locked
```

(from `persistence(..)` → `CapabilityErrorCode::PersistenceFailed`). The
distinguishing signature of issue #4483 is a **burst**: several distinct goals
reaching a terminal in the same few-second window each emit the error, rather
than a single steady failure. The affected file is the typed-outcome ledger:

```
<state-root>/typed-ooda/outcomes.sqlite3
```

## Why it happens (one line)

Multiple `CapabilityHandler` instances opened against that one file used to hold
**independent** connections; concurrent writers collided at SQLite's file lock
and one got `SQLITE_BUSY`. See
[the concept doc](../concepts/typed-outcome-ledger-shared-connection.md) for the
full explanation.

## Step 1 — Recognise the burst signature

Confirm it is the concurrency burst and not an unrelated I/O error:

```bash
# Adjust the log source to your deployment.
journalctl --user -u 'simard*' --since '15 min ago' \
  | grep -E 'typed outcome persistence failed.*database is locked'
```

Look for **several distinct goal / session identifiers** clustered within the
same few seconds. A lone occurrence spread over minutes is more likely an
external writer (Step 4), disk pressure, or a permissions problem.

## Step 2 — Confirm the shared connection registry is in effect

The fix makes every handler for one file share a single connection. Verify the
registry and the shared field type exist:

```bash
cd <repo-root>
grep -n 'OnceLock<Mutex<HashMap<PathBuf' src/typed_ooda/ledger.rs
grep -n 'connection:\s*Arc<Mutex<Connection>>\|SharedConn' src/typed_ooda/ledger.rs
grep -n 'fn apply_pragmas\|fn with_busy_retry\|fn is_sqlite_busy' src/typed_ooda/ledger.rs
```

**Pre-fix (bug present)** you will instead see:

```
connection: Mutex<Connection>,          // per-handler, independent connections
connection: Mutex::new(connection),
```

If you see the pre-fix form, the burst is expected under load — the fix has not
landed on this build.

## Step 3 — Confirm the durability pragmas and retry

```bash
grep -n 'journal_mode.*WAL\|WAL' src/typed_ooda/ledger.rs
grep -n 'busy_timeout' src/typed_ooda/ledger.rs      # PRAGMA busy_timeout=5000 (== 5s)
grep -n 'foreign_keys' src/typed_ooda/ledger.rs      # foreign_keys = ON
```

Then confirm WAL is actually active on a live ledger:

```bash
sqlite3 "<state-root>/typed-ooda/outcomes.sqlite3" 'PRAGMA journal_mode;'
# expect: wal
```

A `-wal` / `-shm` sidecar file next to `outcomes.sqlite3` is the on-disk sign
WAL is in use.

## Step 4 — Localise a recurrence

If the burst still appears **after** the fix is in effect, it must come from
outside the in-process shared connection:

1. **An external process** (a stray CLI, a manual `sqlite3` write session, a
   backup tool holding a write lock) is touching the ledger. Check:
   ```bash
   fuser -v "<state-root>/typed-ooda/outcomes.sqlite3" 2>&1 || \
     lsof "<state-root>/typed-ooda/outcomes.sqlite3"
   ```
   Close the external writer; `with_busy_retry` should absorb brief overlaps.
2. **A bypassed `open` path** — some code constructed a raw `Connection` to the
   ledger instead of going through `CapabilityHandler::open`, escaping the
   registry. Search for it:
   ```bash
   grep -rn 'Connection::open' src/ | grep -i 'outcomes.sqlite3\|typed-ooda\|ledger'
   ```
   All ledger access must flow through `CapabilityHandler::open`.
3. **Disk / filesystem** — a network filesystem with weak locking (NFS) can
   break SQLite locking regardless of the registry. The ledger must live on a
   local filesystem.

## Step 5 — Run the concurrency regression test

The fix ships with a regression that reproduces the #4483 burst — many handlers,
one ledger path, concurrent writes, asserting zero `database is locked`
failures:

```bash
cargo test -p <crate> typed_ooda -- --nocapture 2>&1 | grep -iE 'lock|busy|persist'
# or target the specific test module for the ledger registry
cargo test --test typed_ooda_contracts 2>&1 | tail -20
```

Green means the shared connection, pragmas, and retry are holding. If you can
reproduce a burst in production but the test is green, you are almost certainly
in a Step 4 case (external writer or bypassed `open`).

## Escalation

If none of the above localises it, capture:

- the clustered log lines (Step 1) with goal/session IDs and timestamps,
- `PRAGMA journal_mode;` output and the presence/absence of `-wal`/`-shm`,
- `lsof` / `fuser` output for the ledger file,

and attach them to a new issue referencing #4483 and the
[connection registry reference](../reference/typed-outcome-ledger-connection-registry-api.md).
