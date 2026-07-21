//! Goal-session prompt/input construction.
//!
//! This rail builds deterministic context for the prompt-owned goal-session
//! brain. It gathers state and loads prompt assets, but it does not score
//! progress, select actions, or interpret semantic intent.

/// Build the goal-advance turn input from the goal, recalled context, and
/// fresh environment snapshot. Reinforces the surfaced memory context as a
/// side effect (issue #2395). Pure with respect to [`crate::ooda_loop::OodaState`].
pub(crate) fn build_goal_advance_input(
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    prepared_context: Option<&crate::memory_consolidation::PreparedContext>,
    goal: &crate::goal_curation::ActiveGoal,
    observe_only: bool,
) -> crate::base_types::BaseTypeTurnInput {
    use crate::base_types::BaseTypeTurnInput;
    use crate::goal_curation::GoalProgress;
    use std::fmt::Write;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    let env = crate::ooda_loop::gather_environment();
    let goal_session_objective =
        crate::ooda_brain::prompt_store::global().load("goal_session_objective.md");

    let mut objective = String::with_capacity(1024);
    let _ = write!(
        objective,
        "Goal '{}' ({}% complete): {}\n\n{}\n\nEnvironment context:\n- Git status: ",
        goal.id,
        percent,
        goal.description,
        goal_session_objective.trim(),
    );
    if env.git_status.is_empty() {
        objective.push_str("clean");
    } else {
        let _ = write!(
            objective,
            "{} changed files",
            env.git_status.lines().count()
        );
    }
    objective.push_str("\n- Open issues: ");
    if env.open_issues.is_empty() {
        objective.push_str("none");
    } else {
        for (i, issue) in env.open_issues.iter().enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(issue);
        }
    }
    objective.push_str("\n- Recent commits: ");
    if env.recent_commits.is_empty() {
        objective.push_str("none");
    } else {
        for (i, commit) in env.recent_commits.iter().take(5).enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(commit);
        }
    }

    if goal.is_standing_research_goal() {
        objective.push_str(
            "\n\n## Novelty-first directive [novelty-first: standing research goal]\n\
             This is a STANDING cognition-research goal. Each cycle, your PRIMARY \
             directive is to seek a GENUINELY NEW research direction — do NOT default \
             to another incremental parse-site / dedup refinement of an already-worked \
             seam.\n\
             1. FIRST survey the space of NOVEL, unexplored cognition-research \
             directions you have not yet tried (new graph-memory retrieval strategies, \
             memory-consolidation techniques, reasoner-reliability approaches, \
             ranking/embedding ideas). Draw on your own memory and recent PRs to avoid \
             repeating work already done.\n\
             2. PREFER pursuing a novel direction: prototype it and benchmark against \
             the current recall-precision / fact-yield baseline, producing EITHER a \
             durable PR implementing the technique OR a memory-recorded, reasoned \
             NEGATIVE result explaining why it does not beat the baseline. Prefer this \
             over an incremental refinement of a seam already worked.\n\
             3. Only FALL BACK to incremental maintenance when no novel direction is \
             currently viable — and SAY SO explicitly, naming what you surveyed and why \
             nothing novel was actionable this cycle.\n\
             Preserve STANDING PERPETUAL semantics: never mark this goal complete; \
             durable improvements only.",
        );
    }

    if observe_only {
        objective.push_str(
            "\n\n## Read-only observer contract\n\
             This identity is running with SIMARD_OBSERVE_ONLY=1. Do not ask for, \
             plan, or dispatch an engineer. Perform the allowed read-only inspection \
             in this session using only read commands, then respond with exactly:\n\n\
             NO ACTION\n\
             REASON: <why no engineer was dispatched and what evidence was gathered>\n\
             PROGRESS: NN\n\n\
             Include EVIDENCE/PROPOSALS bullets after REASON when useful. Read-only \
             means no writes, not no progress: if you gathered concrete evidence, \
             use a modest positive progress value such as 5-25. If you cannot gather \
             new evidence, respond with PROGRESS: 0 and explain why in REASON.",
        );
    }

    if let Some(ctx) = prepared_context {
        if !ctx.relevant_facts.is_empty() {
            objective.push_str("\n\nRelevant facts from memory:");
            for fact in &ctx.relevant_facts {
                let _ = write!(objective, "\n- [{}] {}", fact.concept, fact.content);
            }
        }
        if !ctx.triggered_prospectives.is_empty() {
            objective.push_str("\n\nTriggered reminders:");
            for p in &ctx.triggered_prospectives {
                let _ = write!(objective, "\n- {}: {}", p.description, p.action_on_trigger);
            }
        }
        if !ctx.recalled_procedures.is_empty() {
            objective.push_str("\n\nRecalled procedures:");
            for proc in &ctx.recalled_procedures {
                let _ = write!(objective, "\n- {}: {}", proc.name, proc.steps.join(" -> "));
            }
        }
        if !ctx.episodic_recall.is_empty() {
            objective.push_str("\n\n## Prior episodes (ranked by relevance)");
            for ep in &ctx.episodic_recall {
                let content = if ep.content.chars().count() > 200 {
                    let truncated: String = ep.content.chars().take(200).collect();
                    format!("{truncated}...")
                } else {
                    ep.content.clone()
                };
                let _ = write!(
                    objective,
                    "\n- [{}] [t={}] {}",
                    ep.source_label, ep.temporal_index, content
                );
            }
        }

        crate::memory_consolidation::reinforce_prepared_context(memory, ctx);
    }

    const GOAL_SESSION_IDENTITY: &str =
        include_str!("../../../prompt_assets/simard/goal_session_identity.md");
    let identity_context = GOAL_SESSION_IDENTITY.trim().to_string();

    BaseTypeTurnInput {
        objective,
        identity_context,
        prompt_preamble: String::new(),
    }
}
