---
title: Automatic distillation scheduler API
description: Rust API reference for Simard's automatic promotion scheduler that runs episode-to-fact/procedure distillation at the end of every OODA cycle — the maybe_run_promotion entry point, the should_distill pure predicate, the distill_min_episodes / distill_interval_cycles OodaConfig fields, the OodaState.last_distill_cycle field, and the distillation procedures extension (DistilledProcedure, DistillOutput, DistillRecipeRunner::run_full, DistillReport.procedure_count, the additive recipe schema, and store_procedure_with_provenance wiring).
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
> Scheduler: `src/ooda_loop/cycle.rs`; config: `src/ooda_loop/types.rs`;
> distillation extension: `src/memory_consolidation/distillation.rs`;
> recipe: `prompt_assets/simard/recipes/distill-episodes.yaml`.

The scheduler runs episode→fact/procedure distillation **automatically**
at the end of every OODA cycle, decoupled from the OODA brain's action
choice. It is **additive**: the `ConsolidateMemory` action still triggers
distillation when the brain chooses it. This page is the executable
contract; for rationale see
[Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md).

---

## Trigger predicate

### `should_distill`

The pure, deterministic core — extracted so the trigger can be tested
without standing up a full cycle:

```rust
pub fn should_distill(backlog: u32, cycles_since: u32, config: &OodaConfig) -> bool {
    backlog >= config.distill_min_episodes
        || cycles_since >= config.distill_interval_cycles
}
```

- **Backlog trigger** — promote eagerly once enough episodes accumulate.
- **Interval trigger** — guarantee forward progress on quiet runs. It is a
  **cycle-count** delta (`cycle_count − last_distill_cycle`), not
  wall-clock, so it is deterministic in tests.

### `maybe_run_promotion`

The cycle-boundary IO wrapper, invoked from `run_ooda_cycle` immediately
after `state.cycle_count += 1`:

```rust
fn maybe_run_promotion(
    bridges: &OodaBridges,   // provides &dyn CognitiveMemoryOps + repo_root
    config: &OodaConfig,
    state: &mut OodaState,   // reads/updates last_distill_cycle
) -> SimardResult<DistillReport>;
```

**Algorithm:**

1. `let threshold = config.distill_min_episodes;`
2. `let backlog = bridges.memory.list_undistilled_episodes(threshold).len() as u32;`
   — the limit is capped at `threshold` so counting never forces a
   full-table scan; we only need to know whether the count *reaches* the
   threshold.
3. `let cycles_since = state.cycle_count.saturating_sub(state.last_distill_cycle);`
4. If `!should_distill(backlog, cycles_since, config)` → return
   `Ok(DistillReport::skipped())`, no runner call.
5. Otherwise run `distill_recent_episodes(&*bridges.memory, repo_root)`,
   set `state.last_distill_cycle = state.cycle_count`, and log:

   ```
   [simard] promotion: backlog=27 threshold=25 cycles_since=8 → 27 episodes, 4 facts, 1 procedure, 27 marked
   ```

6. **Best-effort.** A distillation `Err` is logged (`eprintln!`) and
   **swallowed at the cycle boundary** — promotion must never abort the
   OODA cycle (mirrors the existing consolidation error handling). The
   retry-safety invariant inside distillation (no markers set on recipe
   error) guarantees the batch is retried on the next interval.

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

Added to both `OodaState` and `OodaStateSnapshot` (with the two
`From` / `apply_to` mappings) so the interval survives recipe-step
round-trips. The scheduler reuses the existing `cycle_count`; there is no
wall-clock dependency.

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
/// `run_full` is the superset entry the production path uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistillOutput {
    pub facts: Vec<DistilledFact>,
    pub procedures: Vec<DistilledProcedure>,
}
```

### Trait evolution — backward compatible

`DistillRecipeRunner::run` is unchanged, so existing stubs compile
untouched. A **defaulted** `run_full` is added:

```rust
pub trait DistillRecipeRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>>;

    /// Default delegates to `run` and yields zero procedures, so every
    /// existing fact-only stub keeps working without edits.
    fn run_full(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        Ok(DistillOutput { facts: self.run(episodes)?, procedures: Vec::new() })
    }
}
```

`distill_recent_episodes_with_runner` calls `runner.run_full(&episodes)`.
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

backlog       = list_undistilled_episodes(25).len() = 27
cycles_since  = 8

should_distill(27, 8, config) = (27 >= 25) || (8 >= 50) = true
  → distill_recent_episodes runs
  → 4 facts + 1 procedure stored with provenance, 27 episodes marked
  → last_distill_cycle = 8

[simard] promotion: backlog=27 threshold=25 cycles_since=8 → 27 episodes, 4 facts, 1 procedure, 27 marked
```

### Interval trigger fires on a quiet run

```
cycle_count = 60, last_distill_cycle = 5
distill_min_episodes = 25, distill_interval_cycles = 50

backlog       = 6      (below threshold)
cycles_since  = 55

should_distill(6, 55, config) = (6 >= 25) || (55 >= 50) = true
  → fires anyway; the trickle of 6 episodes is promoted
```

### Below both thresholds — skip

```
backlog = 6, cycles_since = 10
should_distill(6, 10, config) = false
  → no runner call; DistillReport::skipped() returned
```

### A distilled procedure with provenance

A runner whose `run_full` yields:

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
