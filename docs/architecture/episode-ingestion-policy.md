---
title: Episode ingestion policy & automatic promotion
description: How Simard keeps episodic memory clean and self-promoting — a deterministic classifier that drops or down-scopes operational-noise episodes before they are stored, plus an automatic scheduler that distills recurring episodes into semantic facts and procedures (with provenance) without waiting for the OODA brain to choose ConsolidateMemory.
last_updated: 2026-06-20
owner: simard
doc_type: concept
related:
  - ./episode-distillation.md
  - ./cognitive-memory.md
  - ../reference/episode-ingestion-classifier.md
  - ../reference/automatic-distillation-scheduler.md
  - ../reference/cognitive-memory-provenance.md
  - ../howto/configure-episode-hygiene-and-promotion.md
  - ../memory.md
---

# Episode ingestion policy & automatic promotion

> Shipped in issue [#2327](https://github.com/rysweet/Simard/issues/2327)
> as `feat(cog-mem): episode ingestion policy + automatic promotion
> (distillation) scheduler`. Builds on episode distillation
> ([#2281](https://github.com/rysweet/Simard/issues/2281)) and provenance
> wiring ([#2325](https://github.com/rysweet/Simard/issues/2325) /
> [#2326](https://github.com/rysweet/Simard/issues/2326)).

Two related defects degraded the quality of Simard's episodic memory:

1. **Noise.** Every OODA cycle wrote bookkeeping episodes —
   `Session … started with objective`, `… completed and persisted`,
   `flushing working memory to episodes`, and `brain: continue_skipping
   (no decision keyword …)` markers. These low-signal rows diluted recall,
   inflated the undistilled backlog, and wasted distillation LLM calls.
2. **Stalled promotion.** Distillation only ran when the OODA brain
   *happened* to choose the `ConsolidateMemory` action. On quiet or
   tightly-focused runs the brain rarely chose it, so episodes piled up
   undistilled and never became durable facts or procedures.

This feature closes both gaps:

- **HYGIENE** — a deterministic, pure **classifier** sits in front of
  every `store_episode` intake site. It **drops** or **down-scopes**
  operational-noise episodes and **stores** meaningful ones with
  structured metadata.
- **PROMOTION** — an **automatic scheduler** runs distillation at the end
  of every OODA cycle whenever the undistilled backlog reaches a
  threshold *or* a cycle-count interval elapses — independent of the
  brain's action choice. Distillation now emits **procedures** as well as
  facts, both written **with provenance**.

The two halves are independent bricks and can be reasoned about
separately. This page is the design rationale; the executable contracts
live in the two reference pages linked above.

---

## Part 1 — Episode ingestion policy (hygiene)

### The classifier chokepoint

Every place that stores an episode routes through one helper,
`store_episode_classified`, instead of calling `store_episode` directly.
That helper asks a pure function, `classify`, what to do with the
`(content, source_label, context)` triple and gets back one of three
decisions:

```
                 ┌──────────────────────────────┐
 content,        │          classify()          │
 source_label, ──▶   (pure, no IO, testable)     │
 IntakeContext   └──────────────┬───────────────┘
                                │
              ┌─────────────────┼──────────────────┐
              ▼                 ▼                  ▼
            Drop            DownScope            Store
       (not stored)    (stored, operational)  (stored, durable)
       dropped += 1     downscoped += 1        stored += 1
```

`Drop` never calls `store_episode`. `Store` and `DownScope` both call it,
but attach different metadata — `DownScope` flags the episode
`is_operational = true` with `importance = 0.1`, while `Store` records the
real `event_kind` and a higher importance. The split is observable at the
`store_episode` call boundary (see
[the classifier reference](../reference/episode-ingestion-classifier.md)
for why behaviour is asserted there rather than by reading episodes back).

### Decision logic (strict priority order)

The classifier evaluates four rules top-to-bottom and returns on the
first match:

1. **Failure override (highest priority).** If the content carries a
   **whole-word** failure signal — a word from the `error` / `fail` /
   `failure` / `panic` / `exception` family, matched at word boundaries and
   including inflections (`errors`, `failed`, `panicked`, `exceptions`, …) and
   compound PascalCase type names (`ParseError`, `NullPointerException`), but
   **not** coincidental look-alikes that merely embed a stem (`exceptional`,
   `hispanic`, `terror`, `mirror`) — the episode is **stored** at
   `importance = 0.9` — even if it also looks like noise. A failed "session
   complete" is still a failure worth keeping. The kind is `RecipeFailure`
   when the content/source mentions a recipe, `ActionFailure` otherwise.
2. **Meaningful content → Store.** Content/source matching a durable
   episodic event — user decisions, goal-board promotions/archival,
   handoffs, durable completions (opened/merged PR), or any
   `goal-curator` board summary — stores the episode with the importance
   from the table below. Evaluated **before** the known-noise drop rule so a
   durable signal is retained even when the same episode also mentions a
   bookkeeping noise phrase (a handoff log that ends `… flushing working
   memory`) — the same precedence the failure override applies. Checking
   noise first discarded these dual-signal episodes, losing the durable
   signal to distillation (a fact-yield loss).
3. **Known-noise markers → Drop.** A small allowlist of substrings —
   `started with objective`, `completed and persisted`, `flushing working
   memory`, `continue_skipping`, `no decision keyword` — is dropped. Only
   reached once rules 1–2 find no higher-value signal, so a **pure-noise**
   episode still drops.
4. **Default → DownScope.** Anything unrecognised — including the
   cross-session hydration bookkeeping (`Hydrated N prior-session facts …`)
   — is **stored down-scoped**, never dropped. We never silently lose a
   novel event; we only de-prioritise it.

The "default is down-scope, not drop" rule is the safety valve: `Drop` is
restricted to the explicit known-noise allowlist, so an unforeseen event
type degrades to low importance rather than disappearing.

### Importance & event-kind taxonomy

Every stored or down-scoped episode carries this metadata JSON:

```json
{
  "importance": 0.9,
  "event_kind": "action_failure",
  "goal_id": "improve-foo",
  "cycle": 42,
  "is_operational": false
}
```

| `event_kind` | importance | `is_operational` | Example |
|---|---:|:---:|---|
| `action_failure` / `recipe_failure` | 0.90 | false | An action or recipe returned an error |
| `user_decision` | 0.85 | false | Operator chose a path in a meeting |
| `handoff` | 0.80 | false | Meeting decision handed to the engineer loop |
| `goal_promotion` / `goal_archival` | 0.80 | false | Goal moved into the active top-5 or force-removed |
| `action_completed` | 0.70 | false | An action finished with a durable outcome |
| `operational` | 0.10 | true | Unclassified / down-scoped bookkeeping |
| *(dropped)* | — | — | Known-noise marker, no failure signal — **not stored** |

`goal_id` and `cycle` are threaded from the calling site's
`IntakeContext`; either may be `null` when the site has no such context.

### The intake sites

These are the `store_episode` chokepoints the classifier guards. Each is
wired through `store_episode_classified`:

| Site | What it writes | Default decision |
|---|---|---|
| Session intake | `Session … started with objective …` | **Drop** (unless failure override) |
| Session reflection | concatenated cycle transcript | **Sanitize** (see below) |
| Session persistence | `Session … completed and persisted` | **Drop** |
| Hydration | `Hydrated N prior-session facts …` | **DownScope** (operational) |
| Working-memory flush (per slot) | slot content | **Classify per content** |
| Working-memory flush marker | `Session … flushing working memory …` | **Drop** |
| Goal-curator persistence | active-goal board summary | **Store** (`goal_archival` when goals were force-removed) |

### Reflection transcript sanitization

The reflection path is special: the transcript episode it writes returns
the `episode_id` that fact provenance later links against. Dropping it
outright would orphan that provenance edge. So the classifier
**sanitizes** the transcript instead:

- Lines containing `continue_skipping` or `no decision keyword` are
  stripped.
- If the transcript carries a failure signal (`panic`, `error`, …) the
  **original is kept whole** — we never strip a transcript that records a
  failure.
- If only noise survives **and** facts are still derived this cycle, a
  **down-scoped** transcript episode is kept so its id remains available
  for `store_fact_with_provenance`.
- If only noise survives **and** no facts are derived, the transcript is
  dropped and fact-linking is skipped for that cycle.

This preserves provenance integrity while still suppressing the bulk of
the per-cycle noise volume.

### Observability

Each cycle emits one aggregated counter line:

```
[simard] episode-intake dropped=7 stored=3 downscoped=2
```

backed by low-cardinality `tracing` counters, suitable for grep-based
monitoring.

---

## Part 2 — Automatic promotion (distillation scheduler)

### From opt-in to automatic

Before #2327, distillation fired only inside the `ConsolidateMemory`
action handler — i.e. only when the OODA brain chose to consolidate. The
scheduler adds a **second, automatic trigger** that runs at the end of
every OODA cycle, decoupled from action selection. `ConsolidateMemory`
still works exactly as before; the scheduler is **additive**.

```
run_ooda_cycle:
   … observe → orient → decide → act …
   state.cycle_count += 1
   └─▶ scheduler::run_scheduled_distillation(   ◀── NEW end-of-cycle hook
          &*clients.memory, &clients.repo_root, &schedule, cycles_since_last)
```

### Trigger predicate

The scheduler fires when **either** condition holds:

```
backlog       = list_undistilled_episodes(distill_min_episodes).len()
cycles_since  = cycle_count − last_distill_cycle

fire = backlog >= distill_min_episodes
    OR cycles_since >= distill_interval_cycles
```

- The **backlog** trigger promotes eagerly when episodes accumulate. The
  count query is capped at the threshold so it never forces a full-table
  scan — it only needs to know whether the count *reaches* the threshold.
- The **interval** trigger guarantees forward progress on quiet runs: even
  a trickle of episodes gets promoted at least once every
  `distill_interval_cycles` cycles. It is a **cycle-count** interval, not
  wall-clock, so behaviour is deterministic in tests.

On firing, the scheduler runs one distillation pass and sets
`last_distill_cycle = cycle_count`.

### Configuration

Two `OodaConfig` fields drive the scheduler, both env-overridable:

| Field | Env var | Default |
|---|---|---:|
| `distill_min_episodes` | `SIMARD_DISTILL_MIN_EPISODES` | 25 |
| `distill_interval_cycles` | `SIMARD_DISTILL_INTERVAL_CYCLES` | 50 |

`last_distill_cycle` lives on `OodaState` (not its snapshot), so the interval
resets across restarts — at most one extra pass shortly after boot.

### Distillation now emits procedures too

Distillation already produced **facts** with provenance. The promotion
work extends it to also emit **procedures** — recurring action sequences
distilled from a batch of episodes — stored via
`store_procedure_with_provenance` so they keep a `PROCEDURE_DERIVES_FROM`
edge back to their source episodes:

```
Recipe output: { facts: [ … ], procedures: [ { name, steps, source_episode_ids } ] }
   │
   ├─ for each fact:       store_fact_with_provenance(…, source_episode_ids)
   ├─ for each procedure:  store_procedure_with_provenance(name, steps, [], source_episode_ids)
   ▼
 for EVERY input episode:  mark_episode_distilled(node_id)
```

The recipe schema change is additive (`procedures` defaults to empty), so
fact-only output keeps working. See
[episode distillation](./episode-distillation.md) for the fact half and
[the scheduler reference](../reference/automatic-distillation-scheduler.md)
for the procedure types and trait evolution.

### Idempotent double-firing

Because both the `ConsolidateMemory` action and the scheduler can fire in
the same cycle, distillation must be idempotent — and it is. The first
pass marks its inputs distilled, so the second pass sees a shrunken (or
empty) backlog and skips under threshold. Procedure storage is also
idempotent (upsert-by-name with reinforcement), so a re-derived procedure
reinforces rather than duplicates.

### Failure handling

A distillation error inside the scheduler is logged and **swallowed at the
cycle boundary** — promotion must never abort an OODA cycle. The
retry-safety invariant inside distillation (no `mark_episode_distilled`
calls on recipe error) guarantees the same batch is retried on the next
interval.

---

## Why this is two bricks, not one

The classifier is a pure decision function with a thin IO seam; the
scheduler is a cycle-boundary trigger over the existing distillation
pipeline. They share a goal (a cleaner, self-promoting episodic store) but
have no code coupling: you can change the noise allowlist without touching
the scheduler, and tune the trigger thresholds without touching the
classifier. Each is independently testable and regeneratable.

---

## Related

- [Episode distillation](./episode-distillation.md) — the fact-extraction
  pipeline the scheduler drives
- [Episode ingestion classifier reference](../reference/episode-ingestion-classifier.md) —
  `classify`, `sanitize_transcript`, `EventKind`, `EpisodeMetadata`,
  `IntakeDecision`, `store_episode_classified`
- [Automatic distillation scheduler reference](../reference/automatic-distillation-scheduler.md) —
  `run_scheduled_distillation`, `distill_trigger`, `DistillSchedule`,
  `DistilledProcedure`, `DistillOutput`, config fields
- [Cognitive-memory provenance](../reference/cognitive-memory-provenance.md) —
  the `DERIVES_FROM` / `PROCEDURE_DERIVES_FROM` edges this feature writes
- [Configure episode hygiene and promotion](../howto/configure-episode-hygiene-and-promotion.md) —
  operator tuning and observability
- [Memory architecture](../memory.md) — the six-type overview
