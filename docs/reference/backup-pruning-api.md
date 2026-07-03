---
title: Backup pruning API
description: Reference for pruning verified cognitive-memory backup directories through the current memory_backup module.
last_updated: 2026-07-03
owner: cognitive-memory
doc_type: reference
related:
  - ../operations/cognitive-memory-durability.md
  - ../reference/disk-health-api.md
  - ../howto/configure-disk-health-check.md
  - ../concepts/automated-disk-health.md
---

# Backup pruning API

> **Current API after de-fork #2307.** The native epoch-file pruner
> `NativeCognitiveMemory::prune_old_backups` was deleted with the native fork.
> Current pruning is the free function `memory_backup::prune_old_backups` in
> `src/memory_backup/mod.rs`, operating on verified backup directories described
> by `BackupConfig`.

---

## Public API

```rust
pub fn prune_old_backups(config: &BackupConfig) -> SimardResult<usize>;

pub struct BackupConfig {
    pub backup_dir: PathBuf,
    pub retention_days: u32,
    pub min_backups_to_keep: usize,
}
```

`prune_old_backups` returns the number of backup directories removed. Errors from
listing the backup directory are returned as `SimardError::PersistentStoreIo`.
Individual stale directories are removed with `remove_dir_all`; failed removals
are skipped rather than counted as pruned.

`BackupConfig::default()` uses:

| Field | Default |
|---|---|
| `backup_dir` | `$HOME/.simard/backups` |
| `retention_days` | `30` |
| `min_backups_to_keep` | `3` |

The OODA daemon overrides `backup_dir` to `state_root/backups` before calling the
backup routine.

---

## Behavior

1. If `config.backup_dir` does not exist, return `Ok(0)`.
2. List child directories under `config.backup_dir`.
3. Sort directory paths descending so newest timestamped directories are first.
4. Preserve the first `config.min_backups_to_keep` directories unconditionally.
5. For older entries, read `manifest.json` when present and compare
   `BackupManifest.created_at` with `Utc::now() - retention_days`.
6. Prune directories older than the cutoff. Entries with missing or unreadable
   manifests are prune candidates once they are beyond the minimum-keep window.

Current backups are directories such as:

```text
state_root/backups/20260703_003510/
  manifest.json
  cognitive_snapshot.json
  memory_records.json
```

The removed native fork used files like `cognitive_memory.ladybug.<epoch>` and
WAL siblings. Those files are historical artifacts; the current pruner does not
look for them.

---

## Daemon integration

`run_verified_backup` in `src/operator_commands_ooda/daemon/backup.rs` calls the
pruner only after the new backup verifies:

```rust
let manifest = backup_memory_verified(bridge, &file_store, BACKUP_AGENT, &config)?;
prune_old_backups(&config)?;
Ok(manifest)
```

This ordering is intentional: a failed or partial backup never causes the
last-known-good backup to be reclaimed.

---

## Code location

| Item | File |
|---|---|
| `prune_old_backups` | `src/memory_backup/mod.rs` |
| `BackupConfig` | `src/memory_backup/mod.rs` |
| Daemon call site | `src/operator_commands_ooda/daemon/backup.rs` (`run_verified_backup`) |
| Pruning tests | `src/memory_backup/tests.rs` |

---

## Related

- [Cognitive memory durability (operations)](../operations/cognitive-memory-durability.md)
  — current backup creation, verification, pruning, and restore procedures.
- [Disk health API](./disk-health-api.md) — broader disk cleanup via recipe.
- [Automated disk health (concept)](../concepts/automated-disk-health.md)
  — design rationale for automated cleanup.
- [Configure disk health check](../howto/configure-disk-health-check.md)
  — operator guide for the disk health recipe.
