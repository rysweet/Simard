---
title: Inspect and clean up engineer worktrees
description: Operator guide for the per-engineer git worktrees the OODA daemon allocates under ~/.simard/engineer-worktrees/.
last_updated: 2026-04-24
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/engineer-worktree-isolation.md
  - ./spawn-engineers-from-ooda-daemon.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
---

# Inspect and clean up engineer worktrees

The OODA daemon gives every subordinate engineer its own `git worktree`
under `~/.simard/engineer-worktrees/` (configurable via `SIMARD_STATE_ROOT`).
This guide shows how to observe live worktrees, recover from a crash that
left orphans behind, and tune the build-cache directory.

For the underlying contract see
[Per-Engineer Worktree Isolation](../reference/engineer-worktree-isolation.md).

## List active worktrees

```bash
git -C /home/azureuser/src/Simard worktree list --porcelain \
  | awk '/^worktree .*engineer-worktrees/ { print $2 }'
```

Each line is the absolute path to a live engineer worktree. The
corresponding branch is `engineer/<goal-id>-<epoch_secs>-<6hex>`:

```bash
git -C /home/azureuser/src/Simard branch --list 'engineer/*'
```

## See which engineer owns which worktree

The OODA daemon tags every spawn with the worktree path in its log:

```bash
journalctl --user -u simard-ooda --since '10 min ago' \
  | grep -E 'worktree_path|owned_worktree'
```

You can also tail the per-engineer log:

```bash
ls -lt ~/.simard/logs/engineer-*.log | head
tail -f ~/.simard/logs/engineer-<id>.log
```

## Manually clean up an orphan

The daemon runs an orphan sweep at startup, but you can force one without
restarting the service:

```bash
# Remove git's record of any worktrees whose directories are gone
git -C /home/azureuser/src/Simard worktree prune

# Remove any directory under engineer-worktrees/ that git no longer tracks
LIVE=$(git -C /home/azureuser/src/Simard worktree list --porcelain \
       | awk '/^worktree / { print $2 }')
for d in ~/.simard/engineer-worktrees/*/; do
  case "$LIVE" in
    *"${d%/}"*) ;;                   # still registered, leave it
    *) echo "removing orphan $d"; rm -rf "$d" ;;
  esac
done
```

If a worktree directory is still registered with git but you know its owning
engineer is dead, force-remove it:

```bash
git -C /home/azureuser/src/Simard worktree remove --force \
  ~/.simard/engineer-worktrees/<goal-id>-<epoch>-<hex>
```

## Clean up leftover engineer/* branches

`EngineerWorktree::cleanup` best-effort deletes its own
`engineer/<...>` branch, and the boot-time `sweep_orphaned_worktrees` does
the same for branches whose worktrees are gone. To force a manual sweep of
any leftover engineer branches whose worktrees are no longer registered:

```bash
# After 'git worktree prune', any engineer/* branch whose worktree is gone
# is safe to delete.
git -C /home/azureuser/src/Simard worktree prune
git -C /home/azureuser/src/Simard branch --list 'engineer/*' \
  | sed 's/^[* ] //' \
  | xargs -r git -C /home/azureuser/src/Simard branch -D
```

Branches still attached to a live worktree are protected by git and will
not be deleted by `branch -D`.

## Reclaim disk

Each worktree is a full checkout. To see the cost:

```bash
du -sh ~/.simard/engineer-worktrees/
du -sh ~/.simard/engineer-worktrees/*/ | sort -h | tail
```

If the daemon was restarted after a crash, the startup sweep will already
have reaped abandoned worktrees. If disk is still tight, also see
[Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md).

## Share the cargo build cache across worktrees

Subordinate engineers run with `CARGO_TARGET_DIR=/tmp/simard-engineer-target`
unless the parent daemon already exports `CARGO_TARGET_DIR`. This keeps
incremental `cargo` state shared across all engineer worktrees instead of
forcing a cold `lbug` rebuild per worktree.

To override, set it in the systemd service environment:

```bash
systemctl --user edit simard-ooda
# Add:
# [Service]
# Environment=CARGO_TARGET_DIR=/some/other/path
systemctl --user restart simard-ooda
```

To inspect the value the daemon is currently using:

```bash
systemctl --user show simard-ooda -p Environment
```

## Confirm isolation is working

After a fresh restart, watch for an engineer cycle that lands a PR without
the `worktree state changed during a non-mutating local engineer action`
rejection:

```bash
journalctl --user -u simard-ooda --since '15 min ago' \
  | grep -E 'pull request|worktree state changed'
```

A healthy run shows `https://github.com/.../pull/<n>` lines and **no**
"worktree state changed" lines for that cycle.

## Related

- [Per-Engineer Worktree Isolation](../reference/engineer-worktree-isolation.md) — full API/lifecycle reference
- [How OODA spawns engineer agents](./spawn-engineers-from-ooda-daemon.md)
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md)
