---
title: Disk reclaim — deterministic reclaimable-set enumeration
description: Reference for the src/disk_reclaim/reclaimable.rs deterministic enumerator that guarantees routine reclaim frees real space before emergency thresholds — the reclaimable_targets shared set (idle self-deploy-target / state build caches, stale engineer worktrees), build_tree_roots allow-root widening, the SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS / WORKTREE_IDLE_DAYS knobs, how emergency_cleanup consumes the same set, and how snapshot retention is delegated to MaintenanceThread rather than duplicated.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ./disk-reclaim-api.md
  - ./disk-reclaim-telemetry.md
  - ./engineer-worktree-sweep-safety.md
---

# Disk reclaim — deterministic reclaimable-set enumeration

**Module:** `src/disk_reclaim/reclaimable.rs`

Routine disk reclaim no longer depends on a language model proposing the right
paths. A **deterministic Rust enumerator** always proposes the known-safe,
regenerable space hogs that routine reclaim previously never touched — the idle
`self-deploy-target` build tree, the shared state-root build caches, and stale
engineer worktrees — so a routine reclaim cycle frees real space *before* the
partition reaches emergency thresholds. The agentic proposal (via
`disk-reclaim.yaml`) remains **additive** on top; both sources flow through the
same non-bypassable [`guard::vet_candidate`](./disk-reclaim-api.md#guardrs--the-non-bypassable-rail)
rail. **The enumerator proposes; Rust disposes** — exactly like the agent.

This reference documents the enumerator, the shared reclaimable-set definition it
and `emergency_cleanup` both consume, the idle-age thresholds, the allow-root
widening that keeps containment exact, and the explicit division of ownership with
the existing [`MaintenanceThread`](#relationship-to-maintenancethread-snapshot-ownership).

## Why this exists

Before this capability, routine reclaim sourced candidates *only* from the LLM
recipe. When the agent did not nominate the real consumers, the executor
categorically routed everything to "skip for review" and removed nothing —
logging the steady state:

```text
disk reclaim: 89% -> 89% used, freed 0 bytes, 0 paths removed, 7 skipped for review
```

every cycle while `%-used` climbed monotonically toward 100%.

Emergency cleanup (`disk_health::emergency_cleanup`) was **not** as under-scoped
as first assumed — it already removed `target/debug/`, `target/llvm-cov-target/`,
`worktrees/*/target/`, the state-root `cargo-target/` and `shared-target/`, and
stale backups (keeping the 2 newest). The real gap was twofold:

1. **Routine reclaim froze at `0 bytes`** whenever the agent failed to nominate
   candidates — there was no deterministic floor, so pressure climbed until the
   emergency (≥95%) tier fired.
2. **Neither tier reclaimed the idle `self-deploy-target/` build tree** (a
   regenerable, potentially multi-gigabyte cargo output under the state root) or
   **swept stale engineer worktrees**.

The fix adds a deterministic floor to *routine* reclaim and extends
`emergency_cleanup` to reclaim `self-deploy-target/` via the **same** shared set,
so `bytes_freed > 0` is guaranteed under the stale-artifact scenario. Snapshot
backups are deliberately **out of scope** here — they are already owned by
`MaintenanceThread` (see
[below](#relationship-to-maintenancethread-snapshot-ownership)). See
[Agentic disk reclamation § the deterministic floor](../concepts/agentic-disk-reclamation.md#the-deterministic-floor-why-routine-reclaim-stopped-freeing-0-bytes)
for the design rationale.

## What the enumerator proposes

`reclaimable_targets(state_root)` enumerates two categories of **safe,
regenerable** artifacts. It **only proposes** — it never deletes, and every
candidate it returns is still re-vetted by `vet_candidate` at the syscall
boundary, identically to an LLM-proposed candidate. There is no "trusted
internal" shortcut.

| Category | What | Reclaim primitive | Gating |
| -------- | ---- | ----------------- | ------ |
| **Idle build trees** | `self-deploy-target/`, and the state-root shared build caches `cargo-target/` and `shared-target/` | `stale_build_cache` (`rm -rf`) | proposed only when no live PID references them **and** they are older than the build-idle window |
| **Stale engineer worktrees** | idle engineer worktrees under `<state_root>/engineer-worktrees` | `tracked_worktree` (`git worktree remove --force`) | proposed only when idle beyond the worktree-idle window; still subject to the dirty/unpushed/unknown-PR vetoes at vet time |

All three are pure build output or reconstructable checkouts: `cargo build`
regenerates the target trees, and a swept worktree is re-created on demand. None
of them holds durable cognitive state.

The `kind` each candidate carries is **advisory only** (as for LLM candidates):
the guard re-derives the real primitive at vet time, and a mislabelled `kind` can
only ever *deepen* vetting, never shorten it. A path that is actually a git
worktree is always run through the uncommitted/unpushed + merged/closed-PR vetoes
even if the enumerator labelled it `stale_build_cache`.

## What is never enumerated (live-state protection)

The enumerator **never** proposes live or protected state. These are excluded at
enumeration time *and* would be rejected by the guard even if proposed:

- **Live cognitive store:** `cognitive`, `cognitive.wal`, `cognitive.shadow` —
  never reclaimable.
- **Snapshot / backup / quarantine artifacts:** `cognitive.snapshot-*`,
  `snapshot-*`, `shadow-*`, `backup-*`, `verified-backup-*`, `cognitive.corrupt-*`
  — **owned exclusively by `MaintenanceThread`**, never enumerated here (see
  [below](#relationship-to-maintenancethread-snapshot-ownership)).
- **Protected main / daemon dirs:** `worktrees/main`,
  `HARDCODED_PROTECTED_MAIN`, and every resolved daemon `WorkingDirectory`.
- **Anything referenced by a live PID.**
- **Repos in `SIMARD_GIT_PROTECTED_REPOS`.**

Because the enumerator's candidates pass through `vet_candidate` unchanged, the
live-state protection is enforced **twice**: once by exclusion at enumeration
time, and once — authoritatively — by the guard at the syscall boundary.

## Relationship to MaintenanceThread (snapshot ownership)

Snapshot, backup, and corruption-quarantine retention is **not** duplicated by
this enumerator. `MaintenanceThread`
(`src/cognitive_threads/threads/maintenance.rs`) is the **single owner** of those
directory classes and already prunes them on its own cadence with its own
keep-N floors:

| Directory class | Prefixes | Owner | Retention knob |
| --------------- | -------- | ----- | -------------- |
| Corruption quarantine | `cognitive.corrupt-*` | `MaintenanceThread` | `SIMARD_MAINTENANCE_KEEP_CORRUPT` (default 3) |
| Store snapshots / shadow WAL | `cognitive.snapshot-*`, `snapshot-*`, `shadow-wal-*`, `shadow-*` | `MaintenanceThread` | `SIMARD_MAINTENANCE_KEEP_SNAPSHOTS` (default 5) |
| Verified backups | `backup-*`, `verified-backup-*` | `MaintenanceThread` | `SIMARD_MAINTENANCE_KEEP_BACKUPS` (default 7) |
| Idle build trees / worktrees | `self-deploy-target`, `cargo-target`, `shared-target`, `engineer-worktrees/*` | **`disk_reclaim` (this enumerator)** | `SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS`, `SIMARD_DISK_RECLAIM_WORKTREE_IDLE_DAYS` |

**Ownership is disjoint by directory class** — there is exactly one retention
policy per directory, so the two subsystems can never race or double-count
`bytes_freed`. `disk_reclaim` deliberately does **not** introduce a
`SIMARD_DISK_RECLAIM_KEEP_SNAPSHOTS` knob (that would collide with
`SIMARD_MAINTENANCE_KEEP_SNAPSHOTS` over the same dirs). If snapshot backups are
the source of disk pressure, the lever is `MaintenanceThread`'s retention /
`SIMARD_MAINTENANCE_DRY_RUN` — not this enumerator.

## API

### `reclaimable_targets(state_root: &Path) -> Vec<ReclaimCandidate>`

The single shared deterministic enumerator. Returns the proposed candidate set
described above, read from the current on-disk state (directory listings, mtimes,
`/proc` liveness). Pure proposal: performs **no** deletion and mutates nothing.
Consumed by **both** the routine-reclaim candidate collection (additively, next
to LLM proposals) and `emergency_cleanup`, so the two paths can never diverge.

Thresholds are read from the environment (see [below](#configuration)) with
defensive clamping applied — a `0` or empty value **never** means "purge now"; it
clamps to a safe floor.

### `build_tree_roots(state_root: &Path) -> Vec<PathBuf>`

Pure, side-effect-free helper returning the containment roots the enumerator
needs `allow_roots` to include. In practice this is the **specific**
`<state_root>/self-deploy-target` directory (the shared `cargo-target/`,
`shared-target/`, and `engineer-worktrees/` roots are already in
`allow_roots`). `allow_roots(state_root)` unions this in so guard containment
permits the enumerated categories. This union is an **explicit closed set** of
**specific subdirectories** — it must never resolve to bare `$HOME`, a bare
`state_root`, or any parent that would also contain the live
`cognitive`/`.wal`/`.shadow` store. (Because snapshot backups live *directly*
under `state_root`, the enumerator never adds `state_root` itself as an
allow-root — only leaf build-tree directories. A debug assertion and tests guard
against a widening bug that spanned `state_root` or `$HOME`.)

### Directory-name constants

The enumerator, `allow_roots`, and `emergency_cleanup` reference **one** source
of truth for the self-deploy target name. `SELF_DEPLOY_TARGET_DIRNAME` already
exists in `src/self_deploy/source_prep.rs`; `disk_reclaim` **imports and reuses**
it rather than re-declaring:

| Constant | Value | Defined in |
| -------- | ----- | ---------- |
| `SELF_DEPLOY_TARGET_DIRNAME` | `self-deploy-target` | `src/self_deploy/source_prep.rs` (reused) |

The shared build caches (`cargo-target`, `shared-target`) reuse the existing
literals already used by `allow_roots` and `emergency_cleanup` in
`src/disk_reclaim/mod.rs` / `src/disk_health.rs`. No new `canary-repro-target` or
`bootstrap-target` constants are introduced — no code produces those directories.

## Configuration

All thresholds are environment-overridable with safe, conservative defaults. They
**add to** — never replace — the existing `SIMARD_DISK_RECLAIM_*` knobs
(`SIMARD_DISK_RECLAIM_PCT`, `SIMARD_DISK_RECLAIM_DAEMON_APPLY`), the dry-run /
human-review defaults, and the apply gate. Snapshot retention is **not**
configured here — see
[Relationship to MaintenanceThread](#relationship-to-maintenancethread-snapshot-ownership).

| Variable | Effect | Default | Safe floor when set to `0`/empty/invalid |
| -------- | ------ | ------- | ---------------------------------------- |
| `SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS` | An idle build tree (`self-deploy-target`/`cargo-target`/`shared-target`) is proposed only if its mtime is older than this many days **and** no live PID references it. | `1` | clamps to the default (`1`) |
| `SIMARD_DISK_RECLAIM_WORKTREE_IDLE_DAYS` | A stale engineer worktree is proposed only if idle beyond this many days (still subject to dirty/unpushed/PR-state vetoes). | `7` | clamps to the default (`7`) |

> **Defensive clamping is a safety property, not a convenience.**
> `BUILD_IDLE_DAYS=0` or `WORKTREE_IDLE_DAYS=0` must **never** be interpreted as
> "delete everything now" — the parser clamps out-of-range, empty, and
> unparseable values back to the safe default. This is asserted by a dedicated
> test.

Set a knob for the daemon by exporting it into the service environment:

```bash
# Require a build tree to be idle 3 days, and worktrees 14 days, before sweep.
systemctl --user set-environment SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS=3
systemctl --user set-environment SIMARD_DISK_RECLAIM_WORKTREE_IDLE_DAYS=14
systemctl --user restart simard-ooda
```

No recompile is needed — the enumerator reads these each reclaim cycle.

## Emergency cleanup consumes the same set

`disk_health::emergency_cleanup` (the deterministic Tier-1 hard stop) is extended
to reclaim the idle `self-deploy-target/` build tree through the **same**
`reclaimable_targets(state_root)` definition (plus the live-PID guard), closing
the one regenerable consumer both tiers previously ignored. It **retains every
previously-valid removal** — `target/debug/`, `target/llvm-cov-target/`,
`worktrees/*/target/`, the state-root `cargo-target/` and `shared-target/`, and
stale backups (keep 2). Those state-root build caches are now expressed through
the shared set so routine and emergency reclaim can never disagree about them;
the repo-root target caches and backup pruning remain in the emergency path as
before.

The result: at ≥95% used, emergency cleanup frees its previous set **plus** the
idle `self-deploy-target/` tree — materially more than before — in one
deterministic pass with no LLM and no `gh` dependency. See
[`emergency_cleanup`](./disk-reclaim-api.md#disk-health-emergency-tier-alignment)
in the API reference.

## Reporting

With deterministic enumeration, routine reclaim reports **real** removals. Under
the stale-artifact scenario a routine apply-mode cycle now logs, for example:

```text
disk reclaim: 91% -> 84% used, freed 42949672960 bytes, 44 paths removed, 2 skipped for review
```

instead of the old `freed 0 bytes, 0 paths removed, N skipped for review` steady
state. `ReclaimReport::reclaim_performed()` returns `true`, and the
`simard.disk.reclaim.bytes_freed` / `paths_removed` counters increment (see
[Disk reclaim telemetry](./disk-reclaim-telemetry.md)). Dry-run still frees
nothing but now populates a non-empty `would_remove[]` deterministically.

## Observability

All enumerator activity is structured `tracing` + OTel only — **no**
`println!`/`print!`/`eprintln!` for operational output, and **no silent
fallbacks**:

- The enumeration decision for each category (proposed vs. retained-by-idle-window
  vs. live-PID) is emitted as structured fields, not free text.
- When the **LLM recipe proposal fails**, that failure is now **non-fatal**: it is
  surfaced via `tracing::warn!` + an OTel span (never swallowed), and the
  deterministic enumerator still produces candidates so `bytes_freed > 0` is
  preserved. Previously a recipe failure could leave routine reclaim with no
  candidates at all.
- Every guard veto continues to emit the `simard.disk.reclaim.candidates_skipped`
  counter tagged by `RejectReason`.

## Safety invariants

| Invariant | Enforcement |
| --------- | ----------- |
| Enumerated candidates are **not** self-trusted | every candidate flows through `vet_candidate` identically to LLM candidates — no internal bypass |
| `allow_roots` is an exact closed set of leaf dirs | `build_tree_roots` unions only the specific `self-deploy-target` dir; debug-assert + test forbid bare `$HOME`/`state_root` |
| Snapshot/backup/corrupt dirs are never touched here | owned solely by `MaintenanceThread`; disjoint-by-directory ownership; enumerator excludes those prefixes and the guard rejects them |
| Threshold misconfig cannot purge | `0`/empty/invalid clamps to safe floor; dedicated test |
| Live state is never proposed | `cognitive`/`.wal`/`.shadow` + all snapshot/backup/corrupt prefixes excluded at enumeration **and** rejected by the guard |
| TOCTOU-safe removal | executor re-canonicalizes + re-asserts under-allow-root, rejects leading-dash/non-UTF-8 paths, uses `--` separator and `env_clear()`ed git — all unchanged |
| Additive & non-breaking | new module + two new env knobs only; all existing `SIMARD_DISK_RECLAIM_*` and `SIMARD_MAINTENANCE_*` knobs, dry-run defaults, and the `DAEMON_APPLY` gate preserved |

## Test coverage

| Area | Proves |
| ---- | ------ |
| `build_tree_roots` | returns the specific `self-deploy-target` leaf dir; **never** bare `$HOME`/`state_root` |
| build-tree enumeration | idle trees proposed; live-PID trees withheld; sub-idle-window trees withheld |
| worktree enumeration | idle worktrees proposed; still routed through dirty/unpushed/unknown-PR vetoes |
| maintenance-ownership boundary | snapshot/backup/corrupt prefixes are **never** enumerated by `reclaimable_targets`, and rejected by the guard if injected |
| protected-state rejection | `cognitive`/`.wal`/`.shadow` never enumerated and rejected if injected |
| threshold clamping | `BUILD_IDLE_DAYS=0` / `WORKTREE_IDLE_DAYS=0` / empty / non-numeric → safe floor, never purge |
| routine regression | under the stale-artifact scenario `bytes_freed > 0` / `paths_removed > 0` deterministically |
| emergency parity | `emergency_cleanup` frees the shared set incl. idle `self-deploy-target/`, retaining all prior removals |
| enumerated-still-vetted | an enumerated candidate targeting a protected path is skipped |

Existing safety/override/parsing tests pass unchanged. Tests reuse the existing
seams (`ScriptedDisk`, `RecordingRemover`, `AllowAllWtProbe`, `MapMeasurer`,
`FakeLiveProcessProbe`, `Harness`).

## Related

- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale, "agent proposes, Rust disposes", and the deterministic floor
- [Configure disk reclamation (how-to)](../howto/configure-disk-reclamation.md) — operator usage, CLI, env config
- [Disk reclaim API (reference)](./disk-reclaim-api.md) — module API, the guard, the executor, the recipe contract
- [Disk reclaim telemetry (reference)](./disk-reclaim-telemetry.md) — emitted metrics
- [Worktree reaping safety guards (reference)](./engineer-worktree-sweep-safety.md) — the liveness / uncommitted-work primitives the guard composes
