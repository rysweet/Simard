//! `decompose_goal` — the decomposition driver — plus the [`GoalDecomposer`]
//! seam and the recipe-runner-backed implementation (issue #2405). See
//! `docs/reference/goal-decomposition.md`.
//!
//! `decompose_goal` takes **one** large goal and emits **2..=6** bounded,
//! independently-verifiable sub-goals, each with its own done-criterion, then
//! writes them as **child nodes with typed `decomposes_into` edges back to the
//! parent** (and optional `depends_on` ordering edges between siblings). The
//! edges are written **regardless of placement**, so a child that overflows to
//! the backlog is still a queryable child of its parent.
//!
//! The driver follows Simard's deterministic-fallback pattern: when the
//! decomposer fails or returns an unusable shape (fewer than two sub-goals),
//! it surfaces a **loud** error and leaves the board and graph untouched rather
//! than silently producing zero or malformed children.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::edges::{write_edge, write_node};
use super::prioritize::{PrioritizationSignals, prioritize};
use super::types::{
    ActiveGoal, BacklogItem, GoalBoard, GoalEdge, GoalEdgeType, GoalNode, MAX_ACTIVE_GOALS,
};

/// Lower bound on a real decomposition: a single sub-goal is not a
/// decomposition, it is a rename.
pub const MIN_SUBGOALS: usize = 2;
/// Upper bound on the fan-out so a parent never explodes into an unbounded set
/// of slices.
pub const MAX_SUBGOALS: usize = 6;

/// One proposed sub-goal emitted by a [`GoalDecomposer`].
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SubGoalProposal {
    /// What the sub-goal is.
    pub description: String,
    /// The explicit, independently-verifiable criterion for when it is done.
    pub done_criterion: String,
    /// Optional ordering: indices (into the emitted proposal list) of sibling
    /// sub-goals this one is gated on. Recorded as `depends_on` edges.
    #[serde(default)]
    pub depends_on: Vec<usize>,
}

/// Where a decomposition's children landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildPlacement {
    /// Children replaced the parent on the active board (there was room).
    Board,
    /// Children overflowed to the backlog (promoting them all would have
    /// exceeded [`MAX_ACTIVE_GOALS`]); the parent stays active as the roll-up
    /// anchor.
    Backlog,
}

/// The result of a successful [`decompose_goal`].
#[derive(Clone, Debug, PartialEq)]
pub struct DecomposeOutcome {
    /// The parent goal that was decomposed.
    pub parent_id: String,
    /// The ids assigned to the new child goals (in proposal order).
    pub child_ids: Vec<String>,
    /// Where the children landed.
    pub placement: ChildPlacement,
}

/// The decomposition seam: turn one parent goal into a set of proposed
/// sub-goals. The deterministic-fallback / bounds enforcement lives in
/// [`decompose_goal`], not here, so an implementation just proposes.
pub trait GoalDecomposer {
    /// Propose sub-goals for `parent`. `max_children` is the effective,
    /// already-clamped (`2..=6`) ceiling the caller will enforce.
    fn propose_subgoals(
        &self,
        parent: &ActiveGoal,
        max_children: usize,
    ) -> SimardResult<Vec<SubGoalProposal>>;
}

