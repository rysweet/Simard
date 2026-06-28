---
title: Backup pruning API
description: Reference for retention-limited cleanup of cognitive memory backup files. The native epoch-based pruner was removed in de-fork Phase 2b; the surviving pruner is the date-based memory_backup/ module.
last_updated: 2026-06-27
owner: simard
doc_type: reference
related:
  - ../operations/cognitive-memory-durability.md
  - ../reference/disk-health-api.md
  - ../howto/configure-disk-health-check.md
  - ../concepts/automated-disk-health.md
---

# Backup pruning API

> **De-fork Phase 2b.** The epoch-based pruner documented here lived in the
> native `src/cognitive_memory/backup.rs` module, which was **deleted** along
> with `NativeCognitiveMemory`. The library backend owns its own durability and
> emits no `cognitive_memory.ladybug.<epoch>` files, so there is nothing for this
> pruner to clean. The **surviving** backup pruner is the date-based
> `prune_old_backups()` in `src/memory_backup/mod.rs` (using `BackupConfig`),
> which operates through the `CognitiveMemoryOps` trait on file-level snapshots.
> The remainder of this page is archival: it documents the removed native API.

---

## Current implementation (issue #2420)

The OODA daemon now takes a **scheduled, verified, logical backup of the LIVE
cognitive store** every `SIMARD_BACKUP_INTERVAL_SECS` (default `3600`). The pass
lives in [`src/memory_backup/mod.rs`](../../src/memory_backup/mod.rs) and is
wired into the daemon loop next to the disk-health and worktree-sweep timers.

### `memory_backup::run_scheduled_backup`

```rust
pub fn run_scheduled_backup(
    bridge: &dyn CognitiveMemoryOps,
    state_root: &Path,
    agent_name: &str,
) -> SimardResult<ScheduledBackupOutcome>
```

One pass:

1. **Snapshot the live store** through the same `bridge` (`shared_mem`) the daemon
   writes to — never a stale on-disk path such as the pre-migration
   `cognitive_memory.ladybug`. This closes issue #2420 gap #1 (backups had been
   targeting a Jun-20 stale path while the live store grew daily).
2. **Verify** the fresh backup opens and its facts/procedures/records counts
   match the manifest.
3. **Only if the fresh backup verified clean**, prune old verified backups
   (`prune_old_backups`) and bound the corrupt/shadow quarantine artifacts
   (`prune_corrupt_artifacts`). Pruning is gated on a good fresh backup so a bad
   write can never delete the prior good copy.

`ScheduledBackupOutcome::summary()` produces the one-line daemon log entry,
which reports both the **captured** counts and the **total** live-memory count.

> **What the backup captures (scope).** The snapshot is a logical backup of the
> **durable** cognitive subset — semantic *facts* and *procedures*. The derived
> or transient categories (sensory, working, episodic, prospective) are **not**
> snapshotted; episodes are continuously distilled into facts, and the others
> are short-lived. So "restore round-trips the current memory count" means the
> durable facts + procedures, not the raw episodic event count. To keep this
> honest, every `BackupManifest` records the full per-category
> [`CognitiveStatistics`](cognitive-memory-durability.md) (`store_statistics`)
> alongside the captured counts, so the gap is always visible in the manifest
> and the daemon log.

### `remote_transfer::export_full_memory_snapshot`

```rust
pub fn export_full_memory_snapshot(
    bridge: &dyn CognitiveMemoryOps,
    agent_name: &str,
) -> SimardResult<MemorySnapshot>
```

Backups use this **uncapped** export instead of the replication
`export_memory_snapshot` (which truncates at `MAX_EXPORT_FACTS = 1000`). On the
live host the store held 1237 facts, so the capped path silently dropped 237 —
a backup that quietly loses data. The full export captures the durable subset
(semantic facts + procedures) in full; episodic/working/sensory memory remain
out of scope (derived/transient).

