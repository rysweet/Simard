//! Automatic promotion (distillation) scheduler (issue #2327, R4).
//!
//! Fires episode → fact/procedure distillation **automatically**, decoupled
//! from the OODA `ConsolidateMemory` action. Distillation used to run only
//! when the brain happened to pick `ConsolidateMemory`; that left recurring
//! episodes undistilled for long stretches. The scheduler instead fires on
//! either of two cheap, deterministic conditions evaluated at the end of every
//! OODA cycle:
//!
//! - **Threshold:** the number of undistilled episodes has reached
//!   [`DistillSchedule::min_episodes`] (config default 25), or
//! - **Interval:** [`DistillSchedule::interval_cycles`] cycles (config default
//!   50) have elapsed since the last distillation pass.
//!
//! A cycle-count interval (rather than wall-clock) keeps the trigger
//! deterministic and unit-testable.

use std::path::Path;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

use super::distillation::{
    DistillRecipeRunner, DistillReport, RecipeRunnerSubprocess, distill_recent_episodes_with_runner,
};

/// Configuration for the automatic distillation scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistillSchedule {
    /// Fire when undistilled episode count reaches this value.
    pub min_episodes: u32,
    /// Fire when this many OODA cycles have elapsed since the last pass.
    pub interval_cycles: u32,
}

impl DistillSchedule {
    /// Canonical default undistilled-episode threshold (issue #2327, A3).
    pub const DEFAULT_MIN_EPISODES: u32 = 25;
    /// Canonical default cycle-count interval.
    pub const DEFAULT_INTERVAL_CYCLES: u32 = 50;
}

impl Default for DistillSchedule {
    fn default() -> Self {
        Self {
            min_episodes: Self::DEFAULT_MIN_EPISODES,
            interval_cycles: Self::DEFAULT_INTERVAL_CYCLES,
        }
    }
}

/// Which condition (if any) fired the distillation pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistillTrigger {
    /// Undistilled episode count reached `min_episodes`.
    Threshold,
    /// `interval_cycles` cycles elapsed since the last pass.
    Interval,
    /// Neither condition met — do not distil this cycle.
    None,
}

/// Pure decision: does the schedule fire given the current undistilled count
/// and cycles-since-last-pass? Threshold takes precedence over interval (both
/// just run the same pass, but threshold is the more specific reason).
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

/// Count undistilled episodes, capped at `min_episodes`.
///
/// The threshold trigger only needs to know whether the count has *reached*
/// `min_episodes`, so we pull at most that many ids and never scan the full
/// undistilled set.
fn capped_undistilled_count(
    memory: &dyn CognitiveMemoryOps,
    schedule: &DistillSchedule,
) -> SimardResult<u32> {
    Ok(memory
        .list_undistilled_episodes(schedule.min_episodes)?
        .len() as u32)
}

/// Run a scheduled distillation pass using the supplied runner.
///
/// This is the testable entry point. Evaluates [`distill_trigger`] against the
/// (capped) undistilled count and `cycles_since_last`:
///
/// - [`DistillTrigger::None`] → returns `Ok(None)`; the runner is never
///   invoked, no facts/procedures are stored, no episodes marked.
/// - Threshold / Interval → runs
///   [`distill_recent_episodes_with_runner`], returning `Ok(Some(report))`.
#[tracing::instrument(skip_all)]
pub fn run_scheduled_distillation_with_runner(
    memory: &dyn CognitiveMemoryOps,
    runner: &dyn DistillRecipeRunner,
    schedule: &DistillSchedule,
    cycles_since_last: u32,
) -> SimardResult<Option<DistillReport>> {
    let count = capped_undistilled_count(memory, schedule)?;
    match distill_trigger(count, cycles_since_last, schedule) {
        DistillTrigger::None => {
            tracing::debug!(
                target: "simard::distill",
                count,
                cycles_since_last,
                min = schedule.min_episodes,
                interval = schedule.interval_cycles,
                "distill scheduler: no trigger this cycle"
            );
            Ok(None)
        }
        trigger => {
            tracing::info!(
                target: "simard::distill",
                ?trigger,
                count,
                cycles_since_last,
                "distill scheduler: {trigger:?} trigger fired"
            );
            eprintln!(
                "[simard] distill scheduler: {trigger:?} trigger fired (undistilled≈{count}, cycles_since_last={cycles_since_last})"
            );
            Ok(Some(distill_recent_episodes_with_runner(memory, runner)?))
        }
    }
}

/// Production entry point: run a scheduled distillation pass using the
/// `recipe-runner-rs` subprocess.
///
/// Checks the trigger BEFORE constructing the (potentially expensive) recipe
/// runner so a no-trigger cycle does nothing. Returns `Ok(None)` when the pass
/// does not fire OR when the runner cannot be constructed (recipe-runner-rs not
/// on PATH, recipe file missing, no agent binary) — distillation must never
/// block the OODA cycle.
#[tracing::instrument(skip_all)]
pub fn run_scheduled_distillation(
    memory: &dyn CognitiveMemoryOps,
    repo_root: &Path,
    schedule: &DistillSchedule,
    cycles_since_last: u32,
) -> SimardResult<Option<DistillReport>> {
    let count = capped_undistilled_count(memory, schedule)?;
    match distill_trigger(count, cycles_since_last, schedule) {
        DistillTrigger::None => Ok(None),
        trigger => match RecipeRunnerSubprocess::new(repo_root) {
            Some(runner) => {
                tracing::info!(
                    target: "simard::distill",
                    ?trigger,
                    count,
                    cycles_since_last,
                    "distill scheduler: {trigger:?} trigger fired"
                );
                eprintln!(
                    "[simard] distill scheduler: {trigger:?} trigger fired (undistilled≈{count}, cycles_since_last={cycles_since_last})"
                );
                Ok(Some(distill_recent_episodes_with_runner(memory, &runner)?))
            }
            None => {
                tracing::info!(
                    target: "simard::distill",
                    ?trigger,
                    "distill scheduler: trigger fired but recipe-runner-rs unavailable; skipping pass"
                );
                Ok(None)
            }
        },
    }
}
