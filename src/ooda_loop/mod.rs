//! OODA (Observe-Orient-Decide-Act) loop for continuous autonomous operation.
//!
//! The outer OODA cycle gathers observations from all subsystems, orients by
//! ranking priorities, decides on actions within concurrency limits, and
//! dispatches them. If any bridge is unavailable, the cycle degrades honestly
//! (Pillar 11): the observation records `None` for that subsystem.

mod curate;
mod decide;
mod observe;
mod orient;
mod review;
mod summary;
mod types;

#[cfg(test)]
mod tests_observe;

// Re-export all public items so `crate::ooda_loop::X` still works.
pub use curate::check_meeting_handoffs;
pub use decide::decide;
pub use observe::{gather_environment, observe};
pub use orient::orient;
pub use summary::summarize_cycle_report;
pub use types::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
    OodaBridges, OodaConfig, OodaPhase, OodaState, PlannedAction, Priority,
};

use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::load_goal_board;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;
use crate::memory_consolidation;
use crate::memory_consolidation::preparation_memory_operations;
use crate::self_improve::{ImprovementCycle, ImprovementPhase};
use crate::session::SessionId;

/// Act: dispatch actions. Failures are per-action, not cycle-wide (Pillar 11).
///
/// Delegates to [`crate::ooda_actions::dispatch_actions`] which calls the
/// real subsystems (gym bridge, supervisor, skill builder, etc.).
/// Takes `&mut OodaBridges` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` actions.
pub fn act(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    crate::ooda_actions::dispatch_actions(actions, bridges, state)
}

