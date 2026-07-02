---
title: Monthly self-quality-audit
description: Design rationale for Simard's recurring ~monthly self-quality-audit — why a periodic daemon task (not a standing goal or per-cycle hook), why last-run is persisted to disk (wall-clock epoch) instead of the in-memory Instant the other periodic tasks use, the five-wave SEEK→VALIDATE→FIX quality-audit model run against Simard's OWN code, the crusty-old-engineer proxy-review gate, the bounded review loop, and the fail-open safety model.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: explanation
related:
  - ../reference/self-quality-audit-api.md
  - ../howto/configure-self-quality-audit.md
  - ./brain-introspection.md
  - ../concepts/automated-disk-health.md
---

# Monthly self-quality-audit

> Follows the proven periodic-task pattern shipped for brain introspection
> ([#2419](https://github.com/rysweet/Simard/issues/2419)). New in this feature:
> **restart-surviving last-run persistence** so a ~30-day cadence actually fires
> ~monthly instead of on every daemon restart.

Simard runs a **recurring self-quality-audit** on its own periodic cadence
(default ~monthly). Each run performs a **five-wave** SEEK→VALIDATE→FIX
quality-audit of Simard's *own* code (`rysweet/Simard`) by invoking the
amplihack `quality-audit` skill, then — for every pull request the waves
produce — invokes the `crusty-old-engineer` skill as operator Ryan's **proxy
reviewer**, looping on each PR until crusty is satisfied, and finally
self-merges the PRs that are both crusty-approved and CI-green.

This page explains *why* the feature is built the way it is. For the executable
contract (APIs, structs, markers, config) see the
[Self-quality-audit API](../reference/self-quality-audit-api.md). For operator
tasks see [Configure the monthly self-quality-audit](../howto/configure-self-quality-audit.md).

## What problem this solves

Simard continuously ships engineer PRs against her own repository, but nothing
periodically steps back to audit the *accumulated* state of her own code at a
low frequency: dead code paths, drifted docs, silent-degradation smells,
untested branches, and structural rot that per-PR review does not catch. The
existing periodic tasks each cover a narrow operational surface:

| Periodic task | Cadence | Scope |
| --- | --- | ---: |
| Verified backup (#2420) | minutes–hours | cognitive store durability |
| Disk health | per-cycle | free disk before `ENOSPC` |
| Worktree sweep (#2167) | minutes | reap orphaned engineer worktrees |
| Brain introspection (#2419) | daily | brain decision quality + memory hygiene |
| **Self-quality-audit (this)** | **~monthly** | **audit Simard's own source, gated by a proxy human review** |

The self-quality-audit is the lowest-frequency, highest-level layer: a
standing, self-directed code review of the whole repository that produces real,
crusty-gated, self-merged fixes on a monthly rhythm.

## Why a periodic daemon task (the cadence decision)

The goal says "recurring MONTHLY." Three mechanisms were considered — the same
trade-off the brain-introspection pass resolved:

| Option | Verdict | Why |
| --- | --- | --- |
| **(a) Periodic daemon task on its own interval** | **Chosen** | Reuses the proven, tested `operator_commands_ooda/daemon/mod.rs` periodic-task hook (verified-backup / disk-health / worktree-sweep / brain-introspection). Deterministic cadence, minimal testable Rust, fail-open. |
| (b) Standing low-priority OODA goal | Rejected as the *cadence* source | A low-priority goal can starve indefinitely behind higher-value goals — wrong for a hygiene cadence that must run on schedule. |
| (c) A recipe the brain invokes ad hoc | Rejected as the *cadence* source | No schedule guarantee; the brain may never choose it. |

As with brain introspection, the daemon owns the clock and the recipe owns the
judgment. The daemon task **dispatches** the
`monthly-self-quality-audit` recipe as a `recipe-runner-rs` subprocess for the
agentic five-wave audit + crusty review; the Rust hook only decides *when* and
logs *what happened*.

## The one novel element: restart-surviving last-run persistence

Every existing periodic task gates on an **in-memory `Instant`**:

```rust
// disk-health, worktree-sweep, brain-introspection — all reset on restart
let mut last_brain_introspection = Instant::now();
if last_brain_introspection.elapsed() >= interval { … }
```

That is correct for 15-minute and 24-hour cadences — losing at most one
interval across a restart is harmless. It is **wrong for a ~30-day cadence**:
a daemon that restarts weekly (deploys, crashes, reboots) would *never* let a
monthly `Instant` mature, so the audit would fire on essentially every restart —
or never, depending on which side of the boundary the restart lands. Either way
the "fires ~monthly, not on every restart" requirement is violated.

The fix is the single structural difference from the other tasks: **persist the
last-run timestamp to disk as a wall-clock epoch** and gate on the wall-clock
delta rather than a process-lifetime `Instant`.

```
┌────────────────────────────────────────────────────────────────┐
│ daemon loop                                                     │
│                                                                 │
│   now_epoch  = SystemTime::now() → unix seconds                 │
│   last_epoch = read_last_run(state_root/self_quality_audit_…)   │
│   elapsed    = Duration::from_secs(now_epoch − last_epoch)      │
│                                                                 │
│   if should_run_self_audit(elapsed, interval_secs) {           │
│       run_self_quality_audit(…)                                │
│       write_last_run(path, now_epoch)   // persist, survives    │
│   }                                       // the next restart    │
└────────────────────────────────────────────────────────────────┘
                         │  reads/writes
                         ▼
         {state_root}/self_quality_audit_last_run   (epoch seconds)
```

Because the marker is on disk, a restart mid-month reads back a recent epoch,
computes a small `elapsed`, and correctly declines to fire until a full interval
of *wall-clock* time has passed since the last real run.

### First-run / missing-file behavior (init-to-now)

On startup, if `self_quality_audit_last_run` is absent or unparseable, the
daemon **initializes it to `now` and does not fire this cycle**. The first audit
then fires ~one interval later. This deliberately avoids a heavy five-wave audit
firing instantly on a fresh deploy while still honoring "fires ~monthly, not on
every restart." A garbage file is treated identically to a missing one.

### Last-run is updated on both success and failure

After a run attempt, the daemon persists the new last-run **regardless of
`Ok`/`Err`**, mirroring the `last_X = Instant::now()` reset the other tasks do
unconditionally. This prevents a failing recipe from hot-looping — a broken
audit retries next month, not next cycle.

## Split of labor

```
┌──────────────────────────────┐        ┌──────────────────────────────────┐
│ Rust hook (daemon-side)      │        │ Recipe (recipe-runner-rs)         │
│ src/self_quality_audit.rs    │        │ monthly-self-quality-audit.yaml   │
├──────────────────────────────┤        ├──────────────────────────────────┤
│ • interval_secs_from_env     │        │ • Wave 1..5 SEEK→VALIDATE→FIX      │
│ • read/write last-run epoch  │        │   via amplihack quality-audit     │
│ • should_run_self_audit gate │──run──▶│ • per-PR crusty-old-engineer      │
│ • spawn recipe-runner-rs     │        │   proxy review (≤3 rounds)        │
│ • parse text markers         │◀markers│ • self-merge crusty-approved +    │
│ • log fire + completion      │        │   CI-green PRs                    │
│ • persist last-run (Ok|Err)  │        │ • emit AUDIT_* / WAVE_* / PR_*    │
└──────────────────────────────┘        └──────────────────────────────────┘
```

The Rust hook is a **pure recipe invoker** (modeled on `disk_health.rs`, not on
`brain_introspection.rs`): it does no memory-RPC work, because a code audit
needs none. It owns the clock, the persistence, and the logging; the recipe owns
all judgment.

## The five-wave audit model

Each run drives **five sequential SEEK→VALIDATE→FIX waves** of the amplihack
`quality-audit` skill against `rysweet/Simard`:

1. **SEEK** — scan the codebase for a category of quality issues (dead code,
   silent degradation, missing tests, doc drift, structural smells).
2. **VALIDATE** — confirm each finding is real and worth fixing (no false
   positives, no churn-for-churn's-sake).
3. **FIX** — implement the fix on a branch and open a pull request.

Running five waves per audit lets each wave target a different quality
dimension while sharing one crusty-gated merge pipeline. Waves are sequential so
later waves see earlier waves' merges and do not collide.

## The crusty-old-engineer proxy-review gate

Simard must not merge her own audit fixes unreviewed. For **each** PR a wave
opens, the recipe invokes the `crusty-old-engineer` skill as operator Ryan's
**proxy reviewer** — a curmudgeonly senior-engineer persona that applies
grounded, evidence-linked skepticism. The recipe **loops** on each PR:

```
for each PR:
    round = 1
    loop:
        verdict = crusty_review(PR)
        if verdict == APPROVED:            emit CRUSTY_APPROVED; break
        if round >= MAX_CRUSTY_ROUNDS (3): emit CRUSTY_UNRESOLVED; break
        address crusty's feedback (push a fix commit)
        round += 1
    if crusty approved AND CI is green:
        self-merge the PR;               emit PR_MERGED
```

### Why the loop is bounded

An unbounded agentic review loop can spin forever on a PR crusty never blesses.
The loop is therefore **capped at 3 rounds per PR**. If crusty is still
unsatisfied after three rounds, the PR is **left open**, `CRUSTY_UNRESOLVED` is
emitted, and the audit moves on. Unresolved PRs surface for human follow-up
rather than being force-merged or blocking the whole audit.

### Merge criteria

A PR self-merges only when **both** conditions hold:

- crusty-approved (within the 3-round budget), **and**
- CI is green.

Branch protection is respected — the recipe uses `gh` and merges only when the
merge is actually allowed. PRs that are crusty-approved but CI-red, or CI-green
but crusty-unresolved, stay open.

## Output: PRs and logs, not snapshot docs

Per the no-point-in-time-docs rule, a run produces **real pull requests** and
**log lines**, never a committed snapshot markdown file. The only durable repo
documents are this page, the
[API reference](../reference/self-quality-audit-api.md), and the
[operator how-to](../howto/configure-self-quality-audit.md) — all of which
describe the mechanism and its knobs, not a point-in-time finding.

The daemon logs a **fire** line when the audit starts and a **completion** line
when it finishes (or a WARN on failure), mirroring the other periodic tasks' log
discipline and adding the required "clear line when it fires and when it
completes."

## Fail-open safety model

- **Best-effort, never blocks the cycle.** A recipe failure (missing
  `recipe-runner-rs`, missing recipe YAML, `gh` unauthenticated, non-zero exit)
  is logged as a WARN; the OODA loop continues. The audit is an optimization,
  not a hard dependency.
- **No hot-loop on failure.** Last-run is persisted on `Ok` *and* `Err`, so a
  broken audit retries next interval, not next cycle.
- **Bounded review.** ≤3 crusty rounds per PR; unresolved PRs are surfaced, not
  force-merged.
- **Off by explicit `0`.** `SIMARD_SELF_AUDIT_INTERVAL=0` disables the task
  entirely; garbage values fall back to the conservative monthly default.
- **Conservative default cadence.** ~30 days (2,592,000s), so the heavy
  five-wave audit runs rarely.

## Relationship to brain introspection

This task **reuses** the brain-introspection periodic-task *pattern* (interval
env → startup `daemon_log` → in-loop gate → hook module + recipe YAML +
`lib.rs` registration → `recipe-runner-rs … --output-format json`) but audits a
different surface (source code, not memory) and adds the disk-persisted last-run
so a monthly cadence survives restarts. It does **not** reinvent the scheduler,
does not touch the other periodic tasks, and does not duplicate the
brain-introspection memory hygiene.

## Configuration knobs

| Knob | Env var | Default |
| --- | --- | ---: |
| Cadence | `SIMARD_SELF_AUDIT_INTERVAL` | `2592000` (~30 days, in seconds; `0` = disabled) |

> **Naming note.** The env var is the exact name the goal specifies —
> `SIMARD_SELF_AUDIT_INTERVAL` (value in **seconds**) — rather than the
> `SIMARD_*_INTERVAL_SECS` suffix the sibling tasks use. This deviation is
> intentional and documented so the naming is not mistaken for a bug.

See [Configure the monthly self-quality-audit](../howto/configure-self-quality-audit.md)
for tuning and observability.

## Related

- [Self-quality-audit API](../reference/self-quality-audit-api.md) — the executable contract
- [Configure the monthly self-quality-audit](../howto/configure-self-quality-audit.md) — operator guide
- [Brain introspection + memory hygiene](./brain-introspection.md) — the sibling periodic task whose pattern this reuses
- [Automated disk health management](../concepts/automated-disk-health.md) — the pure recipe-invoker shim this hook is modeled on
- [Daemon mode](../daemon-mode.md) — OODA cycle + periodic-task overview
