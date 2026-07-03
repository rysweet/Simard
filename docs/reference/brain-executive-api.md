---
title: Brain Executive API
description: Reference for the `Brain` executive — the cognitive-thread scheduler at the heart of Simard's one Brain. Covers registry, due-computation, priority budget, failure isolation, health snapshots, the CognitiveThread/ThreadKind surface, and the OodaContext bundle.
last_updated: 2026-07-03
owner: simard
doc_type: reference
related:
  - ../architecture/brain-model.md
  - ./cognitive-thread-scheduling.md
  - ./ooda-brain-api.md
  - ./brain-terminology-migration.md
---

# Reference: Brain Executive API

Crate: `simard` · Module: `simard::cognitive_threads`

The **`Brain`** is the executive of Simard's one cognition — the
cognitive-thread scheduler that owns a registry of threads, computes which are
due each daemon tick, and runs them under a priority budget that never starves
OODA. Conceptually and in code, the `Brain` *is* the whole cognition: it owns
the thread scheduler, the reasoners used by the OODA thread (via
[`OodaContext`](#oodacontext)), and the cognitive-memory handle.

!!! note "Behavior-preserving"
    This surface is a pure rename/facade cleanup — the scheduler's budget,
    backoff, ordering, and telemetry are unchanged. The `CognitiveThread` trait
    and `ThreadKind` enum are the Brain's threads/processes and are kept. The
    non-critical fan-out budget is still read from the frozen env literal
    `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` (an allow-listed wire value) through a
    `BRAIN_*` const. For the historical old→new map see the
    [terminology migration](./brain-terminology-migration.md).

## The `Brain` struct

```rust
pub struct Brain { /* threads: Vec<ThreadEntry>, budget: RunBudget */ }
```

### Construction

```rust
impl Brain {
    /// Build a `Brain` with the non-critical per-tick budget read from
    /// the frozen env literal `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK`
    /// (default 2), via the `BRAIN_NONCRITICAL_BUDGET_ENV` const.
    pub fn new() -> Self;

    /// Build a `Brain` with an explicit non-critical per-tick budget
    /// (test seam — avoids env mutation).
    pub fn with_budget(max_noncritical_per_tick: usize) -> Self;
}

impl Default for Brain { fn default() -> Self { Self::new() } }
```

### Registry

```rust
/// Register a thread (chainable).
pub fn register(&mut self, thread: Box<dyn CognitiveThread>) -> &mut Self;

/// Number of registered threads.
pub fn len(&self) -> usize;

/// Whether no threads are registered.
pub fn is_empty(&self) -> bool;
```

### Due-computation (pure)

```rust
/// Registration-order indices of enabled, non-backed-off, due threads at
/// `now_epoch`. Pure over injected time — fully unit-testable, no sleeps.
pub fn due_threads(&self, now_epoch: u64) -> Vec<usize>;
```

### Running a tick

```rust
/// Run OODA (`Priority::Critical`) first and unconditionally (budget-exempt,
/// never backed off), then non-critical due threads in priority order up to
/// the per-tick budget. Each tick runs inside `catch_unwind`; a panic/`Err`
/// bumps `consecutive_errors`, sets capped exponential backoff, emits an error
/// metric, and never propagates. Once shutdown is requested no new ticks start.
pub fn run_due(&mut self, ctx: &mut ThreadContext<'_>) -> Vec<ThreadOutcome>;
```

Ordering guarantees:

1. **Phase 1 — Critical (OODA) first**, cadence-respecting but budget-exempt and
   never backed off, so a flood of due background threads can never starve it.
2. **Phase 2 — non-critical due threads**, stable-sorted by ascending
   `Priority` (Critical < High < Normal < Low), capped at the per-tick budget.
3. Shutdown is re-checked between phases and between threads.

### Health snapshot

```rust
/// Health snapshot of every registered thread (dashboard heartbeat feed),
/// built from the scheduler's authoritative bookkeeping.
pub fn health(&self) -> Vec<ThreadHealth>;
```

### Backoff constants (unchanged)

| Constant | Value | Meaning |
| --- | --- | --- |
| `BACKOFF_BASE` | 30 s | Base delay of per-thread capped-exponential backoff. |
| `BACKOFF_CAP` | 30 min | Ceiling — a wedged thread retries at most this slowly. |
| `DEFAULT_BUDGET` | 2 | Default non-critical fan-out per tick. |

## The `CognitiveThread` trait (kept)

The Brain's threads are cognitive processes. The trait and its supporting types
are unchanged by this cleanup.

```rust
/// A single scheduled mental process owned by the `Brain`.
pub trait CognitiveThread: Send {
    fn id(&self) -> &str;
    fn name(&self) -> &str { self.id() }
    fn kind(&self) -> ThreadKind;
    fn policy(&self) -> SchedulePolicy;
    fn priority(&self) -> Priority { Priority::Normal }
    fn enabled(&self) -> bool { true }
    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome;
    fn health(&self) -> ThreadHealth;
}

pub enum ThreadKind {
    Ooda, Maintenance, EngineerLogAnalysis,      // implemented
    BackgroundThought, MemoryConsolidation,       // reserved
    SensoryProcessing, LongTermPlanning,          // reserved
}

pub enum Priority { Critical, High, Normal, Low } // Critical == OODA only
```

See [Cognitive-thread scheduling](./cognitive-thread-scheduling.md) for
`SchedulePolicy`, `ThreadOutcome`, `ThreadHealth`, and `ThreadContext`.

## `OodaContext`

The OODA thread carries all the resources it needs — the memory adapter, the
peer clients, the optional session, and the three reasoners — in one bundle,
**`OodaContext`**. The field/param that threads it through the loop is `ctx`.

```rust
/// All resources needed by the OODA loop — memory, peer clients, session,
/// and the orient/decide/act reasoners.
pub struct OodaContext {
    pub memory: Box<dyn CognitiveMemoryOps>,   // via CognitiveMemoryAdapter
    pub knowledge: KnowledgeClient,
    pub gym: GymClient,
    pub session: Option<Box<dyn BaseTypeSession>>,

    /// Act (engineer-lifecycle) reasoner.
    pub act_reasoner: Arc<dyn ActReasoner>,
    /// Optional decide reasoner.
    pub decide_reasoner: Option<Arc<dyn DecideReasoner>>,
    /// Optional orient reasoner.
    pub orient_reasoner: Option<Arc<dyn OrientReasoner>>,

    pub repo_root: PathBuf,
    pub progress_evidence: Arc<dyn ProgressEvidenceChecker>,
    pub completion_evidence: Option<Arc<dyn EvidenceSource>>,
    pub session_factory: Option<Arc<dyn OrchestratorSessionFactory>>,
    // …
}
```

The bundle is constructed by `ooda_loop::context_from_state_root` for stateless
helper-bin invocations, and inline by the daemon at boot. Loop signatures take
`ctx: &mut OodaContext` — ownership and lifetimes are identical to before this
cleanup.

!!! warning "`OodaContext`, not `OodaReasoners`"
    The bundle holds more than reasoners (memory, gym, knowledge, session), so
    it is named `OodaContext`. "Reasoners" names only the three phase reasoners
    it carries — using it for the whole bundle would be a misnomer.

## Worked example

```rust
use simard::cognitive_threads::{Brain, MaintenanceThread, EngineerLogAnalysisThread};

let mut brain = Brain::new();
brain
    .register(Box::new(MaintenanceThread::new(/* … */)))
    .register(Box::new(EngineerLogAnalysisThread::new(/* … */)));

// Each daemon iteration:
let outcomes = brain.run_due(&mut ctx);   // OODA-first, budgeted, panic-isolated
for h in brain.health() {                 // dashboard heartbeat feed
    println!("{}: last_success={:?}", h.id, h.last_success);
}
```

## See Also

- [The Brain](../architecture/brain-model.md) — the model this executive realizes.
- [Cognitive-thread scheduling](./cognitive-thread-scheduling.md) — the design and full thread-type surface.
- [OODA reasoners API](./ooda-brain-api.md) — the reasoners carried in `OodaContext`.
- [Terminology migration](./brain-terminology-migration.md) — the exhaustive old→new map.
