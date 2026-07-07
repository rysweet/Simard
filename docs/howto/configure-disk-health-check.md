---
title: Configure and monitor the disk health check
description: Operator guide for Simard's per-cycle disk health check — tuning thresholds, reading reports, and recovering from disk exhaustion.
last_updated: 2026-06-05
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/automated-disk-health.md
  - ../concepts/agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ../reference/disk-health-api.md
  - ./inspect-and-clean-engineer-worktrees.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
---

# Configure and monitor the disk health check

> **Superseded (2026-07-07).** For self-healing disk cleanup, use the agentic
> [disk-reclamation capability](./configure-disk-reclamation.md) instead. It runs
> an agent that *proposes* reclaimable candidates and a deterministic Rust
> executor that *disposes* of them behind hard safety rails (never removes
> `worktrees/main`, a daemon working directory, a live-PID path, or a worktree
> with unpushed work). The manual `find … -mtime +1 -exec rm -rf` and
> merge-base-is-ancestor cleanup patterns below are **deprecated** — they have no
> safety rails and the merge-base heuristic misfired on fresh worktrees. The
> per-cycle check documented here still exists but is no longer the primary path.

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

The tunables are described in the agent prompt within the recipe YAML. The
agent uses its judgment based on the guidelines in the prompt:

| Guideline              | Default  | What it controls                                   |
| ---------------------- | -------- | -------------------------------------------------- |
| Cleanup threshold      | `80%`    | Disk usage percentage that triggers cleanup         |
| Worktree max age       | `24h`    | Hours before a worktree is eligible for removal     |
| Backup retention       | `~5`     | Approximate number of LadybugDB backups to keep     |

To change these, edit the agent prompt in the recipe YAML. For example,
change "80%" to "70%" in the prompt text, or adjust the backup retention
guidance.

Changes take effect on the next OODA cycle — the daemon re-reads the recipe
YAML each time.

## Read a full disk health report

The recipe outputs key=value text markers inside the agent step's output.
The Rust shim accesses this via the `--output-format json` envelope from
`recipe-runner-rs`. To run the check manually and see the raw JSON envelope:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  --output-format json \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard"
```

This prints a JSON envelope containing the step output:

```json
{
  "success": true,
  "step_results": [
    {
      "step_id": "check-disk-usage",
      "output": "DISK_USED_PCT=72\nFREED_BYTES=53687091200\nACTION: Removed 48 stale worktrees (50.1G)\nACTION: Removed cargo target dirs from 3 worktrees (1.2G)\nACTION: Pruned 19 LadybugDB backups (512M)\nACTION: Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)\n"
    }
  ]
}
```

The Rust shim extracts `step_results[0].output` and parses the key=value
markers from that string. To see only the text-format summary (without the
JSON envelope), omit `--output-format json`:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard"
```

> **Note:** The text-format output only contains the recipe summary line
> (e.g., `Recipe: disk-health-check SUCCESS`), not the step output. This is
> why the daemon uses `--output-format json` — to access the actual agent
> output containing the `DISK_USED_PCT` markers.

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

### 4. Text parse shows unexpected values

The Rust shim extracts the agent step output from the `--output-format json`
envelope and parses key=value lines from it. The agent's output may include
conversational text, `df` output, and other noise alongside the markers —
the parser ignores lines it doesn't recognize.

If values are wrong, check the JSON envelope to see the raw agent output:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  --output-format json \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard" | python3 -m json.tool
```

Look at `step_results[0].output` — the markers (`DISK_USED_PCT`, `FREED_BYTES`,
`ACTION:`) must appear on their own lines. Check for typos in key names and
ensure values are plain integers (no units, no commas).

### 5. JSON deserialization failed

If the daemon logs a parse error mentioning JSON or serde, this means
`recipe-runner-rs --output-format json` returned malformed output. Check:

- `recipe-runner-rs` version supports `--output-format json` (introduced in
  the same version as the JSON envelope)
- The recipe YAML is syntactically valid
- There is no shell wrapper around `recipe-runner-rs` that strips or modifies
  stdout

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

To reclaim disk on demand, prefer the guarded agentic capability — it applies
the hard safety rails and stops once under threshold:

```bash
simard disk-reclaim            # dry-run: preview what would be reclaimed
simard disk-reclaim --apply    # perform guarded reclamation
```

See [Configure disk reclamation](./configure-disk-reclamation.md).

To run the legacy per-cycle recipe directly (JSON envelope mode, same as daemon):

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  --output-format json \
  -c STATE_ROOT="$HOME/.simard" \
  -c REPO_ROOT="/home/azureuser/src/Simard"
```

> **Deprecated — do not use.** The raw commands below have **no safety rails**:
> the `find … -mtime +1 -exec rm -rf` pattern deletes by mtime alone (no
> live-process, uncommitted/unpushed, or PR-state check) and the
> merge-base-is-ancestor "already merged" shortcut misfires on fresh worktrees.
> Use `simard disk-reclaim --apply` instead, which guards every deletion.
>
> ```bash
> # DEPRECATED — unguarded; kept only to document the old runbook.
> # find ~/.simard/engineer-worktrees/ -maxdepth 1 -mindepth 1 -type d \
> #   -mtime +1 -exec rm -rf {} +
> # ls -t ~/.simard/backups/* | tail -n +6 | xargs rm -f
> # rm -rf ~/.simard/cargo-target/* ~/.simard/shared-target/*
> ```

## Related

- [Configure disk reclamation](./configure-disk-reclamation.md) — the agentic, self-healing successor with hard safety rails
- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale
- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale (superseded)
- [Disk health API (reference)](../reference/disk-health-api.md) — module API, structs, data flow
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md) — build artifact scripts
