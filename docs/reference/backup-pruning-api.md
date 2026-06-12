---
title: Backup pruning API
description: Reference for NativeCognitiveMemory::prune_old_backups() — automatic retention-limited cleanup of cognitive memory backup files and paired WAL files.
last_updated: 2026-06-12
owner: simard
doc_type: reference
related:
  - ../operations/cognitive-memory-durability.md
  - ../reference/disk-health-api.md
  - ../howto/configure-disk-health-check.md
  - ../concepts/automated-disk-health.md
---

# Backup pruning API

**Module:** `src/cognitive_memory/backup.rs`

The `NativeCognitiveMemory::prune_old_backups()` method enforces a
retention limit on cognitive memory backup files, preventing unbounded
disk growth in the `backups/` subdirectory of the state root.

> **Note:** A separate `prune_old_backups()` function exists in
> `src/memory_backup/mod.rs` (date-based, using `BackupConfig`). This
> document covers the epoch-based implementation in
> `cognitive_memory/backup.rs` which handles `.ladybug` database files.

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
