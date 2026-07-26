---
title: Agentic disk reclamation
description: Design rationale for Simard's fully agentic disk-reclamation capability — why the reclaim agent proposes candidates while a deterministic Rust executor disposes of them, the non-bypassable protected-path rails, and how the capability self-heals disk pressure without per-cycle hand-crafted cleanup heuristics.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../howto/configure-disk-reclamation.md
  - ../reference/disk-reclaim-api.md
  - ../reference/disk-reclaim-build-cache-producer.md
  - ../reference/disk-reclaim-telemetry.md
  - ../reference/engineer-worktree-sweep-safety.md
  - ./automated-disk-health.md
---

# Agentic disk reclamation

Simard reclaims disk end-to-end as a durable, agentic capability. Each run an
agent inspects live host state — `df`, `git worktree list` across every managed
repo, open/merged/closed PR status, the running-process table, and directory
sizes — reasons about what is safely reclaimable, and reclaims largest-first
until the partition is back under a configurable threshold. The capability
lives in one recipe plus one Rust module, so cleanup logic is never re-derived
in scheduler prompts again.

This document explains **why** the capability is shaped the way it is. For
operator usage see [Configure disk reclamation](../howto/configure-disk-reclamation.md);
for the module API see [Disk reclaim API](../reference/disk-reclaim-api.md).

## The problem: context-dependent cleanup, catastrophic blast radius

What is safe to reclaim is **context-dependent** every single run:

- which worktrees map to merged or closed PRs (reclaimable),
- which are active recipe or Simard-engineer builds (must keep),
- which are orphaned, de-registered leftover directories (reclaimable),
- which build caches are stale versus warm.

The previous approach re-derived this logic imperatively in per-cycle scheduler
prompts as an ad-hoc "disk guard." Those heuristics **misfired**. The worst was
a *merge-base-is-ancestor* rule that treated "this worktree's merge-base is an
ancestor of `main`" as "already merged, safe to delete" — which is **true of
every freshly-created worktree**, including one carrying active, unpushed work.
It deleted active caches and in-use checkouts.

An imperative rule cannot enumerate every edge case ahead of time. An agent
that inspects current state and reasons each run adapts to novel situations
without pre-coding every case. So selection **should** be agentic.

But agentic selection collides with an absolute safety requirement: an LLM
reasoning error must **never** be able to delete a protected path (deleting the
daemon's working directory crash-loops it with `status=200/CHDIR`; deleting an
unpushed worktree loses work). "Fully agentic" and "cannot delete a protected
path" appear to conflict.

## The core resolution: the agent proposes, Rust disposes

The tension resolves by **splitting selection from execution**:

```
┌─────────────────────────────────────────────────────────────┐
│ Recipe agent (disk-reclaim.yaml) — ANALYSIS ONLY            │
│   inspects df / git worktree list / gh pr / /proc / du      │
│   REASONS about what is reclaimable, largest-first          │
│   EMITS a candidate list (JSON) — proposes, never deletes   │
└───────────────────────────┬─────────────────────────────────┘
                            │  candidate JSON
                            ▼
┌─────────────────────────────────────────────────────────────┐
│ Rust executor (src/disk_reclaim/) — DISPOSES                │
│   re-validates EVERY candidate through the deterministic    │
│   protected-path guard immediately before deletion          │
│   performs prune → worktree remove --force → rm -rf         │
│   stops once under target %-used                            │
└─────────────────────────────────────────────────────────────┘
```

The agent **only proposes** a candidate list. The recipe prompt forbids
destructive shell commands, and the delete syscalls live *only* in the Rust
executor. The executor re-derives every safety signal live and vets each
candidate through a deterministic filter (`guard::vet_candidate`) at the moment
before deletion. **The agent proposes; Rust disposes.**

This split makes selection genuinely agentic (the agent decides *what* to
nominate and in *what order*) while safety is genuinely deterministic (Rust
decides *whether* any nominee is actually removed, and can only ever narrow the
set).

