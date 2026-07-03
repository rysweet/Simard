---
title: Add a new cognitive thread
description: Developer guide for adding a new scheduled mental process to Simard's daemon (#2419) — implementing the CognitiveThread trait, choosing a SchedulePolicy and Priority, reading config from the environment, registering with the Mind, emitting telemetry through the single facade seam, obeying the safety rules (no println, least-authority ThreadContext, dry-run, protected-path allow-list, injected GhClient), and unit-testing due-computation, failure isolation, and side-effects with no sleeps or network.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/cognitive-thread-scheduling.md
  - ./configure-cognitive-thread-scheduling.md
  - ./configure-self-quality-audit.md
---

# Add a new cognitive thread

Simard runs many background mental processes through one scheduler, the
**Mind**. Adding a new one — a background-thought pass, a memory-consolidation
step, a new maintenance chore — means implementing the `CognitiveThread` trait
and registering it. You do **not** touch the daemon loop, the OODA path, or the
engineer action-slot scheduler (`src/ooda_scheduler/`, which is unrelated).

This guide walks the full path from a new struct to a registered, observable,
tested thread. For the authoritative surface (exact signatures, derives,
metric names, upstream reuse contracts) see
[Cognitive-thread scheduling](../reference/cognitive-thread-scheduling.md). To
tune and observe threads as an operator, see
[Configure and monitor cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md).

## When to use this

Use this guide when you want to add a scheduled process that runs on the daemon
alongside OODA — anything from the vision list (background thought, memory
consolidation, sensory processing, long-term planning) or a new housekeeping
chore. If instead you want to change *engineer* concurrency, that is the AIMD
action-slot scaler (`SIMARD_MAX_CONCURRENT_ACTIONS`), **not** a cognitive
thread — leave `src/ooda_scheduler/` alone.

## Where the code goes

```
src/cognitive_threads/
  thread.rs        # the trait + supporting types (do not change the trait)
  schedule.rs      # pure is_due / next_run / backoff (reuse; do not fork)
  mind.rs          # the scheduler (register with it; do not modify to add a thread)
  telemetry.rs     # the single metric/span helper (emit through this only)
  threads/
    ooda.rs                    # primary thread (reference example)
    maintenance.rs             # exemplar 1 (safe cleanup)
    engineer_log_analysis.rs   # exemplar 2 (improvement finder)
    your_thread.rs             # <-- add your module here
```