### `memory_backup::prune_corrupt_artifacts`

```rust
pub fn prune_corrupt_artifacts(state_root: &Path, keep: usize) -> SimardResult<usize>
```

Bounds the corrupt quarantine artifacts the library's corrupt-WAL recovery
leaves under `state_root`. It matches **only** the library's definitive
`.corrupt-<timestamp>` marker — `*.corrupt-*` and the concatenated rename
chains (`cognitive.wal.corrupt-…cognitive.corrupt-…cognitive.shadow`, which
still contain the marker; issue #2420 gap #2). It keeps the newest `keep`
(default `CORRUPT_ARTIFACTS_KEEP = 5`) for forensics and removes the rest
(files **and** directories).

The live store file `cognitive` and its active sidecars — including lbug's
shadow-paging file `cognitive.shadow` (`SHADOWING_SUFFIX = "shadow"`) and WAL
`cognitive.wal` — are **never** eligible. A *bare* `*.shadow` (no `.corrupt-`
marker) is deliberately left alone so an active shadow file can never be
deleted by a backup pass.

> **Scope of this PR vs. the library.** This function *bounds* the accumulated
> quarantine artifacts (caps their count). The root cause of the *concatenated
> rename* (issue #2420 gap #2) lives in the LadybugDB engine and is the
> library's responsibility; the lbug `0.15.3 → 0.17.1` bump below moves the
> engine forward but Simard does not itself rewrite the quarantine-rename path.

### Engine pin (lbug 0.17.1)

`amplihack-memory` is pinned to `26d49bf8` and Simard's direct `lbug` pin moves
`0.15.3 → 0.17.1` to match. The bumped library carries the LadybugDB engine
migration to `lbug 0.17.1` (the v40→v41 on-disk format the live store already
uses) and the empty-read data-loss fix that targets the recurring main-store
corruption. The previous engine (lbug 0.15.x) was *behind* the deployed on-disk
format — the version skew was itself a corruption vector. Pinning Simard's
direct `lbug` to the same `0.17.1` keeps a single LadybugDB build (no
duplicate-symbol clash from compiling the C++ engine twice).

---

## Archival: removed native pruner

> **De-fork Phase 2b.** The epoch-based pruner documented below lived in the
> native `src/cognitive_memory/backup.rs` module, which was **deleted**.

**Module (removed in Phase 2b):** `src/cognitive_memory/backup.rs`

The `NativeCognitiveMemory::prune_old_backups()` method enforced a
retention limit on cognitive memory backup files, preventing unbounded
disk growth in the `backups/` subdirectory of the state root.

> **Surviving implementation:** the date-based `prune_old_backups()` in
> `src/memory_backup/mod.rs` (using `BackupConfig`) remains the cognitive-memory
> backup pruner after Phase 2b.

---

## Background

Simard creates epoch-timestamped backup files (e.g.,
`cognitive_memory.ladybug.1718164068`) during cognitive memory
operations. Over long-running sessions, these accumulate without bound.
Before issue [#2270](https://github.com/rysweet/Simard/issues/2270),
there was no automatic cleanup — operators had to manually prune old
backups or rely on the disk health recipe to reclaim space reactively.

---

## Public API

### `NativeCognitiveMemory::prune_old_backups()`

```rust
pub fn prune_old_backups(state_root: &Path, keep: usize) -> PruneOutcome
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `state_root` | `&Path` | Root directory (e.g., `~/.simard/`); backups live in `{state_root}/backups/` |
| `keep` | `usize` | Number of newest backups to retain |

**Return type:**

```rust
pub struct PruneOutcome {
    pub removed: usize,
    pub failed: Vec<(PathBuf, std::io::Error)>,
}
```

The caller can inspect `failed` to log or alert on partial failures
(see daemon integration below).

**Behavior:**

1. Resolves the backup directory: `{state_root}/backups/`
2. If the directory does not exist, returns immediately (`removed: 0`)
3. Lists all files matching the prefix `cognitive_memory.ladybug.`
4. Parses the suffix as a `u64` epoch timestamp; non-parseable files
   are skipped (not counted, not deleted)
5. Sorts by epoch descending (newest first)
6. Deletes all main backup files beyond index `keep`
7. For each deleted main file, also removes paired WAL files with the
   same epoch suffix (`cognitive_memory.ladybug.wal.{epoch}` and
   `cognitive_memory.wal.{epoch}`)

**Error handling:**

- Directory not found → early return, no error
- Individual file deletion failure → logged to stderr, added to
  `failed` list, continues with remaining files (non-fatal)
- WAL file deletion failure → same treatment as main file failures

All errors are non-fatal. Backup pruning must never crash or block the
OODA cycle.

**Retention limit:** The `keep` parameter is configurable by the
caller. The daemon call site (`operator_commands_ooda/daemon/mod.rs`)
passes `db_backup_keep` from configuration. The design spec for
issue #2270 specifies passing `20` from the OODA cycle call site.

---

## Integration points

### OODA daemon

The daemon calls `prune_old_backups` after each successful backup
creation:

```rust
let prune_outcome =
    NativeCognitiveMemory::prune_old_backups(&state_root, db_backup_keep);
if !prune_outcome.failed.is_empty() {
    daemon_log(&state_root, &format!(
        "[simard] WARN: prune_old_backups: {} removed, {} failed",
        prune_outcome.removed, prune_outcome.failed.len()
    ));
}
```

### OODA cycle (new call site for #2270)

The design spec adds a call in `src/ooda_loop/cycle.rs` after
`handle_cleanup()`, passing a retention limit of 20:

```rust
handle_cleanup(&state, &bridge).await?;
NativeCognitiveMemory::prune_old_backups(&state_root, 20);
```

---

## Filename format and sorting rationale

Backup files use **epoch-based** names:

```
cognitive_memory.ladybug.1718164068
cognitive_memory.ladybug.1718250468
```

The suffix is a `u64` Unix epoch timestamp. The function parses this
integer and sorts numerically (descending), which produces correct
chronological ordering regardless of zero-padding or string length.

Paired WAL backup files follow the same epoch convention:

```
cognitive_memory.ladybug.wal.1718164068
cognitive_memory.wal.1718164068
```

Both WAL variants are checked and removed alongside the main file.

---

## Operator notes

- **Manual override:** Operators can delete files from the backups
  directory at any time. The pruning function only removes files when
  there are more than `keep` — manual cleanup does not conflict.

- **Monitoring:** The daemon logs pruning failures. For verbose output,
  check stderr for `[simard] failed to remove old backup` messages.

- **Disk health interaction:** The disk health recipe
  ([Disk health API](./disk-health-api.md)) performs broader cleanup
  (stale worktrees, cargo target dirs). Backup pruning is narrowly
  scoped to `.ladybug` backup files and runs after each backup
  creation, while disk health runs on a configurable schedule.

---

## Code location

| Item | File | Line |
|------|------|------|
| `prune_old_backups()` definition | `src/cognitive_memory/backup.rs` | ~709 |
| `PruneOutcome` struct | `src/cognitive_memory/metrics.rs` | ~49 |
| Daemon call site | `src/operator_commands_ooda/daemon/mod.rs` | ~486 |
| Cycle call site (new, #2270) | `src/ooda_loop/cycle.rs` | ~160 |

---

## Related

- [Cognitive memory durability (operations)](../operations/cognitive-memory-durability.md)
  — backup creation and restore procedures.
- [Disk health API](./disk-health-api.md) — broader disk cleanup via
  recipe.
- [Automated disk health (concept)](../concepts/automated-disk-health.md)
  — design rationale for automated cleanup.
- [Configure disk health check](../howto/configure-disk-health-check.md)
  — operator guide for the disk health recipe.
