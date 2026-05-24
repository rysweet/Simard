---
title: Configure and monitor the disk health check
description: Operator guide for Simard's per-cycle disk health check — tuning thresholds, reading reports, and recovering from disk exhaustion.
last_updated: 2026-05-24
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/disk-health-api.md
  - ../concepts/automated-disk-health.md
  - ./inspect-and-clean-engineer-worktrees.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
---

# Configure and monitor the disk health check

Simard runs an automated disk health check at the start of every OODA cycle.
When the home partition exceeds 80% usage, it cleans stale engineer worktrees,
cargo build artifacts, and old LadybugDB backups — then reports what it freed.

This guide shows how to observe the check in action, tune its thresholds, and
handle the edge cases.

## When to use this

Use this guide when:

- The daemon logged `disk health: N% used` and you want to understand what happened
- You want to change the 80% trigger threshold or 24h worktree age limit
- You want to change how many LadybugDB backups are retained
- The daemon logged `disk health check failed` and you need to diagnose it
- Disk is critically low despite the automated check

## Observe the disk health check

The daemon logs a one-liner per cycle:

```bash
grep "disk health" ~/.simard/ooda.log | tail -5
```

Typical output:

```
[2026-05-24T15:42:01Z] disk health: 72% used, freed 53687091200 bytes, 4 actions
[2026-05-24T15:43:02Z] disk health: 72% used, freed 0 bytes, 0 actions
```

For the detailed action list, look for the structured tracing output in stderr
or `ooda.log`:

```bash
journalctl --user -u simard-ooda --since '1 hour ago' \
  | grep -A5 'disk_health'
```

## Tune cleanup thresholds

All thresholds live in the recipe YAML — no Rust recompile needed:

```bash
$EDITOR prompt_assets/simard/recipes/disk-health-check.yaml
```

The tunables are environment variables set at the top of the bash step:

| Variable               | Default  | What it controls                                   |
| ---------------------- | -------- | -------------------------------------------------- |
| `DISK_THRESHOLD_PCT`   | `80`     | Disk usage percentage that triggers cleanup         |
| `WORKTREE_MAX_AGE_H`   | `24`     | Hours before a worktree is eligible for removal     |
| `BACKUP_RETENTION`     | `5`      | Number of LadybugDB backups to keep                 |

Example — lower the threshold to 70% and keep 10 backups:

```yaml
# In disk-health-check.yaml, modify the bash step env:
env:
  DISK_THRESHOLD_PCT: "70"
  BACKUP_RETENTION: "10"
```

Changes take effect on the next OODA cycle — the daemon re-reads the recipe
YAML each time.

## Read a full disk health report

The recipe outputs a JSON report to stdout, which the daemon captures and
logs. To run the check manually outside the daemon:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard"
```

This prints the JSON report to stdout:

```json
{
  "disk_used_pct": 72,
  "freed_bytes": 53687091200,
  "actions_taken": [
    "Removed 48 stale worktrees (50.1G)",
    "Removed cargo target dirs from 3 worktrees (1.2G)",
    "Pruned 19 LadybugDB backups (512M)",
    "Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)"
  ]
}
```

You can also run just the disk usage check (no cleanup) by looking at the
partition directly:

```bash
df -h /home | awk 'NR==2 {print $5}'
```

## Diagnose a failed check

If the daemon logs `disk health check failed`, check these in order:

### 1. `recipe-runner-rs` not installed

```bash
which recipe-runner-rs
```

If missing, the disk health check cannot run but the daemon continues. The
existing `disk_pressure` module provides the hard stop. Install
`recipe-runner-rs` from the amplihack toolchain.

### 2. Recipe YAML missing

```bash
ls -la prompt_assets/simard/recipes/disk-health-check.yaml
```

If missing (e.g., the file was deleted or the repo is in a detached worktree
that doesn't have it), the shim returns `AdapterInvocationFailed` and the
daemon warns and continues.

### 3. Bash step failed

Check stderr from the recipe:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard" 2>&1
```

Common causes:

- `$STATE_ROOT` directory doesn't exist
- Permission denied on a directory under `$STATE_ROOT`
- `du` or `find` not on PATH (unlikely on standard Linux)

### 4. JSON parse failure

The Rust shim expects exactly one JSON object on stdout. If the bash script
printed extra lines (warnings, debug output), the JSON parse fails. Fix the
recipe to keep non-JSON output on stderr.

## Handle persistent disk pressure

If the automated check cleans everything it can and disk is still above 90%,
the daemon logs:

```
disk still above 90% after cleanup — builds may fail
```

At this point:

1. **Check the main worktree's target dir:**
   ```bash
   du -sh /home/azureuser/src/Simard/target/
   ```
   The automated check does *not* clean the main repo's `target/` — only
   engineer worktree targets and shared caches. If the main target is large,
   clean it manually or use the low-space build scripts:
   ```bash
   scripts/reclaim-build-space --apply
   ```

2. **Check for non-Simard disk consumers:**
   ```bash
   du -sh /home/azureuser/* | sort -h | tail -10
   ```

3. **If the partition is genuinely too small**, the `disk_pressure` module
   will prevent engineer spawning at critical thresholds. Consider expanding
   the partition or moving the state root to a larger disk:
   ```bash
   export SIMARD_STATE_ROOT=/mnt/data/.simard
   ```

## Understand the shared cargo target directory

The repository's `.cargo/config.toml` redirects all `cargo build` output to
`/home/azureuser/.simard/shared-target`:

```bash
cat .cargo/config.toml
```

```toml
[build]
target-dir = "/home/azureuser/.simard/shared-target"
```

This means:

- All worktrees share one build cache instead of each creating its own
- `cargo build` from any worktree writes to the same directory
- Concurrent builds serialize on Cargo's file lock (slower but saves 100G+)
- `CARGO_TARGET_DIR` env var overrides this if set

To check the current size:

```bash
du -sh /home/azureuser/.simard/shared-target/
```

The disk health check cleans this directory when it runs cleanup. It will be
rebuilt incrementally on the next `cargo build`.

## Verify the check is running

After restarting the daemon, confirm the check ran:

```bash
# Look for the first disk health log in this daemon session
journalctl --user -u simard-ooda --since '5 min ago' \
  | grep 'disk health'
```

You should see one `disk health:` line per OODA cycle (default: every 60s).

## Manually trigger cleanup

To run cleanup outside the daemon without waiting for a cycle:

```bash
# Run the recipe directly
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard"
```

Or clean specific categories manually (these skip the claim-file safety
check — only use when you know no engineers are running):

```bash
# Stale worktrees only (no claim-file check — the recipe is safer)
find ~/.simard/engineer-worktrees/ -maxdepth 1 -mindepth 1 -type d \
  -mtime +1 -exec rm -rf {} +

# LadybugDB backups (keep 5 most recent)
ls -t ~/.simard/backups/* | tail -n +6 | xargs rm -f

# Shared cargo caches
rm -rf ~/.simard/cargo-target/* ~/.simard/shared-target/*
```

## Related

- [Disk health API reference](../reference/disk-health-api.md) — full API, JSON contract, error variants
- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md) — build artifact scripts