/// Decompose `goal_id` (which must be on the active board) into 2..=6 linked
/// sub-goals, writing the parent↔child edges into the graph and placing the
/// children on the board (replacing the parent) or in the backlog (parent kept
/// as anchor) without exceeding [`MAX_ACTIVE_GOALS`].
///
/// `max_children` is clamped to `2..=6`. Returns an error — leaving the board
/// and graph **untouched** — when the goal is unknown, the decomposer fails, or
/// the decomposition yields fewer than [`MIN_SUBGOALS`] sub-goals.
pub fn decompose_goal(
    mem: &dyn CognitiveMemoryOps,
    board: &mut GoalBoard,
    goal_id: &str,
    decomposer: &dyn GoalDecomposer,
    max_children: usize,
) -> SimardResult<DecomposeOutcome> {
    let parent = board
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .cloned()
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal_id".to_string(),
            reason: format!("goal '{goal_id}' is not on the active board; cannot decompose"),
        })?;

    let effective_max = max_children.clamp(MIN_SUBGOALS, MAX_SUBGOALS);

    // 1) Propose. A decomposer failure surfaces loudly with no mutation.
    let mut proposals = decomposer.propose_subgoals(&parent, effective_max)?;

    // 2) Enforce the fan-out bounds. Clamp the upper end, reject the lower end
    //    as a loud fallback (a single slice is not a decomposition).
    proposals.truncate(effective_max);
    if proposals.len() < MIN_SUBGOALS {
        return Err(SimardError::InvalidGoalRecord {
            field: "sub_goals".to_string(),
            reason: format!(
                "decomposition of '{goal_id}' produced {} sub-goal(s); a real decomposition needs at least {MIN_SUBGOALS}",
                proposals.len()
            ),
        });
    }

    // 3) Assign deterministic child ids (stable so a re-run dedups its edges).
    let child_ids: Vec<String> = (1..=proposals.len())
        .map(|i| format!("{goal_id}-c{i}"))
        .collect();

    // 3b) Validate every id up front so a malformed id fails loudly *before*
    //     any node/edge/board mutation — decomposition is all-or-nothing.
    crate::engineer_worktree::validate_goal_id(goal_id).map_err(|reason| {
        SimardError::InvalidGoalRecord {
            field: "goal_id".to_string(),
            reason,
        }
    })?;
    for child_id in &child_ids {
        crate::engineer_worktree::validate_goal_id(child_id).map_err(|reason| {
            SimardError::InvalidGoalRecord {
                field: "child_goal_id".to_string(),
                reason,
            }
        })?;
    }

    // 4) Decide placement up front (drives whether the parent is replaced).
    //    The parent is on the active board, so removing it frees one slot.
    let active_without_parent = board.active.len() - 1;
    let placement = if active_without_parent + child_ids.len() <= MAX_ACTIVE_GOALS {
        ChildPlacement::Board
    } else {
        ChildPlacement::Backlog
    };

    // 5) Write the graph: a node anchor + a decomposes_into edge per child, and
    //    depends_on edges for any sibling ordering. Edges are written
    //    regardless of placement, so backlog children are still queryable.
    let parent_node = GoalNode::new(
        parent.id.clone(),
        parent.description.clone(),
        None::<String>,
    );
    write_node(mem, &parent_node)?;
    for (idx, child_id) in child_ids.iter().enumerate() {
        let proposal = &proposals[idx];
        write_node(
            mem,
            &GoalNode::new(
                child_id.clone(),
                proposal.description.clone(),
                Some(proposal.done_criterion.clone()),
            ),
        )?;
        write_edge(
            mem,
            &GoalEdge::new(goal_id, child_id.clone(), GoalEdgeType::DecomposesInto),
        )?;
        for dep in &proposal.depends_on {
            if let Some(dep_id) = child_ids.get(*dep)
                && dep_id != child_id
            {
                write_edge(
                    mem,
                    &GoalEdge::new(child_id.clone(), dep_id.clone(), GoalEdgeType::DependsOn),
                )?;
            }
        }
    }

    // 6) Mutate the board.
    match placement {
        ChildPlacement::Board => {
            // Children replace the parent on the active board; the parent is
            // demoted to a backlog tracking node so it stays the roll-up anchor
            // (its GoalNode + edges remain in the graph regardless).
            board.active.retain(|g| g.id != goal_id);
            board.backlog.push(BacklogItem {
                id: parent.id.clone(),
                description: parent.description.clone(),
                source: "decompose-parent".to_string(),
                // Score 0.0 so the demoted umbrella is not eagerly re-promoted
                // over real backlog work; it is a tracking anchor, not a task.
                score: 0.0,
            });
            // Issue #2695 follow-up: a decomposition's children would otherwise
            // ALL inherit the single flat `parent.priority` — the exact "flat
            // siblings ⇒ no prioritization" case the operator complains about.
            // Differentiate them with the prioritization pass, driven by the
            // structured inter-sibling `depends_on` ordering the decomposer
            // emitted (a sibling others depend on is a bottleneck and ranks up).
            // The pass keeps a no-signal child on the neutral tier, so it
            // spreads the siblings without clobbering.
            let children: Vec<ActiveGoal> = child_ids
                .iter()
                .zip(proposals.iter())
                .map(|(child_id, proposal)| {
                    ActiveGoal::new(
                        child_id.clone(),
                        proposal.description.clone(),
                        parent.priority,
                    )
                    .with_repo(parent.repo.clone())
                    .with_parent(Some(goal_id.to_string()))
                })
                .collect();
            let signals = sibling_dependency_signals(&child_ids, &proposals);
            for child in prioritize(&children, &signals, chrono::Utc::now()) {
                board.active.push(child);
            }
        }
        ChildPlacement::Backlog => {
            // No room to promote the children: the parent stays active as the
            // roll-up anchor and the children overflow to the backlog (still
            // carrying their parent edges).
            for (child_id, proposal) in child_ids.iter().zip(proposals.iter()) {
                board.backlog.push(BacklogItem {
                    id: child_id.clone(),
                    description: proposal.description.clone(),
                    source: format!("decompose:{goal_id}"),
                    score: f64::from(parent.priority),
                });
            }
        }
    }

    Ok(DecomposeOutcome {
        parent_id: goal_id.to_string(),
        child_ids,
        placement,
    })
}

