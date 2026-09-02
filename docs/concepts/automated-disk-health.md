---
title: Automated disk health management
description: Design rationale for Simard's per-cycle disk health check — why it exists, what it cleans, and how it interacts with existing subsystems.
last_updated: 2026-06-05
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../howto/configure-disk-health-check.md
  - ../reference/disk-health-api.md
  - ../reference/engineer-worktree-isolation.md
  - ./agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ./goal-board-persistence.md
---

# Automated disk health management

> **Superseded (2026-07-07).** The primary agentic disk-cleanup capability is now
> [Agentic disk reclamation](./agentic-disk-reclamation.md) — an agent *proposes*
> reclaimable candidates and a deterministic Rust executor *disposes* of them
> behind non-bypassable safety rails. It broadens scope to all managed repos plus
> `~/.simard` engineer worktrees, adds PR-status and running-process reasoning,
> and — critically — moves the delete primitive out of the agent's hands. The
> per-cycle check described below is retained for historical context; new
> operators should use [Configure disk reclamation](../howto/configure-disk-reclamation.md).
> The ad-hoc **merge-base-is-ancestor** "already merged, safe to delete"
> heuristic and manual `find … -mtime +1 -exec rm -rf` runbooks are **deprecated**
> (the former misfired on every fresh worktree; see the reclamation concept doc).

On 2026-05-24, Simard crashed from `ENOSPC` (No space left on device). The
`/home` partition was 100% full: 373G used on a 393G disk. Post-mortem
identified three root causes, each a slow accumulation that existing cleanup
mechanisms did not address aggressively enough.

This document explains the problem, the design of the fix, and the tradeoffs.

## The three root causes

### 1. Stale engineer worktrees — 50G

Engineer worktrees accumulate at `~/.simard/engineer-worktrees/`. The existing
`sweep_orphaned_worktrees` runs once at daemon startup, but between startups,
worktrees from crashed or abandoned engineers pile up. In the crash incident,
48 stale worktrees consumed 50G — engineers that ran, completed or failed,
but whose worktrees were never cleaned because the daemon didn't restart.

### 2. Cargo build artifacts — 206G total

Three independent cargo target directories were growing without bounds:

| Path                               | Size  | Source                                       |
| ---------------------------------- | ----- | -------------------------------------------- |
| Main worktree `target/`            | 191G  | Incremental + debug builds from engineers    |
| `~/.simard/cargo-target/`          | 12G   | Older shared target from pre-config.toml era |
| `~/.simard/shared-target/`         | 2.8G  | Current shared target                        |

Each engineer worktree that didn't use `CARGO_TARGET_DIR` created its own
multi-GB `target/` directory. Even with `CARGO_TARGET_DIR` set, incremental
build state and debug symbols grow monotonically.

### 3. LadybugDB backups — 639M and growing

LadybugDB creates a backup every 5 minutes under `~/.simard/backups/`. No
rotation policy existed. At the time of the crash, 24 backup files had
accumulated. While 639M is small relative to the other causes, unbounded
growth is the pattern that matters — left unchecked, this would eventually
contribute to exhaustion.

## Design principles

### Recipe-driven, not hardcoded

The cleanup logic is a recipe YAML with an agent step, not compiled Rust. This
means:

- **Hot-reloadable policy.** Operators can change the cleanup guidelines,
  threshold language, or target priorities by editing the YAML prompt. No
  rebuild, no restart. The daemon re-reads the recipe each cycle.
- **Inspectable and auditable.** The cleanup prompt is a readable YAML file,
  not compiled into the binary. Operators can `cat` it, `diff` it, or review
  the agent's reasoning in logs.
- **Consistent with Simard's architecture.** Simard's design philosophy is
  recipes for policy, Rust for machinery. The disk health check follows this
  pattern exactly — the recipe defines *what* to clean and *when*, the Rust
  shim handles *how to invoke* and *where to log*.

### Pre-emptive, not reactive

