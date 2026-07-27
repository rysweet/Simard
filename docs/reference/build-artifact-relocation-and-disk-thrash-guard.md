---
title: Build-artifact relocation and disk-thrash guard
description: Reference for the #4803 fix — cargo target relocation resolver, emergency-cleanup hysteresis + backoff marker, build-heavy dispatch preflight, and all associated environment variables and invariants.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/root-disk-saturation-relocation.md
  - ../howto/relocate-build-artifacts-off-root-volume.md
  - ./disk-health-api.md
  - ./resource-admission-api.md
  - ./engineer-worktree-isolation.md
---

# Build-artifact relocation and disk-thrash guard

Fixes [#4803](https://github.com/rysweet/Simard/issues/4803).

This page is the reference for the four components that together stop the root
volume (`/`) from saturating and crash-looping the daemon. For the *why*, see
[Root-disk saturation relocation](../concepts/root-disk-saturation-relocation.md).

> **Prescriptive identifiers.** Code identifiers coined by this spec —
> notably the C1 constant `DEFAULT_CARGO_TARGETS_ROOT_FALLBACK` and the C4
> function `dispatch_spawn_engineer` — are the intended names for the
> implementation. If the implementation lands under different identifiers,
> update this reference in the same change so the two do not drift. Names that
> already exist in the tree (`default_cargo_target_for_worktree`, the
> `disk_pressure` surface in C5) are quoted from current source.

## Components at a glance

| ID | File | Kind | Responsibility |
|----|------|------|----------------|
| C1 | `src/agent_supervisor/tmux.rs` | modified | Cargo target root defaults onto the large volume; single source of truth. |
| C2 | `src/agent_supervisor/lifecycle/spawn.rs` | modified | Single-process spawn delegates to the C1 resolver — no divergent hardcoded path. |
| C3 | `src/disk_health.rs` | modified | Emergency cleanup gains high/low-watermark hysteresis + persistent backoff marker + containment guards. |
| C4 | `src/ooda_actions/advance_goal/spawn.rs` | modified | Build-heavy dispatch preflight probes `/` and refuses under disk pressure. |
| C5 | `src/disk_pressure/` | reused | Existing `PressureLevel` gate; no new constants. |
| C6 | coverage tooling | verified | `target/llvm-cov-target` still resolves under the relocated root. |

## Environment variables

| Variable | Default | Clamp / notes |
|----------|---------|---------------|
| `SIMARD_CARGO_TARGETS_ROOT` | *(unset)* → `/tmp/simard-cargo-targets` | **Override wins.** Empty string is ignored (treated as unset) so it can never yield `/<basename>`. Per-worktree basename is appended. |
| `SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS` | `900` | Parsed with fallback; clamped to `[0, 86400]`. Minimum seconds between two emergency-cleanup runs. |
| `SIMARD_DISK_PRESSURE_MIN_FREE_GB` | `20` | Reused from `disk_pressure`; min-free threshold for the dispatch preflight. |
| `CARGO_TARGET_DIR` | *(operator override)* | If set in the parent environment it is honoured verbatim by both spawn paths; the C1 default is only used when it is unset. |

> **Backward compatibility.** The only default that *changed* is the cargo
> target root: it moved from `$HOME/.cargo-targets` to
> `/tmp/simard-cargo-targets`. Operators who already set
> `SIMARD_CARGO_TARGETS_ROOT` or `CARGO_TARGET_DIR` see no change.

## C1: cargo target root relocation

**`default_cargo_target_for_worktree(worktree_path, parent_pairs) -> String`**
(`src/agent_supervisor/tmux.rs`)

Resolution order for the cargo target root:

1. `SIMARD_CARGO_TARGETS_ROOT` (from `parent_pairs`), if set **and non-empty**.
2. `DEFAULT_CARGO_TARGETS_ROOT_FALLBACK` = `/tmp/simard-cargo-targets`.

The pre-#4803 step 2 — `$HOME/.cargo-targets/<basename>` — is **removed** from
the default chain. `HOME` is no longer consulted to build the default target
root, so artifacts never default onto the volume that hosts `~/.simard`.

The per-worktree basename comes from `worktree_path.file_name()`. If the path
has no terminal component the literal `"engineer-worktree"` is substituted so
the result is always well-formed.

Final path: `<root>/<basename>`.

### Invariants

- **IV-1 (empty-string guard).** An empty `SIMARD_CARGO_TARGETS_ROOT` is
  filtered out, so the resolver can never produce `/<basename>` (which would
  re-saturate `/`).
- **Single source of truth.** This resolver is `pub(crate)` and is the only
  place that computes a default cargo target root. Both the tmux path
  (`compute_tmux_env`) and the single-process path (C2) call it.

## C2: single-process spawn delegation

**`spawn_subordinate`** (`src/agent_supervisor/lifecycle/spawn.rs`)

The previous hardcoded fallback —

```rust
if std::env::var_os("CARGO_TARGET_DIR").is_none() {
    cmd.env("CARGO_TARGET_DIR", "/tmp/simard-engineer-target"); // divergent
}
```

— is replaced by a call to the C1 resolver. The operator `CARGO_TARGET_DIR`
override guard is preserved (an explicit `CARGO_TARGET_DIR` still wins). This
eliminates the divergence where the tmux path and the single-process path
disagreed about the artifact location.

## C3: emergency-cleanup hysteresis + backoff

**`emergency_cleanup(...)`** (`src/disk_health.rs`) — signature preserved.

Tier-1 emergency cleanup (pure Rust, no recipe, no LLM) now runs behind a
hysteresis band and a persistent backoff marker.

### Watermarks

| Watermark | Value | Meaning |
|-----------|-------|---------|
| High | 95% used | Cleanup **may** trigger at or above this. |
| Low | 85% used | Cleanup will not re-fire until usage has fallen back **below** this. |

Between the two watermarks the system is in the hysteresis dead-band: a prior
cleanup does not re-fire even though usage is still elevated. This is what stops
the "delete → cargo rebuilds → delete again 25 min later" thrash.

### Backoff marker

- **Location:** `<state_root>/disk-health/` (a small marker file recording the
  last successful cleanup timestamp).
- **Gate:** a new cleanup is suppressed if fewer than
  `SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS` (default 900, clamped `[0, 86400]`)
  seconds have elapsed since the last run.
- **Fail-open (DP-3).** If the marker cannot be read or is corrupted, cleanup
  is **allowed** to proceed and the condition is logged via `tracing::warn!`.
  The guard can never suppress cleanup forever.

### Deletion safety guards

Before every `remove_dir_all`:

- **IV-4 (symlink check).** `symlink_metadata` is inspected first; a symlinked
  `target` cannot cause deletion to follow the link out of the tree.
- **DP-1 (containment).** The path must `starts_with(repo_root)` **or**
  `starts_with(state_root)`. Deletion roots remain a static code allow-list
  (IV-5); they are never composed from environment input.

## C4: build-heavy dispatch preflight

**`dispatch_spawn_engineer` / build-heavy dispatch**
(`src/ooda_actions/advance_goal/spawn.rs`)

Before dispatching a build-heavy goal session, the preflight probes the **root
filesystem `/`** through the reused `disk_pressure` gate (default threshold —
see C5) and maps the result:

| `PressureLevel` | Preflight action |
|-----------------|------------------|
| `Ok` | Proceed. |
| `Warn` | Proceed, emit `tracing::warn!`. |
| `Refuse` | **Skip this cycle** — benign retry-next-cycle, loud `warn!`. No spawn, no silent fallback. |
| probe error | Proceed (Warn-equivalent), log the probe failure. |

The `Refuse` skip is deliberately **benign**: the goal is not blocked or
failed, it is simply not dispatched until free space recovers, so a transient
disk-pressure spike cannot permanently starve progress. Its visibility comes
from the `warn!` line, never a swallowed error.

## C5: `disk_pressure` reuse

`src/disk_pressure/` is **reused unchanged**. Relevant surface:

- `PressureLevel { Ok, Warn, Refuse }`
- `check_disk_pressure` / `check_disk_pressure_with`
- `DEFAULT_MIN_FREE_GB` = 20
- `exceeds_admission_ceiling` (90% ceiling), `used_pct`

Decision bands (from the module):

- `free >= T` → `Ok`
- `T/2 <= free < T` → `Warn`
- `free < T/2` → `Refuse`

where `T` is `SIMARD_DISK_PRESSURE_MIN_FREE_GB` in bytes. **No constants are
added or changed** by the #4803 fix.

> **Concrete defaults.** With `T = DEFAULT_MIN_FREE_GB = 20 GiB`, `Warn` fires
> when free space drops below **20 GiB** and `Refuse` fires only below
> **10 GiB** (`T/2`). The `Refuse` line is half the `min-free` knob, not equal
> to it — an operator tuning `SIMARD_DISK_PRESSURE_MIN_FREE_GB` moves both
> bands together (`Warn` at the knob, `Refuse` at half the knob).

## C6: coverage-artifact resolution (invariant)

`target/llvm-cov-target` is produced under the relocated `CARGO_TARGET_DIR`,
not `./target`. Coverage runs point llvm-cov at the same env-consistent target
root, so relocation does not break coverage. This is a **verification gate**,
not a code change — CI coverage jobs must resolve artifacts under the relocated
root.

## Tests

| Area | Location |
|------|----------|
| C1 default-root + override precedence + IV-1 empty-string | `src/agent_supervisor/tests_tmux.rs` |
| C3 hysteresis, backoff, symlink/containment guards | inline `#[cfg(test)] mod tests` in `src/disk_health.rs` |
| C4 preflight `Refuse` → skip, probe-error → proceed | inline `#[cfg(test)] mod tests` in `src/ooda_actions/advance_goal/spawn.rs` |

## Verification gates

- `cargo build` green.
- Grep-gate: zero new `print!`/`println!`/`eprintln!`; zero `Bridge` naming.
- `/` stops saturating — emergency cleanup no longer re-fires each cycle.
- llvm-cov coverage artifacts resolve under the relocated target root.