The guard is **authoritative for the disposal path**: nothing the executor
removes bypasses it. But the guard only governs the delete primitive it owns —
it cannot govern a shell the *agent* holds. Today the "agent cannot delete a
protected path" property holds because the analysis recipe step is
**analysis-only** (its prompt forbids destructive commands and it emits candidate
markers, nothing more) and the executor owns the *only* delete primitive,
re-vetting every candidate through the guard. A prompt that merely *asks* the
agent not to run `rm` is necessary but not sufficient on its own; hardening the
recipe step with OS-level confinement — scrubbing *mutating* binaries (`rm`,
`find -delete`/`-exec`, `git worktree remove`) from `PATH`, running the analysis
under a read-only/seccomp confinement, and a post-run reconciliation diff that
asserts nothing was removed outside the executor's guarded path — is a planned
follow-up, **not yet wired in**. See
[Recipe-step sandboxing](../reference/disk-reclaim-api.md#recipe-step-sandboxing).

## The hard rails (deterministic, non-bypassable)

The guard is a **pre-flight filter the agentic step cannot bypass**. Before any
removal, every candidate is independently re-validated. A candidate is
**rejected and routed to the human-review list** — never deleted, never
silently retried — if *any* of these holds:

| Rail | Rejects when… | Why |
| ---- | ------------- | --- |
| **Protected path** | path is `worktrees/main` or any resolved daemon `WorkingDirectory` | deleting it crash-loops the daemon (`200/CHDIR`) |
| **Live process** | path is referenced by any live PID (`/proc/<pid>/cwd` at/under it) | in use right now |
| **Uncommitted / unpushed** | worktree has dirty tree or commits not in a merged/closed PR | would lose work |
| **Active worktree** | an active recipe or engineer worktree (tmux/PID) owns it | still running |
| **Outside allow-root** | path is not under an allow-root, or canonicalization/symlink check fails | out of scope / unsafe |
| **Unknown PR state** | the agent could not positively classify the mapped PR as merged/closed | no guessing on destructive actions |

Only three candidate classes can *ever* be reclaimed, and only after passing
**all** rails:

1. **Tracked worktrees** whose PR is MERGED or CLOSED and which are idle,
2. **Orphaned, de-registered** (untracked) worktree directories,
3. **Stale build caches** (`target/` and shared cargo target dirs).

### Fail-closed, everywhere

Every inconclusive or unknown signal resolves to *keep*. Inconclusive `gh`
output is not "probably safe" — it is `UnknownPrState → reject → human review`.
A canonicalize failure is not "assume fine" — it is `reject`. There is no
override flag, no `--force` that skips the guard, and no silent fallback. This
directly encodes the requirement: *when uncertain about a path, do not delete
it; report it for human review.*

### Defense-in-depth on the protected set

The protected deny-set is the **union** of:

- the hardcoded `/home/azureuser/src/Simard/worktrees/main`,
- the **runtime-resolved** daemon working directory (read from the live
  `simard-ooda` process cwd via `/proc`, the pidfile, and the service file's
  `WorkingDirectory=`),
- `maintenance::protected_paths()` (bare repos, git common dirs),
- any repos named in `SIMARD_GIT_PROTECTED_REPOS`.

Hardcoding *and* runtime-resolving the daemon directory is deliberate: if the
service is ever relocated, the union still protects the live directory and the
canonical one, so the crash-loop cannot happen either way.

### TOCTOU-safe by construction

No verdict is ever persisted. There is no database of "approved" deletions to
go stale. Every signal — live PID, uncommitted/unpushed state, PR status,
allow-root containment — is re-derived at the syscall boundary immediately
before the `remove`. Between the agent's proposal and the executor's delete,
state can change freely; the executor re-checks and skips anything that has
become unsafe.

## Self-healing trigger

A cheap, deterministic probe runs in the OODA/overseer maintenance path: once
per maintenance cadence the daemon reads `df` `%-used` for the home partition.
When usage exceeds `SIMARD_DISK_RECLAIM_PCT` (default `85`), it launches the
agentic reclaim capability. Simard notices pressure, reasons about what to
reclaim under the same hard rails, and emits telemetry for what it found. The
trigger is a *probe*, not a heuristic — it only decides *whether to run the
agent*, never *what to delete*.

### Ships dry-run + human-review first

The daemon trigger runs the capability in **dry-run + human-review mode by
default**. It performs the full analysis and guard vetting and records what it
*would* reclaim, but makes **zero destructive changes** until an operator
explicitly opts in by setting `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`.

This is deliberate and matches the capability's highest-priority risk: the
elegant "agent proposes, Rust disposes" guarantee is only as strong as the
analysis recipe step being confined so it cannot open a second, unguarded
deletion path. OS-level
[recipe-step sandboxing](../reference/disk-reclaim-api.md#recipe-step-sandboxing)
is a planned follow-up that is **not yet wired in**. Until it lands, automatic
deletion stays off; the daemon surfaces the would-remove set and the human-review
list, and an operator can reclaim by hand with `simard disk-reclaim --apply`. Once
that confinement is implemented, flipping `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`
promotes the daemon to closed-loop self-healing. `emergency_cleanup` (below)
remains the deterministic hard stop regardless of this flag.

## Layered defense

Agentic reclamation layers on top of the existing disk mechanisms rather than
replacing the hard stops. Terminology follows the code: within the daemon's
per-cadence **disk-health maintenance block** (`daemon/mod.rs`), two tiers run
in a fixed order every tick, and several independent mechanisms surround it.

Inside the maintenance block, in execution order:

```
Tier 1: emergency_cleanup   (deterministic, no LLM, no recipe)
        ↓ Runs FIRST every tick. At severe pressure (~>=95%) it frees space
          with hardcoded actions — never depends on spawning an agent or gh.
Tier 2: agentic disk-reclaim (run_disk_reclaim, repointed from the old
                              recipe-based disk-health check)
        ↓ Agent proposes candidates → Rust guard disposes, largest-first.
          Dry-run + human-review by default (SIMARD_DISK_RECLAIM_DAEMON_APPLY).
```

Surrounding, independent mechanisms:

```
.cargo/config.toml shared target dir
        ↓ Prevents per-worktree target-dir sprawl (structural, always on).
disk_pressure module (per OODA cycle, at engineer admission)
        ↓ Free-GiB hard stop; defers/blocks engineer spawn when critical.
worktree_gc / engineer sweep + EngineerWorktree RAII
        ↓ Operator GC and per-engineer deterministic cleanup.
```

`emergency_cleanup` runs **first**, not last: it is the deterministic Tier-1
safety net precisely because, at ~100% disk, the host may be unable to spawn an
agent or run `gh`. The self-healing agentic Tier 2 must therefore never be the
*only* thing standing between Simard and `ENOSPC`. The two tiers are
complementary — Tier 1 guarantees a floor of deterministic relief, Tier 2 adds
adaptive, largest-first reclamation on top.

> **Note:** the older [automated disk-health](./automated-disk-health.md)
> document described a `Layer 0–4` scheme for the same stack; that scheme is
> superseded by this Tier terminology, which mirrors the code.

## Relationship to the prior disk-health check

Agentic disk reclamation **supersedes** the per-cycle
[automated disk-health check](./automated-disk-health.md) as the primary
agentic cleanup capability. The prior recipe cleaned a fixed target list
(engineer worktrees, cargo dirs, backups) with the agent choosing aggressiveness
but not *which repos* to inspect. The reclaim capability broadens scope to all
managed repos plus `~/.simard` engineer worktrees, adds PR-status and
running-process reasoning, and — critically — moves the delete primitive out of
the agent's hands and behind the deterministic guard. The two-layer output
contract (JSON envelope → text markers) and the no-fallback error path are
reused unchanged.

## Why this is the right shape

1. **Adaptive selection.** New situations (a novel orphaned-dir layout, a PR
   state the old rule never modeled) are handled by reasoning, not by shipping
   a new heuristic.
2. **Bounded blast radius.** The guard can only ever *shrink* the candidate set.
   No LLM output can widen it past the rails.
3. **Durable capability.** Cleanup logic lives in one recipe + one module, not
   in scheduler prompts. It is inspectable (`cat` the YAML), testable (the
   rails have hermetic refusal proofs), and observable (telemetry per run).
4. **Self-healing.** A cheap probe closes the loop so disk pressure resolves
   without an operator.

## Related

- [Configure disk reclamation (how-to)](../howto/configure-disk-reclamation.md) — operator guide, CLI, env config
- [Disk reclaim API (reference)](../reference/disk-reclaim-api.md) — module API, the guard, the executor, the recipe contract
- [Disk reclaim build-cache producer (reference)](../reference/disk-reclaim-build-cache-producer.md) — the deterministic sub-artifact `StaleBuildCache` producer that lets routine reclaim hold steady-state disk below the emergency threshold
- [Disk reclaim telemetry (reference)](../reference/disk-reclaim-telemetry.md) — emitted metrics
- [Worktree reaping safety guards](../reference/engineer-worktree-sweep-safety.md) — the shared liveness/uncommitted-work primitives the guard composes
- [Automated disk health (concept)](./automated-disk-health.md) — the superseded per-cycle check
