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
second evidence source. `goals_all_achieved` is the board-level conjunction used
for the optional daemon-idle decision. The per-goal predicate is exposed as
`goal_achieved(verdict) -> bool` for the loop-break path.

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

Inside the `run_ooda_daemon` loop, after the done-gate verdicts are computed for
the cycle, the daemon calls `completion::evaluate` per active goal:

```rust
match completion::evaluate(goal, verdict, streak, &bounds) {
    LoopDecision::GracefulComplete => {
        // Mark ACHIEVED, free the goal, emit a terminal tracing span, continue
        // with the rest of the board. NOT a daemon exit.
        mark_goal_achieved(goal, verdict);
        tracing::info!(goal_id = %goal.id, verdict = ?verdict,
            "goal ACHIEVED (gate-verified); reflection loop closed");
    }
    LoopDecision::BoundExceeded => {
        // Record a blocker with the WHY; never claim completion.
        record_reflection_bound_blocker(goal, streak);
    }
    LoopDecision::Continue => { /* normal reflection */ }
}
```

The existing `shutdown` and `max_cycles` break paths are unchanged. Daemon-level
idling on an all-ACHIEVED board only occurs when
`ReflectionBounds::stop_when_idle` is `true` **and** `goals_all_achieved`
returns `true`; with defaults, the daemon stays perpetual.

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
  bound-disabled), streak-reset behavior.
- `tests/issue_1025_graceful_achieved_completion.rs`: the daemon breaks a goal's
  reflection loop on a gate-verified all-ACHIEVED state when
  `stop_when_idle` is set, and stays perpetual by default; a criteria-unmet goal
  keeps reflecting (running path); a stuck non-perpetual goal yields
  `BoundExceeded` with a recorded blocker and no false completion.
