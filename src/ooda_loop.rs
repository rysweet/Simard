//! OODA (Observe-Orient-Decide-Act) loop for continuous autonomous operation.
//!
//! The outer OODA cycle gathers observations from all subsystems, orients by
//! ranking priorities, decides on actions within concurrency limits, and
//! dispatches them. If any bridge is unavailable, the cycle degrades honestly
//! (Pillar 11): the observation records `None` for that subsystem.

use std::fmt::{self, Display, Formatter};

use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, load_goal_board};
use crate::gym_bridge::GymBridge;
use crate::gym_scoring::GymSuiteScore;
use crate::knowledge_bridge::KnowledgeBridge;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::memory_cognitive::CognitiveStatistics;
use crate::self_improve::ImprovementCycle;

/// The four phases of a single OODA cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OodaPhase {
    Observe,
    Orient,
    Decide,
    Act,
}

impl Display for OodaPhase {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observe => f.write_str("observe"),
            Self::Orient => f.write_str("orient"),
            Self::Decide => f.write_str("decide"),
            Self::Act => f.write_str("act"),
        }
    }
}

/// Mutable state carried across OODA cycles.
pub struct OodaState {
    pub current_phase: OodaPhase,
    pub active_goals: GoalBoard,
    pub cycle_count: u32,
    pub last_observation: Option<Observation>,
}

impl OodaState {
    pub fn new(goals: GoalBoard) -> Self {
        Self {
            current_phase: OodaPhase::Observe,
            active_goals: goals,
            cycle_count: 0,
            last_observation: None,
        }
    }
}

/// A single goal's status snapshot for observation.
#[derive(Clone, Debug)]
pub struct GoalSnapshot {
    pub id: String,
    pub description: String,
    pub progress: GoalProgress,
}

impl From<&ActiveGoal> for GoalSnapshot {
    fn from(g: &ActiveGoal) -> Self {
        Self {
            id: g.id.clone(),
            description: g.description.clone(),
            progress: g.status.clone(),
        }
    }
}

/// Everything gathered during the Observe phase.
#[derive(Clone, Debug)]
pub struct Observation {
    pub goal_statuses: Vec<GoalSnapshot>,
    pub gym_health: Option<GymSuiteScore>,
    pub memory_stats: CognitiveStatistics,
    pub pending_improvements: Vec<ImprovementCycle>,
}

/// A ranked priority produced during the Orient phase.
#[derive(Clone, Debug)]
pub struct Priority {
    pub goal_id: String,
    pub urgency: f64,
    pub reason: String,
}

/// The kind of action the OODA loop can dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionKind {
    AdvanceGoal,
    RunImprovement,
    ConsolidateMemory,
    ResearchQuery,
    RunGymEval,
    BuildSkill,
}

impl Display for ActionKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdvanceGoal => f.write_str("advance-goal"),
            Self::RunImprovement => f.write_str("run-improvement"),
            Self::ConsolidateMemory => f.write_str("consolidate-memory"),
            Self::ResearchQuery => f.write_str("research-query"),
            Self::RunGymEval => f.write_str("run-gym-eval"),
            Self::BuildSkill => f.write_str("build-skill"),
        }
    }
}

/// A planned action selected during the Decide phase.
#[derive(Clone, Debug)]
pub struct PlannedAction {
    pub kind: ActionKind,
    pub goal_id: Option<String>,
    pub description: String,
}

/// Outcome of dispatching a single action.
#[derive(Clone, Debug)]
pub struct ActionOutcome {
    pub action: PlannedAction,
    pub success: bool,
    pub detail: String,
}

/// Report for one complete OODA cycle.
#[derive(Clone, Debug)]
pub struct CycleReport {
    pub cycle_number: u32,
    pub observation: Observation,
    pub priorities: Vec<Priority>,
    pub planned_actions: Vec<PlannedAction>,
    pub outcomes: Vec<ActionOutcome>,
}

/// Configuration for the OODA loop.
#[derive(Clone, Debug)]
pub struct OodaConfig {
    pub max_concurrent_actions: u32,
    pub improvement_threshold: f64,
    pub gym_suite_id: String,
}

impl Default for OodaConfig {
    fn default() -> Self {
        Self {
            max_concurrent_actions: 3,
            improvement_threshold: 0.02,
            gym_suite_id: "progressive".to_string(),
        }
    }
}

/// All bridges needed by the OODA loop.
pub struct OodaBridges {
    pub memory: CognitiveMemoryBridge,
    pub knowledge: KnowledgeBridge,
    pub gym: GymBridge,
}

