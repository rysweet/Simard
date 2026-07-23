---
title: Diagnose the typed-OODA "database is locked" crash-loop
description: Runbook for the systemic `typed outcome persistence failed: database is locked` signature (#4483) — confirm the WAL + writer-lock fix is live, read the journal-mode evidence, and interpret a residual lock warning.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/typed-ooda-persistence-concurrency-api.md
  - ../reference/ooda-capability-api.md
  - ./unblock-stuck-ooda-goals.md
  - ./run-ooda-daemon.md
---

# Diagnose the typed-OODA "database is locked" crash-loop

Use this runbook when the OODA daemon logs a synchronized burst of:

```text
typed goal-session cycle failed (ToolFailed): typed outcome persistence failed: database is locked
typed OODA outbox startup recovery incomplete: typed outcome persistence failed: database is locked
```

That signature is issue **#4483**: multiple per-goal cycle writers and the
startup outbox-recovery pass contend for one SQLite ledger that is not in WAL
mode, exhaust the busy timeout, and fail several goals' cycles at once.

The fix ships in the [typed-OODA persistence concurrency API](../reference/typed-ooda-persistence-concurrency-api.md):
WAL + `synchronous=NORMAL` + `busy_timeout` applied on every connection, plus a
process-wide per-file writer lock. This runbook confirms the fix is live and
interprets anything that still looks like a lock.

## 1. Confirm the burst signature

Pull the recent daemon journal:

```bash
journalctl --user -u simard-ooda --since "-6h" --no-pager \
  | grep -E "database is locked|outbox startup recovery incomplete"
```

The #4483 signature is a **synchronized burst** — the same
`database is locked` message across several distinct goals within the same
second — not one isolated slow write. If you see that shape on a build from
before the fix, upgrade the daemon (below). If you see it on a build that
includes the fix, jump to [Step 4](#4-interpret-a-residual-lock).

## 2. Verify the running build includes the fix

The fix is transparent — no new flag — so confirm it by the ledger's journal
mode rather than by config. First locate the ledger:

```bash
ls -l "$SIMARD_STATE_ROOT/typed_ooda/"
# ledger.db        <- the outcome ledger
# ledger.db-wal    <- present only when WAL is active
# ledger.db-shm
```

The presence of `ledger.db-wal` alongside `ledger.db` is the at-a-glance
indicator that WAL is active on the live database.

## 3. Read the journal-mode evidence

Query the live ledger read-only:

```bash
sqlite3 "$SIMARD_STATE_ROOT/typed_ooda/ledger.db" 'PRAGMA journal_mode;'
```

Expected output on a fixed daemon:

```text
wal
```

- `journal_mode = wal` → readers and one writer proceed concurrently; no
  whole-file exclusive write lock on the common path. This value is stored in
  the database file, so an external `sqlite3` reading is authoritative.

If you instead see `journal_mode = delete` (or `truncate`), the running daemon
predates the fix. Rebuild and redeploy from a source tree that includes issue
#4483, then re-run this step. The first write after upgrade promotes the
database to WAL in place; no migration or manual conversion is required.

!!! note "Why not check `PRAGMA synchronous` here?"
    `synchronous` is a **per-connection** setting, not stored in the database
    file. The `sqlite3` CLI opens its own connection and always reports its own
    default (`2`/FULL) — never the daemon's `NORMAL` — so it is useless as an
    external "is the fix live?" signal. Use `journal_mode` (file-persistent) as
    the sole external tell; `synchronous=NORMAL` is verified by the in-process
    regression test instead.

## 4. Interpret a residual lock

After the fix, a `database is locked` line should not appear under normal
in-process contention. If one still does, distinguish the two remaining causes:

| Observation | Meaning | Action |
| --- | --- | --- |
| `typed-ooda writer-lock path canonicalization failed; using raw path` (`tracing::warn!`) | The ledger path could not be canonicalized (e.g., removed mid-run); the writer fell back to the raw path, so two handles *might* not share one lock. | Confirm `$SIMARD_STATE_ROOT/typed_ooda/` is stable and not being deleted/relinked while the daemon runs. |
| `database is locked` with **two `simard-ooda` processes** on the same state root | Cross-process contention — out of scope for the in-process writer lock. | Ensure only one daemon owns a state root. See [Run the OODA Daemon](./run-ooda-daemon.md). |
| `PersistenceFailed` that is **not** `database is locked` | A real SQL/serialization fault, surfaced fail-visible (never swallowed). | Read the full `tracing` span; this is a genuine persistence error, not contention. |

The daemon emits these diagnostics through structured `tracing` /
OpenTelemetry. Read them with:

```bash
journalctl --user -u simard-ooda --since "-1h" --no-pager \
  | grep -E "typed-ooda|writer-lock|PersistenceFailed"
```

## 5. Clear goals stranded by the pre-fix burst

Goals whose cycles failed during a pre-fix burst are not corrupted — the writes
simply did not commit. Once the daemon is on the fixed build, those goals
resume on their next cycle. If any goal was parked as blocked by a downstream
safeguard during the incident, clear it with the standard runbook:
[Unblock Stuck OODA Goals](./unblock-stuck-ooda-goals.md).

## What you should *not* need to do

- **Do not** raise a timeout or add a config flag — there is none to tune; WAL
  plus the writer lock remove the contention a longer timeout would only mask.
- **Do not** delete or rebuild `ledger.db` — the fix is additive
  (`SCHEMA_VERSION` unchanged) and the existing database is promoted to WAL in
  place.
- **Do not** manually delete `ledger.db-wal` / `ledger.db-shm` while the daemon
  runs — SQLite manages those sidecars.

## Related

- [Typed-OODA persistence concurrency API](../reference/typed-ooda-persistence-concurrency-api.md)
  — the WAL + writer-lock contract this runbook verifies.
- [OODA capability API](../reference/ooda-capability-api.md) — the
  `PersistenceFailed` error contract.
- [Run the OODA Daemon](./run-ooda-daemon.md) — single-owner state-root setup.
