---
title: Configure and monitor cognitive-thread scheduling
description: Operator guide for Simard's cognitive-thread scheduler (the Mind) (#2419) — how the daemon runs the OODA loop and its background threads on their own cadences, tuning the per-thread interval knobs and the non-critical per-tick budget, guaranteeing the OODA loop is never starved, observing per-thread metrics/spans/health, driving the MaintenanceThread safely (dry-run + protected paths), reading the EngineerLogAnalysisThread's deduplicated GitHub issues, and diagnosing a backed-off or misconfigured thread.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/cognitive-thread-scheduling.md
  - ./add-a-new-cognitive-thread.md
  - ./configure-brain-introspection.md
  - ./configure-self-quality-audit.md
  - ./configure-disk-health-check.md
---

# Configure and monitor cognitive-thread scheduling

!!! note "Status — shipped, additive, OFF by default (#2419)"
    The cognitive-thread scheduler (the **Mind**), the `src/cognitive_threads/`
    module, its `simard.thread.*` metrics/spans, and the two exemplar threads
    (`MaintenanceThread`, `EngineerLogAnalysisThread`) **have shipped**. In the
    live daemon they are **inactive until you opt in** with
    `SIMARD_COGNITIVE_THREADS_ENABLED` (see [Enable the scheduler](#enable-the-scheduler)).
    Two design goals are intentionally **deferred to follow-ups** and are *not*
    active yet: (1) driving the OODA cycle itself **through** the Mind (today the
    daemon keeps its existing inline OODA cycle for byte-for-byte parity, and the
    Mind runs only the two background threads *after* it), and (2) migrating the
    six pre-existing periodic tasks onto the Mind (they remain hand-rolled). See
    [Cognitive-thread scheduling](../reference/cognitive-thread-scheduling.md)
    for the design and rollout scope.

Simard's daemon does more than run the OODA loop. Every background mental
process — housekeeping, backups, brain introspection, the monthly self-audit,
maintenance, engineer-log analysis — is a **cognitive thread** owned by a single
scheduler, the **Mind**. Each thread declares its own cadence (or trigger), the
Mind computes which threads are due each tick, and runs them under a
priority/resource budget that **always runs the OODA loop first and never lets
it be starved**.

This guide shows how to tune the cadences, guarantee OODA parity, observe each
thread, drive the two exemplar threads (`MaintenanceThread`,
`EngineerLogAnalysisThread`) safely, and diagnose a thread that has stopped
firing. For the design and API contract see
[Cognitive-thread scheduling](../reference/cognitive-thread-scheduling.md); to
add your own thread see
[Add a new cognitive thread](./add-a-new-cognitive-thread.md).

## When to use this

Use this guide when:

- You want to change how often a background thread runs (or turn it off)
- You need to confirm the OODA loop still runs on its exact cadence after
  enabling the scheduler
- You want to read a thread's runs/errors/duration metrics or its next-run time
- The `MaintenanceThread` is (or isn't) pruning artifacts and you want to verify
  it is behaving conservatively
- The `EngineerLogAnalysisThread` filed — or should have filed — a GitHub issue
- A thread has gone quiet and you suspect it has been backed off after repeated
  failures

## The model: one Mind, many threads

The Mind holds a **registry** of threads. Once per outer daemon iteration it:

1. computes the **due** set (enabled, not backed-off, and past its next-run),
2. runs the **OODA thread first, unconditionally, every tick** (it is
   `Priority::Critical` and exempt from the budget),
3. runs the remaining due threads in priority order, up to a **per-tick budget**
   of non-critical threads, then
4. sleeps the same `interval_secs` the daemon always slept and re-checks
   shutdown.

Each thread runs inside a panic/error guard: a thread that panics or returns an
error is **caught, recorded, and backed off** — it can never crash the daemon or
delay a sibling. The OODA thread is the one exception to backoff: its errors are
logged and the cycle continues, exactly as before the scheduler existed.

!!! info "How OODA-first works in the live daemon today"
    The `Mind`'s contract runs `Priority::Critical` (OODA) first and exempt from
    the budget. **As shipped**, the daemon achieves the same guarantee a simpler
    way: it runs its existing **inline OODA cycle first**, then calls the `Mind`
    (which currently holds only the two background threads) *afterward*. Either
    way OODA runs first every iteration and no number of background threads can
    delay it. Registering `OodaThread` in the live `Mind` is a follow-up.

Threads that ship in this build:

| Thread id (telemetry key) | Kind | Priority | Cadence source |
| --- | --- | --- | --- |
| `ooda` | Ooda | **Critical** | `SIMARD_OODA_INTERVAL_SECS` |
| `maintenance` | Maintenance | Low | `SIMARD_MAINTENANCE_INTERVAL_SECS` |
| `engineer_log_analysis` | EngineerLogAnalysis | Low | `SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS` |

The pre-existing periodic tasks — verified backup, disk-health check,
RSS/memory shedding, engineer-worktree sweep, brain introspection, and the
monthly self-quality-audit — are **designed to be subsumed by the same Mind**,
but that migration is a **follow-up**: in this build they still run through the
daemon's existing hand-rolled loop with their current env-var knobs (below). The
scheduler runs *alongside* them.

## Enable the scheduler

The scheduler is **off by default**. Turn it on with a single truthy env var
(`1`, `true`, `yes`, or `on`), then launch the daemon:

```bash
# Enable the cognitive-thread scheduler (maintenance + engineer-log analysis)
export SIMARD_COGNITIVE_THREADS_ENABLED=1
```

| Knob | Env var | Default | What it controls |
| --- | --- | --: | --- |
| Master switch | `SIMARD_COGNITIVE_THREADS_ENABLED` | `false` | When truthy, the daemon builds the `Mind` and runs the background threads after each OODA cycle. Unset ⇒ zero behaviour change. |

When it is enabled, the daemon logs at startup:

```
[simard] OODA daemon: cognitive-thread scheduler ENABLED (2 background thread(s))
```

Recommended first rollout: enable it with **maintenance in dry-run** and
**analysis in dry-run** (below), read the telemetry for a few cycles, then relax
the dry-run switches once you trust the behaviour.

## Tune the cadences

Every knob is an environment variable read once at daemon start. Set it before
launching the daemon.

### The two exemplar threads

| Knob | Env var | Default | What it controls |
| --- | --- | --: | --- |
| Maintenance cadence | `SIMARD_MAINTENANCE_INTERVAL_SECS` | `86400` (daily) | Seconds between housekeeping passes (clamped to a 60 s floor) |
| Maintenance dry-run | `SIMARD_MAINTENANCE_DRY_RUN` | `true` | Ships dry-run-first: logs the actions it *would* take and deletes nothing. Set to `0`/`false` to enable real pruning. |
| Keep corrupt dirs | `SIMARD_MAINTENANCE_KEEP_CORRUPT` | `3` | Retention floor for `cognitive.corrupt-*` quarantine dirs (min 1) |
| Keep snapshots | `SIMARD_MAINTENANCE_KEEP_SNAPSHOTS` | `5` | Retention floor for store snapshots / shadow-WAL copies (min 1) |
| Keep backups | `SIMARD_MAINTENANCE_KEEP_BACKUPS` | `7` | Retention floor for verified backups (min 1) |
| Analysis cadence | `SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS` | `21600` (6 h) | Seconds between engineer-log analysis passes (clamped to a 60 s floor) |
| Analysis dry-run | `SIMARD_ENGINEER_LOG_ANALYSIS_DRY_RUN` | `false` | When truthy, emits findings as structured telemetry and files **no** GitHub issue |

!!! tip "Maintenance ships dry-run-first"
    `SIMARD_MAINTENANCE_DRY_RUN` defaults to **`true`** (SR-7): even with the
    scheduler enabled, maintenance deletes nothing until you explicitly set
    `SIMARD_MAINTENANCE_DRY_RUN=0`. The analysis thread defaults to filing real
    (deduplicated) issues — set `SIMARD_ENGINEER_LOG_ANALYSIS_DRY_RUN=1` to keep
    it observe-only during initial rollout.

### The scheduler budget

| Knob | Env var | Default | What it controls |
| --- | --- | --: | --- |
| Non-critical fan-out | `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` | `2` | Max **non-OODA** threads run in a single tick. OODA is exempt and always runs. |

The budget bounds how many background threads can fire in one iteration so a
burst of simultaneously-due threads can never crowd out the OODA cycle. If two
background threads come due in the same tick and the budget is `2`, both run
after OODA; a third due thread waits for the next tick.

### The pre-existing periodic tasks (unchanged knobs)

These tasks still run through the daemon's existing hand-rolled loop (their
migration onto the `Mind` is a follow-up); their knobs are unchanged:

| Task | Env var | Default |
| --- | --- | --: |
| Verified backup | `SIMARD_BACKUP_INTERVAL_SECS` | (feature default) |
| Disk-health check | `SIMARD_DISK_HEALTH_INTERVAL_SECS` | `900` |
| Engineer-worktree sweep | `SIMARD_WORKTREE_SWEEP_INTERVAL_SECS` | `1800` |
| Brain introspection | `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS` | see [howto](./configure-brain-introspection.md) |
| Monthly self-quality-audit | `SIMARD_SELF_AUDIT_INTERVAL` | `2592000` (0 disables) |
| RSS / memory shedding | — (runs every tick, no interval) | — |

!!! note "Interval floor"
    Interval knobs are clamped to a small minimum floor. Setting an interval to
    `0` (or a negative/garbage value) does **not** make a background thread due
    every tick — it normalizes to the floor (or, for tasks that define `0` as
    "disabled" like `SIMARD_SELF_AUDIT_INTERVAL`, disables the thread). This
    stops a misconfigured environment from spinning a thread.

Examples:

```bash
# Run maintenance every 6 hours instead of daily
export SIMARD_MAINTENANCE_INTERVAL_SECS=21600

# Maintenance in observe-only mode — audit what it would prune, delete nothing
export SIMARD_MAINTENANCE_DRY_RUN=1

# Allow up to 3 background threads per tick (OODA still runs first, always)
export SIMARD_MIND_MAX_NONCRITICAL_PER_TICK=3
```

## Confirm OODA parity (the critical guarantee)

The single most important property: the daemon behaves **identically** from the
outside whether or not the scheduler is enabled. OODA still fires once per outer
iteration on `SIMARD_OODA_INTERVAL_SECS`, spawns engineers the same way, and
writes the same cycle reports, episodes, and health files — its cycle is run by
the daemon's unchanged inline path, with the background threads scheduled
strictly *after* it.

Confirm the OODA cadence at startup and that cycles are still advancing:

```bash
# Configured OODA interval (unchanged by the scheduler)
grep "OODA daemon: cycle interval" ~/.simard/ooda.log | tail -1

# Cycle number is still incrementing once per interval
python3 -c "import json;print(json.load(open('$HOME/.simard/daemon_health.json'))['cycle_number'])"
```

The OODA thread carries `Priority::Critical` and is **exempt from the per-tick
budget** — no number of background threads can delay or skip an OODA cycle. If
you ever see the cycle number stall while background threads keep firing, that
is a bug, not a budget effect; capture `~/.simard/ooda.log` and file an issue.

## Observe a thread

Each run opens a span and records metrics under the stable name
`simard.thread.<id>`:

| Metric | Type | Meaning |
| --- | --- | --- |
| `simard.thread.<id>.runs` | counter | Completed runs |
| `simard.thread.<id>.errors` | counter | Runs that panicked or returned `Err` |
| `simard.thread.<id>.duration_seconds` | histogram | Per-run wall-clock |
| `simard.thread.<id>.next_run_epoch` | gauge | When the thread is next due (epoch s) |
| `simard.thread.<id>.active` | gauge | `1` while ticking, `0` otherwise |

The span `simard.thread.<id>` carries fields `ran`, `success`, `duration_ms`,
and thread-specific `detail`. These are emitted as **structured `tracing`
events** (and OTel metrics when a meter is configured) — there are no ad-hoc
`println!` lines. To eyeball recent activity in the daemon's tracing output:

```bash
# Every scheduler span/event, newest last
grep "simard.thread." ~/.simard/ooda.log | tail -20

# Just the maintenance thread
grep "simard.thread.maintenance" ~/.simard/ooda.log | tail -10
```

The Mind also exposes a **health snapshot** per thread (last-run, next-run,
consecutive errors, backoff-until) that feeds the operator dashboard heartbeat.
See [the dashboard](../dashboard.md) for the live per-thread view.

## Drive the MaintenanceThread safely

`MaintenanceThread` performs conservative housekeeping under `~/.simard` on a
slow cadence, reusing the existing cleanup/backup helpers (it reimplements
nothing):

- prunes old `cognitive.corrupt-*` databases and stale store snapshots / shadow
  WAL copies beyond a retention floor,
- rotates stale binary backups and caps runaway cargo `target/` dirs,
- verifies and prunes verified backups to a retention count,
- reports disk pressure.

**It is conservative by construction:**

- It **never** deletes protected paths — `worktrees/main`, `~/.simard/repo`, the
  **live** cognitive store (and its shadow/WAL), or any engineer worktree. Every
  candidate is matched against a canonicalized allow-list and rejected if it is a
  symlink or escapes the allowed root.
- It honours **dry-run** (`SIMARD_MAINTENANCE_DRY_RUN`): it logs the actions it
  *would* take and deletes nothing.
- It always keeps **≥ N newest** of each artifact class (retention floors), so a
  misconfigured retention count can never delete everything.
- Every action is emitted as **structured telemetry** (path, bytes freed,
  kept/pruned counts) — never a snapshot doc committed to the repo.

Recommended first rollout: it already ships in dry-run (`SIMARD_MAINTENANCE_DRY_RUN`
defaults to `true`), so just read the telemetry for a few cycles before enabling
deletion.

```bash
# 1. Observe-only (this is the default): see what it would prune
export SIMARD_COGNITIVE_THREADS_ENABLED=1   # maintenance is dry-run by default
# ... start the daemon, then:
grep "simard.thread.maintenance" ~/.simard/ooda.log | tail -20

# 2. When satisfied, enable real pruning
export SIMARD_MAINTENANCE_DRY_RUN=0
```

## Read the EngineerLogAnalysisThread's findings

`EngineerLogAnalysisThread` scans **recent** engineer run logs and structured
OODA/brain telemetry (cycle reports, self-metrics, cost records, parse
telemetry) for improvement opportunities: recurring engineer failures,
stuck/looping patterns, brain parse-failure spikes, restart churn, distill
failure rate, and repeated CI failure modes. Its work is **bounded** — capped
records, window, and findings per run.

Its durable output is a **deduplicated GitHub issue**, not a repo snapshot doc.
It computes a stable failure signature, searches for an existing open issue
carrying that signature, and creates a new issue **only when none exists**
(create-suppression). Re-running is idempotent: the same recurring failure will
not spawn a second issue.

```bash
# Issues this thread filed (they embed a stewardship-signature marker)
gh issue list --repo rysweet/Simard --state open --search "stewardship-signature in:body" --limit 20
```

When issue filing is unavailable (no `gh`, offline, or in tests) it **degrades
to structured telemetry only** — it never writes a committed file. To see a run
that produced findings but couldn't file:

```bash
grep "simard.thread.engineer_log_analysis" ~/.simard/ooda.log | tail -20
```

!!! warning "Secrets and untrusted log content"
    Engineer logs are untrusted input. Before any excerpt is placed in an issue
    body or a telemetry field it is passed through the crate's secret scrubber
    (redacts `token=`/`Authorization:`/`_secret`/`_token` lines) and fenced so it
    cannot emit GitHub `@mentions`/`#refs` or poison dedup. You should never see a
    real credential in a filed issue; if you do, treat it as a security bug.

## Diagnose a thread that stopped firing

A thread that panics or errors repeatedly is **backed off** with capped
exponential backoff so it can't hot-loop. Backoff is per-thread and never
touches OODA or siblings.

Check, in order:

### 1. Is it disabled or clamped?

```bash
# Interval config lines are logged at startup
grep "OODA daemon:.*interval" ~/.simard/ooda.log | tail -10
```

An interval of `0` for a task that treats `0` as "disabled" (e.g. the
self-audit) means it is off. A very small configured interval is silently raised
to the floor.

### 2. Is it backed off after errors?

Look for the thread's error counter climbing and a future `next_run_epoch`:

```bash
grep "simard.thread.<id>.errors" ~/.simard/ooda.log | tail -5
grep "simard.thread.<id>" ~/.simard/ooda.log | grep -i "backoff\|error" | tail -10
```

The per-thread health snapshot shows `consecutive_errors` and
`backoff_until_epoch`. A thread recovers automatically on its next successful
run (the error streak resets); backoff only delays retries, it never disables
the thread permanently.

### 3. Was it starved by the budget?

If several non-critical threads are due at once and
`SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` is small, a lower-priority thread may be
deferred to a later tick. Raise the budget or stagger the intervals. OODA is
never affected.

### 4. Is the daemon shutting down between threads?

The Mind checks for a shutdown request between threads and returns early to keep
graceful drain fast, so a thread late in a tick may be skipped during shutdown.
This is expected and coexists with the existing `interruptible_sleep` +
drain/checkpoint path.

## Related

- [Cognitive-thread scheduling (reference)](../reference/cognitive-thread-scheduling.md) — trait, `Mind` API, `SchedulePolicy`, telemetry contract, security requirements
- [Add a new cognitive thread (howto)](./add-a-new-cognitive-thread.md) — implement and register your own thread
- [Configure and monitor brain introspection](./configure-brain-introspection.md) — a subsumed periodic task
- [Configure and monitor the monthly self-quality-audit](./configure-self-quality-audit.md) — the disk-persisted-gate example
- [Configure and monitor the disk health check](./configure-disk-health-check.md) — a subsumed periodic task