/// Observe: gather goal statuses, gym health, memory stats.
/// Gym failures produce `None` rather than aborting (Pillar 11).
pub fn observe(state: &OodaState, bridges: &OodaBridges) -> SimardResult<Observation> {
    let goal_statuses: Vec<GoalSnapshot> = state
        .active_goals
        .active
        .iter()
        .map(GoalSnapshot::from)
        .collect();
    let gym_health = match bridges.gym.run_suite("progressive") {
        Ok(result) => {
            use crate::gym_scoring::suite_score_from_result;
            Some(suite_score_from_result(&result))
        }
        Err(e) => {
            eprintln!("[simard] OODA observe: gym bridge unavailable: {e}");
            None
        }
    };
    let memory_stats = match bridges.memory.get_statistics() {
        Ok(stats) => stats,
        Err(e) => {
            eprintln!("[simard] OODA observe: memory bridge unavailable: {e}");
            CognitiveStatistics::default()
        }
    };
    Ok(Observation {
        goal_statuses,
        gym_health,
        memory_stats,
        pending_improvements: Vec::new(),
    })
}

/// Orient: rank goals by urgency. Blocked > not-started > in-progress.
pub fn orient(observation: &Observation, goals: &GoalBoard) -> SimardResult<Vec<Priority>> {
    let mut priorities: Vec<Priority> = goals
        .active
        .iter()
        .map(|g| {
            let (urgency, reason) = match &g.status {
                GoalProgress::Blocked(r) => (1.0, format!("blocked: {r}")),
                GoalProgress::NotStarted => (0.8, "not yet started".to_string()),
                GoalProgress::InProgress { percent } => (
                    0.6 * (1.0 - (*percent as f64 / 100.0)),
                    format!("{percent}% complete"),
                ),
                GoalProgress::Completed => (0.0, "completed".to_string()),
            };
            Priority {
                goal_id: g.id.clone(),
                urgency,
                reason,
            }
        })
        .collect();

    if observation.memory_stats.episodic_count > 100 {
        priorities.push(Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: format!(
                "episodic memory has {} entries, consolidation needed",
                observation.memory_stats.episodic_count
            ),
        });
    }

    if let Some(ref score) = observation.gym_health
        && score.overall < 0.7
    {
        priorities.push(Priority {
            goal_id: "__improvement__".to_string(),
            urgency: 0.7,
            reason: format!("gym overall {:.1}% below 70% target", score.overall * 100.0),
        });
    }

    priorities.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(priorities)
}

/// Decide: select actions from priorities, capped by `max_concurrent_actions`.
pub fn decide(priorities: &[Priority], config: &OodaConfig) -> SimardResult<Vec<PlannedAction>> {
    let limit = config.max_concurrent_actions as usize;
    let mut actions = Vec::with_capacity(limit);
    for priority in priorities {
        if actions.len() >= limit {
            break;
        }
        if priority.urgency < f64::EPSILON {
            continue;
        }
        let kind = match priority.goal_id.as_str() {
            "__memory__" => ActionKind::ConsolidateMemory,
            "__improvement__" => ActionKind::RunImprovement,
            _ => ActionKind::AdvanceGoal,
        };
        actions.push(PlannedAction {
            kind,
            goal_id: if priority.goal_id.starts_with("__") {
                None
            } else {
                Some(priority.goal_id.clone())
            },
            description: priority.reason.clone(),
        });
    }
    Ok(actions)
}

/// Act: dispatch actions. Failures are per-action, not cycle-wide (Pillar 11).
///
/// Delegates to [`crate::ooda_actions::dispatch_actions`] which calls the
/// real subsystems (gym bridge, supervisor, skill builder, etc.).
pub fn act(
    actions: &[PlannedAction],
    bridges: &OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    crate::ooda_actions::dispatch_actions(actions, bridges, state)
}

/// Run one complete OODA cycle: Observe -> Orient -> Decide -> Act.
pub fn run_ooda_cycle(
    state: &mut OodaState,
    bridges: &OodaBridges,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    // Only replace board if loaded one is non-empty (cold memory = keep local).
    if let Ok(board) = load_goal_board(&bridges.memory)
        && !board.active.is_empty()
    {
        state.active_goals = board;
    }
    state.current_phase = OodaPhase::Observe;
    let observation = observe(state, bridges)?;
    state.current_phase = OodaPhase::Orient;
    let priorities = orient(&observation, &state.active_goals)?;
    state.current_phase = OodaPhase::Decide;
    let planned_actions = decide(&priorities, config)?;
    state.current_phase = OodaPhase::Act;
    let outcomes = act(&planned_actions, bridges, state)?;
    state.cycle_count += 1;
    Ok(CycleReport {
        cycle_number: state.cycle_count,
        observation,
        priorities,
        planned_actions,
        outcomes,
    })
}

/// Summarize a cycle report for logging/persistence.
pub fn summarize_cycle_report(report: &CycleReport) -> String {
    let succeeded = report.outcomes.iter().filter(|o| o.success).count();
    let total = report.outcomes.len();
    format!(
        "OODA cycle #{}: {} priorities, {} actions ({}/{} succeeded), goals={}",
        report.cycle_number,
        report.priorities.len(),
        total,
        succeeded,
        total,
        report.observation.goal_statuses.len(),
    )
}
