---
title: Automatic distillation scheduler API
description: Rust API reference for Simard's automatic promotion scheduler that runs episode-to-fact/procedure distillation at the end of every OODA cycle — the run_scheduled_distillation entry point, the distill_trigger pure predicate and DistillSchedule, the distill_min_episodes / distill_interval_cycles OodaConfig fields, the OodaState.last_distill_cycle field, and the distillation procedures extension (DistilledProcedure, DistillOutput, DistillRecipeRunner::run_all, DistillReport.procedure_count, the additive recipe schema, and store_procedure_with_provenance wiring).
last_updated: 2026-06-20
owner: simard
doc_type: reference
related:
  - ../architecture/episode-ingestion-policy.md
  - ../architecture/episode-distillation.md
  - ./episode-ingestion-classifier.md
  - ./cognitive-memory-provenance.md
  - ./ooda-procedural-memory.md
  - ../howto/configure-episode-hygiene-and-promotion.md
---

# Automatic distillation scheduler API

> Shipped in issue [#2327](https://github.com/rysweet/Simard/issues/2327).
> Scheduler: `src/memory_consolidation/scheduler.rs`; end-of-cycle wiring:
> `src/ooda_loop/cycle.rs`; config: `src/ooda_loop/types.rs`;
> distillation extension: `src/memory_consolidation/distillation.rs`;
> recipe: `prompt_assets/simard/recipes/distill-episodes.yaml`.

The scheduler runs episode→fact/procedure distillation **automatically**
at the end of every OODA cycle, decoupled from the OODA brain's action
choice. It is **additive**: the `ConsolidateMemory` action still triggers
distillation when the brain chooses it. This page is the executable
contract; for rationale see
[Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md).

---

## Schedule + trigger

### `DistillSchedule`

The scheduler's configuration, built each cycle from [`OodaConfig`](#configuration):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistillSchedule {
    pub min_episodes: u32,    // undistilled-count threshold
    pub interval_cycles: u32, // cycle-count interval
}

impl DistillSchedule {
    pub const DEFAULT_MIN_EPISODES: u32 = 25;
    pub const DEFAULT_INTERVAL_CYCLES: u32 = 50;
}
// `Default` yields { 25, 50 }.
```

### `distill_trigger`

The pure, deterministic core — extracted so the trigger can be tested
without standing up a full cycle:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistillTrigger { Threshold, Interval, None }

pub fn distill_trigger(
    undistilled_count: u32,
    cycles_since_last: u32,
    schedule: &DistillSchedule,
) -> DistillTrigger {
    if undistilled_count >= schedule.min_episodes {
        DistillTrigger::Threshold
    } else if cycles_since_last >= schedule.interval_cycles {
        DistillTrigger::Interval
    } else {
        DistillTrigger::None
    }
}
```

- **Threshold trigger** — promote eagerly once enough episodes accumulate
  (takes precedence over the interval).
- **Interval trigger** — guarantee forward progress on quiet runs. It is a
  **cycle-count** delta (`cycle_count − last_distill_cycle`), not
  wall-clock, so it is deterministic in tests.

### `run_scheduled_distillation_with_runner`

The testable IO seam: evaluates the trigger against the (capped) undistilled
count and runs the pass with an injected runner.

```rust
pub fn run_scheduled_distillation_with_runner(
    memory: &dyn CognitiveMemoryOps,
    runner: &dyn DistillRecipeRunner,
    schedule: &DistillSchedule,
    cycles_since_last: u32,
) -> SimardResult<Option<DistillReport>>;
```

**Algorithm:**

1. `let count = memory.list_undistilled_episodes(schedule.min_episodes).len() as u32;`
   — the limit is capped at `min_episodes` so counting never forces a
   full-table scan; we only need to know whether the count *reaches* the
   threshold.
2. `match distill_trigger(count, cycles_since_last, schedule)`:
   - `None` → return `Ok(None)`; the runner is never invoked, no
     facts/procedures stored, no episodes marked.
   - `Threshold` / `Interval` → run
     `distill_recent_episodes_with_runner(memory, runner)` and return
     `Ok(Some(report))`.

### `run_scheduled_distillation` (production)

The production wrapper used by the cycle. Checks the trigger BEFORE
constructing the (potentially expensive) `recipe-runner-rs` subprocess
runner, and returns `Ok(None)` both when no trigger fires AND when the runner
cannot be constructed (recipe-runner-rs absent, recipe file missing, no agent
binary) — promotion must never block the OODA cycle.

