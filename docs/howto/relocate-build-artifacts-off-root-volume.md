---
title: "How to relocate build artifacts off the root volume"
description: Move cargo build artifacts off a small root volume, tune the emergency-cleanup backoff, and verify the daemon no longer crash-loops on a full disk.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/root-disk-saturation-relocation.md
  - ../reference/build-artifact-relocation-and-disk-thrash-guard.md
  - ./configure-disk-health-check.md
  - ./configure-disk-reclamation.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
---

# How to relocate build artifacts off the root volume

By default Simard writes cargo build artifacts to
`/tmp/simard-cargo-targets/<worktree>` so they land on a large volume instead
of the small root filesystem that hosts `~/.simard`. This guide shows how to
confirm the relocation, point it at a different volume, tune the emergency
cleanup, and verify the [#4803](https://github.com/rysweet/Simard/issues/4803)
crash-loop is gone.

## When to use this

Use this guide when:

- `/home` and `~/.simard` share a small root volume that keeps filling up.
- `~/.simard/ooda.log` shows emergency disk cleanup re-firing every ~25 minutes.
- You see `database is locked`, `cognitive-open-lock` refusals, or
  `memory-ipc` write failures that correlate with `/` at 0 bytes free.
- You want build artifacts on a specific data volume rather than `/tmp`.

## Step 1: Confirm which volume is filling up

```bash
df -h / /tmp
```

If `/` is at or near 100% while `/tmp` (or another data volume) has room, the
default relocation applies. On the reference host this looked like a 28 GiB `/`
at 100% next to a 196 GiB `/tmp` with ~26 GiB free.

## Step 2: Confirm where cargo artifacts are being written

With no override set, the default target root is `/tmp/simard-cargo-targets`.
Verify the daemon is using it:

```bash
ls -d /tmp/simard-cargo-targets/*/ 2>/dev/null
```

You should see one directory per active engineer worktree, each containing a
`debug/` (and, during coverage runs, `llvm-cov-target/`) subtree. If instead
you find large artifacts under `~/.cargo-targets` or `~/.simard/**/target`, the
daemon is running an old build — deploy the #4803 fix and restart.

## Step 3: (Optional) point relocation at a specific volume

To use a dedicated data volume instead of `/tmp`, set
`SIMARD_CARGO_TARGETS_ROOT` before launching the daemon:

```bash
export SIMARD_CARGO_TARGETS_ROOT=/data/simard-cargo-targets
```

Rules:

- The override **wins** over the default.
- An **empty** value is ignored (treated as unset) so it can never resolve to
  `/<worktree>` and re-saturate `/`.
- A per-worktree basename is appended automatically; point this at the volume
  root, not at a single worktree.

To pin an exact directory for a one-off local build instead, set
`CARGO_TARGET_DIR` directly — it is honoured verbatim by both spawn paths.

## Step 4: Tune the emergency-cleanup backoff (optional)

Emergency cleanup now uses hysteresis (trigger at ≥ 95% used, do not re-fire
until usage drops below 85%) plus a minimum re-fire interval. Adjust the
interval with:

```bash
# Minimum seconds between two emergency cleanups (default 900, clamped 0–86400).
export SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS=1800
```

Leave this at the default unless cleanup still fires too often after
relocation — post-relocation, `/` is no longer the fill target, so cleanup
should become rare on its own.

The backoff marker lives under `<state_root>/disk-health/`. If it is deleted or
corrupted, cleanup **fails open** (runs anyway) and logs a warning — it can
never be stuck suppressed.

## Step 5: Confirm the dispatch preflight threshold

Build-heavy goal dispatch is gated by the `disk_pressure` classifier, which
compares `/` free space against a single min-free threshold `T` set by
`SIMARD_DISK_PRESSURE_MIN_FREE_GB` (default **20 GiB**):

- **`Warn`** — free space below `T` (below **20 GiB** at defaults): dispatch
  still proceeds, but logs a `warn!`.
- **`Refuse`** — free space below `T/2` (below **10 GiB** at defaults):
  dispatch is skipped this cycle.

Note the `Refuse` line is **half** the `min-free` knob, not equal to it. Tune
the knob if you want a different floor:

```bash
# T = 20 GiB → Warn below 20 GiB, Refuse below 10 GiB.
export SIMARD_DISK_PRESSURE_MIN_FREE_GB=20
```

When the preflight refuses, the daemon logs a `warn!` and **retries the goal
next cycle** — it does not block or fail the goal.

## Step 6: Verify the crash-loop is gone

After deploying and restarting the daemon:

```bash
# Emergency cleanup should NOT re-fire every ~25 minutes anymore.
grep -i "emergency" ~/.simard/ooda.log | tail -10

# Root volume should hold steady well below 95%.
watch -n 60 'df -h /'

# Preflight refusals (if any) are visible and benign:
grep -i "disk pressure" ~/.simard/ooda.log | tail -10
```

Success looks like: `/` stays below the high watermark, emergency cleanup
entries become sparse instead of appearing every cycle, and the
`database is locked` / `cognitive-open-lock` / `memory-ipc` errors clear.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| Artifacts still under `~/.cargo-targets` | Old binary before the #4803 fix | Redeploy and restart the daemon. |
| `SIMARD_CARGO_TARGETS_ROOT` set but ignored | Value is empty | Set a non-empty absolute path (empty is intentionally ignored). |
| Cleanup still thrashes | `/tmp` itself is small / is `/` | Point `SIMARD_CARGO_TARGETS_ROOT` at a genuinely large volume. |
| Coverage jobs can't find artifacts | Tooling hardcodes `./target` | Point llvm-cov at the relocated `CARGO_TARGET_DIR` (env-consistent). |
| Goals never dispatch | `/` chronically below the refuse line | Reclaim space (see [reclaim disk](./reclaim-disk-space-and-run-low-space-rust-builds.md)); the preflight is protecting you from ENOSPC. |

## See also

- Concept: [Root-disk saturation relocation and thrash guard](../concepts/root-disk-saturation-relocation.md)
- Reference: [Build-artifact relocation and disk-thrash guard](../reference/build-artifact-relocation-and-disk-thrash-guard.md)
- [Configure and monitor the disk health check](./configure-disk-health-check.md)
