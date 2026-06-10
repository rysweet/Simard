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
  - ./goal-board-persistence.md
---

# Automated disk health management

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

### Two-tier cleanup: deterministic first, then intelligent

The disk health module runs two tiers of cleanup, in strict order, each
OODA cycle:

**Tier 1 — Deterministic emergency cleanup (≥95% disk)**

Pure Rust. No LLM, no recipe, no external process beyond `df`/`du`. When
disk usage is critically high (≥95%), `emergency_cleanup()` immediately
deletes known-safe regenerable artifacts:

| Target                              | Why safe to delete                                 |
| ----------------------------------- | -------------------------------------------------- |
| `repo_root/target/debug/`           | Main build cache — `cargo build` regenerates       |
| `repo_root/target/llvm-cov-target/` | Coverage artifacts — `cargo llvm-cov` regenerates  |
| `worktrees/*/target/`               | Engineer worktree build caches — all regenerable   |
| `state_root/cargo-target/`          | Legacy shared target — cold rebuild on next build  |
| `state_root/shared-target/`         | Current shared target — cold rebuild on next build |
| `state_root/backups/*` (beyond 2)   | Stale LadybugDB backups — keeps 2 most recent      |

This tier exists because the recipe tier (Tier 2) needs disk space to spawn
an LLM agent process. At 100% disk, the recipe deadlocks — it can't write
a temp file or spawn a process. The deterministic Rust cleanup runs first
to ensure there's enough headroom for the agent to start.

Failures in Tier 1 are silent per-item (`is_ok()` guards) — if one
`remove_dir_all` fails, the rest still attempt. The function returns a
`DiskHealthReport` with what was actually freed.

**Tier 2 — Recipe-based LLM cleanup (≥80% disk)**

After Tier 1 runs (or skips because disk is below 95%), the daemon invokes
the recipe-based cleanup (`run_disk_health_check()`). This tier uses an LLM
agent that can make nuanced decisions — for example, selectively removing
only the oldest worktrees, or prioritizing by size.

The recipe tier is more capable but less reliable:
- Requires `recipe-runner-rs` on PATH
- Requires the recipe YAML to exist
- Depends on LLM availability and correct output parsing
- Needs disk space to spawn the agent process

This is why Tier 1 is the *primary* defense: it always works (it's compiled
Rust with no external dependencies beyond basic coreutils), and it creates
the headroom Tier 2 needs to function.

### Layered defense

The disk health system does not replace existing mechanisms — it layers on
top of them:

```
Layer 0: .cargo/config.toml shared target dir
         ↓ Prevents per-worktree target dir creation
Layer 1: emergency_cleanup() — deterministic Rust (≥95%, per-cycle)
         ↓ Hard-coded artifact deletion, no LLM needed
Layer 2: disk_health recipe — LLM agent (≥80%, per-cycle)
         ↓ Intelligent, adaptive cleanup with nuanced decisions
Layer 3: disk_pressure module (per-cycle)
         ↓ Hard stop at critical thresholds, prevents engineer spawn
Layer 4: sweep_orphaned_worktrees (boot-time)
         ↓ Catches orphans from prior crashes
Layer 5: EngineerWorktree RAII cleanup (per-engineer)
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
| Main repo `target/` (full)    | Tier 1 cleans `target/debug/` and `target/llvm-cov-target/` subdirs, but does not wipe the entire `target/` directory — release artifacts may be in use. For a full wipe, use manual `reclaim-build-space`. |
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

## Why both Rust and a recipe

The critical-path cleanup (≥95%) is pure Rust — no external dependencies,
no failure modes beyond filesystem errors. This is the deterministic
backstop that prevents ENOSPC deadlock.

The moderate-pressure cleanup (≥80%) uses a recipe with an LLM agent
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

The Rust code serves two roles: the `emergency_cleanup()` function is a
full cleanup implementation (not a shim), while `run_disk_health_check()`
is a thin shim that delegates to the recipe. The recipe's cleanup prompt
lives in the YAML as a readable agent step, not compiled into the binary.
Operators can `cat` it, `diff` it, or review the agent's decisions in logs.

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