The check runs **every cycle**, not just at startup. The existing
`sweep_orphaned_worktrees` only runs at boot — useless for a daemon that runs
for days between restarts. The disk health check catches accumulation
continuously.

The 80% threshold provides a 20% buffer. On a 393G partition, that's ~79G of
headroom after cleanup — enough for several concurrent engineer builds plus
incremental compilation.

### Warn-and-continue, not block-and-fail

A failure in the disk health check never blocks the OODA cycle. The rationale:

1. The disk health check is a *best-effort optimization*. The existing
   `disk_pressure` module provides the hard stop when disk is truly critical.
2. If `recipe-runner-rs` is not installed or the recipe YAML is missing, the
   daemon should still function — just without proactive cleanup.
3. A flaky filesystem stat or a transient permission error should not prevent
   Simard from doing useful work.

The tradeoff is that a persistently broken health check degrades silently to
the `disk_pressure` hard-stop behavior. The warning in `ooda.log` (under
`$SIMARD_STATE_ROOT`) is the operator's signal to investigate.

### Layered defense

The disk health system does not replace existing mechanisms — it layers on
top of them:

```
Layer 0: .cargo/config.toml shared target dir
         ↓ Prevents per-worktree target dir creation
Layer 1: disk_health recipe (per-cycle)
         ↓ Proactive cleanup at 80% usage
Layer 2: disk_pressure module (per-cycle)
         ↓ Hard stop at critical thresholds, prevents engineer spawn
Layer 3: sweep_orphaned_worktrees (boot-time)
         ↓ Catches orphans from prior crashes
Layer 4: EngineerWorktree RAII cleanup (per-engineer)
         ↓ Deterministic cleanup on normal exit
```

Each layer catches what the layer above missed. No single layer is
sufficient alone.

## What it cleans and what it doesn't

### Cleaned automatically

| Target                               | Condition                                    | Impact if removed           |
| ------------------------------------ | -------------------------------------------- | --------------------------- |
| Engineer worktrees > 24h old         | No `.simard-engineer-claim` active process   | None — engineer is dead     |
| `target/` in surviving worktrees     | Always (when cleanup triggers)               | Engineer rebuilds (~10 min) |
| LadybugDB backups beyond 5 most recent | Always (when cleanup triggers)             | Reduced rollback window     |
| `~/.simard/cargo-target/` contents   | Always (when cleanup triggers)               | Next build is cold          |
| `~/.simard/shared-target/` contents  | Always (when cleanup triggers)               | Next build is cold          |

### Not cleaned (by design)

| Target                        | Why not                                                         |
| ----------------------------- | --------------------------------------------------------------- |
| Main repo `target/`           | May be actively used by operator; manual `reclaim-build-space`  |
| Active engineer worktrees     | Still running; claim file present                               |
| Worktrees < 24h old           | May be in use; conservative age threshold                       |
| Git objects (`.git/objects/`)  | Shared across all worktrees via git's alternates                |
| Log files (`~/.simard/logs/`) | Needed for diagnostics; small relative to build artifacts       |

## Tradeoffs

### Shared cargo target serializes concurrent builds

With `.cargo/config.toml` pointing all worktrees to one target directory,
concurrent `cargo build` invocations serialize on Cargo's file lock. This
slows parallel engineer builds compared to per-worktree targets.

The tradeoff is acceptable: the 191G saved outweighs the build-time cost,
and the daemon typically runs one engineer at a time. The lock is Cargo's
built-in `flock` mechanism — no custom locking needed.

### Backup retention of 5 reduces rollback window

At a 5-minute backup interval, keeping 5 backups provides only 25 minutes
of rollback coverage. The prior unlimited retention covered the entire daemon
uptime.

25 minutes is sufficient for the operational scenarios where backup restore
is needed (goal board corruption, meeting record loss). Extended rollback
needs are better served by explicit snapshots or database-level recovery.

### 24h worktree age is conservative