/// Run one complete OODA cycle: Observe -> Orient -> Decide -> Act -> Curate.
///
/// After dispatching actions, the cycle archives completed goals and promotes
/// the highest-scoring backlog items to fill any freed active slots. This
/// implements the meta-goal of continually seeking the best goals to pursue.
///
/// Takes `&mut OodaBridges` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` dispatch.
#[tracing::instrument(skip_all, fields(cycle = state.cycle_count))]
pub fn run_ooda_cycle(
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    // Budget enforcement: refuse to run if daily or weekly spend is exceeded.
    if let Ok(daily) = crate::cost_tracking::daily_summary() {
        if daily.total_cost_usd >= config.daily_budget_usd {
            return Err(SimardError::BudgetExceeded {
                period: "daily".to_string(),
                spent: format!("${:.4}", daily.total_cost_usd),
                limit: format!("${:.2}", config.daily_budget_usd),
            });
        }
    }
    if let Ok(weekly) = crate::cost_tracking::weekly_summary() {
        if weekly.total_cost_usd >= config.weekly_budget_usd {
            return Err(SimardError::BudgetExceeded {
                period: "weekly".to_string(),
                spent: format!("${:.4}", weekly.total_cost_usd),
                limit: format!("${:.2}", config.weekly_budget_usd),
            });
        }
    }

    // Only replace board if loaded one is non-empty (cold memory = keep local).
    if let Ok(board) = load_goal_board(&bridges.memory)
        && !board.active.is_empty()
    {
        state.active_goals = board;
    }

    // Seed with default goals if the board is still empty.
    let seeded = crate::goal_curation::seed_default_board(&mut state.active_goals);
    if seeded > 0 {
        eprintln!("[simard] OODA start: seeded {seeded} default goal(s)");
    }

    // Ingest meeting handoff decisions as new goals.
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match check_meeting_handoffs(&mut state.active_goals, &handoff_dir) {
        Ok(n) if n > 0 => {
            eprintln!(
                "[simard] OODA start: ingested {n} goal/backlog item(s) from meeting handoff"
            );
        }
        Err(e) => {
            eprintln!("[simard] OODA start: meeting handoff check failed: {e}");
        }
        _ => {}
    }

    // --- Memory consolidation: intake at cycle start ---
    let cycle_session_id = crate::session::SessionId::from_uuid(uuid::Uuid::now_v7());
    let cycle_objective = state
        .active_goals
        .active
        .first()
        .map(|g| g.description.clone())
        .unwrap_or_else(|| "ooda-cycle".to_string());
    if let Err(e) = memory_consolidation::intake_memory_operations(
        &cycle_objective,
        &cycle_session_id,
        &bridges.memory,
    ) {
        eprintln!("[simard] OODA consolidation: intake failed: {e}");
    }

    // --- Observe ---
    state.current_phase = OodaPhase::Observe;
    eprintln!("[simard] OODA cycle: entering Observe phase");
    let observation = observe(state, bridges)?;
    eprintln!("[simard] OODA cycle: Observe complete");

    // --- Prepare: gather relevant context from cognitive memory ---
    // Build an objective summary from active goals so memory retrieval is targeted.
    let objective_summary: String = state
        .active_goals
        .active
        .iter()
        .map(|g| g.description.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    let cycle_session_id = SessionId::from_uuid(uuid::Uuid::now_v7());
    match preparation_memory_operations(&objective_summary, &cycle_session_id, &bridges.memory) {
        Ok(ctx) => {
            eprintln!(
                "[simard] OODA cycle: prepared context ({} facts, {} triggers, {} procedures)",
                ctx.relevant_facts.len(),
                ctx.triggered_prospectives.len(),
                ctx.recalled_procedures.len(),
            );
            state.prepared_context = Some(ctx);
        }
        Err(e) => {
            eprintln!("[simard] OODA cycle: preparation failed (degraded): {e}");
            state.prepared_context = None;
        }
    }

    // --- Orient ---
    state.current_phase = OodaPhase::Orient;
    eprintln!("[simard] OODA cycle: entering Orient phase");
    let priorities = orient(&observation, &state.active_goals)?;
    eprintln!(
        "[simard] OODA cycle: Orient complete ({} priorities)",
        priorities.len()
    );

    // --- Memory consolidation: preparation (cross-session recall) ---
    if let Err(e) = memory_consolidation::preparation_memory_operations(
        &cycle_objective,
        &cycle_session_id,
        &bridges.memory,
    ) {
        eprintln!("[simard] OODA consolidation: preparation failed: {e}");
    }

    // --- Decide ---
    state.current_phase = OodaPhase::Decide;
    eprintln!("[simard] OODA cycle: entering Decide phase");
    let planned_actions = decide(&priorities, config)?;
    eprintln!(
        "[simard] OODA cycle: Decide complete ({} actions)",
        planned_actions.len()
    );

    // --- Act ---
    state.current_phase = OodaPhase::Act;
    eprintln!("[simard] OODA cycle: entering Act phase");
    let act_start = Instant::now();
    let outcomes = act(&planned_actions, bridges, state)?;
    let act_elapsed = act_start.elapsed();
    eprintln!(
        "[simard] OODA cycle: Act complete ({} outcomes, {:.1}s)",
        outcomes.len(),
        act_elapsed.as_secs_f64()
    );

    // --- Review: analyze outcomes and propose improvements ---
    let review_proposals = review::review_outcomes(&outcomes, act_elapsed);

    // --- Memory consolidation: reflection ---
    {
        let transcript = outcomes
            .iter()
            .map(|o| format!("{}: {}", o.action.description, o.detail))
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(e) = memory_consolidation::reflection_memory_operations(
            &transcript,
            &[],
            &cycle_session_id,
            &bridges.memory,
        ) {
            eprintln!("[simard] OODA consolidation: reflection failed: {e}");
        }
    }

    // --- Consolidate: best-effort memory maintenance after each cycle ---
    if let Err(e) = bridges.memory.consolidate_episodes(10) {
        eprintln!("[simard] OODA consolidate: episode consolidation failed: {e}");
    }
    if let Err(e) = bridges.memory.prune_expired_sensory() {
        eprintln!("[simard] OODA consolidate: sensory prune failed: {e}");
    }

    if !review_proposals.is_empty() {
        eprintln!(
            "[simard] OODA review: generated {} improvement proposal(s)",
            review_proposals.len()
        );
        // Persist proposals to cognitive memory (best-effort).
        for directive in &review_proposals {
            if let Err(e) = bridges.memory.store_fact(
                &format!("improvement-{}", crate::goals::goal_slug(&directive.title)),
                &format!(
                    "priority={} status={} rationale={}",
                    directive.priority, directive.status, directive.rationale
                ),
                0.8,
                &["improvement".to_string(), "ooda-review".to_string()],
                "ooda-review",
            ) {
                eprintln!("[simard] OODA review: failed to persist proposal: {e}");
            }
        }
        // Convert to ImprovementCycle signals for the next observe() pass.
        let gym_baseline = observation
            .gym_health
            .clone()
            .unwrap_or_else(|| GymSuiteScore {
                suite_id: "ooda-review".to_string(),
                overall: 0.0,
                dimensions: ScoreDimensions::default(),
                scenario_count: 0,
                scenarios_passed: 0,
                pass_rate: 0.0,
                recorded_at_unix_ms: None,
            });
        for _proposal in &review_proposals {
            state.review_improvements.push(ImprovementCycle {
                baseline: gym_baseline.clone(),
                proposed_changes: Vec::new(),
                post_score: None,
                regressions: Vec::new(),
                decision: None,
                final_phase: ImprovementPhase::Eval,
                weak_dimensions: Vec::new(),
                target_dimension: None,
            });
        }
    }

    // --- Curate: archive completed goals, promote from backlog ---
    let archived = crate::goal_curation::archive_completed(&mut state.active_goals);
    if !archived.is_empty() {
        eprintln!(
            "[simard] OODA curate: archived {} completed goal(s): {}",
            archived.len(),
            archived
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    // Promote highest-scoring backlog items to fill freed slots.
    curate::promote_from_backlog(&mut state.active_goals);

    // Persist the updated board to cognitive memory (best-effort).
    if let Err(e) = crate::goal_curation::persist_board(&state.active_goals, &bridges.memory) {
        eprintln!("[simard] OODA curate: failed to persist goal board: {e}");
    }

    // Also write the board to disk so the dashboard can read it.
    {
        let state_root = std::env::var("SIMARD_STATE_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into());
                std::path::PathBuf::from(home).join(".simard")
            });
        let goal_path = state_root.join("goal_records.json");
        if let Err(e) = std::fs::create_dir_all(&state_root) {
            eprintln!("[simard] OODA curate: failed to create state dir: {e}");
        }
        match serde_json::to_string_pretty(&state.active_goals) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&goal_path, json) {
                    eprintln!("[simard] OODA curate: failed to write goal_records.json: {e}");
                }
            }
            Err(e) => eprintln!("[simard] OODA curate: failed to serialize goal board: {e}"),
        }
    }

    // --- Memory consolidation: persistence at cycle end ---
    if let Err(e) =
        memory_consolidation::persistence_memory_operations(&cycle_session_id, &bridges.memory)
    {
        eprintln!("[simard] OODA consolidation: persistence failed: {e}");
    }

    state.cycle_count += 1;
    Ok(CycleReport {
        cycle_number: state.cycle_count,
        observation,
        priorities,
        planned_actions,
        outcomes,
    })
}
