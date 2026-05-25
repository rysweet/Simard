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
The recipe uses an **agent step** — an LLM that inspects disk usage, decides
what to clean based on current conditions, and reports what it freed. When the
home partition exceeds ~80% usage, the agent cleans stale engineer worktrees,
cargo build artifacts, and old LadybugDB backups.

This guide shows how to observe the check in action, tune its behavior, and
handle the edge cases.

## When to use this

Use this guide when:

- The daemon logged `disk health: N% used` and you want to understand what happened
- You want to change the cleanup aggressiveness or target priorities
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

## Tune cleanup behavior

The cleanup logic is driven by an agent prompt in the recipe YAML. To change
the agent's behavior, edit the prompt text:

```bash
$EDITOR prompt_assets/simard/recipes/disk-health-check.yaml
```

The prompt provides guidance values that the agent uses as starting points.
These are prose instructions, not shell variables:

| Guidance                 | Default  | Where in prompt                                     |
| ------------------------ | -------- | --------------------------------------------------- |
| Disk usage trigger       | ~80%     | "If disk usage is below 80%..."                     |
| Worktree age threshold   | ~24h     | "older than 24 hours"                               |
| Backup retention count   | ~5       | "keep approximately 5 most recent"                  |
| Cargo cache cleaning     | all      | "clean incremental and debug build artifacts"       |

Example — lower the threshold to 70% and keep 10 backups:

Edit the prompt text to change "below 80%" to "below 70%" and "keep
approximately 5 most recent" to "keep approximately 10 most recent".

Changes take effect on the next OODA cycle — the daemon re-reads the recipe
YAML each time. No rebuild or restart required.

## Read a full disk health report

The agent outputs a key=value text report to stdout, which the daemon captures
and logs. To run the check manually outside the daemon:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard"
```

This invokes the agent, which inspects disk, performs cleanup if needed, and
prints the text report to stdout:

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 12 stale engineer worktrees older than 24h
ACTION: Removed cargo target dirs from 3 worktrees
ACTION: Pruned LadybugDB backups to 5 most recent
ACTION: Cleaned shared-target/ incremental and debug dirs
```

Note: because this is an agent step (not a bash step), running it manually
requires an LLM provider to be available. The agent may also produce
reasoning text before the markers — the Rust parser ignores non-marker lines.

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

### 3. Agent step failed or LLM unavailable

The disk health check uses an agent step, which requires an LLM provider.
If the LLM is unavailable, the recipe will fail and the daemon will log the
error and continue (warn-and-continue behavior).

Check the recipe output manually:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard" 2>&1
```

Common causes:

- LLM provider not configured or rate-limited
- `state_root` directory doesn't exist
- Agent emitted malformed markers (check stdout for `DISK_USED_PCT=`)

### 4. Text parse shows unexpected values

The Rust shim parses key=value lines from the agent's stdout. The agent is
instructed to emit these markers, but if the markers are missing or malformed:

- Missing `DISK_USED_PCT` → parser returns an error
- Missing `FREED_BYTES` → defaults to 0
- Non-numeric values → parser returns an error
- Extra lines (agent reasoning) → silently ignored

If the agent consistently fails to emit markers, check the prompt in the
recipe YAML — the output format instructions may have been accidentally
edited.

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
# Run the recipe directly (requires LLM provider)
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard"
```

Or clean specific categories manually (these skip the agent's judgment and
claim-file safety check — only use when you know no engineers are running):

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

- [Disk health API reference](../reference/disk-health-api.md) — full API, text contract, error variants
- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md) — build artifact scripts
