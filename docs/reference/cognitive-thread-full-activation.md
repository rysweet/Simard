---
title: Cognitive threads — always-on scheduling, telemetry, and Overseer auto-remediation
description: >
  Reference for Simard's fully-activated cognitive-thread model (issue #4845).
  Every cognitive thread is registered and ENABLED by default (opt-out), ticks
  on a purpose-derived cadence, emits real simard.thread.<id>.* telemetry, and is
  supervised by the acting Overseer, which detects any failed/stalled/erroring
  thread and dispatches exactly one de-duplicated agentic remediation goal per
  failure signature. This page is the single source of truth for the default-ON
  env model, the per-thread cadence roster, the durable thread-intent table, and
  the detect→dispatch→dedupe→notify remediation contract.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference — issue #4845 (all threads default-ON, telemetry + auto-remediation)
related:
  - ./cognitive-threads-catalog.md
  - ./cognitive-thread-scheduling.md
  - ./cognitive-thread-observability.md
  - ./recipe-invoker-seam.md
  - ./telemetry-metrics.md
  - ./overseer-tick-self-healing.md
  - ../howto/configure-reflective-cognitive-threads.md
  - ../howto/configure-cognitive-thread-scheduling.md
---

# Cognitive threads — always-on scheduling, telemetry, and Overseer auto-remediation

Modules: `simard::cognitive_threads::{mind, recipe_rail, telemetry, threads::*}`,
`simard::operator_commands_ooda::daemon`, `simard::overseer::{mod, thread_oversight}`.

Simard's cognitive-thread "brain" is **fully active**. On a stock deployment the
daemon registers **all** cognitive threads, enables them **by default**, ticks
each one on a per-thread cadence derived from its purpose, emits real per-thread
telemetry, and supervises the whole roster with the acting Overseer. There is no
longer a dormant majority: the model that was defined in code now actually
**runs**.

This page documents the finished behaviour. For the per-thread *vision, recipe
envelopes, and memory prefixes* see the
[cognitive-threads catalog](./cognitive-threads-catalog.md); for the *metric
catalog and oversight seams* see
[cognitive-thread observability](./cognitive-thread-observability.md); for the
*scheduler contract* see
[cognitive-thread scheduling](./cognitive-thread-scheduling.md). This page is the
authoritative reference for **what is on by default, on what cadence, and how
failures are auto-remediated**.

## What changed (and why it matters)

