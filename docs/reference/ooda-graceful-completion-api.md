---
title: "OODA graceful-completion API"
description: Reference for the issue #1025 terminal-completion decision layer — the pure ooda_loop::completion module (goals_all_achieved, ReflectionBounds, LoopDecision, evaluate), the no-progress streak plumbed through ooda_loop::cycle, and the run_ooda_daemon wiring that breaks the reflection loop on a gate-verified ACHIEVED goal while preserving perpetual-by-default operation.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#1025"]
related:
  - ../concepts/graceful-ooda-completion.md
  - ../howto/configure-graceful-ooda-completion.md
  - ./completion-evidence-gate-api.md
  - ./completion-gate-issue-fallback-api.md
  - ./ooda-per-goal-cycle-api.md
  - ./durable-ooda-cycle-counter.md
---

# OODA graceful-completion API

The graceful-completion layer (issue #1025) lives in
`src/ooda_loop/completion.rs`. It is a **pure** module: every function is
side-effect free and depends only on values the OODA cycle already computed —
principally the [`CompletionVerdict`](./completion-evidence-gate-api.md)
produced by the deploy-aware done-gate. The daemon
(`src/operator_commands_ooda/daemon/mod.rs`) consumes its decision; the module
never touches the network, `gh`, or the goal store.

> **Naming.** No symbol added by this feature contains `bridge`/`Bridge`
> (enforced by `tests/no_bridge_naming.rs`). All diagnostics use structured
> `tracing`; there is no `print!`/`println!` in this path.

## `goals_all_achieved`

```rust
/// Returns `true` only when every goal on the board is complete by
/// gate-verified evidence (`verdict.is_complete()`).
///
/// Consumes the done-gate verdicts already computed this cycle. It never
/// re-derives evidence and never treats a model-reported "done" as complete.
/// (The goal's success criteria — its `description` — are already evaluated
/// inside the `CompletionVerdict`; there is no separate criteria check here.)
pub fn goals_all_achieved(
    board: &GoalBoard,
    verdicts: &BTreeMap<GoalId, CompletionVerdict>,
) -> bool;
```

A single goal is achieved when its verdict `is_complete()`. That verdict already
encapsulates the goal's success-criteria evaluation, so the predicate adds no
second evidence source. `goals_all_achieved` is the board-level conjunction
exposed for an all-verified check (the daemon's own idle stop instead uses a
board-drain predicate; see [Daemon wiring](#daemon-wiring-run_ooda_daemon)). The
per-goal predicate is exposed as `goal_achieved(verdict) -> bool` for the
loop-break path.

## `ReflectionBounds`

```rust
/// Policy for the bounded no-progress safeguard. Perpetual by default.
#[derive(Clone, Debug)]
pub struct ReflectionBounds {
    /// Consecutive no-progress reflection cycles a non-perpetual goal may burn
    /// before `evaluate` yields `BoundExceeded`. `0` disables the bound.
    pub max_reflection_cycles: u32,

    /// When `true`, an all-ACHIEVED board lets the daemon loop idle. Sourced
    /// from `SIMARD_OODA_STOP_WHEN_ACHIEVED`. Defaults to `false` (perpetual).
    pub stop_when_idle: bool,
}

impl Default for ReflectionBounds {
    /// Perpetual-safe defaults: `max_reflection_cycles = 0`-guarded via env
    /// (see `from_env`), `stop_when_idle = false`.
    fn default() -> Self { /* ... */ }
}

impl ReflectionBounds {
    /// Build from the environment. Malformed values fall back to the safe
    /// default and emit a `tracing::warn!` — never a panic.
    ///
    /// * `SIMARD_OODA_MAX_REFLECTION_CYCLES` -> `max_reflection_cycles`
    /// * `SIMARD_OODA_STOP_WHEN_ACHIEVED`    -> `stop_when_idle`
    pub fn from_env() -> Self;
}
```

## `LoopDecision`

```rust
/// The single decision `evaluate` returns for one reflection tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopDecision {
    /// Goal not yet achieved and reflection budget not exhausted — reflect again.
    Continue,
    /// Terminal predicate holds (gate-verified achieved) — break the loop cleanly.
    GracefulComplete,
    /// Non-perpetual goal still not achieved after `max_reflection_cycles`
    /// consecutive no-progress cycles — yield with a recorded blocker.
    BoundExceeded,
}
```

## `evaluate`

```rust
/// Map one reflection tick to a `LoopDecision`.
///
/// Precedence:
///   1. `GracefulComplete` if `goal_achieved(verdict)` (i.e. `verdict.is_complete()`).
///   2. `BoundExceeded`   if the goal is not perpetual, `bounds.max_reflection_cycles > 0`,
///                          and `no_progress_streak >= bounds.max_reflection_cycles`.
///   3. `Continue`        otherwise.
///
/// `goal` is used only to determine perpetual/standing status (for the
/// exemption); achievement is decided from `verdict` alone.
/// Perpetual/standing goals never receive `BoundExceeded`; they fall through to
/// `Continue`, preserving the perpetual-goal no-progress exemption.
pub fn evaluate(
    goal: &ActiveGoal,
    verdict: &CompletionVerdict,
    no_progress_streak: u32,
    bounds: &ReflectionBounds,
) -> LoopDecision;
```

`evaluate` checks achievement **before** the bound, so a goal that becomes
achieved on the same cycle it would have tripped the bound completes gracefully
rather than yielding.

## No-progress streak (in `ooda_loop::cycle`)

`src/ooda_loop/cycle.rs` threads a `no_progress_streak: u32` through the
per-cycle result:

- The streak **increments** on a reflection cycle that produced no shippable
  progress (no new commit, no PR state change, no criterion newly satisfied, no
  blocker resolved).
- The streak **resets to `0`** on any cycle that produced shippable progress.

The streak is the input to `evaluate`'s `BoundExceeded` clause. It counts
*consecutive* stalled cycles, so an actively progressing goal never approaches
the bound. The streak is surfaced on the cycle result for the daemon log and
the dashboard thinking-cycle history.

## Daemon wiring (`run_ooda_daemon`)

`run_ooda_daemon` reads `ReflectionBounds::from_env()` once at daemon start and
consumes the decision layer through three concrete mechanisms — it does **not**
call `completion::evaluate` per goal (that function is the pure, unit-tested
decision spec; the daemon reuses its shared predicate rather than re-running it):

1. **Graceful completion.** For each goal the done-gate reports newly complete
   this cycle (`newly_done`), the daemon emits one terminal line —
   `OODA graceful completion: goal {id} ACHIEVED (gate-verified) — closing
   reflection loop` — and moves on to the rest of the board. This is the
   loop-break for a delivered goal, **not** a daemon exit.

2. **Bounded no-progress safeguard (opt-in).** When
   `max_reflection_cycles > 0`, `reflection_bound_yields(active, tracker, bounds)`
   selects the non-perpetual, non-terminal goals whose consecutive no-progress
   streak has reached the cap. Each is marked `GoalProgress::Blocked(..)` with the
   streak in the WHY and logged for human review. It delegates the per-goal
   decision to `ReflectionBounds::bound_exhausted` — the same predicate
   `evaluate`'s `BoundExceeded` arm uses (locked by
   `bound_exhausted_matches_evaluate_bound_arm`), so there is one source of truth
   for the bound. It never fabricates completion.

3. **Graceful idle stop (opt-in).** When `stop_when_idle` is set,
   `should_graceful_idle_stop(stop_when_idle, had_active_at_cycle_start,
   active_now_empty, backlog_now_empty)` breaks the perpetual loop once a board
   that held delivery work at cycle start has been fully drained. A board that
   started empty never trips it, and with the flag unset (the default) it never
   fires.

The existing `shutdown` and `max_cycles` break paths are unchanged. With
defaults (`max_reflection_cycles == 0`, `stop_when_idle == false`) none of the
opt-in paths fire and the daemon stays perpetual. `goals_all_achieved` is the
board-level conjunction exposed for callers (and the acceptance tests) that want
an all-verified check; the daemon's own idle stop uses the board-drain predicate
above.

## Failure and safety semantics

- **No panics.** Malformed env values degrade to safe defaults with a
  `tracing::warn!`.
- **Evidence-only completion.** `GracefulComplete` is reachable only through a
  `CompletionVerdict::Complete`; there is no self-report shortcut.
- **Bounded spin.** `max_reflection_cycles` caps self-inflicted no-progress spin
  for non-perpetual goals; `0` (disabled) keeps prior behavior for operators who
  want no cap.
- **Perpetual preservation.** Perpetual goals and the default daemon posture are
  never terminated by this layer.

## Tests

- `src/ooda_loop/completion.rs` inline `#[cfg(test)]`: `goals_all_achieved`
  truth table, `evaluate` decision matrix (precedence, perpetual exemption,
  bound-disabled), `ReflectionBounds::from_env_values` parsing (aliases, blank,
  malformed-degrades-without-panic), and the shared `bound_exhausted` predicate
  (including `bound_exhausted_matches_evaluate_bound_arm`).
- `src/operator_commands_ooda/daemon/mod.rs` inline `#[cfg(test)]`: the daemon
  glue — `reflection_bound_yields` (disabled cap, stuck non-perpetual yield,
  perpetual/terminal exemption, moving-goal left alone) and
  `should_graceful_idle_stop` (opt-in + drained-board matrix).
- `tests/issue_1025_graceful_achieved_completion.rs`: locks the pure decision
  contract at the crate's public boundary — terminal path (gate-verified goal ⇒
  `GracefulComplete`), running path (criteria unmet ⇒ `Continue`), bound path (a
  stuck non-perpetual goal ⇒ `BoundExceeded`, explicitly **not** a completion),
  perpetual exemption, `goals_all_achieved` only when every goal is verified, and
  perpetual-safe `from_env` defaults.