/// Build the [`PrioritizationSignals`] for a freshly-decomposed sibling set from
/// the decomposer's structured `depends_on` ordering (issue #2695 follow-up).
///
/// Maps each child id to the ids of the siblings it is gated on (its blockers),
/// mirroring the `depends_on` edges written into the graph. The prioritization
/// pass reads this to rank a depended-on sibling (a bottleneck) above the leaves
/// that wait on it. Out-of-range and self-referential indices are dropped so a
/// malformed proposal cannot forge a signal.
fn sibling_dependency_signals(
    child_ids: &[String],
    proposals: &[SubGoalProposal],
) -> PrioritizationSignals {
    let mut depends_on: HashMap<String, Vec<String>> = HashMap::new();
    for (idx, child_id) in child_ids.iter().enumerate() {
        let Some(proposal) = proposals.get(idx) else {
            continue;
        };
        let blockers: Vec<String> = proposal
            .depends_on
            .iter()
            .filter_map(|&dep| child_ids.get(dep))
            .filter(|dep_id| *dep_id != child_id)
            .cloned()
            .collect();
        if !blockers.is_empty() {
            depends_on.insert(child_id.clone(), blockers);
        }
    }
    PrioritizationSignals { depends_on }
}

// ---------------------------------------------------------------------------
// Recipe-runner-backed decomposer (the production path)
// ---------------------------------------------------------------------------

const RECIPE_FILENAME: &str = "goal-decomposition.yaml";

/// Resolve the recipe YAML path: hot-reload dir first, then in-tree.
fn resolve_recipe_path(repo_root: &Path) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// A [`GoalDecomposer`] that delegates to `recipe-runner-rs` executing the
/// `goal-decomposition.yaml` recipe, then parses the agent's JSON output. This
/// is the production path the operator CLI (`simard goal decompose`) uses.
pub struct RecipeGoalDecomposer {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeGoalDecomposer {
    /// Construct the decomposer if both the recipe and `recipe-runner-rs` are
    /// available; returns `None` otherwise so the caller can surface a clear
    /// configuration error rather than silently degrading.
    pub fn new(repo_root: &Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
    }
}

impl GoalDecomposer for RecipeGoalDecomposer {
    fn propose_subgoals(
        &self,
        parent: &ActiveGoal,
        max_children: usize,
    ) -> SimardResult<Vec<SubGoalProposal>> {
        let plan = parent
            .current_activity
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();

        // Bound the free-text goal vars before they ride on argv (issues
        // #2640/#2692). A goal description / plan is goal-scoped and small in
        // practice, but bounding closes the E2BIG argv-overflow class defensively
        // and — reusing the ooda_brain sanitizer — also collapses newlines so a
        // multi-line description can never break YAML interpolation (#2127). The
        // cap is generous (8000 chars) so real goal text is never truncated.
        let goal_description =
            crate::ooda_brain::sanitize::sanitize_context_var(&parent.description, 8000);
        let plan = crate::ooda_brain::sanitize::sanitize_context_var(&plan, 8000);

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("goal_id={}", parent.id))
            .arg("-c")
            .arg(format!("goal_description={goal_description}"))
            .arg("-c")
            .arg(format!("plan={plan}"))
            .arg("-c")
            .arg(format!("max_children={max_children}"))
            .output()
            .map_err(|e| SimardError::InvalidGoalRecord {
                field: "decomposer".to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::InvalidGoalRecord {
                field: "decomposer".to_string(),
                reason: format!(
                    "recipe-runner-rs exited with {}: {}",
                    output.status,
                    truncate(&stderr, 200)
                ),
            });
        }

