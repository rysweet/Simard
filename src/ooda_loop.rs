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

/// Snapshot of the local environment: git state, issues, recent commits.
#[derive(Clone, Debug, Default)]
pub struct EnvironmentSnapshot {
    /// Output of `git status --porcelain` (empty string if unavailable).
    pub git_status: String,
    /// Open issue titles from `gh issue list` (empty if unavailable).
    pub open_issues: Vec<String>,
    /// Recent commit one-liners from `git log --oneline -10`.
    pub recent_commits: Vec<String>,
}

/// Everything gathered during the Observe phase.
#[derive(Clone, Debug)]
pub struct Observation {
    pub goal_statuses: Vec<GoalSnapshot>,
    pub gym_health: Option<GymSuiteScore>,
    pub memory_stats: CognitiveStatistics,
    pub pending_improvements: Vec<ImprovementCycle>,
    /// Local environment state for goal assessment.
    pub environment: EnvironmentSnapshot,
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
    /// Optional base-type session for real autonomous work (e.g. RustyClawd).
    /// When present, `AdvanceGoal` actions use `run_turn` to delegate work
    /// to an LLM agent instead of just bumping a progress percentage.
    pub session: Option<Box<dyn crate::base_types::BaseTypeSession>>,
}

/// Gather a snapshot of the local environment (git status, issues, commits).
///
/// Each sub-command degrades honestly: if the tool is unavailable the
/// corresponding field is empty rather than causing a cycle failure.
pub fn gather_environment() -> EnvironmentSnapshot {
    let git_status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let open_issues = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "title",
            "--jq",
            ".[].title",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let recent_commits = std::process::Command::new("git")
        .args(["log", "--oneline", "-10"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    EnvironmentSnapshot {
        git_status,
        open_issues,
        recent_commits,
    }
}

/// Observe: gather goal statuses, environment state, gym health, memory stats.
/// Sub-system failures produce degraded fields rather than aborting (Pillar 11).
pub fn observe(state: &OodaState, bridges: &OodaBridges) -> SimardResult<Observation> {
    let goal_statuses: Vec<GoalSnapshot> = state
        .active_goals
        .active
        .iter()
        .map(GoalSnapshot::from)
        .collect();

    let environment = gather_environment();

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
        environment,
    })
}

/// Orient: rank goals by urgency, informed by environment context.
///
/// Base urgency: Blocked > not-started > in-progress > completed.
/// Environment signals (dirty working tree, open issues mentioning a goal)
/// can boost a goal's urgency so the OODA loop prioritises actionable work.
pub fn orient(observation: &Observation, goals: &GoalBoard) -> SimardResult<Vec<Priority>> {
    let env = &observation.environment;
    let has_dirty_tree = !env.git_status.is_empty();

    let mut priorities: Vec<Priority> = goals
        .active
        .iter()
        .map(|g| {
            let (mut urgency, mut reason) = match &g.status {
                GoalProgress::Blocked(r) => (1.0, format!("blocked: {r}")),
                GoalProgress::NotStarted => (0.8, "not yet started".to_string()),
                GoalProgress::InProgress { percent } => (
                    0.6 * (1.0 - (*percent as f64 / 100.0)),
                    format!("{percent}% complete"),
                ),
                GoalProgress::Completed => (0.0, "completed".to_string()),
            };

            // Boost urgency if an open issue mentions this goal.
            let mentioned_in_issues = env
                .open_issues
                .iter()
                .any(|title| title.to_lowercase().contains(&g.id.to_lowercase()));
            if mentioned_in_issues {
                urgency = (urgency + 0.1).min(1.0);
                reason = format!("{reason}; mentioned in open issue");
            }

            // Slight boost for in-progress goals when the tree is dirty
            // (indicates active development that may relate to this goal).
            if has_dirty_tree && matches!(g.status, GoalProgress::InProgress { .. }) {
                urgency = (urgency + 0.05).min(1.0);
                reason = format!("{reason}; dirty working tree");
            }

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
pub fn run_ooda_cycle(
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    // Only replace board if loaded one is non-empty (cold memory = keep local).
    if let Ok(board) = load_goal_board(&bridges.memory)
        && !board.active.is_empty()
    {
        state.active_goals = board;
    }

    // --- Observe ---
    state.current_phase = OodaPhase::Observe;
    let observation = observe(state, bridges)?;

    // --- Orient ---
    state.current_phase = OodaPhase::Orient;
    let priorities = orient(&observation, &state.active_goals)?;

    // --- Decide ---
    state.current_phase = OodaPhase::Decide;
    let planned_actions = decide(&priorities, config)?;

    // --- Act ---
    state.current_phase = OodaPhase::Act;
    let outcomes = act(&planned_actions, bridges, state)?;

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
    promote_from_backlog(&mut state.active_goals);

    // Persist the updated board to cognitive memory (best-effort).
    if let Err(e) = crate::goal_curation::persist_board(&state.active_goals, &bridges.memory) {
        eprintln!("[simard] OODA curate: failed to persist goal board: {e}");
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

/// Promote the highest-scoring backlog items into free active slots.
///
/// Backlog items are sorted by score descending and promoted until the
/// active board is at capacity or the backlog is empty.
fn promote_from_backlog(board: &mut GoalBoard) {
    // Sort backlog by score descending so we promote the best first.
    board.backlog.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    while board.active_slots_remaining() > 0 && !board.backlog.is_empty() {
        let item_id = board.backlog[0].id.clone();
        match crate::goal_curation::promote_to_active(board, &item_id, 3, None) {
            Ok(()) => {
                eprintln!("[simard] OODA curate: promoted backlog item '{item_id}' to active");
            }
            Err(e) => {
                eprintln!("[simard] OODA curate: failed to promote '{item_id}': {e}");
                break;
            }
        }
    }
}

/// Summarize a cycle report for logging/persistence.
pub fn summarize_cycle_report(report: &CycleReport) -> String {
    let succeeded = report.outcomes.iter().filter(|o| o.success).count();
    let total = report.outcomes.len();
    let env = &report.observation.environment;
    let dirty = if env.git_status.is_empty() {
        "clean"
    } else {
        "dirty"
    };
    format!(
        "OODA cycle #{}: {} priorities, {} actions ({}/{} succeeded), goals={}, issues={}, tree={}",
        report.cycle_number,
        report.priorities.len(),
        total,
        succeeded,
        total,
        report.observation.goal_statuses.len(),
        env.open_issues.len(),
        dirty,
    )
}