Add `mod your_thread;` and a `pub use` in `src/cognitive_threads/threads/mod.rs`,
and register the thread in the daemon setup block (see
[Register with the Mind](#step-4-register-with-the-mind)). Nothing else in the module
needs to change to host a new thread.

!!! warning "Naming rule"
    No type or module may contain the word **`Bridge`** (operator preference).
    Stay within Scheduler / Mind / CognitiveThread / Faculty / Context / Client.

## Step 1 — Implement the trait

The trait is small and **object-safe** (the Mind stores `Box<dyn
CognitiveThread>`). Threads tick **synchronously**; async work runs inside the
tick via `ctx.runtime.block_on(...)`, exactly as the OODA cycle already does.

```rust
use crate::cognitive_threads::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth,
    ThreadKind, ThreadOutcome,
};
use std::time::{Duration, Instant};

pub struct BackgroundThoughtThread {
    interval_secs: u64,
    // in-memory bookkeeping the Mind reads back via health():
    last_run_epoch: Option<u64>,
    consecutive_errors: u32,
}

impl CognitiveThread for BackgroundThoughtThread {
    fn id(&self) -> &str { "background_thought" }          // stable snake_case telemetry key
    fn kind(&self) -> ThreadKind { ThreadKind::BackgroundThought }
    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(self.interval_secs))
    }
    fn priority(&self) -> Priority { Priority::Low }        // never Critical — that is OODA only
    fn enabled(&self) -> bool { self.interval_secs > 0 }    // env-gated on/off

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let started = Instant::now();

        // Do the work. Best-effort: prefer returning ThreadOutcome::failed(..)
        // over panicking (the Mind catches panics as a backstop, but a clean
        // Err keeps telemetry meaningful).
        // Async I/O? run it on the shared runtime:
        //   let result = ctx.runtime.block_on(async { /* ... */ });

        self.last_run_epoch = Some(ctx.now_epoch);
        ThreadOutcome::ok("thought pass complete", started.elapsed())
            .with_detail(serde_json::json!({ "considered": 3 }))
    }

    fn health(&self) -> ThreadHealth {
        // Report what the thread itself knows. The Mind overlays the
        // authoritative scheduling bookkeeping it owns (next_run, backoff)
        // from its private RunBudget when it builds the dashboard snapshot.
        ThreadHealth {
            id: self.id().to_string(),
            enabled: self.enabled(),
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: None,       // filled by the Mind
            last_success: Some(true),
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,  // filled by the Mind
        }
    }
}
```

Key points:

- **`id()` is a compile-time constant** used verbatim as the telemetry key
  (`simard.thread.background_thought.*`). Never derive it from untrusted input —
  that would inject metric cardinality.
- **`priority()` is never `Critical`.** `Critical` is reserved for OODA, which is
  budget-exempt and never backed off. Background work is `Low` (or `Normal`).
- **`tick()` is best-effort and self-contained.** Return `ThreadOutcome::failed`
  on a handled error; the Mind wraps every tick in `catch_unwind` and applies
  capped exponential backoff, so a bad tick can never crash the daemon or a
  sibling.

## Step 2 — Choose a scheduling policy

```rust
pub enum SchedulePolicy {
    Interval(Duration),        // fixed cadence: next_run = last_run + interval
    OnDemand,                  // only when explicitly requested; never auto-due
    EventDriven,               // due when an external predicate/flag fires
    Adaptive { min, max, current }, // reserved; behaves as Interval(current) for now
}
```

- Use **`Interval`** for anything on a clock (almost all threads).
- Use **`OnDemand`** / **`EventDriven`** only for internally-triggered work — the
  trigger must come from the in-process operator/event channel, never from
  attacker-influenceable external input.
- **`Adaptive`** is representable but conservatively degrades to
  `Interval(current)` in this build; don't rely on it changing cadence yet.

Due-ness is computed by pure, injected-clock functions in `schedule.rs`
(`is_due`, `next_run_epoch`, `backoff_until_epoch`) — reuse them; do not
reimplement timing.

## Step 3 — Read config from the environment

Follow the sibling pattern: a `Config` struct with safe defaults and a
`from_env()` constructor. Interval knobs are named `SIMARD_<AREA>_INTERVAL_SECS`
and clamped to a minimum floor (reject `0`/negative so the thread can't become
due every tick — unless `0` intentionally means "disabled" for your thread).

```rust
pub struct BackgroundThoughtConfig {
    pub interval_secs: u64,   // SIMARD_BACKGROUND_THOUGHT_INTERVAL_SECS
    pub dry_run: bool,        // if it has side-effects
}

impl BackgroundThoughtThread {
    pub fn from_env() -> Self {
        let interval_secs = std::env::var("SIMARD_BACKGROUND_THOUGHT_INTERVAL_SECS")
            .ok().and_then(|v| v.parse().ok())
            .unwrap_or(3600)
            .max(/* floor */ 60);
        Self { interval_secs, last_run_epoch: None, consecutive_errors: 0 }
    }
}
```

If your thread needs a **durable** cadence that survives restarts (e.g. a
30-day gate where an in-memory `last_run` would wrongly reset on reboot), use
the file-based epoch-marker pattern rather than a database — reuse
`self_quality_audit::{read_last_run, write_last_run, LAST_RUN_FILENAME,
now_epoch_secs}` and write the marker on **both** `Ok` and `Err`. See the
[self-audit howto](./configure-self-quality-audit.md) and the reference doc's
persistent-gate contract. **Do not add a schema, migration, or table** — the
scheduler is deliberately DB-free.

## Step 4 — Register with the Mind

Registration is one chained call in the daemon setup block. You do **not** edit
the daemon loop body or `mind.rs`.

```rust
mind.register(Box::new(BackgroundThoughtThread::from_env()));
```

The Mind then computes due-ness, runs it in priority order under the per-tick
non-critical budget (`SIMARD_MIND_MAX_NONCRITICAL_PER_TICK`), isolates its
failures, and records its telemetry — automatically.

## Step 5 — Emit telemetry (the only observability path)

!!! danger "No `println!` / `eprintln!` / `print!` in new code"
    Use structured `tracing` events + spans and OTel metrics **only**. The Mind
    already opens the per-run span and records `runs` / `duration_seconds` /
    `next_run_epoch` / `active` for you. For any *extra* thread-specific metric,
    emit it **through `cognitive_threads::telemetry`** — that one helper is the
    single seam that a later rebase onto the unified telemetry facade
    (`src/telemetry/`) will retarget. Do not call metric/logging APIs directly
    from your thread.

Return rich `detail` on the `ThreadOutcome` (bounded, no secrets) and let the
Mind's span carry it:

```rust
ThreadOutcome::ok("done", elapsed)
    .with_detail(serde_json::json!({ "items": n, "skipped": s }))
```

Metric/span **names** must stay `simard.thread.<id>.*` with `<id>` a constant;
untrusted content may only appear as **length-bounded field values**, never in a
name and never as a format-string argument.

## Safety rules (enforced in review)

These apply to every new thread and are non-negotiable:

- **Least authority.** Take only what you need from `ThreadContext`
  (`state_root`, `repo_root`, `memory`, `runtime`, `shutdown`, `now_epoch`,
  `dry_run`). Do **not** reach into globals, and have **no** code path to
  `self_deploy` / `self_relaunch` / redeploy.
- **Honour `dry_run`.** If your thread mutates anything, `ctx.dry_run` must
  short-circuit every destructive call and log what it *would* do.
- **Destructive filesystem ops are guarded.** Before any `remove_dir_all` /
  `remove_file`: canonicalize the path, require it to sit inside an explicit
  **allow-list** root, **refuse symlinks**, and reject the deny-list
  (`worktrees/main`, `~/.simard/repo`, the live cognitive store + shadow/WAL,
  any engineer worktree). Enforce **retention floors** (keep ≥ N newest) before
  pruning. Reuse existing cleanup helpers rather than reimplementing them.
- **Untrusted content is scrubbed.** Anything derived from engineer logs/telemetry
  that leaves the process (issue body, telemetry field) must pass the secret
  scrubber (`sanitization::sanitize_terminal_text`) and be fenced/escaped so it
  can't emit `@mentions`/`#refs` or poison dedup.
- **External processes are argv-only.** Never `sh -c`; never interpolate tainted
  data into a command line. Inject clients (e.g. `Box<dyn GhClient>`) so tests
  use a fake — no network, no ambient credentials.
- **Bound your work.** Cap records scanned, findings, and side-effects per run;
  clamp intervals to a floor. A hot-failing thread is already capped by the
  Mind's backoff, but don't rely on it as your only bound.
- **Durable artifacts are GitHub issues or code, never repo snapshot docs.** If
  your thread produces findings, file a **deduplicated** issue via the
  stewardship path or emit structured telemetry — do not commit a point-in-time
  markdown snapshot.

## Test it (fixtures only — no sleeps, no network)

The abstraction is built to be unit-testable with an **injected clock**
(`ctx.now_epoch`) and injected collaborators. Cover, at minimum:

- **Due computation** — assert `schedule::is_due` / `next_run_epoch` for your
  policy against hand-fed `now_epoch` values (including the `None` last-run
  "due immediately" case for `Interval`, and "never auto-due" for
  `OnDemand`/`EventDriven`).
- **Failure isolation** — a `tick()` that panics or returns `Err` is caught,
  bumps `consecutive_errors`, sets backoff, and does **not** stop OODA or a
  sibling thread. Drive it through `Mind::run_due` with a fake OODA thread and a
  fake panicking thread and assert both survive.
- **Side-effects** — run `tick()` against a fixture `~/.simard` (via `tempfile`)
  and assert exactly the intended effect; assert **no** protected path is
  touched and that `dry_run` performs zero mutations.
- **Injected client** — if you call out (GitHub, etc.), inject a fake via a
  constructor seam (`with_client(cfg, Box::new(FakeGhClient::new()))`) and assert
  idempotency (a second run finds the existing issue and does **not** create a
  duplicate).

Place tests in `src/cognitive_threads/tests.rs` (shared harness) or a
`#[cfg(test)] mod tests` in your thread module, matching the exemplars.

## Checklist

- [ ] `id()` is a stable snake_case constant; `kind()` set (reserved kinds are
      allowed for vision threads)
- [ ] `priority()` is not `Critical`
- [ ] `policy()` uses `schedule.rs`; interval clamped to a floor
- [ ] `from_env()` reads `SIMARD_<AREA>_INTERVAL_SECS` (+ `_DRY_RUN` if it
      mutates) with safe defaults
- [ ] registered via `mind.register(...)`; daemon loop and `ooda_scheduler/`
      untouched
- [ ] telemetry only through `cognitive_threads::telemetry`; **zero**
      `println!`/`eprintln!`
- [ ] safety rules obeyed (least authority, dry-run, allow/deny + symlink
      refusal, scrub, argv-only, bounds, no repo snapshot docs)
- [ ] tests: due-computation, failure isolation, side-effects, injected client —
      no sleeps, no network

## Related

- [Cognitive-thread scheduling (reference)](../reference/cognitive-thread-scheduling.md) — the authoritative trait/`Mind`/telemetry/security contract
- [Configure and monitor cognitive-thread scheduling (howto)](./configure-cognitive-thread-scheduling.md) — operate and observe threads
- [Configure and monitor the monthly self-quality-audit](./configure-self-quality-audit.md) — the disk-persisted-gate reuse pattern
