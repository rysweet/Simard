---
title: Cognitive memory durability
description: Current durability, verified-backup, pruning, and restore contract for the library-backed cognitive memory store.
last_updated: 2026-07-03
owner: cognitive-memory
doc_type: reference
---

# Cognitive Memory Durability

> **Current backend after de-fork #2307.** Simard no longer has a native
> LadybugDB cognitive-memory fork. `NativeCognitiveMemory` and its methods were
> deleted. The only cognitive-memory backend is `LibraryCognitiveMemory`, an
> adapter over the external `amplihack-memory` library (crate version `0.4.0`,
> git rev `26d49bf864ac2c03b80c4ab075c4a907c51f82a8`, feature `persistent`).
> That library internally uses `lbug = "=0.17.1"`. Simard still has a direct
> `lbug = "=0.17.1"` dependency for the standalone `simard-tui` goal-board
> reader, not for cognitive memory.
>
> Operator-facing backup, verification, restore, and pruning are free functions
> in `src/memory_backup/mod.rs`: `backup_memory`, `backup_memory_verified`,
> `verify_backup`, `verify_backup_memory_count`, `ensure_backup_valid`,
> `list_backups`, `prune_old_backups`, and `restore_from_backup`. The OODA
> daemon entry point is `run_verified_backup` in
> `src/operator_commands_ooda/daemon/backup.rs`.
>
> Any `NativeCognitiveMemory` mention below is historical: it refers to the
> removed native fork and must not be used as a current API.

