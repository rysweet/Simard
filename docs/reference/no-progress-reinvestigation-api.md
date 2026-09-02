---
title: No-progress re-investigation API reference
description: Reference for the OODA no-progress re-investigation pass (#17) — the `is_bare_no_progress_block` deterministic rail, the `ALL_CLASS_TOKENS` vocabulary, the `NoProgressTracker.reinvestigated` persisted dedupe set, the population-driven `reinvestigate_bare_blocked_goals` cycle pass, the shared `apply_resolution` helper and `ResolutionSite`, the WHY reasoner seam it reuses, the resolution ladder, and the serde/persistence contract.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/ooda-reinvestigate-blocked-goals.md
  - ./no-progress-breaker-api.md
  - ./completion-evidence-gate-api.md
  - ./goal-board-api.md
  - ../howto/reinvestigate-bare-blocked-goals.md
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/goal_curation/no_progress_why.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/ooda_loop/cycle.rs
---

# No-progress re-investigation API reference

> **Status: implemented.** The deterministic rail and the persisted dedupe set live
> in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs).
> The WHY vocabulary (`NoProgressClass`) and reasoner seam live in
> [`src/goal_curation/no_progress_why.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_why.rs).
> The population-driven pass and the shared `apply_resolution` helper live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs);
> the cycle wiring is in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs).

This reference specifies the API added in issue #17 to re-investigate goals that
are **already** parked in a bare `[OODA-SAFEGUARD] … needs human review` state. For
the rationale and the gap it closes, see
[Re-investigating already-blocked OODA goals](../concepts/ooda-reinvestigate-blocked-goals.md).
For the base breaker (threshold, sentinel constants, `NoProgressTracker` counter,
load-time self-heal), see the
[no-progress breaker API reference](./no-progress-breaker-api.md).

## Contents

- [The thin deterministic rail](#the-thin-deterministic-rail)
- [Class-token vocabulary](#class-token-vocabulary)
- [WHY reasoner seam (reused)](#why-reasoner-seam-reused)
- [`NoProgressTracker` dedupe set](#noprogresstracker-dedupe-set)
- [Population-driven pass: `reinvestigate_bare_blocked_goals`](#population-driven-pass-reinvestigate_bare_blocked_goals)
- [Shared resolution: `apply_resolution` and `ResolutionSite`](#shared-resolution-apply_resolution-and-resolutionsite)
- [Resolution ladder](#resolution-ladder)
- [`NoProgressBreakerReport` extension](#noprogressbreakerreport-extension)
- [Cycle wiring](#cycle-wiring)
- [Persisted-data contract](#persisted-data-contract)
- [Safety invariants](#safety-invariants)
- [What is unchanged](#what-is-unchanged)

## The thin deterministic rail

`is_bare_no_progress_block` is the **only** new deterministic string check in the
feature. A *bare* block is a safeguard-authored no-progress block that never
received a WHY classification: it carries `NO_PROGRESS_BLOCKED_PREFIX` but embeds no
`CLASS_*` token.

```rust
/// True when `reason` is a no-progress-breaker block that carries NO WHY
/// classification token — i.e. it was authored by the bare safeguard path (or by
/// an older daemon build before the WHY reasoner shipped). This is the gate that
/// keeps the agentic classification behind a deterministic rail: it NEVER parses
/// the narrative, only tests for marker-presence and class-token-absence.
pub fn is_bare_no_progress_block(reason: &str) -> bool {
    is_no_progress_marker(reason)
        && !ALL_CLASS_TOKENS.iter().any(|t| reason.contains(t))
}
```

Property obligations (tested):

- `is_bare_no_progress_block(no_progress_blocked_reason(n)) == true` — a bare reason
  is bare.
- `is_bare_no_progress_block(no_progress_blocked_reason_with_why(n, why)) == false`
  for **every** `NoProgressClass` — a WHY-bearing reason is never bare.
- Non-marker strings (operator / scope / dependency / brain-failure blocks) →
  `false` — the rail never mistakes another block kind for a bare no-progress block.

## Class-token vocabulary

The six class tokens are the single source of truth for the rail, so it can never
drift from `NoProgressClass::token()`.

```rust
/// Every WHY class token, in one array, consumed by `is_bare_no_progress_block`.
/// Kept in lockstep with `NoProgressClass` (a compile-time-exhaustive `token()`
/// match guarantees no variant is missing a token).
pub(crate) const ALL_CLASS_TOKENS: [&str; 6] = [
    CLASS_ALREADY_COMPLETE,      // "ALREADY-COMPLETE"
    CLASS_OBSOLETE,              // "OBSOLETE"
    CLASS_MISSING_PRECONDITION,  // "MISSING-PRECONDITION"
    CLASS_UPSTREAM_DEPENDENCY,   // "UPSTREAM-DEPENDENCY"
    CLASS_UNCLEAR_CRITERIA,      // "UNCLEAR-CRITERIA"
    CLASS_GENUINELY_STUCK,       // "GENUINELY-STUCK"
];
```

The tokens are **upper-case, bracketless** literals. `no_progress_blocked_reason_with_why`
embeds the selected token into the block reason as a `why=<TOKEN>` segment, so a
WHY-bearing reason renders as
`🔒 [OODA-SAFEGUARD] … <n> consecutive no-action cycles; why=UPSTREAM-DEPENDENCY evidence=[…]`.
`is_bare_no_progress_block` tests `reason.contains("UPSTREAM-DEPENDENCY")` (etc.),
never a `[why:…]` form.

## WHY reasoner seam (reused)

The re-investigation pass reuses the reasoner seam introduced with the
on-transition path (PR #2960) **unchanged**. There is no second reasoner.

```rust
/// The agentic classifier. `investigate` returns a structured verdict — a class
/// plus a human-readable narrative citing the evidence — or an error (fail-closed).
pub trait NoProgressWhyReasoner {
    fn investigate(&self, goal: &ActiveGoal) -> Result<NoProgressWhy, NoProgressWhyError>;
}

/// The reasoner's structured result. Downstream code consumes `class` (never a
/// re-parse of `narrative`).
pub struct NoProgressWhy {
    pub class: NoProgressClass,
    pub narrative: String,
}

/// The six mutually-exclusive reasons a goal makes no progress. Derives
/// `Clone, Copy, Debug, PartialEq, Eq` — deliberately NOT `Serialize`/`Deserialize`
/// (see the persisted-data contract: the enum is in-memory only; the dedupe set
/// stores `token()` strings on disk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoProgressClass {
    AlreadyComplete,
    Obsolete,
    MissingPrecondition,
    UpstreamDependency,
    UnclearCriteria,
    GenuinelyStuck,
}