```rust
pub fn run_scheduled_distillation(
    memory: &dyn CognitiveMemoryOps,
    repo_root: &Path,
    schedule: &DistillSchedule,
    cycles_since_last: u32,
) -> SimardResult<Option<DistillReport>>;
```

### End-of-cycle wiring

`run_ooda_cycle_inner` (`src/ooda_loop/cycle.rs`), immediately after
`state.cycle_count += 1`, builds a `DistillSchedule` from `config`, computes
`cycles_since_last = cycle_count − last_distill_cycle`, and calls
`run_scheduled_distillation`. On `Ok(Some(report))` it sets
`state.last_distill_cycle = state.cycle_count` and logs:

```
[simard] OODA distill scheduler: 27 episodes → 4 facts, 1 procedures, 27 marked
```

A distillation `Err` is logged (`eprintln!`) and **swallowed at the cycle
boundary** — promotion must never abort the OODA cycle (mirrors the existing
consolidation error handling). The retry-safety invariant inside distillation
(no markers set on recipe error) guarantees the batch is retried on the next
trigger.

### Relationship to `ConsolidateMemory`

Unchanged. `dispatch_consolidate_memory` (`src/ooda_actions/simple_actions.rs`)
still calls `distill_recent_episodes` when the brain emits the `__memory__`
synthetic priority. The scheduler is an independent, automatic second
trigger. Double-firing in one cycle is harmless and **idempotent**: the
first pass marks its inputs distilled, so the second
`list_undistilled_episodes` returns a shrunken/empty set and skips under
threshold.

---

## Configuration

Two fields are added to `OodaConfig` (`src/ooda_loop/types.rs`), both with
env-driven defaults via the existing `env_u32` helper:

```rust
pub struct OodaConfig {
    // … existing fields …
    /// Undistilled-backlog size that triggers automatic promotion.
    pub distill_min_episodes: u32,    // default 25
    /// Cycle-count interval that guarantees promotion on quiet runs.
    pub distill_interval_cycles: u32, // default 50
}
```

| Field | Env var | Default |
|---|---|---:|
| `distill_min_episodes` | `SIMARD_DISTILL_MIN_EPISODES` | 25 |
| `distill_interval_cycles` | `SIMARD_DISTILL_INTERVAL_CYCLES` | 50 |

> **Const vs config.** The internal
> `const DISTILL_MIN_EPISODES = 20` in `distillation.rs` is **retained as
> a hard floor** for `distill_recent_episodes_with_runner` (which has no
> `config` handle). The scheduler in `cycle.rs` owns the **config**
> threshold (`distill_min_episodes`). The two are distinct: the const is
> the pass-internal skip floor; the config field is the scheduler policy.

### `OodaState.last_distill_cycle`

```rust
pub struct OodaState {
    // … existing fields …
    pub last_distill_cycle: u32, // default 0
}
```

Added to `OodaState` (it reuses the existing `cycle_count`; there is no
wall-clock dependency). It is intentionally **not** part of
`OodaStateSnapshot` — the interval resets to 0 across recipe-step round-trips
and daemon restarts, whose only effect is at most one extra distillation pass
shortly after boot.

---

## Distillation procedures extension

