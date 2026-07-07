---
title: "How to configure and diagnose resource-aware engineer admission"
description: >
  Operator guide for the resource-aware admission gate — set the hard disk
  ceiling (SIMARD_DISK_ADMISSION_CEILING_PCT), understand what ADMIT / DEFER /
  RECLAIM-FIRST mean operationally, read back the resource_admission_decision
  metric and the ResourceAdmission judgment records, and diagnose a spawn that
  keeps getting resource-deferred.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ../reference/resource-admission-api.md
  - ../reference/ooda-resource-admission-recipe.md
  - ../operations/resource-admission-kill-switch.md
  - ./configure-disk-health-check.md
  - ./configure-adaptive-scaling.md
---

# How to configure and diagnose resource-aware engineer admission

> **Status: implemented.** The gate this page configures lives in
> [`src/ooda_actions/advance_goal/resource_admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/resource_admission.rs).
> For the rationale, see
> [resource-aware engineer admission](../concepts/resource-aware-engineer-admission.md);
> for the typed surface, the [API reference](../reference/resource-admission-api.md).

Before spawning a new engineer, Simard reasons about disk / build-cache / load
and decides **admit**, **defer**, or **reclaim-first**, with a deterministic disk
ceiling that blocks admission regardless of the reasoning. This page is the
operator guide: how to tune it, how to read what it decided, and what to do when a
spawn keeps deferring.

## Set the hard disk ceiling

The one knob that changes the **safety** behavior is the ceiling — the disk
used-percent at or above which admission is refused regardless of the brain.

| Variable | Default | Range | Effect |
| --- | --- | --- | --- |
| `SIMARD_DISK_ADMISSION_CEILING_PCT` | `90.0` | clamped to `1..=99` | Refuse admission when the engineer-worktree filesystem is at/above this used-percent. |

The default `90.0` sits one point below the `91%` the motivating incident
reached, so Simard refuses new disk-consuming work *before* re-entering the
danger band. Lower it on a smaller disk (builds are large relative to capacity);
raise it only if you have verified headroom and want more parallelism.

```bash
# One-shot for an interactive run — refuse admission at 85% instead of 90%.
SIMARD_DISK_ADMISSION_CEILING_PCT=85 simard daemon
```

Persist it the same way as any daemon env var (systemd unit `Environment=`; see
the [kill-switch page](../operations/resource-admission-kill-switch.md#how-to-set-it)
for the exact systemd recipe). Out-of-range or unparseable values fall back to
`90.0` with a `WARN` in the log, so a typo is visible rather than silently
neutralizing the guard.

> **The ceiling cannot be turned off by typo.** The clamp forbids `0` (which
> would refuse everything) and `100` (which would neutralize the guard). To widen
> the reasoning window while keeping a hard guard, set a high ceiling like `99`.

## Tune the reasoning (optional)

The *quality* of the admit/defer/reclaim reasoning below the ceiling lives in the
hot-reloadable recipe, not in code. To make Simard defer earlier, reclaim more
eagerly, or weigh load differently, edit the prompt and the next admission uses
it — no rebuild:

```bash
$EDITOR ~/.simard/prompt_assets/simard/recipes/ooda-resource-admission.yaml
```

See the [recipe & prompt schema](../reference/ooda-resource-admission-recipe.md)
for the context variables and the decision contract. You cannot break the ENOSPC
guarantee this way — the ceiling rail is in Rust.

## What each decision means operationally

| Decision | What Simard does | What you should read into it |
| --- | --- | --- |
| **admit** | Allocates a worktree and spawns the engineer (unless the hard rail blocks). | Healthy — resource headroom exists. |
| **defer** | Skips the spawn this cycle, no worktree, **no failure counted**; retries next round. | Transient pressure; several builds in flight or disk climbing. Self-clears as builds finish. |
| **reclaim-first** | Runs the [disk-health reclaim](./configure-disk-health-check.md) (stale worktrees, orphaned caches, old backups), then defers and retries next cycle. | Recoverable pressure — stale space is being freed before more is added. |
| **hard-rail block** | Disk is at/above the ceiling; even an `admit` is downgraded to a benign defer. The rail blocks but does not itself reclaim. | You are at the safety line. The periodic disk-health check reclaims automatically on its interval; to free space now, investigate what filled the disk and consider `simard worktree-gc --apply`. |

A `defer` or `reclaim-first` is **backpressure, not an error** — it never marks
the goal failed, never counts toward the "needs human review" safeguard, and
never shrinks the AIMD concurrency window.

## Reading back what the gate decided

Every admission emits both a metric and a judgment record.

### The metric stream

```bash
simard metrics query --name resource_admission_decision | tail -n 10
```

Each entry's `context` carries the decision, `disk_used_pct`, `worktree_count`,
`in_flight_engineers`, and the ceiling — enough to see *why* a cycle deferred.

### The cycle-report / judgment records

The decision is pushed as a `ResourceAdmission` judgment record with the scrubbed
rationale. In the daemon log:

```bash
journalctl --user -u simard-ooda -n 300 | grep -i 'resource-admission\|resource_admission'
```

Look for lines like:

```
[simard] spawn_engineer resource-deferred for 'add-signal-channel': disk 86% and climbing with 5 in-flight builds — let running builds finish
```

Hard-rail overrides and fail-closed brain-errors are recorded with
`fallback = true`, so a deterministic block is as visible as a reasoned one.

## Diagnosing a resource-deferred spawn

If a goal is not spawning an engineer and you suspect resource admission:

1. **Confirm it is a resource defer, not something else.** Grep for the
   `resource-deferred` / `reclaim-first` line above for that `goal_id`. If absent,
   the deferral is elsewhere (the [overlap gate](../operations/engineer-admission-kill-switch.md),
   the depth guard, or the byte-level `MIN_FREE_GB` precheck).
2. **Read the reason.** The rationale names the dominant signal — high
   `disk_used_pct`, many `in_flight_engineers`, saturated load, or a large
   worktree/cache footprint.
3. **If disk is genuinely high**, let the reclaim work (a `reclaim-first`
   decision runs it automatically) or run it yourself:
   ```bash
   simard worktree-gc --apply         # reclaim stale engineer worktrees
   ```
   Then confirm `disk_used_pct` dropped below the ceiling in the next metric
   entry.
4. **If disk is low but the gate still defers**, the load or in-flight-build
   signal is driving it. Wait a cycle; deferrals self-clear as builds finish. A
   *persistent* wrong defer at low disk means a probe is misreading — check for a
   `WARN` about a failed disk stat or an unparseable ceiling env var.
5. **If the reasoning itself is broken** (repeated fail-closed defers with
   `fallback = true` and an `error!` about the recipe), the reasoning has errored.
   Fix the recipe/transport, or fall back to count-only admission with the
   [kill-switch](../operations/resource-admission-kill-switch.md) — the hard
   ceiling still protects you while you do.

## Related

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md)
- [Resource-admission API reference](../reference/resource-admission-api.md)
- [OODA resource-admission recipe & prompt schema](../reference/ooda-resource-admission-recipe.md)
- [Resource-admission kill-switch](../operations/resource-admission-kill-switch.md)
- [How to configure and monitor the disk health check](./configure-disk-health-check.md) — the reclaim capability this gate invokes.
- [How to configure adaptive scaling](./configure-adaptive-scaling.md) — the AIMD count control this augments.
