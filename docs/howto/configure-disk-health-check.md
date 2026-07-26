---
title: Configure and monitor the disk health check
description: Operator guide for Simard's per-cycle disk health check — tuning thresholds, reading reports, and recovering from disk exhaustion.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/automated-disk-health.md
  - ../concepts/agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ../howto/reclaim-disk-with-the-simard-disk-tool.md
  - ../reference/simard-disk-tool.md
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

- The daemon logged `disk health recipe: OK` and you want to understand what happened
- You want to change the 80% trigger threshold or 24h worktree age limit
- You want to change how many LadybugDB backups are retained
- The daemon logged `disk health check failed` and you need to diagnose it
- Disk is critically low despite the automated check

## Observe the disk health check

The daemon logs a one-liner per cycle recording the recipe's exit status
(issue #4722 — the recipe now acts via the `simard disk` tool and prints no
report to parse):

```bash
grep "disk health" ~/.simard/ooda.log | tail -5
```

Typical output:

```
[2026-07-26T15:42:01Z] [simard] disk health recipe: OK
[2026-07-26T15:43:02Z] [simard] disk health recipe: OK
```

A failure logs `WARN: disk health recipe reported failure (non-zero exit)` or
`WARN: disk health check failed: <error>` (spawn/recipe-not-found). The actual
reclamations are performed — and logged — by the `simard disk` tool the recipe
calls; see [Reclaim disk with the simard disk tool](./reclaim-disk-with-the-simard-disk-tool.md).

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

## Run the disk health check manually

Since issue #4722 the recipe **acts via the `simard disk` tool** and prints no
key=value markers or JSON envelope. Simard interprets the run by its **exit
status alone** (`src/disk_health.rs::run_disk_health_check` → `Ok(true)` on exit
`0`, `Ok(false)` on non-zero, `Err(..)` on spawn/recipe-not-found). There is no
`--output-format json` and nothing to parse.

To run the check manually exactly as the daemon does:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard" \
  -c repo_path="/home/azureuser/src/Simard/worktrees/main"
echo "exit status: $?"   # 0 = OK, non-zero = failure
```

The agent surveys with `simard disk report` and reclaims with
`simard disk reclaim --path <P>` (or `--paths @<file>`). Those tool invocations
are the effect; whatever the agent prints is ignored. To see *what* was
reclaimed or skipped, run the tool directly — see
[Reclaim disk with the simard disk tool](./reclaim-disk-with-the-simard-disk-tool.md):

```bash
simard disk report --path /home/azureuser/.simard/engineer-worktrees/<name>
```

You can inspect current usage independently of the check:

```bash
df -h /home | awk 'NR==2 {print $5}'
```

## Diagnose a failed check

If the daemon logs `disk health check failed` or `disk health recipe reported
failure`, check these in order:

### 1. `recipe-runner-rs` not installed

```bash
which recipe-runner-rs
```

If missing, the disk health check cannot run but the daemon continues. The
`disk_pressure` module and the Tier-1 deterministic `emergency_cleanup` provide
the hard stop. Install `recipe-runner-rs` from the amplihack toolchain.

### 2. Recipe YAML missing

```bash
ls -la prompt_assets/simard/recipes/disk-health-check.yaml
```

If missing (e.g., the file was deleted or the repo is in a detached worktree
that doesn't have it), `run_disk_health_check` returns `AdapterInvocationFailed`
and the daemon warns and continues.

### 3. Recipe exited non-zero

The trigger records failure by exit status. Re-run it manually to see the
agent's stderr:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard" \
  -c repo_path="/home/azureuser/src/Simard/worktrees/main" 2>&1
echo "exit status: $?"
```

Common causes:

- `simard disk` not on PATH (the agent cannot act) — build/install Simard
- `$state_root` directory doesn't exist
- Permission denied on a directory under `$state_root`

### 4. Candidates were skipped, not reclaimed

A `simard disk` skip is **not** a failure — the guard is refusing to delete a
path whose PR is not merged/closed, that has a live session, or that lies
outside an allow-root. Run `simard disk report --path <P>` to see the
`reject_reason`. This is correct, protective behavior; see the
[guard reasons table](../reference/simard-disk-tool.md#guard-reject-reasons).

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

You should see one `disk health recipe:` line per OODA cycle (default: every 60s).

## Manually trigger cleanup

To reclaim disk on demand, prefer the guarded agentic capability — it applies
the hard safety rails and stops once under threshold:

```bash
simard disk-reclaim            # dry-run: preview what would be reclaimed
simard disk-reclaim --apply    # perform guarded reclamation
```

Or use the agent-facing tool the disk-health recipe itself calls (apply is the
default; add `--dry-run` to preview):

```bash
simard disk report  --path <candidate>   # vet + summarise, never deletes
simard disk reclaim --path <candidate>   # guarded delete (apply by default)
```

See [Configure disk reclamation](./configure-disk-reclamation.md) and
[Reclaim disk with the simard disk tool](./reclaim-disk-with-the-simard-disk-tool.md).

To run the per-cycle recipe directly (exit-status only — no JSON, no markers):

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-health-check.yaml \
  -c state_root="$HOME/.simard" \
  -c repo_path="/home/azureuser/src/Simard/worktrees/main"
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

- [Reclaim disk with the simard disk tool](./reclaim-disk-with-the-simard-disk-tool.md) — the agent-facing tool the recipe now calls to act (no JSON emitted, exit-status only)
- [The simard disk tool (reference)](../reference/simard-disk-tool.md) — CLI grammar, exit codes, guard reasons, thin-trigger wiring
- [Configure disk reclamation](./configure-disk-reclamation.md) — the agentic, self-healing successor with hard safety rails
- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale
- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale (superseded)
- [Disk health API (reference)](../reference/disk-health-api.md) — module API, structs, data flow
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md) — build artifact scripts