> Quick reference for contributors: see the
> [Cognitive Memory Durability section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#cognitive-memory-durability-per-write-barrier--sigterm--periodic-backups).

---

## Current architecture

Simard opens persistent cognitive memory through `LibraryCognitiveMemory::open`,
which delegates to the library-backed store at `state_root/cognitive`
(production: `~/.simard/cognitive`). Callers use the backend-agnostic
`CognitiveMemoryOps` trait.

```
+---------------------------+
|   OODA Daemon             |
|                           |
|  CognitiveMemoryOps       |
|        |                  |
|        v                  |
|  LibraryCognitiveMemory   |
|        |                  |
|        v                  |
|  amplihack-memory library |
+-------------+-------------+
              |
              v
+---------------------------+
| state_root/               |
|   cognitive/              |  live library GraphStore
|   backups/                |  memory_backup manifest directories
|   memory_records.json     |  file-backed memory records
+---------------------------+
```

The old native single-file store (`cognitive_memory.ladybug`) and its raw lbug
backup helpers were removed by issue #2307. Migration-aware code still recognizes
legacy names where needed, but current writes go through the library backend.

---

## Durability contract

Durability now has three layers:

1. **Library-backed persistence.** The `amplihack-memory` backend owns the
   persistent lbug store. Simard does not implement a separate per-write lbug
   barrier or expose any raw lbug write API for cognitive memory.
2. **Graceful shutdown checkpoint.** The OODA daemon handles
   `SIGTERM`/`SIGINT`/`SIGHUP`, persists the active goal board, then calls
   `CognitiveMemoryOps::checkpoint`. `LibraryCognitiveMemory::checkpoint`
   delegates to the library's `close()` path; source comments document this as
   the current checkpoint hook.
3. **Verified backups.** `run_verified_backup` snapshots the live cognitive
   store through the trait, verifies the resulting backup, and only then prunes
   old backups.

For `SIGKILL`, OOM-kill, or power loss, Simard does not add a current
`post_write_barrier`. Completed-write durability is the library backend's
responsibility; in-flight writes remain undefined. A committed write reaches the
backend's **write-through WAL**, so it survives a *process-level* non-graceful
exit (`SIGKILL`, OOM-kill, or a deploy restart) where the OS and its page cache
stay up and replay the WAL on the next open. Surviving **power loss or kernel
panic** additionally requires the backend to `fsync` the WAL to durable media
before acknowledging the commit — write-through alone does not assert that
barrier, and Simard does not itself enforce it. For those failure classes the
operator recovery point is the most recent verified backup under
`state_root/backups`.

### Historical incidents

- **2026-05-09 / issue #1631** — `systemctl restart simard-ooda` sent
  `SIGTERM`; the daemon did not run its shutdown sequence, so WAL-resident writes
  could be lost. The current signal handler and checkpoint sequence address that
  class.
- **2026-05-18 / issue #1973** — the removed native fork added a Simard-side
  per-write fsync barrier after a lost-write incident. That barrier was deleted
  with `NativeCognitiveMemory` in issue #2307; it is not a current API or current
  guarantee.
- **2026-07-04 / issue #2550** — a corrupt WAL
  (`wal/wal_record.cpp:76 UNREACHABLE_CODE`) triggered a resilient prefix-recovery
  that salvaged 40,488 records, but the **checkpoint after recovery failed**
  (`Cannot open … cognitive.wal(.checkpoint)`), so nothing durable landed; a later
  open re-quarantined and **reset the store to empty**, dropping ~20,480 memories
  to 128, with no restore path. The #2550 change locks durable prefix-recovery
  with a boundary regression test (the pinned library already recovers without
  resetting a store that salvaged records), adds `simard memory import` + startup
  auto-restore, completes the snapshot to all durable memory types, and preserves
  recovery assets. See the
  [Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md).

---

## Shutdown sequence

`shutdown_daemon` in `src/operator_commands_ooda/daemon/mod.rs` performs the
current graceful shutdown sequence:

| Step | Operation | Failure mode when signal-driven |
|---|---|---|
| 1 | `persist_board(&state.active_goals, &*clients.memory)` | Logged, next step still runs. |
| 2 | `shared_mem.checkpoint()` | Logged, next step still runs. |
| 3 | `clients.session.close()` | Logged, next step still runs. |
| 4 | `memory_ipc::clear_in_process_writer()` | Cannot fail. |
| 5 | Clients and the daemon-owned `Arc<dyn CognitiveMemoryOps>` drop on return. | Drop-time failures are logged by the backend. |

Normal-exit callers receive errors. Signal-driven shutdown logs errors and keeps
progressing because the process is already exiting.

---

## Periodic verified backups

The current backup loop is in `src/operator_commands_ooda/daemon/backup.rs`.
`backup_interval_secs_from_env` reads `SIMARD_BACKUP_INTERVAL_SECS` and defaults
to `86_400` seconds (one day). `last_backup: Option<Instant>` starts as `None`,
so the first daemon cycle always runs a backup.

`run_verified_backup(client, state_root)` does three things:

1. Builds a `BackupConfig` rooted at `state_root/backups` and opens the
   file-backed memory store at `state_root/memory_records.json`.
2. Calls `memory_backup::backup_memory_verified`, which composes
   `backup_memory` with `ensure_backup_valid` / `verify_backup`.
3. Calls `memory_backup::prune_old_backups` only after the new backup verifies.

A failed backup logs:

```text
[simard] WARN: verified backup FAILED, prune skipped: <error>
```

A successful backup logs counts and the backup directory:

```text
[simard] verified backup OK: <facts> facts + <procedures> procedures + <records> records -> <dir>
```

### Backup layout

Backups are timestamped directories, newest-first by directory name:

```text
state_root/backups/
  20260703_003510/
    manifest.json
    cognitive_snapshot.json
    memory_records.json
```

`BackupManifest` records the backup directory, created time, counts, data-file
paths, and checksum. Verification derives file paths from the backup directory
and fixed filenames; it does not trust manifest paths as read targets.

### Retention

`BackupConfig` controls retention:

```rust
pub struct BackupConfig {
    pub backup_dir: PathBuf,
    pub retention_days: u32,
    pub min_backups_to_keep: usize,
}
```

`BackupConfig::default()` keeps at least 3 backups and prunes directories older
than 30 days. The daemon overrides only `backup_dir` to `state_root/backups`.

---

## Restore procedure

Restore is an API operation, not a raw `cp` of lbug files. Use
`memory_backup::restore_from_backup` after verifying the selected directory:

```rust
use simard::memory_backup::{ensure_backup_valid, restore_from_backup};

ensure_backup_valid(&backup_dir)?;
let restored = restore_from_backup(client, file_store, &backup_dir)?;
```

`restore_from_backup` rejects `Corrupted` and `Incomplete` backups before it
imports the cognitive snapshot and file-backed records. If a backup fails
verification, choose the next-most-recent directory from `list_backups`.

### Session-boundary snapshot recovery

Separate from the `memory_backup/` verified backups above, a session teardown
writes a JSON snapshot of cognitive memory to `~/.simard/snapshots/`
(`memory_snapshot::save_session_snapshot`) through the crash-safe writer
(temp + fsync + rename + parent fsync, issue #1918) and prunes to the 10 most
recent (`prune_snapshots`). The writer is atomic, so a crash *during* a save
leaves only an unrenamed temp file — never a corrupt final snapshot.

On reload, `memory_snapshot::load_latest_snapshot` walks the retained snapshots
**newest → oldest** and returns the first one that loads. If the newest snapshot
is unreadable — a partial write from a binary that predates the crash-safe
writer, on-disk corruption, or a payload the current schema can no longer parse
— the loader transparently degrades to the most recent snapshot that *does*
load rather than returning "no memory" and discarding the entire retained
history. Only when **every** snapshot in the directory fails to load does it
report `None`. Skipped files and the all-unreadable case are logged to stderr
with the `[simard] snapshot:` prefix:

```text
[simard] snapshot: failed to load .../agent-1750000200.json (1 of 10): ...; trying an older snapshot
[simard] snapshot: recovered older snapshot .../agent-1750000100.json after skipping 1 newer unreadable snapshot(s)
```

This is the same graceful-degradation contract as `restore_from_backup` above
(fall back to the next-most-recent good copy): a single bad snapshot at the tip
of the retained history must never silently wipe durable recall across a
restart.
### Operator restore and self-healing (issue #2550)

Two operator-facing paths built on the same snapshot format make a
corruption-reset recoverable without a code harness:

- **`simard memory import <snapshot.json>`** ingests a backup's
  `cognitive_snapshot.json` back into the store, deduplicating by content so it
  is idempotent and safe to re-run or run onto a partially-populated store. See
  the [Memory introspection CLI](../reference/simard-memory-cli.md#simard-memory-import).
- **Startup auto-restore.** When the daemon starts and finds the live store
  empty **and** a newer non-empty snapshot on disk, it restores from
  the newest good snapshot and logs a `store was empty — auto-restored <n>
  memories from … cognitive_snapshot.json` line, so a corruption-reset self-heals
  instead of losing everything.

Both paths **preserve prospective status** across the restore (issue #2562): a
trigger that was already `triggered` or `resolved` when the snapshot was taken is
restored to a terminal, non-firing state rather than reset to `pending`, so an
auto-restore or `simard memory import` can never re-fire a goal the daemon had
already completed. Genuinely `pending` triggers restore as `pending` and stay
eligible to fire.

For a corruption incident (recover the pre-reset store, choose a snapshot, and
run the import), follow the
[Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md).

---

## Current public API

### `src/memory_backup/mod.rs`

```rust
pub fn backup_memory(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<BackupManifest>;

pub fn backup_memory_verified(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<BackupManifest>;

pub fn verify_backup(backup_dir: &Path) -> SimardResult<BackupVerification>;
pub fn verify_backup_memory_count(backup_dir: &Path, expected_total: usize) -> SimardResult<()>;
pub fn ensure_backup_valid(backup_dir: &Path) -> SimardResult<BackupManifest>;
pub fn list_backups(config: &BackupConfig) -> SimardResult<Vec<BackupVerification>>;
pub fn prune_old_backups(config: &BackupConfig) -> SimardResult<usize>;
pub fn restore_from_backup(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    backup_dir: &Path,
) -> SimardResult<usize>;
```

Public types:

```rust
pub struct BackupConfig;
pub struct BackupManifest;
pub struct BackupVerification;
pub enum BackupStatus;
```

### Daemon entry point

```rust
// src/operator_commands_ooda/daemon/backup.rs
pub fn run_verified_backup(
    client: &dyn CognitiveMemoryOps,
    state_root: &Path,
) -> SimardResult<BackupManifest>;
```

### Shutdown checkpoint

```rust
// src/cognitive_memory/mod.rs
pub trait CognitiveMemoryOps {
    fn checkpoint(&self) -> SimardResult<()> { Ok(()) }
}

// src/cognitive_memory/library_adapter.rs
impl CognitiveMemoryOps for LibraryCognitiveMemory {
    fn checkpoint(&self) -> SimardResult<()> {
        self.lock()?.close();
        Ok(())
    }
}
```

---

## Historical native-fork APIs removed by issue #2307

The following symbols belonged to the deleted native fork. They are retained here
only as migration history and must not be cited as current API:

| Removed native-fork symbol | Current replacement / status |
|---|---|
| `NativeCognitiveMemory::create_verified_backup` | `memory_backup::backup_memory_verified` |
| `NativeCognitiveMemory::prune_old_backups` | `memory_backup::prune_old_backups` |
| `NativeCognitiveMemory::open` | `LibraryCognitiveMemory::open` |
| `NativeCognitiveMemory::open_or_recover` | Deleted native recovery path; use verified backup APIs. |
| `NativeCognitiveMemory::in_memory` | `LibraryCognitiveMemory::in_memory` |
| `NativeCognitiveMemory::post_write_barrier` | Deleted native durability internal; no current Simard-side equivalent. |
| `NativeCognitiveMemory::assert_hermetic_for` | Deleted native test helper; current tests use `HermeticState`, serial isolation, launcher guards, and the adapter's `cfg(test)` lock-write guard. |

The removed fork also had single-file backups named
`cognitive_memory.ladybug.<epoch>` plus possible WAL siblings. Current verified
backups are manifest directories under `state_root/backups`.

---

## Verification coverage

Source-level coverage for the current API includes:

| Test | Location | Purpose |
|---|---|---|
| `backup_memory_verified_returns_valid_manifest` | `src/memory_backup/tests.rs` | Verified backup returns a valid manifest. |
| `backup_memory_verified_round_trips_count` | `src/memory_backup/tests.rs` | Snapshot counts round-trip. |
| `verify_backup_count_passes_on_exact_total` / `verify_backup_count_fails_loudly_on_mismatch` | `src/memory_backup/tests.rs` | Count verification accepts exact totals and rejects mismatches. |
| `restore_from_valid_backup` / `restore_rejects_corrupted_backup` | `src/memory_backup/tests.rs` | Restore accepts valid backups and rejects corrupted ones. |
| `prune_old_backups_respects_min_keep` | `src/memory_backup/tests.rs` | Retention preserves the configured minimum. |
| `run_verified_backup_produces_valid_backup` | `src/operator_commands_ooda/daemon/backup.rs` | Daemon entry point produces a valid backup under `state_root/backups`. |
| `first_backup_is_always_due` | `src/operator_commands_ooda/daemon/backup.rs` | First daemon cycle runs a backup. |

---

## Operational runbook

### Graceful restart did not log shutdown completion

Check for the shutdown banner and systemd escalation:

```bash
journalctl -u simard-ooda --since "10 min ago"   | grep -E "shutdown sequence start|shutdown complete|SIGKILL|killed"
```

If systemd escalated to `SIGKILL`, increase `TimeoutStopSec` so the daemon can
observe the shutdown flag and call `shutdown_daemon`.

### Verified backup failed

Look for the current failure line:

```bash
journalctl -u simard-ooda --since "1 hour ago"   | grep 'verified backup FAILED'
```

Fix the underlying filesystem, permissions, serialization, or integrity error.
The daemon skips pruning on failure so the last-known-good backup remains.

### Backup directory is growing

The current pruner removes backup directories older than
`BackupConfig.retention_days` while keeping at least
`BackupConfig.min_backups_to_keep`. If directories are not pruned, verify their
manifest timestamps and the configured retention values. There is no current
`SIMARD_DB_BACKUP_KEEP` knob; that belonged to the removed native backup loop.

### Temporarily reduce backup frequency

Set a longer interval and restart the daemon:

```bash
sudo systemctl edit simard-ooda
# Add or merge:
[Service]
Environment=SIMARD_BACKUP_INTERVAL_SECS=86400
sudo systemctl restart simard-ooda
```

`0` is not a disable switch; the parser treats zero as invalid and falls back to
the default. To stop backups entirely, stop the daemon.

### Historical `cognitive_memory.ladybug` artifacts

Files such as `cognitive_memory.ladybug`, `cognitive_memory.ladybug.<epoch>`, or
`cognitive_memory.ladybug.kuzu-backup` are artifacts from pre-#2307 native-fork
storage and backup paths. They are not the current live cognitive-memory store.
Review them only for migration forensics; current live data is under
`state_root/cognitive`, and current backups are manifest directories.

---

## See Also

- [`docs/memory.md`](../memory.md) — cognitive-memory data model
- [`docs/daemon-mode.md`](../daemon-mode.md) — OODA daemon overview
- [Creative Ideas durable read-after-write](../reference/creative-ideas-durable-read-after-write.md) — the dashboard read-after-write (state-root resolver) fix; persisted creative ideas are durable across a non-graceful restart via the engine's write-through WAL (#2798)
- [Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md) — corrupt-WAL recovery, `memory import`, startup auto-restore
- [Verified Backups of the Live Cognitive Store](verified-backups.md) — verify-before-prune, whole-store export, bounded quarantines
- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — contributor durability notes
- [GitHub issue #2307](https://github.com/rysweet/Simard/issues/2307) — native cognitive-memory fork deletion
- [GitHub issue #2420](https://github.com/rysweet/Simard/issues/2420) — verified backup of the live library store
- [GitHub issue #2550](https://github.com/rysweet/Simard/issues/2550) — durable WAL recovery + snapshot import/auto-restore
- [GitHub issue #1631](https://github.com/rysweet/Simard/issues/1631) — SIGTERM-safe shutdown
- [GitHub issue #1973](https://github.com/rysweet/Simard/issues/1973) — historical native per-write barrier
