---
title: Configure and monitor resource-aware engineer admission
description: Operator guide for Simard's resource-aware admission gate — set the disk ceiling, read ADMIT/DEFER/RECLAIM-FIRST decisions, edit the reasoning prompt, and walk a worked example.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ../reference/resource-aware-admission-api.md
  - ../reference/ooda-resource-admission-recipe.md
  - ./configure-adaptive-scaling.md
  - ./configure-disk-health-check.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
  - ./spawn-engineers-from-ooda-daemon.md
---

# Configure and monitor resource-aware engineer admission

Resource-aware admission stops the daemon from spawning another engineer when
the host cannot afford one — even if the [AIMD count
cap](./configure-adaptive-scaling.md) still has headroom. Before each **fresh**
engineer spawn, an admission brain reasons over disk, build-cache size, and load,
and decides **ADMIT**, **DEFER**, or **RECLAIM-FIRST**. One deterministic disk
ceiling backs it up so `ENOSPC` is unreachable.

This guide is for operators: what to tune, how to read decisions, and how to
change the reasoning. For the *why*, see the
[concept](../concepts/resource-aware-engineer-admission.md); for the Rust
contract, see the [API reference](../reference/resource-aware-admission-api.md).

---

## At a glance

| What | Where |
|---|---|
| Tune the hard ceiling | `SIMARD_DISK_ADMISSION_CEILING_PCT` env (default `90`) |
| Change the reasoning | `prompt_assets/simard/recipes/ooda-resource-admission.yaml` (hot-reload) |
| Read decisions | `brain_admission_decision` metric, `resource_admission` judgment phase, `[simard] resource admission` tracing |
| Reclaim path | reuses the [disk-health reclaim recipe](./configure-disk-health-check.md) |

Nothing here changes the count cap — admission runs **underneath** it.

---

## 1. Set the disk admission ceiling

The ceiling is the only hard threshold. When disk usage is read successfully and
is **at or above** the ceiling, admission is refused regardless of what the brain
decided.

```bash
# Default is 90%. Lower it on a small/busy partition for more headroom:
SIMARD_DISK_ADMISSION_CEILING_PCT=85 simard ooda run

# Raise it on a large partition where 90% still leaves plenty of room:
SIMARD_DISK_ADMISSION_CEILING_PCT=93 simard ooda run
```

Rules:

- Default **90** if unset or unparseable.
- Clamped to **1–99**. `0` (would deadlock all spawns) and `≥100` (would disable
  the rail) are impossible after clamping.
- Read fresh each cycle — no rebuild or restart needed to change it.

Choosing a value:

| Partition size / pressure | Suggested ceiling |
|---|---|
| Small (< 200G) or many engineers | `80`–`85` |
| Default host | `90` |
| Large (> 1T), light engineer count | `92`–`93` |

Keep the ceiling **below** the [≥95% emergency-cleanup
tier](./configure-disk-health-check.md) so the admission gate throttles *before*
the emergency tier ever has to fire.

> **Fail-open, by design.** If the `df` probe fails, disk reads as *unknown* and
> the ceiling does **not** engage — the decision falls through to the brain. A
> transient probe error can never deadlock spawning. The layered
> [disk-health](./configure-disk-health-check.md) protection covers the unknown
> case.

---

## 2. Read admission decisions

Every fresh-spawn admission emits these signals.

### Tracing

```
INFO simard::ooda_brain::admission: resource admission decided
    goal=improve-test-coverage
    choice=defer
    disk_pct=91 ceiling=90 cache_bytes=53687091200 load_1m=18.4 cpus=8 in_flight=12
    rationale="disk over ceiling — hard rail engaged"
```

`choice` is one of `admit`, `defer`, `reclaim_first`. The context fields show
exactly what the brain saw. `reason`/`rationale` is untrusted model text, emitted
as a structured `tracing` field (target `simard::ooda_brain`) — never
shell-interpreted — so it is safe to read directly. (The example above is
illustrative; the live lines read like `resource-aware admission: DEFER …` with
`goal` and `reason` fields.)

### Metric — `brain_admission_decision`

One event per admission, labeled **only** by `choice` (`admit` / `defer` /
`reclaim_first`, bounded cardinality) — emitted by the recipe brain, so it
records the brain's *intent* before the disk hard rail. Use it to watch how
often the daemon is deferring:

```bash
# via the status/metrics surface (see the telemetry reference)
simard status --metrics | grep brain_admission_decision
```

A rising `defer` / `reclaim_first` share is the early-warning signal that the
host is under resource pressure — long before `ENOSPC`.

### Parse health — surfaced as an explicit error, not a metric series

Unlike the decide / orient / lifecycle brains, admission does **not** route
through the shared verdict-parse chokepoint, so there is **no**
`brain_verdict_parsed_total{phase="resource_admission"}` series. Admission uses a
direct parse with **NO FALLBACK**: if the recipe output can't be parsed into a
`{"choice":..,"rationale":..}` decision, `judge_admission` returns an error and
the seam surfaces it as a visible cycle failure (`success=false`) — it never
fabricates an admit. So "the recipe output can no longer be parsed" shows up as
admission **failures** in the daemon log / cycle outcomes (grep for
`resource-admission brain failure`), not as a parse-rate metric.

### Judgment record — `resource_admission` phase

Admission decisions appear in [brain
introspection](../reference/brain-introspection-api.md) under the
`resource_admission` phase, alongside `decide` / `act` / `orient`, each carrying
the rationale and the prompt version that produced it.

---

## 3. Interpret each outcome

