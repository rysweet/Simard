---
title: Automated disk health management
description: Design rationale for Simard's per-cycle disk health check — why it exists, what it cleans, and how it interacts with existing subsystems.
last_updated: 2026-05-24
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/disk-health-api.md
  - ../howto/configure-disk-health-check.md
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

### Agent-driven, not hardcoded

The cleanup logic is a recipe YAML with an **agent step** — an LLM that
receives a prompt describing the disk situation and uses bash tools to
inspect and clean. This replaced the prior v1.0.0 bash-script approach
(issue #2051) because hardcoded `find`/`rm` commands were brittle: they
couldn't adapt to unexpected directory layouts, couldn't reason about which
artifacts are most valuable to keep, and required YAML edits for any
threshold change.

With the agent step:

- **Adaptive cleanup.** The agent decides what to clean and how aggressively
  based on current disk pressure. It can prioritize by size, skip directories
  that look unusual, and escalate cleanup intensity if the first pass doesn't
  free enough space. No hardcoded thresholds to maintain.
- **Hot-reloadable prompt.** Operators can edit the prompt text in the recipe
  YAML to change cleanup guidance — no rebuild, no restart. The daemon
  re-reads the recipe each cycle.
- **Inspectable and auditable.** The recipe YAML is a readable prompt, not
  compiled into the binary. Operators can `cat` it to see exactly what
  guidance the agent receives. Each cleanup run emits `ACTION:` lines
  describing what was done.
- **Consistent with Simard's architecture.** Simard's design philosophy is
  recipes for policy, Rust for machinery. The disk health check follows this
  pattern exactly — the recipe prompt defines *what to consider* and *how
  to reason*, the Rust shim handles *how to invoke* and *where to log*.

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

If disk pressure is severe, operators can adjust the guidance in the recipe
prompt — for example, changing "older than 24 hours" to "older than 4 hours"
in the worktree cleanup section of the prompt text.

### TOCTOU in age-based deletion

There is a time-of-check-to-time-of-use window between stat'ing a worktree's
mtime and deleting it. An engineer could theoretically start using a worktree
in that window. The `.simard-engineer-claim` lockfile check mitigates this —
a newly-started engineer writes the claim before touching the worktree. The
residual TOCTOU window (between claim creation and the health check's stat)
is sub-second and matches the accepted risk in `sweep_orphaned_worktrees`.

## Why an agent step and not pure Rust or hardcoded bash

The cleanup logic could be written entirely in Rust, or as a deterministic
bash script (as it was in v1.0.0). We chose an agent step because:

1. **Cleanup requires judgment.** Not all stale-looking worktrees are safe
   to delete. Not all build caches are equally expendable. An agent can
   inspect the situation (disk pressure, directory sizes, claim file PIDs)
   and make proportional decisions — clean lightly at 81%, aggressively
   at 95%.

2. **Hardcoded scripts are brittle.** The v1.0.0 bash script had 8
   `find`/`rm` pipelines with specific `-maxdepth`, `-mmin`, and
   `-print0` invocations. Any change to the directory layout required
   editing the script. The agent reads the prompt guidance and adapts.

3. **The prompt is the policy.** Operators can edit the prompt to change
   cleanup priorities without understanding bash pipeline syntax. "Keep
   approximately 10 backups instead of 5" is a prose edit, not a
   `head -z -n -"$BACKUP_KEEP"` pipeline change.

4. **Consistency.** Simard already uses agent-step recipes for merge
   readiness judgement (`merge-readiness-judge.yaml`) and progress
   assessment (`progress-assessment.yaml`). Disk health now follows
   the same pattern.

The Rust code remains a thin shim. The agent uses bash tools (via its own
tool-use capability) to run `df`, `find`, `du`, `rm` — but the *logic*
of what to clean and how aggressively is agentic, not scripted.

The agent outputs key=value text markers to stdout (`DISK_USED_PCT=N`,
`FREED_BYTES=N`, `ACTION: ...`) — the Rust shim parses these with simple
string splitting. No JSON, no serde deserialization of recipe output.

## Related

- [Disk health API reference](../reference/disk-health-api.md) — full API surface
- [Configure disk health check (how-to)](../howto/configure-disk-health-check.md) — operator guide
- [Per-Engineer Worktree Isolation](../reference/engineer-worktree-isolation.md) — worktree lifecycle
- [Daemon mode](../daemon-mode.md) — OODA cycle overview