impl NoProgressClass {
    /// The stable on-disk / in-marker token for this class (e.g.
    /// `"UPSTREAM-DEPENDENCY"`). The ONLY serialized form of a class.
    pub fn token(&self) -> &'static str;
}

/// The production reasoner injected into both paths. Evidence-driven; no
/// wall-clock timeout (idle/liveness only).
pub struct DeterministicNoProgressReasoner { /* … */ }

/// Render a WHY-bearing block reason: the sentinel prefix + count (so the marker
/// contract is preserved) followed by a `why=<TOKEN>` segment and the cited
/// evidence — i.e. `{PREFIX}{n} consecutive no-action cycles; why={TOKEN} evidence=[…]`.
/// The output is NEVER bare.
pub fn no_progress_blocked_reason_with_why(consecutive: u32, why: &NoProgressWhy) -> String;
```

The reasoner treats evidence as **untrusted context to investigate**, not as
directives — it reuses the existing `engineer_task_for_why` / `render_evidence`
framing and length bounds, so a hostile issue body cannot inject instructions
(prompt-injection hardening carried over from the on-transition path).

## `NoProgressTracker` dedupe set

`NoProgressTracker` (the per-goal counter documented in the
[breaker API](./no-progress-breaker-api.md#noprogresstracker)) gains one persisted
field and two accessors.

```rust
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoProgressTracker {
    #[serde(default)] counts: HashMap<String, u32>,       // existing: no-action counter
    #[serde(default)] guided_retries: HashSet<String>,    // existing (#16): one guided retry/goal
    /// #17: (goal_id, class_token) pairs already taken to a TERMINAL
    /// re-investigation action. The token is a String (NOT the `NoProgressClass`
    /// enum) so it is downgrade-safe under the fail-to-empty board read.
    /// `#[serde(default)]` keeps pre-#17 snapshots loading with an empty set.
    #[serde(default)] reinvestigated: HashSet<(String, String)>,
}

impl NoProgressTracker {
    /// Checked BEFORE any terminal re-investigation action. True ⇒ this goal has
    /// already been taken to a terminal action for this class; skip it.
    pub fn reinvestigated(&self, goal_id: &str, class: NoProgressClass) -> bool;