Provenance for **facts** already shipped (#2325/#2326). This feature adds
**procedures**.

### Types

```rust
/// A reusable action sequence distilled from a batch of episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledProcedure {
    pub name: String,
    pub steps: Vec<String>,
    pub source_episode_ids: Vec<String>,
}

/// Full distillation output. `run` keeps returning facts only;
/// `run_all` is the superset entry the production path uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistillOutput {
    pub facts: Vec<DistilledFact>,
    pub procedures: Vec<DistilledProcedure>,
}
```

### Trait evolution — backward compatible

`DistillRecipeRunner::run` is unchanged, so existing stubs compile
untouched. A **defaulted** `run_all` is added:

```rust
pub trait DistillRecipeRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>>;

    /// Default delegates to `run` and yields zero procedures, so every
    /// existing fact-only stub keeps working without edits.
    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        Ok(DistillOutput { facts: self.run(episodes)?, procedures: Vec::new() })
    }
}
```

`distill_recent_episodes_with_runner` calls `runner.run_all(&episodes)`.
After the existing fact-store loop it stores procedures with provenance:

```rust
for proc in &output.procedures {
    memory.store_procedure_with_provenance(
        &proc.name,
        &proc.steps,
        &[],                       // prerequisites
        &proc.source_episode_ids,  // PROCEDURE_DERIVES_FROM provenance
    )?;
}
// then, for every input episode:
memory.mark_episode_distilled(&episode.node_id)?;
```

`store_procedure_with_provenance` is idempotent (upsert-by-name with
reinforcement), so a re-derived procedure reinforces rather than
duplicates. See
[Cognitive-memory provenance](./cognitive-memory-provenance.md) and
[OODA procedural memory](./ooda-procedural-memory.md).

### `DistillReport.procedure_count`

```rust
pub struct DistillReport {
    pub input_count: u32,
    pub fact_count: u32,
    pub procedure_count: u32, // NEW
    pub marked_count: u32,
}
```

`DistillReport` is `Default`, so `skipped()` and `was_skipped()` are
unaffected — a skipped pass reports `procedure_count == 0`.

### Recipe schema (additive)

`prompt_assets/simard/recipes/distill-episodes.yaml` now emits an optional
`procedures` array alongside `facts`:

```json
{
  "facts":      [ { "concept": "...", "content": "...", "source_episode_id": "..." } ],
  "procedures": [ { "name": "...", "steps": ["..."], "source_episode_ids": ["..."] } ]
}
```

The parser is forward/backward compatible:

```rust
#[derive(serde::Deserialize)]
struct RecipeEnvelope {
    facts: Vec<RecipeFact>,
    #[serde(default)]                 // old fact-only stdout still parses
    procedures: Vec<RecipeProcedure>,
}

#[derive(serde::Deserialize)]
struct RecipeProcedure {
    name: String,
    steps: Vec<String>,
    source_episode_ids: Vec<String>,
}
```

`procedures` defaults to empty, so fact-only recipe output (and every
existing fact-only stub runner) continues to work unchanged.

---

## Examples

### Threshold trigger fires distillation

```
cycle_count = 8, last_distill_cycle = 0
distill_min_episodes = 25, distill_interval_cycles = 50
schedule = DistillSchedule { min_episodes: 25, interval_cycles: 50 }

count         = list_undistilled_episodes(25).len() = 25 (capped)
cycles_since  = 8

distill_trigger(25, 8, &schedule) = Threshold   // 25 >= 25
  → distill_recent_episodes runs
  → 4 facts + 1 procedure stored with provenance, 27 episodes marked
  → last_distill_cycle = 8

[simard] OODA distill scheduler: 27 episodes → 4 facts, 1 procedures, 27 marked
```

### Interval trigger fires on a quiet run

```
cycle_count = 60, last_distill_cycle = 5
distill_min_episodes = 25, distill_interval_cycles = 50
schedule = DistillSchedule { min_episodes: 25, interval_cycles: 50 }

count         = 6      (below threshold)
cycles_since  = 55

distill_trigger(6, 55, &schedule) = Interval   // 55 >= 50
  → fires anyway; the trickle of 6 episodes is promoted
```

### Below both thresholds — skip

```
count = 6, cycles_since = 10
distill_trigger(6, 10, &schedule) = None
  → no runner call; run_scheduled_distillation returns Ok(None)
```

### A distilled procedure with provenance

A runner whose `run_all` yields:

```rust
DistillOutput {
    facts: vec![],
    procedures: vec![DistilledProcedure {
        name: "rebuild-after-cargo-toml-edit".into(),
        steps: vec!["cargo update".into(), "cargo build --release".into()],
        source_episode_ids: vec!["ep-12".into(), "ep-19".into()],
    }],
}
```

produces one `store_procedure_with_provenance("rebuild-after-cargo-toml-edit",
["cargo update", "cargo build --release"], [], ["ep-12", "ep-19"])` call,
a `PROCEDURE_DERIVES_FROM` edge to each source episode, and
`report.procedure_count == 1`.

---

## Related

- [Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md) —
  design rationale
- [Episode distillation](../architecture/episode-distillation.md) — the
  fact-extraction pipeline this scheduler drives
- [Episode ingestion classifier API](./episode-ingestion-classifier.md) —
  the hygiene half
- [Cognitive-memory provenance](./cognitive-memory-provenance.md) — the
  `DERIVES_FROM` / `PROCEDURE_DERIVES_FROM` edges
- [Configure episode hygiene and promotion](../howto/configure-episode-hygiene-and-promotion.md) —
  operator tuning and observability
