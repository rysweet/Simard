---
title: Diagnose a degraded engineer cognitive access
description: >
  How to read, verify, and act on a
  `simard.enrichment.degraded{reason="cognitive_open_lock"}` signal — the marker
  that an OODA engineer lost the cognitive open-lock race and degraded to
  deferred/read-only cognition instead of hard-exiting artifact-less. Confirms the
  engineer still shipped its commit/PR, distinguishes this from the benign #2860
  broken-pipe reconnect, and shows when a degrade is healthy vs. when contention
  needs attention.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/engineer-cognitive-access-degradation.md
  - ../reference/engineer-cognitive-access-degradation-api.md
  - ../reference/cognitive-memory-open-serialization.md
  - ../reference/enrichment-observability-api.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../reference/telemetry-metrics.md
---

# Diagnose a degraded engineer cognitive access

If you see a log line or metric like:

```
WARN simard::enrichment: cognitive access degraded — engineer lost the open-lock
     race; continuing with deferred/read-only cognition reason=cognitive_open_lock
```

```
DEBUG simard::enrichment: cognitive open-lock degrade detail
      reason=cognitive_open_lock holder="pid=3177076"
```

```
simard.enrichment.degraded{reason="cognitive_open_lock"}  +1
```

an OODA engineer tried to open the **shared** cognitive store, another process
already held the cross-process open-lock, and the engineer **degraded** to
deferred/read-only cognition rather than shutting down and exiting artifact-less.
This is the *designed* graceful-degradation path — see the
[concept doc](../concepts/engineer-cognitive-access-degradation.md). This guide
confirms the degrade was healthy and tells you when contention needs attention.

## First: this is a survival signal, not a crash

Before the fix, a lost open-lock race printed
`cognitive store is held open by another process … after waiting 15000ms`, shut
down the `MeterProvider`, and exited with **no commit, no PR, no artifact**. The
`cognitive_open_lock` degrade WARN is the **opposite** signal: the engineer *did
not* die — it kept working with reduced cognition.

> **Do not confuse this with `reason="memory_ipc"`.** A `memory_ipc` degrade (or a
> `memory_errors`-clean broken-pipe reconnect,
> [#2860](https://github.com/rysweet/Simard/issues/2860)) is a different,
> already-handled transient. `cognitive_open_lock` specifically means the engineer
> lost the *open-lock* race and took the deferred/read-only path.

## Step 1 — confirm the engineer still produced its artifact

The whole point is that a degrade must not cost an artifact. Verify the engineer
finished:

```bash
# The engineer's worktree and branch (from the spawn log or the worktree list)
git -C /home/azureuser/src/Simard worktree list --porcelain \
  | grep -E '^worktree .*/engineer-worktrees/'

# Did the engineer commit / open a PR for its goal?
gh pr list --search "head:engineer/<goal-id>-" --state all
```

A healthy degrade shows a **commit and/or PR present** for the goal despite the
`cognitive_open_lock` WARN. If the artifact is there, the feature worked exactly
as intended — the degrade is benign. Record nothing further.

## Step 2 — confirm no fatal open occurred

A degrade must never coincide with a store wipe or a 15000ms fatal open:

```bash
# There must be NO new fatal open-lock error for this run …
journalctl -u simard --since '-1h' | grep -i 'held open by another process'

# … and NO new corruption quarantine on the shared store
ls -1d ~/.simard/cognitive.corrupt-* 2>/dev/null
```

Both should be empty for the degraded run. The
[open-serialization guard](../reference/cognitive-memory-open-serialization.md)
still prevents any wipe; the degrade path prevents the fatal exit.

## Step 3 — read the degrade rate, not a single line

One `cognitive_open_lock` degrade is normal under bursty concurrent spawns.
Watch the **rate** via the enrichment degrade counter (see the
[enrichment observability API](../reference/enrichment-observability-api.md) and
[telemetry metrics](../reference/telemetry-metrics.md)):

```
simard.enrichment.degraded{reason="cognitive_open_lock"}
```

| What you see | Interpretation | Action |
|---|---|---|
| Occasional degrades, artifacts always produced | Healthy graceful degradation under concurrency | None — working as designed |
| Degrade rate climbing while engineers still ship | High shared-store contention; cognition is frequently deferred | Confirm the **daemon memory-IPC socket is up** so engineers get *shared-read* (tier 1) instead of falling to tier 3 |
| Degrades **and** a no-progress block on the same goal | Cognition-degrade is *not* the cause (artifacts still ship), but something else is stuck | Follow [diagnose a no-progress block](./diagnose-a-no-progress-block.md); the open-lock is not your culprit |

## Step 4 — make sure engineers are getting shared-read (tier 1)

The best outcome is that engineers never reach the deferred tier because they
route through the daemon IPC. Confirm the socket exists at the shared state root:

```bash
# The daemon's memory-IPC socket — engineers prefer this (shared read + serialized write)
ls -l "$(printf '%s/memory.sock' "${SIMARD_STATE_ROOT:-$HOME/.simard}")"
```

- **Socket present** → engineers resolve to **tier 1 (live, shared read)**. A
  `cognitive_open_lock` degrade should then be rare (only a genuine IPC hiccup +
  contended direct open reaches tier 3).
- **Socket absent** → an engineer opens the state root directly. If the open is
  **uncontended** it runs **live (tier 2)**; if it **contends** the shared store
  it takes **tier 3 (deferred/read-only)** rather than the fatal 15 s error.
  Frequent `cognitive_open_lock` degrades with no socket mean many engineers are
  racing the *same shared* root — bring the daemon IPC socket up (tier 1) so they
  share one store. (A per-worktree isolated cognitive root that would make these
  standalone opens *live* instead of deferred is a designed, deferred follow-up.)

## Step 5 — understand a deferred write

Under a `cognitive_open_lock` degrade, cognitive **writes are deferred**
(dropped-with-metric via a bounded in-memory counter — nothing is buffered or
spilled) and **never reported as persisted**. This is intentional
anti-hollow-success behaviour:

- Recall the engineer *read* still works (served via IPC or last-known snapshot),
  so decisions still benefit from cognition.
- A write the engineer produced during the degrade may not have been stored — but
  it is **counted**, never silently claimed as saved. Cognition is advisory; the
  artifact is what matters, and it shipped.

## What you should *not* do

- **Do not** raise `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` or the guard's
  `DEFAULT_BUDGET` to "fix" contention. A longer wait does not let 7 engineers
  share a single-writer lock; it only hides the contention. The value is clamped
  to `[1, DEFAULT_BUDGET]` and cannot disable the guard.
- **Do not** treat a `cognitive_open_lock` WARN as a crash. If the artifact
  shipped (Step 1), the degrade is a success signal.
- **Do not** conflate this with the #2860 broken-pipe reconnect — different code
  path, different reason attribute, out of scope.
- **Do not** "recover" a supposedly lost store — no wipe occurs on this path
  (Step 2). If you *do* see a `cognitive.corrupt-*` quarantine, that is the
  open-serialization guard's separate concern; follow the
  [WAL recovery runbook](../operations/cognitive-memory-wal-recovery-runbook.md).

## Related

- [Concept: engineer cognitive-access degradation](../concepts/engineer-cognitive-access-degradation.md)
- [Engineer cognitive-access degradation API](../reference/engineer-cognitive-access-degradation-api.md)
- [Cognitive-Memory Open Serialization](../reference/cognitive-memory-open-serialization.md)
- [Diagnose a no-progress block](./diagnose-a-no-progress-block.md)
- [How OODA spawns engineer agents](./spawn-engineers-from-ooda-daemon.md)
