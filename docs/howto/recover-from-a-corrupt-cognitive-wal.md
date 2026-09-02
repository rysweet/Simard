---
title: How to recover from a corrupt cognitive WAL
description: "Operator how-to for the cognitive-memory write-ahead-log crash-consistency fix (#4687) — confirm the single-owner / clean-shutdown-replay contract holds, read the WAL-recovery counter, alert on unexpected recoveries, and act on a genuine crash-provenance salvage without silent memory loss."
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: how-to
status: active
related:
  - ../reference/cognitive-memory-wal-crash-consistency.md
  - ../reference/cognitive-memory-open-serialization.md
  - ./browse-the-simard-journal.md
---

# How to recover from a corrupt cognitive WAL

This guide is for operators of the `simard-ooda` daemon. It explains how the
cognitive store's write-ahead log (WAL) behaves after the crash-consistency fix
(**#4687**), how to confirm a healthy restart, and what to do when recovery does
fire.

Background reference:
[Cognitive-memory WAL crash-consistency (#4687)](../reference/cognitive-memory-wal-crash-consistency.md).

The store lives at `state_root/cognitive` with its WAL at
`state_root/cognitive.wal`. In the default deployment `state_root` is
`$HOME/.simard`, so the commands below use `~/.simard/…`.

## What "healthy" looks like

Before #4687 the daemon logged a checksum failure and a failed checkpoint rename
on **every** start. After the fix, a normal restart is **silent** on the WAL
path. To confirm:

```bash
journalctl --user -u simard-ooda --since "1 hour ago" \
  | grep -i "lbug_store\|WAL"
```

A healthy daemon shows **no** `Checksum verification failed`, **no**
`recovered from corrupt WAL`, and **no** `No such file or directory` rename
error.

## Why a clean restart is now safe

The fix makes the wrapper the **single** checkpoint owner (the `lbug` engine's
own background auto-checkpoint is disabled on the read-write path), and it makes
the clean-shutdown checkpoint **fsync-durable** (the WAL is folded into the main
DB and both the file and its parent directory are fsync'd before exit).

The practical consequence: a **clean** shutdown leaves no WAL tail to replay, so
the next open takes the fast strict-open path and reports
`WalRecoveryOutcome::Clean` — it never enters the recovery ladder, so it cannot
emit a checksum failure. A recovery after a clean restart is therefore a
regression, not normal behaviour.

## Step 1 — Read the WAL-recovery counter

The corrupt-WAL recovery paths emit an `error!`-level event carrying the OTel
counter `cognitive_memory_wal_recovery_total`. It is exported on the OTel path
and readable in-process via
`amplihack_memory::graph::wal_recovery_event_count()`.

| Signal | Healthy value | Meaning if rising |
|--------|---------------|-------------------|
| `cognitive_memory_wal_recovery_total` | **flat across restarts** | A store had to recover a corrupt WAL. After #4687 this should only follow a genuine crash — never a clean restart. |
| Simard `cognitive_memory_silent_drop_count(kind, site)` on a WAL site during a **clean** reopen | **0** | A clean replay must lose nothing; a non-zero value is a hard regression. |

Recommended dashboard alert:

```promql
increase(cognitive_memory_wal_recovery_total[1h]) > 0
```

Treat any increase that coincides with a **clean** stop/start as a hard integrity
alert (see [Step 3a](#step-3a--if-a-recovery-fired-after-a-clean-restart-hard-alert)).

## Step 2 — Distinguish clean vs. crash provenance

There is no sidecar marker to inspect; provenance is inferred from **how the
daemon last stopped**:

- **Clean stop** (`systemctl --user stop simard-ooda`, graceful shutdown) → the
  store was fsync-checkpointed on `Drop`. A recovery here is unexpected.
- **Unclean stop** (OOM kill, power loss, `SIGKILL`) → the un-checkpointed WAL
  tail may be torn. A recovery here is expected and correct.

Correlate the recovery event's timestamp with the preceding shutdown in the
journal:

```bash
journalctl --user -u simard-ooda --since "6 hours ago" \
  | grep -i "Stopped\|Stopping\|Killed\|lbug_store\|WAL"
```

## Step 3a — If a recovery fired after a clean restart (hard alert)

This is the case the fix is designed to make **impossible under normal
operation**. If you see `cognitive_memory_wal_recovery_total` increase across a
clean stop/start:

1. **Do not restart in a loop.** The event is logged at `error!` level and
   metered — committed, checkpointed writes are preserved in the main DB and are
   not being dropped silently. Only an un-checkpointed tail is ever quarantined.
2. Capture the evidence:
   ```bash
   journalctl --user -u simard-ooda --since "6 hours ago" \
     | grep -i "lbug_store\|WAL\|checksum\|rename"
   ls -l ~/.simard/cognitive*        # store, .wal, any .corrupt-* quarantine
   ```
3. The recovery quarantines the corrupt WAL to `~/.simard/cognitive.wal.corrupt-<ts>`
   (moved aside, never deleted). Preserve it for diagnosis.
4. Restore from the most recent snapshot rather than accepting a truncated store
   — see the snapshot/restore path documented for issue #2550
   ([memory architecture](../memory.md)).
5. File an issue: a clean shutdown that still needed recovery points at disk
   corruption underneath the store or a durability regression.

## Step 3b — If a recovery ran after an unclean stop (expected)

After an OOM kill, power loss, or `SIGKILL`, a corrupt WAL tail is expected and
the #2550 salvage path is correct behaviour:

- `cognitive_memory_wal_recovery_total` increments.
- The log shows an `error!`-level line naming the recovered record count and the
  quarantined WAL path — this is now **observable**, not silent.
- The good prefix is salvaged and the daemon comes up; the recovery is durable
  across a later strict reopen (the store is never reset to empty).

No action is required beyond noting the event. If it recurs across *clean*
restarts, escalate as in Step 3a.

## Step 4 — Verify a clean restart end-to-end

To confirm the durability contract on your host:

```bash
# 1. Stop the daemon cleanly (runs the fsync-durable drop checkpoint).
systemctl --user stop simard-ooda

# 2. Inspect the store; after a clean stop the WAL is folded/consumed.
ls -l ~/.simard/cognitive ~/.simard/cognitive.wal 2>&1

# 3. Start again — this must NOT log any checksum/rename error.
systemctl --user start simard-ooda
journalctl --user -u simard-ooda --since "2 minutes ago" \
  | grep -i "WAL\|checksum\|rename" || echo "clean restart: no WAL faults"
```

A clean stop/start cycle that logs nothing on the WAL path — and leaves
`cognitive_memory_wal_recovery_total` unchanged — confirms the fix is active and
the store is durable.

## Watch: WAL growth on write-heavy hosts

Because the engine's own auto-checkpoint is now disabled (single-owner
checkpointing), the **wrapper** cadence alone bounds WAL growth — a checkpoint
every 128 writes plus on shutdown. On a very write-heavy daemon, keep an eye on:

```bash
ls -lh ~/.simard/cognitive.wal
```

If it grows unbounded between checkpoints, lower the wrapper checkpoint cadence
(see [Configuration](../reference/cognitive-memory-wal-crash-consistency.md#configuration)).

## See also

- [Cognitive-memory WAL crash-consistency (#4687)](../reference/cognitive-memory-wal-crash-consistency.md)
- [Cognitive-memory open serialization (lock-contention safety net)](../reference/cognitive-memory-open-serialization.md)
- [Browse the Simard journal](./browse-the-simard-journal.md)