Most engineers complete in under 2 hours. A 24h age threshold means worktrees
from stuck-but-not-crashed engineers survive for a full day. This is
deliberate — we'd rather waste 1G of disk per stale worktree for 24 hours
than risk deleting a worktree that's genuinely still making progress.

If disk pressure is severe, operators can lower the threshold in the recipe:

```yaml
env:
  WORKTREE_MAX_AGE_H: "4"
```

### TOCTOU in age-based deletion

There is a time-of-check-to-time-of-use window between stat'ing a worktree's
mtime and deleting it. An engineer could theoretically start using a worktree
in that window. The `.simard-engineer-claim` lockfile check mitigates this —
a newly-started engineer writes the claim before touching the worktree. The
residual TOCTOU window (between claim creation and the health check's stat)
is sub-second and matches the accepted risk in `sweep_orphaned_worktrees`.

## Why a recipe and not pure Rust

The cleanup logic could be written entirely in Rust. We chose a recipe
because:

1. **Thresholds change.** The 80% trigger, 24h age, and 5-backup retention
   are operational knobs that operators should be able to change without
   rebuilding Simard. A YAML file is editable; compiled Rust is not.

2. **Agent-driven cleanup is adaptive.** The agent uses `df`, `find`, and
   `rm` via bash tools, but the *logic* of what to clean and how aggressively
   is agentic — it adapts to disk pressure level rather than following a
   hardcoded script.

3. **Recipes are inspectable.** An operator debugging disk issues can
   `cat` the recipe YAML and see exactly what the agent is instructed to
   do. The agent's reasoning is logged for auditability.

4. **Consistency.** Simard already uses recipes for merge readiness
   judgement, progress checking, and other policy decisions. Disk health
   follows the same pattern.

The Rust code is a thin shim. The cleanup prompt lives in the recipe YAML as
a readable agent step, not compiled into the binary. Operators can `cat` it,
`diff` it, or review the agent's decisions in logs.

### Two-layer output: JSON envelope → text markers

The agent step outputs key=value text markers to stdout (`DISK_USED_PCT=N`,
`FREED_BYTES=N`, `ACTION: ...`). However, these markers are embedded in the
agent's conversational output — the step may also contain LLM reasoning,
`df` output, and other noise.

The Rust shim does not read `recipe-runner-rs` text-format stdout directly.
Text-format stdout only contains the recipe summary line (e.g.,
`Recipe: disk-health-check SUCCESS`), not the agent step output where the
markers live.

Instead, the shim passes `--output-format json` to `recipe-runner-rs`, which
wraps each step's output in a structured JSON envelope:

```json
{
  "success": true,
  "step_results": [
    {
      "step_id": "check-disk-usage",
      "output": "Running df -h /home...\nDISK_USED_PCT=72\nFREED_BYTES=0\n..."
    }
  ]
}
```

The shim deserializes the envelope into `RecipeOutput` / `StepResult` structs
via serde, extracts `step_results[0].output`, and feeds that string to
`parse_disk_health_text()` — the same simple key=value line parser. This
two-layer approach keeps the recipe output format simple (text markers from
bash/agent) while giving the Rust shim reliable access to each step's actual
output.

> **Historical note (issue #2212):** Prior to this fix, the shim read
> `recipe-runner-rs` text-format stdout directly. That stream only contained
> the summary line, not the step output. `parse_disk_health_text()` never found
> `DISK_USED_PCT` in the summary line, causing 946 consecutive false failures
> and allowing the disk to fill to 100%. The fix was surgical: add
> `--output-format json`, deserialize the envelope, extract step output.

## Related

- [Configure disk health check (how-to)](../howto/configure-disk-health-check.md) — operator guide
- [Disk health API (reference)](../reference/disk-health-api.md) — module API, structs, data flow
- [Per-Engineer Worktree Isolation](../reference/engineer-worktree-isolation.md) — worktree lifecycle
- [Daemon mode](../daemon-mode.md) — OODA cycle overview