    /// Insert (goal_id, class.token()) — call ON SUCCESS ONLY, never after a
    /// reasoner error (fail-closed).
    pub fn mark_reinvestigated(&mut self, goal_id: &str, class: NoProgressClass);
}
```

Two lifecycle hooks keep the set from leaking:

- `retain_goals(live)` prunes it — `self.reinvestigated.retain(|(id, _)| live.contains(id))`
  — so ids for removed goals do not accumulate (cascade delete). Cardinality is
  bounded by `live goals × 6`, pruned every cycle.
- `record_progress(goal_id)` clears that goal's entries — real forward progress
  earns a fresh future re-investigation (symmetric with `guided_retries`).

## Population-driven pass: `reinvestigate_bare_blocked_goals`

The new cycle pass. Unlike the on-transition breaker (which takes this cycle's
`outcomes`), it is **population-driven** — it takes no outcomes and scans the board
state directly, exactly like the sibling auto-clear pass.

```rust
/// #17: scan the ACTIVE board each cycle for goals in a BARE Blocked state
/// (safeguard marker, no WHY class) and investigate them — closing the gap where a
/// goal blocked before the reasoner shipped, or on a cycle the reasoner erred, is
/// never re-examined. Reuses the SAME injected seams as the on-transition breaker.
/// Skips perpetual goals. Fully idempotent and fail-closed.
pub(crate) fn reinvestigate_bare_blocked_goals(
    state: &mut OodaState,
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport;
```

**Per-goal algorithm** (the goal is definitionally past threshold; its counter was
reset to 0 when it blocked, so `consecutive = threshold`):

1. `tracker = std::mem::take(&mut state.no_progress_tracker)` — borrow-decouple, as
   auto-clear does, so the board can be borrowed immutably while the tracker mutates.
2. **Collect** (immutable borrow) `(goal_id, ActiveGoal clone)` for every active goal
   where `matches!(status, Blocked(r) if is_bare_no_progress_block(r))` **and**
   `!goal.is_perpetual()`.
3. For each `(goal_id, goal)`:
   1. `let why = match reasoner.investigate(&goal) { Ok(w) => w, Err(e) => { /* FAIL
      CLOSED: leave bare Blocked; report.investigation_errors.push(goal_id);
      tracing::error!; NO dedupe insert; continue */ } };`
   2. **Rewrite first (primary idempotency):**
      `status = Blocked(no_progress_blocked_reason_with_why(threshold, &why))` — the
      goal is now non-bare and excluded from this population next cycle.
   3. **Dedupe belt:** `if tracker.reinvestigated(goal_id, why.class) { continue; }`.
   4. `let resolution = resolution_for_why(threshold, why, tracker.guided_retry_used(goal_id));`
   5. `apply_resolution(state, &mut tracker, &mut report, goal_id, threshold,
      resolution, healer, dispatcher, filer, ResolutionSite::Reinvestigation);`
   6. On a terminal (non-`Continue`) outcome: `tracker.mark_reinvestigated(goal_id, why.class);`.
4. `tracker.retain_goals(&live); state.no_progress_tracker = tracker;` return `report`.

## Shared resolution: `apply_resolution` and `ResolutionSite`

The transition-path match body is extracted verbatim into one helper used by
**both** passes, so the ladder cannot drift. `apply_no_progress_breaker_investigated`
(the on-transition driver) calls it with `site = OnTransition`; the re-investigation
pass calls it with `site = Reinvestigation`.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolutionSite {
    /// The goal crossed the threshold this cycle (outcome-driven path).
    OnTransition,
    /// The goal was already parked bare and is being re-investigated (#17).
    Reinvestigation,
}

/// Apply one `NoProgressResolution`'s side effects through the injected seams.
/// `site` changes EXACTLY two arms (Heal(Ok) and SpawnEngineer un-block on
/// Reinvestigation); every other arm is byte-identical, preserving the
/// on-transition behavior (R1).
fn apply_resolution(
    state: &mut OodaState,
    tracker: &mut NoProgressTracker,
    report: &mut NoProgressBreakerReport,
    goal_id: &str,
    consecutive: u32,
    resolution: NoProgressResolution,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    site: ResolutionSite,
);
```

## Resolution ladder

`resolution_for_why(consecutive, why, guided_retry_used)` maps a class to a
`NoProgressResolution`; `apply_resolution` executes it. Every arm yields a
**non-bare** post-state.

| Resolution | `OnTransition` (unchanged) | `Reinvestigation` (new) | Post-state |
| --- | --- | --- | --- |
| `MarkDone` | status → `Completed` | *(same)* | ✅ Completed |
| `Drop` | remove from board | *(same)* | ✅ removed |
| `Heal(Ok)` | reset counter, **leave status** | reset counter **+ status → `NotStarted`** (un-block) | ✅ NotStarted |
| `Heal(Err)` | `Blocked(why-bearing)` + file issue | *(same)* | ✅ Blocked-with-why |
| `Defer` | status → `Paused` + defer `WipRef` | *(same)* | ✅ Paused |
| `SpawnEngineer` | queue spawn, mark guided-retry, reset counter, **leave status** | + status → `NotStarted` (un-block) | ✅ NotStarted |
| `Escalate` | `Blocked(why-bearing)` + file issue | *(same)* | ✅ Blocked-with-why |

**Why `Reinvestigation` un-blocks on `Heal(Ok)` / `SpawnEngineer`:** an already-blocked
goal is not brain-selectable. If a precondition was healed or a fixer was spawned,
the goal must become selectable again so the fix can actually advance it — otherwise
it strands as a `Blocked` goal the brain never re-selects. If the fixer later fails,
the goal re-enters the **on-transition** path with `guided_retry_used = true` and
escalates **with** a WHY (non-bare) — a self-converging terminal. On the
on-transition path the goal is already selectable, so those two arms leave the
status untouched.

`Defer` records the named upstream via a `[no-progress-defer]` `WipRef`; the goal
auto-clears when that upstream resolves. See
[goal-board-api](./goal-board-api.md) for `WipRef` and the auto-clear scan.

## `NoProgressBreakerReport` extension

The report gains exactly **one** new ephemeral (tracing-only, not persisted) field,
`reinvestigated`, so a cycle log shows which goals were re-investigated. All other
fields already exist (issue #16) — in particular `investigation_errors`, which the
fail-closed re-investigation path **reuses** rather than adds.

```rust
pub(crate) struct NoProgressBreakerReport {
    // ── existing fields (issue #16) ──────────────────────────────────────────
    pub marked_done: Vec<String>,
    pub dropped: Vec<String>,
    pub escalated: Vec<String>,
    pub healed: Vec<String>,
    pub deferred: Vec<String>,
    pub engineer_spawned: Vec<String>,
    pub auto_cleared: Vec<String>,
    /// Reused by #17's fail-closed path — goal ids whose reasoner call errored
    /// (still bare, no terminal action, retried next cycle).
    pub investigation_errors: Vec<String>,
    pub perpetual_idled: Vec<String>,
    // ── new (#17) ────────────────────────────────────────────────────────────
    /// goal ids re-investigated this cycle (had a bare block, now WHY-classified).
    pub reinvestigated: Vec<String>,
}
```

`fired()` and `log_line()` are extended to include `reinvestigated={n}`. A cycle
whose only activity is re-investigation counts as a firing.

## Cycle wiring

In the `no_progress_investigation_enabled()` branch of the cycle driver
(`src/ooda_loop/cycle.rs`), construct **one** shared dispatcher, run both passes,
and drain **once** through the existing `dispatch_spawn_engineer` loop — so there is
zero new subprocess call site and one funnel for all spawns.

```rust
let dispatcher = QueueingEngineerDispatcher::new();

// 1. On-transition path (outcome-driven) — runs the auto-clear scan internally first.
let report   = apply_no_progress_breaker_investigated(
    state, &outcomes, source_ref, &reasoner, &healer, &dispatcher, &GhIssueFiler, threshold);

// 2. Re-investigation pass (population-driven) — runs AFTER, so transition goals are
//    already WHY-bearing and the two populations are disjoint this cycle.
let reinvest = reinvestigate_bare_blocked_goals(
    state, source_ref, &reasoner, &healer, &dispatcher, &GhIssueFiler, threshold);

let requests = dispatcher.into_requests();          // drains BOTH passes' spawns
// … existing dispatch_spawn_engineer drain loop, unchanged …
let breaker_dropped = [report.dropped, reinvest.dropped].concat();
```

**Ordering matters** for population disjointness: auto-clear (inside the transition
fn) → on-transition breaker → re-investigation. Transition goals become WHY-bearing
*before* the scan, so a goal is never processed by both passes in one cycle.

## Persisted-data contract

There is no SQL database; persistence is `state/goal_board.json` (atomic
temp+rename under `flock`). Two properties of that store are load-bearing here:

- **Fail-to-empty read (C1).** Any deserialize failure discards the entire board
  (`read_unlocked = from_str(raw).unwrap_or_else(|_| default())`). Therefore every
  additive field must load on older binaries, and the dedupe key must never be able
  to fail to parse. The key is a `String` token; a rollback that meets an unknown
  token still parses `[[String, String], …]` and simply ignores it — it can never
  trigger a board wipe. An externally-tagged **enum** on disk *could* fail on an
  older/rolled-back binary or a future 7th variant → the reason
  `NoProgressClass` is **not** `Serialize`/`Deserialize`.
- **`version` is informational (C2).** `write` stamps `STORE_VERSION = 1`; `read`
  never gates on it. An additive `#[serde(default)]` field needs no version bump and
  no migration code, and `deny_unknown_fields` is **not** used (forward compat).

```text
state/goal_board.json → PersistentGoalState {
    version: 1 (informational),
    board,
    cycle_count,
    no_progress: NoProgressTracker {
        counts:         HashMap<goal_id → u32>,
        guided_retries: HashSet<goal_id>,
        reinvestigated: HashSet<(goal_id, class_token)>   ← NEW (#17)
    }
}
```

| Compatibility direction | Behavior |
| --- | --- |
| Backward (new binary, old file) | missing key → `#[serde(default)]` empty set ✅ |
| Forward (old binary, new file) | unknown field ignored (no `deny_unknown_fields`) ✅ |
| Rollback value parse | value is `[[String, String], …]` — parses even for an unknown token → never a C1 wipe ✅ |
| Cross-restart | set persists in `no_progress`; idempotency holds across a daemon restart ✅ |

## Safety invariants

The pass upholds these testable invariants (let `bare(G)` mean `G`'s reason is a
no-progress marker with no class token):

- **I1 — No bare survivors.** After a cycle where the reasoner did not error for `G`,
  `¬bare(G')`. Every `Reinvestigation` arm yields Completed | removed | NotStarted |
  Paused | Blocked-with-why.
- **I2 — Fail-closed.** `investigate(G) = Err ⟹` reason unchanged, nothing spawned,
  `(G, ·)` not inserted — retriable next cycle.
- **I3 — Single-fixer idempotency.** At most one fixer spawn per `(G, class)`, via
  (a) the non-bare rewrite removing `G` from the population and (b) the persisted
  dedupe set short-circuiting before any terminal action, surviving restart.
- **I4 — Population disjointness.** Transition runs first and stamps a class token,
  so `bare(G)` is false when the scan collects; auto-clear touches only `Paused` goals.
- **I5 — Perpetual exemption.** `is_perpetual(G) ⟹ G ∉ population` (mirrors transition).
- **I6 — Marker-contract preservation.** Every rewritten reason keeps
  `NO_PROGRESS_BLOCKED_PREFIX` + a parseable leading count, so the overseer
  (`root_cause.rs`, `sensor.rs`) and load-time self-heal keep recognizing the marker.
- **I7 — Atomic persistence.** Board + tracker (incl. the dedupe set) persist in one
  snapshot write, so the rewrite (I1) and the insert (I3) never split across a crash.

## What is unchanged

- `NO_PROGRESS_BREAKER_THRESHOLD`, both sentinel constants, `is_no_progress_marker`,
  and `no_progress_blocked_reason` — unchanged.
- The `NoProgressClass` enum and its derives — unchanged (no `Serialize`/`Deserialize`
  added; `STORE_VERSION` stays `1`).
- The on-transition driver `apply_no_progress_breaker_investigated`'s external
  signature and behavior — unchanged (it now delegates to the shared
  `apply_resolution` with `site = OnTransition`).
- The single `dispatch_spawn_engineer` path — reused; **zero** new `Command::new`
  call sites. No memory-engine (amplihack-memory-lib) change.

## See also

- [Concept: Re-investigating already-blocked OODA goals](../concepts/ooda-reinvestigate-blocked-goals.md)
- [No-progress breaker API reference](./no-progress-breaker-api.md) — base breaker, sentinel, load-time self-heal.
- [Completion-evidence gate API](./completion-evidence-gate-api.md) — the sibling gate the `MarkDone` arm respects.
- [Goal board API reference](./goal-board-api.md) — `WipRef`, the auto-clear scan, and load/save semantics.
- [Re-investigate bare-blocked OODA goals](../howto/reinvestigate-bare-blocked-goals.md)
