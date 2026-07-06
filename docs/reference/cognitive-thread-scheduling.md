# Cognitive-Thread Scheduling (`src/cognitive_threads/`)

**Status:** Design (issue #2419). Design covers the full "mind of many
processes" vision; **this PR implements the scheduler + three threads**:
the primary `OodaThread`, plus two exemplars — `MaintenanceThread` and
`EngineerLogAnalysisThread`.

!!! note "As shipped in this PR (additive & OFF by default)"
    The scheduler landed **additively and disabled by default** to honour the
    byte-for-byte OODA-parity hard constraint:

    - The `Mind` and all three threads exist and are fully unit-tested.
    - In the **live daemon** the `Mind` is created only when
      `SIMARD_COGNITIVE_THREADS_ENABLED` is truthy, and it hosts **only the two
      background exemplars** (`MaintenanceThread`, `EngineerLogAnalysisThread`).
      It runs **after** the daemon's existing inline OODA cycle each iteration,
      so it can never delay or starve OODA.
    - The daemon's authoritative OODA cycle stays **inline and unchanged**
      (wrap-don't-replace). `OodaThread` is the faithful primary-thread
      realization used by the scheduler tests and ready for the full cutover;
      routing the live loop through it, and migrating the six periodic tasks
      onto the `Mind`, are tracked as **follow-ups** (see §6, A.9).

## 1. Motivation

Simard should eventually run **many concurrent cognitive processes**, each on
its own cadence/trigger: the active OODA loop; background thought; memory
consolidation (sleep/dream); sensory processing; long-term planning;
maintenance & cleanup; engineer-log analysis.

Today `src/operator_commands_ooda/daemon/mod.rs` hardcodes a single OODA loop
plus six ad-hoc, inlined periodic tasks in one hand-rolled `loop {}`:

| Periodic task (current) | Gate mechanism |
| --- | --- |
| Verified cognitive-store backup (#2420) | `Instant` elapsed vs interval |
| Disk-health check + emergency cleanup | `Instant` elapsed vs interval |
| RSS health / memory shedding (#2167/#2183) | every iteration |
| Engineer-worktree sweep (#2167) | `Instant` elapsed vs interval |
| Brain introspection + memory hygiene (#2419) | `Instant` elapsed vs interval |
| Monthly self-quality-audit (#2419) | **disk-persisted** last-run epoch |

Five of these repeat an identical shape: read interval-from-env, track a
last-run marker, test "is it due?", run best-effort, log on `Ok`/`Err`, reset
the marker (the sixth, RSS/shed, runs every iteration with no interval gate).
This design **generalizes that shape** into a single reusable abstraction — it
does **not** add a seventh copy.

> **Non-goal / keep intact:** `src/ooda_scheduler/` is the **engineer
> action-slot** scheduler (AIMD-governed engineer concurrency, bounded by
> `SIMARD_MAX_CONCURRENT_ACTIONS`). It schedules *engineers within an OODA
> cycle*, not top-level cognitive processes. It is **unrelated** to this work
> and must not be modified.

## 2. Critical runtime decision: synchronous scheduler

`run_ooda_daemon` is a **synchronous** `fn`. Its outer `loop {}` runs
housekeeping gates, then one synchronous `run_ooda_cycle(...)`, then
`interruptible_sleep(interval_secs, &shutdown)`. A multi-thread Tokio runtime
is built *inside* the daemon and used *within* the cycle (engineer I/O, LLM
sessions) via `block_on` — the top-level scheduling loop itself is not async.

**Decision:** the `Mind`/`Scheduler` and `CognitiveThread::tick()` are
**synchronous**. Threads that need async I/O receive a `tokio::runtime::Handle`
in their `ThreadContext` and `block_on` it, exactly as the OODA cycle already
does.

**Rationale (hard constraint: identical external OODA behaviour/cadence):**
converting the top-level loop to `async` would rewrite the single most
correctness-critical path — the exact thing #2419 forbids disturbing. A
synchronous scheduler is a **structural refactor with zero behavioural delta**:
the outer loop still sleeps `interval_secs`, OODA still fires once per outer
iteration, and every periodic gate keeps its current firing semantics.

The trait is designed so a *future* fully-async `Mind` is additive: a later
`async fn tick_async` default can wrap the sync `tick`, and the runtime handle
is already threaded through. "Async `tick()`" from the issue is honoured in
**spirit** (async work runs inside a tick) without paying the correctness risk
of an async top-level loop in this PR.

## 3. Module layout

```
src/cognitive_threads/
  mod.rs           // re-exports; module docs
  thread.rs        // CognitiveThread trait, ThreadKind, SchedulePolicy,
                   // Priority, ThreadOutcome, ThreadHealth, ThreadContext
  schedule.rs      // SchedulePolicy::next_run / is_due pure functions
  mind.rs          // Mind (Scheduler): registry, due-computation, budget,
                   // failure isolation, graceful shutdown, run_due()
  telemetry.rs     // small metric/span helper (rebase target for telemetry facade)
  threads/
    ooda.rs        // OodaThread (primary; wraps run_ooda_cycle + heartbeat)
    maintenance.rs // MaintenanceThread (exemplar 1)
    engineer_log_analysis.rs // EngineerLogAnalysisThread (exemplar 2)
  tests.rs         // scheduler due-computation, failure isolation, parity
```

Declared in `src/lib.rs` as `pub mod cognitive_threads;` (sibling of
`ooda_scheduler`). No `Bridge` in any new type/module name (operator rule);
naming stays within Scheduler / Mind / CognitiveThread / Faculty / Context /
Client.

## 4. The `CognitiveThread` trait

```rust
/// A single scheduled mental process owned by the `Mind`.
pub trait CognitiveThread: Send {
    /// Stable, unique, snake_case id used in telemetry metric/span names.
    fn id(&self) -> &str;

    /// Human-facing name for logs/dashboard.
    fn name(&self) -> &str { self.id() }

    /// Coarse class of process.
    fn kind(&self) -> ThreadKind;

    /// When this thread wants to run.
    fn policy(&self) -> SchedulePolicy;

    /// Priority / resource class. OODA is always `Priority::Critical`.
    fn priority(&self) -> Priority { Priority::Normal }

    /// Runtime enable/disable (e.g. env-gated). Disabled threads never tick.
    fn enabled(&self) -> bool { true }

    /// Execute exactly one step. MUST be best-effort and self-contained:
    /// return `Err` rather than panic where possible; the `Mind` also
    /// catches panics as a backstop. May `block_on` `ctx.runtime`.
    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome;

    /// Current health/heartbeat snapshot (last-run, next-run, last outcome).
    fn health(&self) -> ThreadHealth;
}
```

### Supporting types

```rust
pub enum ThreadKind {
    Ooda,                 // implemented (primary)
    Maintenance,          // implemented (exemplar 1)
    EngineerLogAnalysis,  // implemented (exemplar 2)
    // Reserved for the vision — hosted by the same Mind, not implemented here:
    BackgroundThought,
    MemoryConsolidation,
    SensoryProcessing,
    LongTermPlanning,
}

pub enum SchedulePolicy {
    /// Fixed cadence. `next_run = last_run + interval`.
    Interval(Duration),
    /// Only when explicitly requested (operator/event). Never auto-due.
    OnDemand,
    /// Due when an external predicate fires (closure/flag on ThreadContext).
    EventDriven,
    /// Cadence adapts to load/outcome (min..max bounds). Reserved; degrades to
    /// Interval(current) for now so it is representable but conservative.
    Adaptive { min: Duration, max: Duration, current: Duration },
}

pub enum Priority { Critical, High, Normal, Low }  // Critical == OODA only

pub struct ThreadOutcome {
    pub ran: bool,               // false => was not due / skipped
    pub success: bool,
    pub summary: String,         // structured, human-readable
    pub duration: Duration,
    pub detail: serde_json::Value, // thread-specific structured fields
}

pub struct ThreadHealth {
    pub id: String,
    pub enabled: bool,
    pub last_run_epoch: Option<u64>,
    pub next_run_epoch: Option<u64>,
    pub last_success: Option<bool>,
    pub consecutive_errors: u32,
    pub backoff_until_epoch: Option<u64>,
}
```

`ThreadContext<'_>` carries shared, borrowed daemon resources so threads do
**not** reach into globals:

```rust
pub struct ThreadContext<'a> {
    pub state_root: &'a Path,               // ~/.simard state root
    pub repo_root: &'a Path,
    pub memory: &'a dyn CognitiveMemoryOps, // live cognitive store handle
    pub runtime: tokio::runtime::Handle,    // for block_on async work
    pub shutdown: &'a AtomicBool,           // cooperative cancellation
    pub now_epoch: u64,                     // injected clock (testable)
    pub dry_run: bool,                      // global safety switch
}
```

Injecting `now_epoch` and a `Clock` makes due-computation and backoff **purely
unit-testable** with no sleeps.

## 5. The `Mind` (Scheduler)

```rust
pub struct Mind { threads: Vec<ThreadEntry>, budget: RunBudget }
```

Responsibilities:

1. **Registry** — owns `Vec<Box<dyn CognitiveThread>>` plus per-thread runtime
   bookkeeping (`last_run`, `next_run`, `consecutive_errors`, `backoff_until`).
2. **Due computation** — `due_threads(now)` returns enabled, non-backed-off
   threads whose `policy.is_due(last_run, now)` holds. Pure function over
   injected time (see `schedule.rs`); fully unit-tested.
3. **Priority budget (never starve OODA)** — `run_due(ctx)` sorts due threads
   by `Priority` and runs **OODA first, unconditionally, every outer tick**.
   Non-critical threads run after OODA under a per-tick budget
   (`SIMARD_MIND_MAX_NONCRITICAL_PER_TICK`, default small, e.g. 2) so a burst
   of due background threads can never delay or crowd out an OODA cycle. OODA
   is exempt from the budget. This preserves the current ordering (housekeeping
   gates run, then the OODA cycle) while bounding non-critical fan-out.
4. **Failure isolation** — each `tick()` runs inside
   `std::panic::catch_unwind(AssertUnwindSafe(..))`. A panic or `Err`:
   - is caught and recorded (`consecutive_errors += 1`),
   - triggers **exponential backoff** (`backoff_until = now + base * 2^min(n,cap)`,
     capped) so a hot-failing thread cannot spin,
   - emits an error span/metric,
   - **never** propagates — the daemon and sibling threads keep running.
   OODA errors keep their **current** semantics (logged, cycle continues);
   OODA is never backed-off (it is the daemon's reason to exist).
5. **Graceful shutdown** — `run_due` checks `ctx.shutdown` between threads and
   returns early; it coexists with the existing `interruptible_sleep` +
   `shutdown_daemon` drain (checkpoint/close) unchanged. No thread holds the
   loop past a shutdown request.
6. **Telemetry** — every run opens span `simard.thread.<id>` and records the
   per-thread metrics (§7). Health snapshots feed the dashboard heartbeat.

The daemon integration is **minimal and additive**: build a `Mind`, register
the threads, and replace the six inlined gate-blocks + the `run_ooda_cycle`
call with a single `mind.run_due(&mut ctx)` at the same point in the loop. The
outer `interruptible_sleep(interval_secs)` and `shutdown_daemon` stay exactly
as they are.

## 6. Fitting OODA in — the parity constraint (critical)

`OodaThread` is the primary thread: `kind = Ooda`, `priority = Critical`,
`policy = Interval(SIMARD_OODA_INTERVAL_SECS)`. Its `tick()` performs the exact
current per-cycle work, in the same order:

1. cycle-start heartbeat file write,
2. `run_ooda_cycle(&mut state, &mut clients, &config)`,
3. on `Ok`: summarize, persist cycle report, persist episode to memory, write
   health file, `self_metrics::collect_and_record_all(elapsed)`,
4. on `Err`: `daemon_log` the error, continue (identical to today).

Because the outer loop already sleeps `interval_secs` and OODA runs once per
outer iteration, modelling OODA as `Interval(interval_secs)` and running it
**every** `run_due` yields **byte-for-byte identical cadence and side-effects**.
Parity is asserted by tests that drive N iterations through the `Mind` and
compare cycle count, ordering, and emitted side-effects against the legacy
path.

State that the OODA cycle mutates (`OodaState`, `OodaClients`, `config`) is
owned by `OodaThread` (moved into it at daemon start), not the generic
`ThreadContext` — only OODA needs it, and this keeps the trait clean.

!!! note "As shipped: `OodaThread` is faithful; the live loop keeps OODA inline"
    `OodaThread::tick()` performs the core per-cycle work and the **same
    canonical persistence** as the daemon — it calls `run_ooda_cycle`, then the
    shared `persist_cycle_report` / `persist_cycle_to_memory` helpers and
    `self_metrics::collect_and_record_all`, and sets the `Sleep` phase. (The
    dashboard **heartbeat file** writes remain a daemon-loop concern tied to the
    loop's `cycles_run` counter — they are not part of `tick()`.) To guarantee
    byte-for-byte parity **without** an untestable live-loop rewrite, the daemon
    keeps its inline OODA cycle authoritative for now and does **not** register
    `OodaThread` in the live `Mind`. Cutting the live loop over to
    `OodaThread` (using `OodaThread::into_parts()` to hand `state`/`clients`
    back to the graceful-shutdown drain) is a **follow-up**, gated on adding a
    daemon-loop parity harness.

### Migrating the existing periodic tasks

The six other hooks are **low-risk** to migrate onto the `Mind` because each
already matches the trait shape. Plan:

- Backup, disk-health, worktree-sweep → thin `CognitiveThread` wrappers of the
  **existing** functions (no logic rewrite), `Priority::Low`, `Interval(...)`
  from their current env vars. RSS/shed keeps its **every-iteration** cadence
  (a thread due every tick, no interval env var). Behaviour-preserving.
- Brain introspection (`brain_introspection::*`) and monthly self-audit
  (`self_quality_audit::*`, **disk-persisted** last-run) → wrapped as threads;
  the self-audit thread keeps the on-disk `LAST_RUN_FILENAME` gate (its
  `next_run` is disk-backed, surviving restarts) instead of an in-memory
  `last_run`. This proves the abstraction hosts persistent gates.

If any wrapper proves risky under review, it stays inlined for this PR and is
tracked as an explicit follow-up — the abstraction still lands with OODA +
the two new exemplars. **Subsume, never duplicate.**

!!! note "As shipped: periodic-task migration is deferred"
    None of the six periodic tasks were migrated onto the `Mind` in this PR;
    they remain hand-rolled in the daemon loop exactly as before. The `Mind`
    hosts only the two **new** exemplar threads. Migrating the existing tasks
    (behaviour-preserving wrappers, including the disk-persisted self-audit
    gate in A.10) is the explicit follow-up this section anticipates.

## Persistence & data model (Step 5c determination — no relational database)

**Determination: the cognitive-thread scheduler requires no relational
database work.** No schema, no migrations, no indexes, no constraints, no
referential integrity — because it introduces no entities-with-relationships
that a table would model. Step 5c is *not applicable* in the relational sense;
the durable-state design below is the file-based equivalent.

**Grounding (verified against `origin/main`):**

- The crate embeds SQLite (`rusqlite = { version = "=0.31.0", features =
  ["bundled"] }`, `Cargo.toml:75`), but **exclusively for domain data
  stores** — `cognitive_memory` (graph via `library_adapter`), `gym_history`,
  and `native_knowledge`. Those tables are created inline with `CREATE TABLE`
  in code; the repo has **no `migrations/` directory and no migration
  framework**. Adding one for the scheduler would be a net-new subsystem the
  design deliberately avoids.
- Every daemon periodic-task's scheduling state is persisted as **flat files
  under `state_root`**, never in a table: cycle reports
  (`cycle_reports/cycle_<N>.json`, `operator_commands_ooda/persistence.rs`),
  daemon health (`daemon_health.json`), and the monthly self-audit's last-run
  epoch (`self_quality_audit_last_run`, a single integer via `read_last_run`
  /`write_last_run`). Backups are snapshot directories under
  `state_root/backups/<ts>/`.
- The scheduler's reuse anchors — `cmd_cleanup`, `memory_backup`,
  `stewardship`, `self_quality_audit`, `brain_introspection` — touch **no**
  relational schema (verified: zero `rusqlite`/`CREATE TABLE` in those
  modules). `EngineerLogAnalysisThread`'s durable artifact is a *deduplicated
  GitHub issue* (`stewardship::gh_client`), not a DB row.

**The data-model equivalent that IS required** — durable per-thread scheduling
state that survives daemon restarts — is satisfied by the established
file-based epoch-marker pattern (see §A.10), not a database:

| Concern | Design (file-based, no DB) |
|---|---|
| "Schema" (on-disk record) | One plain-text file per **persistent-gate** thread under `state_root`, containing a single unix-epoch-seconds integer = last run. Reuses `self_quality_audit::{LAST_RUN_FILENAME, read_last_run, write_last_run, now_epoch_secs}` verbatim. Filename: `<thread_id>_last_run`; the self-audit thread keeps its existing `self_quality_audit_last_run` name for backward compatibility. |
| Who persists | **Only** genuinely long-cadence gates (self-audit, ~30 d) opt in. All other threads keep `RunBudget` bookkeeping (`last_run`, `next_run`, `consecutive_errors`, `backoff_until`) **in memory**, intentionally reset on restart — identical to today's four `Instant`-gated hooks. An in-memory `Instant` is "fine at 24 h, wrong at 30 d." |
| Constraints / integrity | Corrupt-tolerant read (absent/garbled marker → `None` → re-initialize to now; never crashes the loop); parent-dir auto-create on write; write on **both** `Ok` and `Err` to prevent a failing task hot-looping a full interval. These invariants are already implemented and unit-tested in `self_quality_audit`. |
| Relationships | **None.** Each marker is an independent flat file keyed by thread id — a degenerate key→scalar map on the filesystem. No joins, ordering, or foreign keys. |
| Indexes | **None / not applicable.** Direct path lookup by thread id; no query surface to index. |
| Migrations | **None.** Markers are additive files with no schema version; a first run with no file self-initializes to now. No `ALTER`, no forward/backward migration, no downtime. The existing `self_quality_audit_last_run` file is preserved (no rename), so shipping the scheduler is a **zero-migration** change. |

**Conclusion:** skip relational schema/migrations/indexes for this feature. The
durable-state contract is the per-thread epoch marker above — already prototyped
and tested by `self_quality_audit`, specified as the §A.10 persistent-gate
contract, and requiring no changes to the existing SQLite domain stores.

## 7. Observability contract (STRICT)

- **No `println!`/`eprintln!`/`print!` in any new code.** Structured `tracing`
  events + spans and OTel metrics only. (Existing `daemon_log`/`eprintln!` in
  untouched code is out of scope — that is the parallel logging workstream's
  purge; new code must not add to it.)
- **Telemetry facade coordination:** `src/telemetry/` (unified OTel
  `MeterProvider`, names `simard.<area>.<name>`) is **not yet on this base**.
  All metric emission is centralized behind `cognitive_threads::telemetry`
  (one small helper), so rebasing onto the facade is a single-file change.
  Until then the helper emits via `tracing` structured fields (and OTel
  counters/histograms if a global meter is present).
- **Per-thread metrics** (consistent, facade-ready names):
  - `simard.thread.<id>.runs` — counter
  - `simard.thread.<id>.errors` — counter
  - `simard.thread.<id>.duration_seconds` — histogram
  - `simard.thread.<id>.next_run_epoch` — gauge
  - `simard.thread.<id>.active` — gauge (1 while ticking)
- **Spans:** every run opens `simard.thread.<id>` with fields
  `outcome`, `success`, `duration_ms`, `ran`, plus thread-specific detail.
- **Minimal edits at OODA/daemon signal sites** — scheduling integration only;
  the repo-wide print purge is left to the dedicated logging workstream.

## 8. Exemplar 1 — `MaintenanceThread` (SAFE cleanup)

`kind = Maintenance`, `priority = Low`, `policy = Interval(...)` from
`SIMARD_MAINTENANCE_INTERVAL_SECS` (slow cadence, e.g. daily).

Reuses **existing** helpers (no reimplementation):

| Task | Existing helper |
| --- | --- |
| Prune old `cognitive.corrupt-*` dirs | `cmd_cleanup::disk::remove_old_corrupt_dbs` |
| Trim old store snapshots / shadow WAL copies | `cmd_cleanup::disk::trim_simard_snapshots` |
| Rotate stale binary backups | `cmd_cleanup::disk::rotate_simard_binary_backups` |
| Cap runaway target dirs | `cmd_cleanup::disk::cap_simard_target_dirs` |
| Verify + prune verified backups (retention) | `memory_backup::{list_backups, ensure_backup_valid, prune_old_backups}` |
| Report disk pressure | `disk_pressure::check_with_default_threshold` |

**Safety (conservative by construction):**
- **Never** deletes protected paths: `worktrees/main`, `~/.simard/repo`, the
  **live** cognitive store, or any engineer worktree. An explicit deny-list +
  allow-list gate every path; anything not on the allow-list is skipped.
- Honours `ThreadContext.dry_run` (env `SIMARD_MAINTENANCE_DRY_RUN`): logs the
  actions it *would* take, deletes nothing.
- Retention counts (how many corrupt/snapshot/backup copies to keep) are
  env-configurable with safe defaults; always keeps ≥ N most-recent.
- Every action is emitted as **structured telemetry** (path, bytes freed,
  kept/pruned counts) — never a snapshot doc.

## 9. Exemplar 2 — `EngineerLogAnalysisThread` (improvement finder)

`kind = EngineerLogAnalysis`, `priority = Low`, `policy = Interval(...)` from
`SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS`.

**Inputs (durable telemetry already written under `state_root`):** persisted
cycle reports (`persist_cycle_report`), self-metrics (`self_metrics`), cost
records (`cost_tracking`), and brain/parse telemetry. It scans **recent** data
only (bounded window + bounded record count → bounded cost).

**Signals it looks for:** recurring engineer failures, stuck/looping patterns,
brain parse-failure spikes, restart churn, distill failure-rate, repeated CI
failure modes.

**Durable output (operator rule — artifacts are GitHub issues or durable code,
never repo snapshot docs):** produces a **deduplicated GitHub issue** via the
**existing deterministic** stewardship path (the same code-level dedup
`stewardship::process_orchestrator_run` uses — *not* the heavier agentic-recipe
path `brain_introspection` delegates to):
- `stewardship::dedup::{normalize, failure_signature, find_existing}` to build a
  stable signature and detect an existing **open** issue carrying it,
- `stewardship::gh_client::{GhClient, GhIssue, RealGhClient}` to search + create.

The `GhClient` trait exposes exactly `search_issues(repo, signature)` and
`create_issue(repo, title, body)` — **there is no update/comment op**. Dedup is
therefore *create-suppression*: embed `stewardship-signature:<sig>` in the body,
`search_issues` → `find_existing`; create **only** when no open issue with the
signature exists, otherwise it is a no-op (structured telemetry only). When
issue filing is unavailable (no `gh`/offline/tests), it degrades to **structured
telemetry only** — never a committed file. The `Box<dyn GhClient + Send>` seam lets
tests inject a fake client (fixture-driven, no network).

**Bounding:** hard caps on records scanned, findings emitted, and at most one
issue create/update per run.

## 10. Ambiguity resolution (decisive)

| Question | Resolution |
| --- | --- |
| `async tick()` vs sync? | **Sync** trait + sync `Mind`; async work via `ctx.runtime.block_on`. The daemon's top-level loop is synchronous and correctness-critical; an async top-level loop is out of scope. Async-`Mind` reserved as additive future. |
| Module name? | `src/cognitive_threads/` (not `mind/`). No `Bridge` anywhere. Scheduler type is `Mind`. |
| Reuse or fork `ooda_scheduler`? | **Neither** — it is the engineer action-slot scheduler; leave untouched. New module is independent. |
| Migrate the 6 existing periodic hooks now? | **Yes, if low-risk** as behaviour-preserving wrappers (incl. disk-persisted self-audit gate). Any risky one stays inlined + explicit follow-up. Subsume, don't duplicate. |
| Migrate `memory_consolidation` / rewrite it? | **No.** Reserved `ThreadKind::MemoryConsolidation`; the `Mind` can host it later. Not rewritten here. |
| How does OODA keep exact cadence? | `Interval(SIMARD_OODA_INTERVAL_SECS)`, `Priority::Critical`, run every `run_due`, outer loop still sleeps `interval_secs`. Parity tests assert identical cadence/side-effects. |
| Concurrency model? | Cooperative, single-threaded scheduler tick (matches today). "Concurrency budget" = per-tick cap on **non-critical** threads; OODA is exempt and never starved. Engineer concurrency (AIMD / `SIMARD_MAX_CONCURRENT_ACTIONS`) is untouched. |
| Telemetry facade dependency? | Not present on base → centralize emission behind one helper, emit via `tracing` + OTel-if-present, use the `simard.thread.<id>.*` names so rebase is one file. |
| Where do maintenance/analysis findings go? | Maintenance → structured telemetry. Analysis → dedup'd GitHub issue via stewardship (fallback: telemetry). **No repo snapshot docs.** |
| Safety of cleanup? | Allow-list + deny-list of protected paths, `dry_run` switch, retention floors, structured audit of every action. |

## Security requirements (Step 5d review)

**Determination: the scheduler introduces no new network endpoint, no new
third-party crate, and no new privilege — but it does add two new
*tainted-data → privileged-action* paths (engineer logs → public GitHub issue;
timer → destructive filesystem delete). The requirements below make those two
paths safe by construction and reuse the crate's existing hardening.**

### Threat model (trust boundaries & assets)

- **Principal / authority.** The daemon runs as the operator user with (a) full
  local filesystem authority over `~/.simard`, worktrees, and cargo target
  dirs, and (b) **ambient GitHub credentials** via the `gh` CLI. Every
  `CognitiveThread` inherits that authority — the `ThreadContext` least-authority
  seam (§4) is the primary containment boundary.
- **Untrusted input (taint source).** Engineer log / telemetry content
  (`cycle_reports`, `self_metrics`, `cost_tracking`, brain/parse telemetry). The
  adversary model is a *misbehaving or compromised engineer agent* (or malformed
  tool output) that controls the bytes `EngineerLogAnalysisThread` reads.
- **Assets to protect.** (1) Integrity/availability of the **live cognitive
  store** and protected paths; (2) **GitHub credentials**; (3) **confidentiality
  of secrets** that may appear in logs; (4) **availability of the OODA cycle**
  (never starved); (5) integrity of the dedup / issue-filing channel.

### Requirements

| ID | Requirement | Control / reuse anchor | Verified state |
| --- | --- | --- | --- |
| **SR-1** | **No shell interpretation.** Every subprocess call MUST use a `Command::args([...])` argv array — never `sh -c`, never string interpolation of tainted data into a command line. | argv-only exec. | `RealGhClient` already does this (`gh_client.rs`); new code must add no shell layer. Grep-guard `sh -c`. |
| **SR-2** | **Redact secrets before any excerpt leaves the process.** Every log excerpt placed in an issue title/body **or** a telemetry field MUST pass through the crate's secret scrubber first. `dedup::normalize` strips ANSI **for the signature only** and is *not* sufficient for display content. | `sanitization::sanitize_terminal_text` (strips terminal control **and** redacts `token=`/`Authorization:`/`_secret`/`_token`/… lines → `[REDACTED]`). | `src/sanitization.rs:60`. Prevents exfiltrating engineer-log secrets into a GitHub issue (broader audience than a local log). |
| **SR-3** | **Neutralize markdown / mention / dedup-poison injection.** Untrusted excerpts MUST be fenced in a code block and MUST NOT emit GitHub `@mentions`, `#refs`/auto-links (notification & cross-link spam), nor smuggle a `stewardship-signature:` line that poisons dedup. The trusted signature marker lives in a controlled, delimited location; matching relies on the **server-side** `--search "stewardship-signature:<sig> in:body"` query, not a client-side scan of arbitrary bodies. | Fence + escape leading `@`/`#`; controlled marker placement. | `gh_client::search_issues` already searches server-side (`gh_client.rs`). |
| **SR-4** | **Argument-injection hardening for `gh`.** Tainted title/body MUST NOT be interpretable as `gh` options. Values are passed as dedicated argv elements (consumed as option-arguments), and large/at-risk bodies SHOULD use `--body-file -` (stdin) as defense-in-depth. | argv positioning; optional stdin body. | Low residual risk; review-gated. |
| **SR-5** | **Destructive-op path safety (MaintenanceThread).** Before any `remove_dir_all`/`remove_file`, the target MUST be (a) matched against an **allow-list of canonicalized absolute roots** (not filename-prefix only); (b) rejected if it **is a symlink** or falls outside the allowed root after `fs::canonicalize` (defeats `..`/symlink traversal and the TOCTOU where a symlink is swapped in before delete); (c) checked against the deny-list (`worktrees/main`, `~/.simard/repo`, live store, engineer worktrees). | Canonical allow/deny gate **in the thread**, wrapping the reused helpers. | ⚠ `cmd_cleanup::disk` gates by **filename prefix** with **no** `symlink_metadata`/`canonicalize`/`is_symlink` guard (verified: zero occurrences in `disk.rs`). The thread MUST add this gate itself and MUST refuse to `remove_dir_all` a path whose `symlink_metadata` is a symlink. |
| **SR-6** | **Never delete the live store.** The candidate path MUST be asserted `!=` the active cognitive-store path (and its shadow/WAL) reachable via `ThreadContext.memory`. | Runtime equality assert (belt-and-suspenders with the SR-5 deny-list). | New check in the thread. |
| **SR-7** | **Conservative default posture.** Global `dry_run` honored (`SIMARD_MAINTENANCE_DRY_RUN`); retention **floors** (always keep ≥ N newest) enforced *before* any prune; destructive maintenance ships **dry-run-first / opt-in** until validated in production. Availability > reclaimed bytes. | `ThreadContext.dry_run` + env retention floors. | Design §8. |
| **SR-8** | **Resource-exhaustion / DoS bounds.** (a) `Mind` caps non-critical fan-out per tick and keeps OODA `Critical`/exempt so no thread flood starves the control loop; (b) `catch_unwind` + **capped** exponential backoff stops a hot-failing thread pinning a core; (c) interval env vars are clamped to a **minimum floor** (reject/normalize `0`/negative) so a hostile/misconfigured env can't make a thread due every tick; (d) `EngineerLogAnalysis` enforces record/window/finding caps **before** reading and ≤ 1 `create_issue` per run. | §5 budget + backoff; interval clamp; scan caps. | Budget/backoff in §5; add interval-floor clamp + pre-read caps. |
| **SR-9** | **Least authority.** Threads receive only borrowed, scoped resources via `ThreadContext` and MUST NOT reach globals. The two new threads MUST have **no** code path to `self_deploy`/`self_relaunch`/redeploy. `GhClient` is injected as `Box<dyn GhClient + Send>` (fake in tests → no network, no credentials). | `ThreadContext` seam (§4); trait-object `GhClient`. | ✅ Verified: none of `cmd_cleanup`, `memory_backup`, `stewardship`, `disk_pressure` reference `self_deploy`/`self_relaunch`/`redeploy`. |
| **SR-10** | **Credential confidentiality.** New code MUST NOT read, log, embed in an issue body, or emit to telemetry any token/`GH_TOKEN`. Error strings may include `gh` stderr (gh prints no tokens) but MUST NOT be augmented with env dumps. | No secret in fields/bodies/errors. | `gh` auth stays ambient in the CLI. |
| **SR-11** | **Telemetry integrity (no metric-cardinality injection).** Metric/span **names** use the fixed `simard.thread.<id>` scheme where `<id>` is a per-thread compile-time constant — never derived from untrusted input. Untrusted content appears only as **length-bounded structured field values**, never as a format-string arg (no log forging) and never as a name. | Constant ids; bounded structured fields. | Design §7. |
| **SR-12** | **Trigger authenticity.** `OnDemand`/`EventDriven` due-ness MUST originate from the internal operator/event channel (in-process flag/predicate on `ThreadContext`), not attacker-influenceable external input. Blast radius of a spurious trigger is bounded by SR-8. | Internal-only trigger source. | Design §4/§5. |

### Residual & accepted risks

- **Ambient `gh` trust.** The issue path trusts the operator's `gh` auth and
  network; compromise of `gh`/the network is pre-existing and out of scope
  (unchanged by this feature).
- **Reused prefix-matching retained.** Per *subsume-don't-duplicate*, the
  filename-prefix logic inside `cmd_cleanup::disk` is **not** rewritten here;
  SR-5's canonical allow/deny + symlink-refusal gate **wraps** those helpers.
  Hardening `cmd_cleanup::disk` itself (symlink refusal at the source) is a
  tracked follow-up, not a blocker for this PR.
- **No new attack surface otherwise.** No new crates, no new network endpoints;
  the only external process remains the existing `gh` invocation.

## 11. Hard constraints honoured

- **Additive & non-breaking:** OODA cadence, engineer spawning, memory writes,
  self-deploy, self-relaunch, and graceful shutdown all preserved; wrap over
  replace. Parity tests guard it.
- **Do not touch** the live daemon, `~/.simard`, `worktrees/main`, or any
  engineer worktree. Branch off `origin/main`. **No redeploy** — operator
  deploys.
- **No `Bridge`** in new names.
- **Minimal edits** at OODA/daemon signal sites; leave the repo-wide print
  purge to the logging workstream.

## 12. Test plan (fixtures, no network, no sleeps)

- `schedule.rs` — `is_due` / `next_run` for `Interval`, `OnDemand`,
  `EventDriven`, `Adaptive` against injected clocks.
- `mind.rs` — **failure isolation**: a panicking/erroring fake thread is
  caught, backed off, and does **not** stop OODA or siblings; **priority
  budget**: OODA runs every tick and is never starved by a flood of due
  non-critical threads.
- **OODA-as-thread parity**: N iterations through the `Mind` produce the same
  cycle count/order/side-effects as the legacy inline path (fake OODA cycle).
- `MaintenanceThread` — fixture `~/.simard` tree: prunes only allow-listed
  stale artifacts, **never** protected paths, honours `dry_run`, respects
  retention floors.
- `EngineerLogAnalysisThread` — fixture logs/telemetry: detects seeded
  recurring failure, files exactly one dedup'd issue via a **fake** `GhClient`,
  is idempotent on re-run (second run's `search_issues` returns the existing
  open issue → `find_existing` hits → **no second `create_issue`**), degrades to
  telemetry when the client is absent.
- **Security (Step 5d) fixtures:**
  - *Secret/injection scrub (SR-2/3)* — a seeded log line containing
    `token=SECRET`, `@here`, `#1`, and a spoofed `stewardship-signature: deadbeef`
    → asserts the emitted body has `[REDACTED]` (no secret), the mention/ref are
    neutralized (fenced, no auto-link), and dedup uses **our** computed signature
    (the spoofed marker neither suppresses the create nor causes a second one).
  - *Path safety (SR-5/6)* — a symlink placed inside the fixture `~/.simard`
    pointing at a protected path is **refused** (never followed or removed); a
    `..`-escaping candidate is rejected by the canonical allow-list; the live
    store path is never a delete candidate; retention floor keeps ≥ N newest.
  - *DoS bounds (SR-8)* — `*_INTERVAL_SECS=0` clamps to the minimum floor (thread
    is **not** due every tick); a flood of due non-critical threads cannot delay
    the OODA cycle (cross-references the §5 priority-budget parity test).

## 13. Deliverables for the implementation phase

Design doc (this file) → implementation (`src/cognitive_threads/` + minimal
daemon integration) → tests (above) → how-to doc + this reference doc + mkdocs
nav entry → single merge-ready PR against `rysweet/Simard` off `origin/main`,
green CI, no redeploy.

### Companion how-to guides

- [Configure and monitor cognitive-thread scheduling](../howto/configure-cognitive-thread-scheduling.md)
  — operator guide: cadence knobs, the non-critical per-tick budget, OODA-parity
  checks, per-thread telemetry/health, driving the two exemplars safely, and
  diagnosing a backed-off thread.
- [Add a new cognitive thread](../howto/add-a-new-cognitive-thread.md)
  — developer guide: implementing the trait, choosing a policy/priority,
  env-config, registering with the `Mind`, emitting telemetry through the single
  seam, the safety rules, and the fixture-only test plan.

---

## Appendix A — API Contracts (the "studs")

This appendix pins the **exact public surface** the implementation must expose
and the **exact upstream signatures** it calls. Every signature below was
verified against the current tree; the implementer must not drift from them.
Anything not listed here is a private implementation detail.

### A.1 Public module surface — `src/cognitive_threads/mod.rs`

```rust
//! Cognitive-thread scheduling: a `Mind` runs many `CognitiveThread`s on
//! their own cadence/trigger. See docs/reference/cognitive-thread-scheduling.md.

mod schedule;
mod thread;
mod mind;
mod telemetry;
pub mod threads;
#[cfg(test)]
mod tests;

pub use mind::Mind;
pub use thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth,
    ThreadKind, ThreadOutcome,
};
pub use threads::{EngineerLogAnalysisThread, MaintenanceThread, OodaThread};
```

Registered in `src/lib.rs` as `pub mod cognitive_threads;` (sibling of
`ooda_scheduler`). No `Bridge` in any name.

### A.2 `CognitiveThread` trait (object-safe)

Object-safe: the `Mind` stores `Box<dyn CognitiveThread>`. `Send` (not `Sync`)
— the scheduler ticks threads sequentially on one thread; `Send` only supports
moving a thread into the `Mind` at registration.

```rust
pub trait CognitiveThread: Send {
    fn id(&self) -> &str;                       // stable snake_case; telemetry key
    fn name(&self) -> &str { self.id() }
    fn kind(&self) -> ThreadKind;
    fn policy(&self) -> SchedulePolicy;
    fn priority(&self) -> Priority { Priority::Normal }
    fn enabled(&self) -> bool { true }
    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome;
    fn health(&self) -> ThreadHealth;
}
```

### A.3 Supporting types (derives are part of the contract)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ThreadKind {
    Ooda, Maintenance, EngineerLogAnalysis,
    // reserved (hosted later, not implemented in this PR):
    BackgroundThought, MemoryConsolidation, SensoryProcessing, LongTermPlanning,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchedulePolicy {
    Interval(std::time::Duration),
    OnDemand,
    EventDriven,
    Adaptive { min: std::time::Duration, max: std::time::Duration, current: std::time::Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Priority { Critical, High, Normal, Low } // Ord: Critical < High < Normal < Low
                                                  // (sort ascending ⇒ Critical/OODA first)

#[derive(Clone, Debug, serde::Serialize)]
pub struct ThreadOutcome {
    pub ran: bool,
    pub success: bool,
    pub summary: String,
    pub duration: std::time::Duration,
    pub detail: serde_json::Value,
}
impl ThreadOutcome {
    pub fn skipped() -> Self;                          // ran=false, success=true, empty
    pub fn ok(summary: impl Into<String>, duration: std::time::Duration) -> Self;
    pub fn failed(summary: impl Into<String>, duration: std::time::Duration) -> Self;
    pub fn with_detail(self, detail: serde_json::Value) -> Self;
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ThreadHealth {
    pub id: String,
    pub enabled: bool,
    pub last_run_epoch: Option<u64>,
    pub next_run_epoch: Option<u64>,
    pub last_success: Option<bool>,
    pub consecutive_errors: u32,
    pub backoff_until_epoch: Option<u64>,
}
```

`ThreadContext<'a>` — borrowed daemon resources; no globals. `now_epoch` is an
**injected** clock so due-computation and backoff are unit-testable with no
sleeps.

```rust
pub struct ThreadContext<'a> {
    pub state_root: &'a std::path::Path,
    pub repo_root: &'a std::path::Path,
    pub memory: &'a dyn crate::cognitive_memory::CognitiveMemoryOps, // : Send + Sync
    pub runtime: tokio::runtime::Handle,
    pub shutdown: &'a std::sync::atomic::AtomicBool,
    pub now_epoch: u64,
    pub dry_run: bool,
}
```

### A.4 `schedule.rs` — pure, injected-clock functions (fully unit-tested)

```rust
/// `None` last_run ⇒ Interval/Adaptive are due immediately; OnDemand/EventDriven never auto-due.
pub fn is_due(policy: &SchedulePolicy, last_run_epoch: Option<u64>, now_epoch: u64) -> bool;

/// `None` ⇒ no scheduled next run (OnDemand/EventDriven). Interval/Adaptive ⇒ Some(last + interval).
pub fn next_run_epoch(policy: &SchedulePolicy, last_run_epoch: Option<u64>, now_epoch: u64) -> Option<u64>;

/// Exponential backoff, saturating & capped: now + min(base * 2^min(errors,shift_cap), cap).
pub fn backoff_until_epoch(
    now_epoch: u64,
    consecutive_errors: u32,
    base: std::time::Duration,
    cap: std::time::Duration,
) -> u64;
```

### A.5 `Mind` (the Scheduler) — public API

```rust
pub struct Mind { /* threads: Vec<ThreadEntry>, budget: RunBudget (both private) */ }

impl Mind {
    pub fn new() -> Self;                                  // budget from env, default 2
    pub fn with_budget(max_noncritical_per_tick: usize) -> Self;
    pub fn register(&mut self, thread: Box<dyn CognitiveThread>) -> &mut Self; // chainable

    /// Pure: indices (registration order) of enabled, non-backed-off, due threads.
    pub fn due_threads(&self, now_epoch: u64) -> Vec<usize>;

    /// Run OODA (Priority::Critical) first & unconditionally (budget-exempt, never
    /// backed off), then non-critical due threads in Priority order up to the
    /// per-tick budget. Each tick is wrapped in catch_unwind; a panic/Err bumps
    /// consecutive_errors, sets backoff, emits an error metric, and never
    /// propagates. Checks `ctx.shutdown` between threads and returns early.
    pub fn run_due(&mut self, ctx: &mut ThreadContext<'_>) -> Vec<ThreadOutcome>;

    pub fn health(&self) -> Vec<ThreadHealth>;             // dashboard heartbeat feed
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
impl Default for Mind { fn default() -> Self { Self::new() } }
```

Budget env: `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` (default 2). `ThreadEntry`,
`RunBudget` are **private** bookkeeping (`last_run`, `next_run`,
`consecutive_errors`, `backoff_until`).

### A.6 `telemetry.rs` — the single facade-rebase seam

One small helper; the **only** place metrics/spans are emitted, so a later
rebase onto `src/telemetry/` is a one-file change. No `println!`/`eprintln!`.

```rust
/// Opens span `simard.thread.<id>` with fields ran/success/duration_ms + detail,
/// and records runs / duration_seconds counters+histogram.
pub fn record_run(id: &str, outcome: &ThreadOutcome);
/// Bumps `simard.thread.<id>.errors` and emits an error-level structured event.
pub fn record_error(id: &str, reason: &str);
/// Sets the `simard.thread.<id>.next_run_epoch` gauge.
pub fn record_next_run(id: &str, next_run_epoch: Option<u64>);
/// RAII guard: sets `simard.thread.<id>.active` gauge to 1, back to 0 on drop.
pub fn enter_active(id: &str) -> ActiveGuard;
pub struct ActiveGuard { /* private */ }
```

Emitted names (facade-ready): `simard.thread.<id>.{runs, errors,
duration_seconds, next_run_epoch, active}`.

### A.7 Thread constructors — `src/cognitive_threads/threads/`

```rust
// ooda.rs — owns the mutable OODA state (moved in at daemon start).
pub struct OodaThread { /* state, clients, config, interval_secs, health (private) */ }
impl OodaThread {
    pub fn new(
        state: crate::ooda_loop::OodaState,
        clients: crate::ooda_loop::OodaClients,
        config: crate::ooda_loop::OodaConfig,
        interval_secs: u64,
    ) -> Self;
}
// policy() = Interval(interval_secs); priority() = Critical; enabled() = true.

// maintenance.rs
pub struct MaintenanceConfig {
    pub interval_secs: u64,   // SIMARD_MAINTENANCE_INTERVAL_SECS (default: daily)
    pub keep_corrupt: usize,  // retention floors (≥1)
    pub keep_snapshots: usize,
    pub keep_backups: usize,
    pub target_cap_bytes: u64,
    pub dry_run: bool,        // SIMARD_MAINTENANCE_DRY_RUN
}
impl Default for MaintenanceConfig { /* safe defaults, floors ≥ 1 */ }
pub struct MaintenanceThread { /* cfg, health (private) */ }
impl MaintenanceThread {
    pub fn from_env() -> Self;                 // reads env, safe defaults
    pub fn new(cfg: MaintenanceConfig) -> Self;
}
// policy() = Interval(cfg.interval_secs); priority() = Low.

// engineer_log_analysis.rs
pub struct EngineerLogAnalysisConfig {
    pub interval_secs: u64,   // SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS
    pub repo: String,         // "rysweet/Simard"
    pub window_secs: u64,     // bounded scan window
    pub max_records: usize,   // hard cap on records scanned
    pub max_findings: usize,  // hard cap on findings emitted
    pub dry_run: bool,        // suppress issue creation; telemetry only
}
impl Default for EngineerLogAnalysisConfig { /* safe bounded defaults */ }
pub struct EngineerLogAnalysisThread { /* cfg, gh: Box<dyn GhClient + Send>, health (private) */ }
impl EngineerLogAnalysisThread {
    pub fn from_env() -> Self;   // uses stewardship::gh_client::RealGhClient
    pub fn with_client(          // test seam: inject a fake GhClient
        cfg: EngineerLogAnalysisConfig,
        gh: Box<dyn crate::stewardship::gh_client::GhClient + Send>,
    ) -> Self;
}
// policy() = Interval(cfg.interval_secs); priority() = Low.
```

### A.8 Reuse-anchor call contracts (verified upstream signatures — do not drift)

**`MaintenanceThread::tick` calls (all behaviour-preserving, no re-impl):**

```rust
// cmd_cleanup::disk — mutate a shared CleanupReport accumulator:
cmd_cleanup::disk::remove_old_corrupt_dbs(&mut CleanupReport);
cmd_cleanup::disk::trim_simard_snapshots(&mut CleanupReport);
cmd_cleanup::disk::rotate_simard_binary_backups(&mut CleanupReport);
cmd_cleanup::disk::cap_simard_target_dirs(&mut CleanupReport, cap_bytes: u64);
cmd_cleanup::disk::dir_size(&Path) -> std::io::Result<u64>;
// memory_backup — retention:
memory_backup::list_backups(&BackupConfig) -> SimardResult<Vec<BackupVerification>>;
memory_backup::ensure_backup_valid(&Path)  -> SimardResult<BackupManifest>;
memory_backup::prune_old_backups(&BackupConfig) -> SimardResult<usize>;
// disk_pressure — reporting only:
disk_pressure::check_with_default_threshold(&Path) -> Result<DiskPressureReport, std::io::Error>;
```

The thread builds one `CleanupReport`, runs the allow-listed steps into it, then
reports its accumulated totals as **structured telemetry** (§7). `dry_run`
short-circuits every deleting call.

**`EngineerLogAnalysisThread::tick` calls (deterministic dedup path):**

```rust
stewardship::dedup::failure_signature(failure_kind: &str, error_text: &str) -> String;
stewardship::dedup::normalize(msg: &str) -> String;
stewardship::dedup::find_existing<'a>(&'a [GhIssue], signature: &str) -> Option<&'a GhIssue>;
// GhClient trait — search + create ONLY (no update/comment):
GhClient::search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>>;
GhClient::create_issue(&self, repo: &str, title: &str, body: &str) -> SimardResult<GhIssue>;
```

Contract: embed `stewardship-signature:<sig>` in the issue body (this is exactly
what `RealGhClient::search_issues` greps for); `search_issues` → `find_existing`;
call `create_issue` **iff** `find_existing` returned `None` and `!dry_run`.
`GhIssue { number: u64, url: String, title: String, body: String }`.

### A.9 Daemon integration seam — `src/operator_commands_ooda/daemon/mod.rs`

Additive, minimal, at the **same point** in the outer loop. The six inlined
gate-blocks + the `run_ooda_cycle(...)` call collapse to one `mind.run_due`.

*Before start of loop (setup):*

```rust
let mut mind = Mind::new();
mind.register(Box::new(OodaThread::new(state, clients, config, interval_secs)))
    .register(Box::new(MaintenanceThread::from_env()))
    .register(Box::new(EngineerLogAnalysisThread::from_env()));
// (behaviour-preserving wrappers for backup / disk-health / RSS / worktree-sweep
//  / brain-introspection / self-audit may also be registered here — see §6.)
```

*Inside the loop (replaces the inlined gates + the `run_ooda_cycle` match):*

```rust
let now_epoch = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
let mut ctx = ThreadContext {
    state_root: &state_root,
    repo_root: &repo_root,
    memory: &*shared_mem,
    runtime: runtime.handle().clone(),
    shutdown: &shutdown,
    now_epoch,
    dry_run: global_dry_run,
};
let _outcomes = mind.run_due(&mut ctx);
```

The trailing `interruptible_sleep(interval_secs, &shutdown)` and
`shutdown_daemon(...)` drain are **unchanged**. `OodaState`/`OodaClients`/
`OodaConfig` move **into** `OodaThread` (only OODA needs them), keeping
`ThreadContext` generic. Parity tests (§6, §12) assert identical cycle
count/order/side-effects vs. the legacy path.

!!! note "As shipped in this PR"
    The seam is **additive** and does not collapse the inline OODA cycle. The
    daemon builds the `Mind` only when `SIMARD_COGNITIVE_THREADS_ENABLED` is
    truthy, registers **only** `MaintenanceThread::from_env()` and
    `EngineerLogAnalysisThread::from_env()`, and calls `mind.run_due(&mut ctx)`
    **after** the existing `run_ooda_cycle(...)` match — not in place of it. The
    context is built with `memory: shared_mem.as_ref()`, a dedicated
    single-worker runtime handle, `repo_root` cloned from `clients.repo_root`,
    and `dry_run: false` (each exemplar carries its own dry-run default). This
    keeps edits to the OODA emission sites nil (parity) while the two new
    threads run under the `Mind`'s budget + failure isolation. Registering
    `OodaThread` here and deleting the inline match is the documented follow-up.

### A.10 Persistent-gate contract (optional migrated self-audit thread)

The disk-persisted monthly gate is representable without changing the trait: the
thread returns `policy() = Interval(interval)` but derives its effective
`next_run` from the on-disk epoch via the **existing** helpers, so it survives
restarts (an in-memory `last_run` would reset on reboot):

```rust
self_quality_audit::interval_secs_from_env(raw: Option<&str>) -> u64;
self_quality_audit::should_run_self_audit(elapsed: Duration, interval_secs: u64) -> bool;
self_quality_audit::now_epoch_secs() -> u64;
self_quality_audit::read_last_run(path: &Path) -> Option<u64>;
self_quality_audit::write_last_run(path: &Path, epoch_secs: u64) -> std::io::Result<()>;
self_quality_audit::LAST_RUN_FILENAME: &str;   // gate file under state_root
```

Write last-run on **both** `Ok` and `Err` (prevents a failing recipe from
hot-looping a full interval), exactly as the current inlined gate does.