| Before | Now (issue #4845) |
|---|---|
| Only `creative_ideas` ticked; 13 threads were registered-but-dormant behind a **double-AND, default-OFF** gate. | **All** scheduled threads are registered and **ENABLED by default** (opt-out). The scheduler runs *N* background threads, not 1. |
| `SIMARD_COGNITIVE_THREADS_ENABLED` and each `SIMARD_THREAD_<NAME>_ENABLED` had to **both** be truthy to run a thread. | Master + per-thread gates are **default-ON opt-out**: a thread runs unless explicitly disabled. |
| Per-tick budget `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` defaulted to **2**, starving long-cadence threads and risking false stalls. | Budget default raised to **13** so every scheduled non-critical thread ticks within its cadence. OODA stays `Critical`/budget-exempt. |
| No `simard.thread.*` telemetry for most threads. | Real per-thread telemetry for **every** registered thread in the metrics snapshot. |
| Thread failures landed only in `~/.simard/ooda.log`; the Overseer was blind. | Every failure is **dual-routed** (telemetry counter + durable failure sink), detected by the Overseer, and drives **exactly one** de-duplicated agentic remediation goal. |

> **Compatibility.** `creative_ideas` behaviour, its gate
> (`SIMARD_CREATIVE_IDEAS_ENABLED`, already default-ON), and its cadence are
> **preserved unchanged**. The `MIN_INTERVAL_SECS = 60` floor and
> `SIMARD_THREAD_INTERVAL_SCALE` clamp are retained. Truthy/falsy parsing is
> unchanged in spelling; only the **default** flips from OFF to ON.

## The roster — 15 ThreadKind variants, 14 live + 1 reserved

The `ThreadKind` enum defines **15** variants. Every variant maps to a live
cognitive process **except one**:

- **14 live threads** = **13 Mind-hosted background threads + the OODA main loop**.
- **1 reserved variant**: `SensoryProcessing` exists as a `ThreadKind` variant
  but ships **no thread**.

The clean reconciliation is therefore:

> **15 `ThreadKind` variants = 14 live (OODA + 13 Mind-hosted) + 1 reserved
> (`SensoryProcessing`).**

- **13 Mind-hosted scheduled threads** (non-critical, budgeted): `metacognition`,
  `consolidation`, `reflection`, `prospection`, `salience`, `operator_model`,
  `analogy`, `values_deliberation`, `interoception`, `narrative`,
  `creative_ideas`, `engineer_log_analysis`, `maintenance`.
- **1 OODA main loop** (`ooda`): the authoritative inline cycle, `Critical`
  priority, **budget-exempt**, unchanged in cadence and side-effects.
- **1 reserved variant**: `SensoryProcessing` — a `ThreadKind` value with **no
  scheduled thread**. It is the only permitted "dormant" entry and is marked
  *reserved* in the intent table below; a deliberately-dormant variant must
  always carry a written reason.

> **Note on `BackgroundThought`.** `BackgroundThought` is **not** reserved — it
> is the `ThreadKind` reported by the live `creative_ideas` thread
> (`creative_ideas.rs` → `kind() = ThreadKind::BackgroundThought`). It is
> counted among the 14 live variants, not the reserved set.

The intent table below is the durable reconciliation.

## Configuration — the default-ON env model

Every scheduled thread is **enabled unless explicitly opted out**. Three env knobs
per thread, plus one master switch:

| Variable | Scope | Default | Effect |
|---|---|---|---|
| `SIMARD_COGNITIVE_THREADS_ENABLED` | master | **on** | `0`/`false`/`no`/`off` disables the **entire** reflective roster. Any other value (including unset) keeps it on. |
| `SIMARD_THREAD_<NAME>_ENABLED` | per-thread | **on** | `0`/`false`/`no`/`off` opts **that one** thread out. Unset/other ⇒ enabled. |
| `SIMARD_THREAD_<NAME>_INTERVAL_SECS` | per-thread | (thread default) | Overrides the cadence (clamped to `MIN_INTERVAL_SECS = 60`). |
| `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` | scheduler | **13** | Max non-critical threads that may tick per scheduler pass. OODA is exempt. |
| `SIMARD_THREAD_INTERVAL_SCALE` | scheduler | `1.0` | Global multiplier applied to every interval, then clamped to the floor. |

### Gate semantics (the crux)

`recipe_rail::env_gate_open(master, thread)` is a **pure, fail-closed** predicate:

```text
enabled = !falsy(master) && !falsy(thread)
```

- **Falsy token set** (case-sensitive as listed, surrounding whitespace ignored):
  `0`, `false`, `FALSE`, `no`, `off`.
- **Anything else — including unset/`None` and unrecognised garbage — is treated
  as ENABLED.** This is the opt-out contract: an operator must *explicitly* say
  "off" to disable a thread.
- **Fail-closed on explicit falsy**: a real falsy value always wins, at both the
  master and per-thread level. The master gate opting out disables the whole
  roster regardless of per-thread settings.

> **Env gates are rollout controls, not an authorization boundary.** They decide
> *whether a thread is scheduled*, never *what a scheduled thread is allowed to
> do*. Thread-proposed goals remain enforcement-equivalent to operator goals.

### Name exceptions

Per-thread gate names are mechanically `SIMARD_THREAD_<UPPER_NAME>_ENABLED`, with
these deliberate exceptions (the **Env gate** / **Interval override** columns in
the roster table are the source of truth — do not derive them by rule):

- `values_deliberation` → `SIMARD_THREAD_VALUES_ENABLED` (abbreviated).
- `creative_ideas` → `SIMARD_CREATIVE_IDEAS_ENABLED` (its own default-ON gate,
  independent of the master switch; preserved unchanged).
- `engineer_log_analysis` cadence override → `SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS`.
- `maintenance` cadence override → `SIMARD_MAINTENANCE_INTERVAL_SECS`.
- `ooda` cadence → `SIMARD_OODA_INTERVAL_SECS`.

## Cadence roster (single source of truth)

Cadence and purpose live **only** on each thread's `Config::default().interval_secs`
and `policy()` in `src/cognitive_threads/threads/*.rs` — there is no duplicate
list. Tiering is derived from each thread's recovered purpose:

| Thread (`id`) | Tier | Cadence (default) | Priority | Per-thread gate | Interval override |
|---|---|---|---|---|---|
| `salience` | reflective/light | `1800 s` (30 m) | **Normal** | `SIMARD_THREAD_SALIENCE_ENABLED` | `SIMARD_THREAD_SALIENCE_INTERVAL_SECS` |
| `interoception` | reflective/light | `3300 s` (55 m) | **Normal** | `SIMARD_THREAD_INTEROCEPTION_ENABLED` | `SIMARD_THREAD_INTEROCEPTION_INTERVAL_SECS` |
| `metacognition` | reflective/light | `3600 s` (1 h) | Low | `SIMARD_THREAD_METACOGNITION_ENABLED` | `SIMARD_THREAD_METACOGNITION_INTERVAL_SECS` |
| `prospection` | mid | `4500 s` (75 m) | Low | `SIMARD_THREAD_PROSPECTION_ENABLED` | `SIMARD_THREAD_PROSPECTION_INTERVAL_SECS` |
| `reflection` | mid | `5400 s` (90 m) | Low | `SIMARD_THREAD_REFLECTION_ENABLED` | `SIMARD_THREAD_REFLECTION_INTERVAL_SECS` |
| `operator_model` | mid | `7200 s` (2 h) | Low | `SIMARD_THREAD_OPERATOR_MODEL_ENABLED` | `SIMARD_THREAD_OPERATOR_MODEL_INTERVAL_SECS` |
| `analogy` | heavy | `9000 s` (2.5 h) | Low | `SIMARD_THREAD_ANALOGY_ENABLED` | `SIMARD_THREAD_ANALOGY_INTERVAL_SECS` |
| `values_deliberation` | mid | `10800 s` (3 h) | Low | `SIMARD_THREAD_VALUES_ENABLED` | `SIMARD_THREAD_VALUES_INTERVAL_SECS` |
| `consolidation` | mid | `21600 s` (6 h) | Low | `SIMARD_THREAD_CONSOLIDATION_ENABLED` | `SIMARD_THREAD_CONSOLIDATION_INTERVAL_SECS` |
| `engineer_log_analysis` | heavy | `21600 s` (6 h) | Low | `SIMARD_THREAD_ENGINEER_LOG_ANALYSIS_ENABLED` | `SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS` |
| `narrative` | heavy | `43200 s` (12 h) | Low | `SIMARD_THREAD_NARRATIVE_ENABLED` | `SIMARD_THREAD_NARRATIVE_INTERVAL_SECS` |
| `maintenance` | maintenance | `86400 s` (daily) | Low | `SIMARD_THREAD_MAINTENANCE_ENABLED` | `SIMARD_MAINTENANCE_INTERVAL_SECS` |
| `creative_ideas` | heavy (**unchanged**) | (preserved) | Low | `SIMARD_CREATIVE_IDEAS_ENABLED` | `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS` |
| `ooda` | critical loop | `SIMARD_OODA_INTERVAL_SECS` | **Critical** (exempt) | — (always on) | `SIMARD_OODA_INTERVAL_SECS` |

Intervals are non-harmonic so threads diverge after the first budgeted burst; all
are clamped up to `MIN_INTERVAL_SECS = 60` and scaled by
`SIMARD_THREAD_INTERVAL_SCALE`.

## Durable thread-intent table

This is the durable design artifact recovered in Phase 1 (name → purpose →
inputs/outputs → cadence → status → gap). It is reference material, **not** a
point-in-time report.

> **Companion source.** The per-thread narrative reference lives in the
> [cognitive-threads catalog](./cognitive-threads-catalog.md); this table is the
> durable reconciliation that the catalog links back to.

| Thread | Purpose (one line) | Reads / Writes | Cadence | Status | Gap closed by #4845 |
|---|---|---|---|---|---|
| `ooda` | Authoritative Observe→Orient→Decide→Act loop. | Goal board, memory / actions, decisions. | `SIMARD_OODA_INTERVAL_SECS` | **Always on** (Critical, exempt) | — |
| `salience` | Appraise "what matters most right now"; bias next Decide. | Goals + health facts / `salience_signal.json` + `salience:` facts. | 30 m | **On by default** | Was dormant; now scheduled + supervised. |
| `interoception` | Deterministic self-sensing (disk/RSS/health). | System sensors / `interocept:` facts, ≤1 health goal. | 55 m | **On by default** | Was dormant. |
| `metacognition` | Self-audit of reasoning quality / calibration. | OODA telemetry / `metacog:` facts, ≤1 recalibration goal. | 1 h | **On by default** | Was dormant. |
| `prospection` | Simulate futures; stage prospective triggers. | Goals + episodes / `foresight:` facts + triggers. | 75 m | **On by default** | Was dormant. |
| `reflection` | Post-mortems → durable `lesson:` procedures. | Completed/failed goals / `postmortem:` + `lesson:`. | 90 m (guarded) | **On by default** | Was dormant. |
| `operator_model` | Live theory-of-mind model of the operator. | `USER_PREFERENCES` + episodes / `operator:` facts. | 2 h | **On by default** | Was dormant. |
| `analogy` | Cross-domain analogical mapping. | Concepts/episodes / `analogy:` facts. | 2.5 h | **On by default** | Was dormant. |
| `values_deliberation` | Deliberate on value tensions. | Values context / `values:` facts, ≤1 goal (no veto). | 3 h | **On by default** | Was dormant. |
| `consolidation` | "Sleep": replay episodes, form schemas, prune. | Undistilled episodes / `schema:` facts + advisory forgets. | 6 h | **On by default** | Was dormant. |
| `engineer_log_analysis` | Mine engineer/agent logs for failure patterns. | `ooda.log` / analysis facts. | 6 h | **On by default** | Was master-gated OFF. |
| `narrative` | Maintain identity narrative + chapters. | Episodes / `narrative:*` facts. | 12 h | **On by default** | Was dormant. |
| `maintenance` | Housekeeping (backups, cleanup, hygiene). | State root / maintenance side-effects. | daily | **On by default** | Was master-gated OFF. |
| `creative_ideas` (kind `BackgroundThought`) | Divergent idea generation. | Memory / creative goals. | (preserved) | **On** (already default-ON) | Unchanged; reports `ThreadKind::BackgroundThought`. |
| `SensoryProcessing` | *Reserved variant* — external-stimulus processing. | — | — | **Reserved** (no thread) | The only dormant variant; future work. |

## Scheduler behaviour

Each scheduler pass runs OODA first (`Critical`, budget-exempt), then up to
`SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` (**default 13**) due non-critical threads.
With the budget covering the full scheduled roster, every enabled thread ticks
within its cadence, so stall detection cannot false-positive on a starved thread.

- The `Mind` runs on a single-worker Tokio runtime; threads tick **sequentially**
  (the budget caps *count*, not concurrency).
- A panicking thread is isolated (RAII `active` guard resets, failure is recorded)
  and can never bring down the daemon or other threads.
- Backoff and the `MIN_INTERVAL_SECS = 60` floor bound a misbehaving thread.

### Startup log (`~/.simard/ooda.log`)

On boot the daemon logs one line per thread plus a summary. Example, stock config:

```text
[simard] OODA daemon: cognitive-thread scheduler ENABLED (13 background thread(s))
[simard] OODA daemon: cognitive thread 'maintenance' ENABLED (interval=86400s)
[simard] OODA daemon: cognitive thread 'engineer_log_analysis' ENABLED (interval=21600s)
[simard] OODA daemon: cognitive thread 'metacognition' ENABLED (interval=3600s)
[simard] OODA daemon: cognitive thread 'consolidation' ENABLED (interval=21600s)
[simard] OODA daemon: cognitive thread 'reflection' ENABLED (interval=5400s)
[simard] OODA daemon: cognitive thread 'prospection' ENABLED (interval=4500s)
[simard] OODA daemon: cognitive thread 'salience' ENABLED (interval=1800s)
[simard] OODA daemon: cognitive thread 'operator_model' ENABLED (interval=7200s)
[simard] OODA daemon: cognitive thread 'analogy' ENABLED (interval=9000s)
[simard] OODA daemon: cognitive thread 'values_deliberation' ENABLED (interval=10800s)
[simard] OODA daemon: cognitive thread 'narrative' ENABLED (interval=43200s)
[simard] OODA daemon: cognitive thread 'interoception' ENABLED (interval=3300s)
[simard] OODA daemon: cognitive thread 'creative_ideas' ENABLED (interval=…s)
```

A thread opted out logs `DISABLED (operator opt-out)`; the
master gate off logs the roster as disabled. Reactive/cadence-less members (none
today) would log `ENABLED (reactive)` and be excluded from stall
detection by design.

## Telemetry — `simard.thread.<id>.*`

Every registered thread emits real OpenTelemetry instruments through the shared
facade; identity lives in the **metric name** (never as an attribute):

| Suffix | Type | Meaning |
|---|---|---|
| `runs` | counter | Attempts that actually ran. |
| `successes` | counter | Runs that completed successfully. |
| `failures` | counter | Runs that failed or panicked (same branch that records the durable diagnosis). |
| `duration_seconds` | histogram | Per-run wall-clock duration. |
| `last_run_epoch` | gauge | Epoch of last completed run (`now − last_run_epoch` = last-run age). |
| `next_run_epoch` | gauge | Epoch of next scheduled run — the primary **stall** signal. |
| `active` | gauge | `1` while mid-tick, `0` otherwise. |

`runs = successes + failures` by construction. These series ride the existing
registry → `metrics_snapshot.json` → OTLP path with no schema bump. Values are
**real** — never hardcoded or synthesised. Reuse the #4786 telemetry facade; do
not duplicate it.

Inspect the live snapshot:

```bash
simard status --metrics | grep '^simard.thread.'
# or read the raw snapshot
jq '.metrics | to_entries[] | select(.key|startswith("simard.thread."))' \
  ~/.simard/state/metrics_snapshot.json
```

## Overseer auto-remediation

The acting Overseer knows **every** thread from the single-source-of-truth
registry (`CognitiveThread::purpose()` + `policy()`, enumerated via
`Mind::health()`). On each `health_review` it:

1. **Detects** any scheduled thread that has **failed**, is **stalled/silent**
   past its expected cadence (`now − next_run_epoch` beyond grace), or is
   **erroring** — using the per-thread telemetry + a bounded `~/.simard/ooda.log`
   tail. Reactive/cadence-less threads are **excluded from stall detection** by
   design.
2. **Dispatches exactly one** de-duplicated agentic remediation goal per failure
   signature, reusing the **existing** pipeline in `overseer/mod.rs`:

   ```text
   Signal::Anomaly
     → Problem { ProcessHealth, dedup_key = "anomaly:{detail}" }
       → agentic Act (recipe_dedup_key)
         → dedup + rate-limit
           → Signal notify (fan-out)
   ```

   The dedup key is the **content-free anomaly signature**, so a recurring
   failure never spawns duplicate fix-goals. The existing per-cycle anomaly cap +
   rate-limiter prevent a remediation storm when many threads fail at once. No
   parallel remediation rail is built.
3. **Notifies** via the existing Signal fan-out.

Detection is **structural** — it keys on stable anomaly signatures and telemetry
gauges, never by scraping JSON or prose from agent stdout (respects the
antipattern-removal epic #4719).

### Failures are never swallowed

Every thread failure is **dual-routed**:

- incremented on `simard.thread.<id>.failures`, **and**
- written to the durable Overseer-readable `failure_sink`
  (`FailureCause::CognitiveThread` → `Signal::StepFailureDiagnosed`).

A failure inside a thread is therefore visible to the Overseer through **both**
channels, and a panic is isolated so it cannot down the daemon.

## Examples

### Run everything on defaults

No configuration needed — start the daemon and all 13 scheduled threads plus OODA
run:

```bash
simard daemon start
grep 'thread .* ENABLED' ~/.simard/ooda.log
```

### Opt a single thread out

```bash
# Disable just narrative; everything else stays on.
export SIMARD_THREAD_NARRATIVE_ENABLED=0
simard daemon restart
```

### Disable the whole reflective roster (keep OODA + creative_ideas)

```bash
# Master opt-out. OODA (always on) and creative_ideas (its own gate) are unaffected.
export SIMARD_COGNITIVE_THREADS_ENABLED=off
simard daemon restart
```

### Speed up salience for a debugging session

```bash
export SIMARD_THREAD_SALIENCE_INTERVAL_SECS=120   # clamped up to 60s floor
simard daemon restart
```

### Throttle the per-tick fan-out

```bash
# Cap non-critical ticks at 4 per pass (long-cadence threads simply wait a pass).
export SIMARD_MIND_MAX_NONCRITICAL_PER_TICK=4
simard daemon restart
```

## Tutorial — verify detect → dispatch → dedupe → notify

Reproduce the auto-remediation contract end-to-end:

1. **Start** the daemon on defaults and confirm the roster is enabled:

   ```bash
   simard daemon start
   grep 'cognitive-thread scheduler ENABLED' ~/.simard/ooda.log
   ```

2. **Inject a failure** into one thread (e.g. point a thread's recipe at a
   failing invoker in a test harness, or use the fault-injection test fixture).
   The thread's next tick fails.

3. **Confirm the failure is counted**:

   ```bash
   simard status --metrics | grep 'simard.thread.<id>.failures'
   ```

4. **Confirm detection + a single dispatch**. The Overseer's next `health_review`
   records an anomaly and dispatches **one** remediation goal:

   ```bash
   grep -E 'anomaly:.*<id>|remediation goal dispatched' ~/.simard/ooda.log
   simard goals list | grep -i '<id>'   # exactly one fix goal
   ```

5. **Confirm dedupe**. Let the same failure recur — no second goal appears; the
   content-free `anomaly:{detail}` dedup key suppresses the duplicate.

6. **Confirm notification** via the Signal fan-out (operator notification /
   Signal channel).

## Security posture

This feature adds **no new security mechanism**; it rides existing hardened
contracts:

- **Fail-closed gate (SR-12).** `env_gate_open` is a pure predicate; explicit
  falsy always disables, and unknown/empty values honour the opt-out default.
- **Exec-boundary injection (SR-7/8).** Now-live threads feed memory/log-derived
  text into remediation `-c` values. Distinct argv pairs (no `sh -c`),
  control-char stripping via `sanitize_value`, and fenced `ContextFile` transport
  ensure `\n`/`\r`/`\0`/`-c evil` cannot inject an argv pair or prompt line.
- **Recipe-dir hardening (SR-4).** A group/world-writable hot recipe dir is
  rejected with a logged in-tree fallback.
- **DoS bounds.** `MIN_INTERVAL_SECS = 60` floor + interval-scale clamp prevent a
  zero-interval busy loop; the existing dedup + rate-limiter bound remediation
  storms.
- **Info-leak.** Telemetry labels use the static thread `id` only; failure logs
  are bounded and never echo payloads or executable paths.
- **No swallowed failures (SR-9).** Every failure is dual-routed; panics are
  isolated.

## See also

- [Cognitive-threads catalog](./cognitive-threads-catalog.md) — per-thread vision,
  recipe envelopes, memory prefixes.
- [Cognitive-thread observability](./cognitive-thread-observability.md) — metric
  catalog and Overseer oversight seams.
- [Cognitive-thread scheduling](./cognitive-thread-scheduling.md) — the scheduler
  contract and `Mind` internals.
- [Configure reflective cognitive threads](../howto/configure-reflective-cognitive-threads.md)
  — task-oriented enable/disable/tune guide.
- [Overseer tick self-healing](./overseer-tick-self-healing.md) — the remediation
  pipeline this feature reuses.
