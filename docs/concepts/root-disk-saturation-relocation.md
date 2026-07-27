---
title: Root-disk saturation relocation and thrash guard
description: Why Simard relocates cargo build artifacts off the small root volume, adds hysteresis to emergency disk cleanup, and refuses build-heavy dispatch under disk pressure — the fix for the #4803 crash-loop.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: explanation
related:
  - ./automated-disk-health.md
  - ./agentic-disk-reclamation.md
  - ../howto/relocate-build-artifacts-off-root-volume.md
  - ../reference/build-artifact-relocation-and-disk-thrash-guard.md
  - ../reference/resource-admission-api.md
---

# Root-disk saturation relocation and thrash guard

Fixes [#4803](https://github.com/rysweet/Simard/issues/4803).

## The failure this prevents

On hosts where `/home` and `~/.simard` live on a small root volume (a 28 GiB
`/` in the reference incident), cargo build artifacts written under
`~/.simard` — chiefly `target/debug` and `target/llvm-cov-target` — filled the
root filesystem to **0 bytes free**. That single condition cascaded across the
whole daemon:

- `cognitive-open-lock` refusals and typed `database is locked` errors,
- `memory-ipc` write failures in goal-session engineers,
- stalled goal advancement across every workstream.

The daemon's Tier-1 **emergency disk cleanup** re-fired roughly every 25
minutes (observed: 21:58 → 99%, 23:00 → 94%, 23:35 → 94%, 00:10 → 96%, 00:42
→ 97%). Each run deleted `target/debug` + `target/llvm-cov-target` (0.7–2.6 GB)
and pruned backups, but `/` refilled within one cycle because cargo
immediately rebuilt into the same location. Cleanup was an **ineffective
band-aid that thrashed** — it never removed the *cause*, only the symptom, and
each pass burned I/O and starved progress.

Meanwhile a 196 GiB `/tmp` volume sat with ~26 GiB free, unused for build
artifacts.

## Root cause

Two independent defaults pointed the fastest-refilling artifacts at the small
root volume:

1. **`default_cargo_target_for_worktree`** (in `src/agent_supervisor/tmux.rs`)
   defaulted `CARGO_TARGET_DIR` to `$HOME/.cargo-targets/<worktree>` — i.e.
   under `~` on the 28 GiB `/`.
2. A divergent hardcoded fallback in
   `src/agent_supervisor/lifecycle/spawn.rs` set
   `CARGO_TARGET_DIR=/tmp/simard-engineer-target` for the single-process spawn
   path, so the two spawn paths disagreed about where artifacts lived.

Emergency cleanup could not win against a build target that lived on the volume
it was trying to protect.

## The fix, in three moves

The fix is **additive and non-breaking** — it changes defaults and adds guard
rails; it does not change any public CLI surface.

### 1. Relocate build artifacts onto the large volume (primary)

The default cargo target root moves off `/`. The existing
`SIMARD_CARGO_TARGETS_ROOT` override still wins; when it is unset the default
now resolves to the large-volume fallback (`/tmp/simard-cargo-targets`) instead
of `$HOME/.cargo-targets`. Both spawn paths delegate to **one** resolver, so
they can no longer diverge. This alone stops `/` from re-saturating.

See [`default_cargo_target_for_worktree`](../reference/build-artifact-relocation-and-disk-thrash-guard.md#c1-cargo-target-root-relocation).

### 2. Give emergency cleanup hysteresis (stops the thrash)

Emergency cleanup gains a **high/low watermark** (trigger at ≥ 95% used, do
not re-fire until usage falls back below a distinct low watermark) plus a
**persistent backoff marker** under `<state_root>/disk-health/`. Cleanup can no
longer re-fire on every timer tick inside a single build window. After the
relocation in move 1, `/` is no longer the fill target, so cleanup becomes
rare rather than perpetual.

See [emergency-cleanup hysteresis](../reference/build-artifact-relocation-and-disk-thrash-guard.md#c3-emergency-cleanup-hysteresis--backoff).

### 3. Refuse build-heavy dispatch under disk pressure (belt)

Before dispatching a build-heavy goal session, a **preflight** probes `/`
through the existing [`disk_pressure`](../reference/resource-admission-api.md)
gate. That gate classifies against a single min-free threshold `T`
(`SIMARD_DISK_PRESSURE_MIN_FREE_GB`, default **20 GiB**) in two bands:
`Warn` when free space is below `T`, and `Refuse` when it drops below `T/2`.
So at defaults the preflight **warns below 20 GiB free and refuses below
10 GiB free**. On a `Refuse`, dispatch is **loudly skipped for this cycle**
(retried next cycle) rather than writing the daemon into ENOSPC again. This
reuses the existing min-free threshold and 90% admission ceiling; it introduces
no parallel constants.

See [dispatch preflight](../reference/build-artifact-relocation-and-disk-thrash-guard.md#c4-build-heavy-dispatch-preflight).

## Design principles honoured

- **No silent fallbacks.** The preflight refuses loudly (`warn!`) and the
  worktree allocator returns a hard `Err`; a degraded spawn is never preferred
  over a visible refusal.
- **Structured tracing + OTel only.** No `print!`/`println!`/`eprintln!` is
  added; every signal flows through `tracing`.
- **Containment before deletion.** Every `remove_dir_all` site is guarded by a
  `symlink_metadata` check and a `starts_with(repo_root | state_root)`
  containment assertion, so a hostile `target -> /` symlink cannot make cleanup
  escape the tree.
- **Fail-open on the marker, never fail-shut.** A corrupted or unreadable
  backoff marker allows cleanup to proceed (logged), so the guard can never
  suppress cleanup forever.

## Where to go next

- Operators: [Relocate build artifacts off the root volume](../howto/relocate-build-artifacts-off-root-volume.md).
- Engineers: [Build-artifact relocation and disk-thrash guard — reference](../reference/build-artifact-relocation-and-disk-thrash-guard.md).