        let raw = String::from_utf8_lossy(&output.stdout);
        parse_subgoals_json(&raw)
    }
}

/// JSON shapes the decomposition prompt is allowed to emit: either a wrapped
/// object `{"sub_goals": [...]}` or a bare array `[...]`.
#[derive(Deserialize)]
#[serde(untagged)]
enum SubGoalsPayload {
    Wrapped { sub_goals: Vec<SubGoalProposal> },
    Bare(Vec<SubGoalProposal>),
}

impl SubGoalsPayload {
    fn into_subgoals(self) -> Vec<SubGoalProposal> {
        match self {
            Self::Wrapped { sub_goals } => sub_goals,
            Self::Bare(list) => list,
        }
    }
}

/// Parse the decomposition agent's stdout into sub-goal proposals.
///
/// The agent emits a JSON object `{"sub_goals": [{description, done_criterion,
/// depends_on?}, …]}` (or a bare array). Prose / markdown fences around the
/// JSON are tolerated by scanning for the outermost object, then array.
pub fn parse_subgoals_json(text: &str) -> SimardResult<Vec<SubGoalProposal>> {
    for candidate in json_candidates(text) {
        if let Ok(payload) = serde_json::from_str::<SubGoalsPayload>(candidate) {
            return Ok(payload.into_subgoals());
        }
    }
    Err(SimardError::InvalidGoalRecord {
        field: "sub_goals".to_string(),
        reason: format!(
            "could not parse sub-goals from decomposition output: {}",
            truncate(text.trim(), 200)
        ),
    })
}

/// Candidate JSON slices to try, in order: the whole trimmed text, the
/// outermost `{…}` object, then the outermost `[…]` array.
fn json_candidates(text: &str) -> Vec<&str> {
    let trimmed = text.trim();
    let mut candidates = vec![trimmed];
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && end >= start
    {
        candidates.push(&trimmed[start..=end]);
    }
    if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']'))
        && end >= start
    {
        candidates.push(&trimmed[start..=end]);
    }
    candidates
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wrapped_object() {
        let text = r#"{"sub_goals":[
            {"description":"A","done_criterion":"a done"},
            {"description":"B","done_criterion":"b done","depends_on":[0]}
        ]}"#;
        let subs = parse_subgoals_json(text).expect("parse");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].description, "A");
        assert_eq!(subs[1].depends_on, vec![0]);
    }

    #[test]
    fn parse_bare_array() {
        let text = r#"[{"description":"A","done_criterion":"x"},{"description":"B","done_criterion":"y"}]"#;
        let subs = parse_subgoals_json(text).expect("parse");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].description, "B");
        assert!(subs[0].depends_on.is_empty());
    }

    #[test]
    fn parse_tolerates_prose_and_fences() {
        let text = "Here is the decomposition:\n```json\n{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\n```\nDone.";
        let subs = parse_subgoals_json(text).expect("parse");
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn parse_rejects_non_json() {
        assert!(parse_subgoals_json("no json here").is_err());
    }
}