| You see | It means | Operator action |
|---|---|---|
| `admit` (steady) | Host is healthy. | None — normal operation. |
| `defer` (occasional) | Brief resource tightness; goals retry next cycle. | None — this is the gate working. |
| `defer` (sustained) | Persistent pressure (full disk / high load). | Reclaim space, lower engineer count, or lower the ceiling if it is too tight. |
| `reclaim_first` (repeating) | Disk is high but reclaimable each cycle. | Investigate what keeps growing (stale worktrees, caches); see [reclaim disk space](./reclaim-disk-space-and-run-low-space-rust-builds.md). |
| Hard-rail `defer` at `disk_pct ≥ ceiling` | The deterministic floor engaged. | Free disk or raise the ceiling *only if* genuinely safe. |

A `defer` is **not** a goal failure — it never bumps the goal's failure counter
and never blocks the goal. If you see a goal "not progressing" purely because of
`defer`, the fix is host resources, not the goal.

---

## 4. Edit the reasoning (prompt hot-reload)

All the judgment lives in the recipe prompt — change it without a rebuild:

```bash
# In-tree (takes effect next cycle):
$EDITOR prompt_assets/simard/recipes/ooda-resource-admission.yaml

# Or hot-reload copy (wins over in-tree):
$EDITOR ~/.simard/prompt_assets/simard/recipes/ooda-resource-admission.yaml
```

Typical edits:

- Make the brain more conservative under load (tighten the `defer` guidance).
- Prefer `reclaim_first` earlier when the cache is large.
- Adjust the ROLE wording for a specific host profile.

Bump the recipe `version:` field when you change decision-affecting wording so
the change is traceable in the judgment record. See the
[recipe & prompt schema](../reference/ooda-resource-admission-recipe.md) for the
context variables and the required output envelope.

> Do **not** try to encode thresholds in the prompt as the enforcement mechanism.
> The prompt is judgment; the *only* hard threshold is
> `SIMARD_DISK_ADMISSION_CEILING_PCT`.

---

## 5. Disable / floor behavior

There is no "off" switch for admission — a gate that can be turned off cannot
protect against `ENOSPC`. But you can control its aggressiveness:

- **Effectively permissive:** raise `SIMARD_DISK_ADMISSION_CEILING_PCT` toward
  `99` and relax the prompt's `defer` guidance. The gate still blocks a genuinely
  full disk.
- **No recipe available:** if `recipe-runner-rs` or the recipe YAML is missing,
  admission falls back to the **deterministic floor** (`DeterministicAdmissionBrain`),
  which always returns `Admit`. The hard ceiling still guards ENOSPC, so the
  daemon stays safe even with no LLM.

---

## Worked example / tutorial

Simulate the incident this feature was built for and watch it throttle.

1. **Start the daemon with a tight ceiling** to make the gate observable:

   ```bash
   SIMARD_DISK_ADMISSION_CEILING_PCT=85 SIMARD_SCALING=auto simard ooda run
   ```

2. **Fill the partition toward the ceiling** (e.g., let several engineers build,
   or drop a large scratch file). As disk approaches 85%, watch the admission
   log:

   ```
   INFO … resource admission decided goal=… choice=reclaim_first
        disk_pct=84 ceiling=85 cache_bytes=61055517081 … rationale="disk near ceiling; caches reclaimable"
   ```

   The brain chooses `reclaim_first`: it runs the disk-health reclaim recipe and
   defers this cycle — no new worktree is created while disk is tight.

3. **Push past the ceiling.** Now the deterministic rail takes over regardless of
   the model:

   ```
   INFO … resource admission decided goal=… choice=defer
        disk_pct=86 ceiling=85 … rationale="disk over ceiling — hard rail engaged"
   ```

   Even if the model had said `admit`, the gate downgrades to `defer`. The
   partition never reaches `ENOSPC`.

4. **Confirm goals are not penalized.** The deferred goals show `success=true`
   skip outcomes (`deferred: resource pressure`) and their failure counters are
   unchanged:

   ```bash
   simard status | grep -i defer
   ```

5. **Reclaim and recover.** After the reclaim recipe frees space (or you clean up
   manually — see
   [reclaim disk space](./reclaim-disk-space-and-run-low-space-rust-builds.md)),
   disk drops back under the ceiling and admissions return to `admit`:

   ```
   INFO … resource admission decided goal=… choice=admit
        disk_pct=63 ceiling=85 … rationale="disk 63% under ceiling; room to spawn"
   ```

You have now seen the full cycle: reason → reclaim → hard-rail floor →
benign-skip → recover, all without an `ENOSPC` crash and without any goal being
marked failed.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Spawns blocked, disk looks fine | Ceiling set too low, or high load driving `defer` | Raise `SIMARD_DISK_ADMISSION_CEILING_PCT`; check `load_1m` vs `cpus` in the log |
| Constant `reclaim_first` | Something regrows disk each cycle | Inspect worktrees/caches; [inspect & clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) |
| Never any `defer` even when full | Recipe missing → deterministic floor; **or** `df` probe failing (unknown disk) | Confirm recipe path resolves; check for `df` errors; the ceiling only fires on a *successful* read |
| Prompt edit not taking effect | Editing in-tree while a hot-reload copy exists | The `~/.simard/…` copy wins — edit that, or remove it |
| Goal seems stuck but only `defer` outcomes | Host resource pressure, not a goal problem | Free resources; `defer` never blocks a goal |

---

## See also

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md)
- [Resource-aware admission API reference](../reference/resource-aware-admission-api.md)
- [OODA resource-admission recipe & prompt schema](../reference/ooda-resource-admission-recipe.md)
- [Configure adaptive scaling](./configure-adaptive-scaling.md) — the count cap this layers under
- [Configure and monitor the disk health check](./configure-disk-health-check.md) — the reclaim path and emergency tier
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md)
- [Spawn engineers from the OODA daemon](./spawn-engineers-from-ooda-daemon.md)
