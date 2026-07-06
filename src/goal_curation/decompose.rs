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

/// The `sub_goals` transport error (issue #2708). Every failure to obtain a
/// usable sub-goals result from the decomposition agent's dedicated result file
/// — missing, empty, oversized, malformed, or fewer than [`MIN_SUBGOALS`] — is
/// the same loud `InvalidGoalRecord` on the `sub_goals` field.
fn sub_goals_error(reason: String) -> SimardError {
    SimardError::InvalidGoalRecord {
        field: "sub_goals".to_string(),
        reason,
    }
}

/// The `decomposer` process error: the `recipe-runner-rs` subprocess itself
/// failed (result dir could not be created, spawn failed, or a non-zero exit) —
/// a loud `InvalidGoalRecord` on the `decomposer` field, kept distinct from a
/// `sub_goals` transport failure so the two failure classes never blur.
fn decomposer_error(reason: String) -> SimardError {
    SimardError::InvalidGoalRecord {
        field: "decomposer".to_string(),
        reason,
    }
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
        return Err(sub_goals_error(format!(
            "decomposition of '{goal_id}' produced {} sub-goal(s); a real decomposition needs at least {MIN_SUBGOALS}",
            proposals.len()
        )));
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
/// `goal-decomposition.yaml` recipe, then reads the agent's sub-goal proposals
/// from a dedicated result **file** it was told to write (never recipe-runner
/// stdout; issue #2708). This is the production path the operator CLI (`simard
/// goal decompose`) uses.
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

        // Dedicated, per-invocation result file the decomposition agent writes
        // its `{"sub_goals":[…]}` envelope to (issue #2708). recipe-runner
        // stdout is full of ANSI escapes, tracing lines, and the launcher
        // banner, so scraping it for the outermost `{…}` slice discarded good
        // (successful, ~48s) runs. A fresh tempdir (mode 0700 via the
        // `tempfile` crate) gives a unique absolute path with no
        // cross-invocation races; the directory and its contents are removed
        // when `result_dir` drops at the end of this call, AFTER the file has
        // been read. The agent is told this path via `-c sub_goals_output=…`
        // and the recipe prompt instructs it to write ONLY the envelope there
        // — stdout is never read for the result. This mirrors the distillation
        // clean-result-channel fix (issues #2622/#2619) and aligns with the
        // amplihack-rs semanticchannel work (the durable substrate this file
        // channel can migrate onto later).
        let result_dir = tempfile::Builder::new()
            .prefix("simard-decompose-")
            .tempdir()
            .map_err(|e| {
                decomposer_error(format!("failed to create sub-goals output tempdir: {e}"))
            })?;
        let result_path = result_dir.path().join("sub_goals.json");
        let result_path_arg = result_path.to_string_lossy().into_owned();

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
            .arg("-c")
            .arg(format!("sub_goals_output={result_path_arg}"))
            .output()
            .map_err(|e| decomposer_error(format!("recipe-runner-rs spawn failed: {e}")))?;

        // Read the agent's proposals from the dedicated result file — NEVER
        // from stdout (issue #2708). `result_dir` stays alive until this call
        // returns, so the file exists while it is read.
        harvest_subgoals_file(&output, &result_path)
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

/// Maximum size (bytes) accepted for the dedicated sub-goals result file. A
/// runaway agent that writes an enormous file must be rejected loudly *before*
/// the read, never allowed to OOM the process.
const MAX_SUBGOALS_FILE_BYTES: u64 = 1024 * 1024;

/// Post-process a finished `recipe-runner-rs` invocation into the decomposition
/// agent's sub-goal proposals, reading them from the dedicated result **file**
/// the agent was told to write (`-c sub_goals_output=…`) — NEVER from stdout
/// (issue #2708).
///
/// * A non-zero exit is a **loud** terminal `decomposer` failure carrying the
///   truncated stderr and stdout, so a failed run is never silent.
/// * On a clean (exit-0) run the proposals are read from `path`. A missing,
///   empty/whitespace-only, or oversized file is a **loud** `sub_goals` error.
///   There is deliberately NO stdout fallback: scraping stdout is exactly the
///   launcher-banner contamination this fix removes.
///
/// Split out of [`RecipeGoalDecomposer::propose_subgoals`] so the "stdout noise
/// is inert" contract is hermetically testable without spawning a subprocess.
fn harvest_subgoals_file(
    output: &std::process::Output,
    path: &Path,
) -> SimardResult<Vec<SubGoalProposal>> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(decomposer_error(format!(
            "recipe-runner-rs exited with {}: stderr={} stdout={}",
            output.status,
            truncate(stderr.trim(), 200),
            truncate(stdout.trim(), 200)
        )));
    }

    // Size guard BEFORE the read. A missing file surfaces here as a loud
    // `sub_goals` error (the agent produced no result), never a stdout fallback.
    let meta = std::fs::metadata(path).map_err(|e| {
        sub_goals_error(format!(
            "decomposition result file {} was not written by the agent: {e}",
            path.display()
        ))
    })?;
    if meta.len() > MAX_SUBGOALS_FILE_BYTES {
        return Err(sub_goals_error(format!(
            "decomposition result file {} is {} bytes, exceeding the {MAX_SUBGOALS_FILE_BYTES}-byte cap",
            path.display(),
            meta.len()
        )));
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        sub_goals_error(format!(
            "decomposition result file {} could not be read: {e}",
            path.display()
        ))
    })?;

    if contents.trim().is_empty() {
        return Err(sub_goals_error(format!(
            "decomposition result file {} was empty; the agent wrote no sub-goals",
            path.display()
        )));
    }

    parse_subgoals_json(&contents)
}

/// Parse the decomposition agent's result-file contents into sub-goal proposals.
///
/// The agent writes a JSON object `{"sub_goals": [{description, done_criterion,
/// depends_on?}, …]}` (or a bare array) to its dedicated result file. Because
/// that file is a clean channel — not noisy recipe-runner stdout (issue #2708)
/// — this is a **strict** deserializer, not a scraper: tolerance is bounded to
/// leading/trailing whitespace and at most one wrapping markdown code fence.
/// Any prose around the JSON is a **loud** error; the old outermost-brace scan
/// that used to salvage prose is exactly the brittle transport being deleted.
pub fn parse_subgoals_json(text: &str) -> SimardResult<Vec<SubGoalProposal>> {
    let cleaned = strip_optional_code_fence(text);
    serde_json::from_str::<SubGoalsPayload>(cleaned)
        .map(SubGoalsPayload::into_subgoals)
        .map_err(|e| {
            sub_goals_error(format!(
                "could not parse sub-goals from decomposition result: {e} (input: {})",
                truncate(text.trim(), 200)
            ))
        })
}

/// Strip a single markdown code fence that wraps the *entire* payload, returning
/// the fenced body. Only a fence around the whole text is tolerated (```` ```json
/// … ``` ```` or a bare ```` ``` … ``` ````); a fence embedded in prose, or prose
/// around a fence, is left intact so the strict parse rejects it.
fn strip_optional_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("```")
        && let Some(inner) = rest.strip_suffix("```")
    {
        // Drop the opening fence's info string (e.g. `json`) up to the first
        // newline, keeping only the fenced body.
        let body = match inner.find('\n') {
            Some(nl) => &inner[nl + 1..],
            None => inner,
        };
        return body.trim();
    }
    trimmed
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
    use crate::cognitive_memory::LibraryCognitiveMemory;
    use crate::goal_curation::edges::{children_of, edges_of_type};

    // ── Group A: `parse_subgoals_json` is now a STRICT deserializer over CLEAN
    //    file contents — not a stdout brace-scraper (issue #2708). Tolerance is
    //    bounded to leading/trailing whitespace + at most one wrapping ```json
    //    fence. Any prose around the JSON must be REJECTED, because the outermost
    //    brace/bracket scan (`json_candidates`) that used to salvage prose is the
    //    exact defect being deleted. ──────────────────────────────────────────

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
    fn parse_tolerates_leading_trailing_whitespace() {
        let text = "\n\n   {\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}   \n\t\n";
        let subs = parse_subgoals_json(text).expect("surrounding whitespace must be tolerated");
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn parse_tolerates_single_json_fence() {
        // A single wrapping ```json … ``` fence (and nothing else) is the only
        // markdown tolerance allowed. No prose before or after.
        let text = "```json\n{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\n```";
        let subs = parse_subgoals_json(text).expect("a single json fence must be tolerated");
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn parse_tolerates_bare_fence_without_lang() {
        let text = "```\n[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]\n```";
        let subs = parse_subgoals_json(text).expect("a bare ``` fence must be tolerated");
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn parse_rejects_prose_around_json() {
        // THE #2708 regression guard: the old `json_candidates` brace-scan would
        // salvage the JSON out of surrounding prose. That path is deleted, so a
        // payload with prose around the object is now a LOUD parse failure — the
        // agent must write ONLY its JSON to the dedicated file.
        let text = "Here is the decomposition:\n{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\nDone.";
        let err = parse_subgoals_json(text)
            .expect_err("prose around JSON must be rejected once brace-scanning is removed");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_fence_with_surrounding_prose() {
        // A fenced block embedded in prose must also be rejected: only a single
        // *wrapping* fence (the whole payload) is tolerated, never fence-inside-prose.
        let text = "Sure!\n```json\n{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\n```\nHope that helps.";
        assert!(
            parse_subgoals_json(text).is_err(),
            "a fenced block surrounded by prose must not be salvaged"
        );
    }

    #[test]
    fn parse_rejects_non_json() {
        let err = parse_subgoals_json("no json here").expect_err("non-json is a loud error");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    // ── Group B: `harvest_subgoals_file` is the hermetic AGENT→SIMARD transport
    //    seam. It reads the dedicated result FILE the agent wrote and NEVER
    //    parses stdout. Constructed with a synthetic `std::process::Output` so
    //    the "stdout noise is inert" contract is provable without a subprocess.
    //    Mirrors distillation's `harvest_facts_file`, but STRICTER (adds a size
    //    cap + an empty/whitespace rejection distill lacks). ────────────────────

    /// A realistic slice of noisy recipe-runner stdout: ANSI, tracing lines, and
    /// the copilot launcher banner. Under the old code its outermost `{…}` slice
    /// was scraped as the result and failed to parse, discarding a good run.
    #[cfg(unix)]
    const NOISY_RECIPE_STDOUT: &str = "\u{1b}[2m2026-07-06T22:31:04.101010Z\u{1b}[0m  INFO recipe_runner: launching copilot binary=/home/azureuser/.npm-global/bin/copilot version=\"GitHub Copilot CLI 1.0.69-2\"\n\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference)\n\u{1b}[32m  INFO\u{1b}[0m step goal-decomposition running… {\"noise\":true}\nRun 'copilot update' to update\n";

    #[cfg(unix)]
    fn output_with(stdout: &[u8], code: i32) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    /// The headline #2708 regression: recipe-runner stdout is nothing but noise
    /// (ANSI + tracing + launcher banner), yet the agent wrote a clean sub-goals
    /// envelope to the dedicated file. Harvest MUST succeed from the file and the
    /// noisy stdout must be completely inert — no parse failure, no discarded run.
    #[cfg(unix)]
    #[test]
    fn noisy_stdout_does_not_break_decomposition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        std::fs::write(
            &path,
            r#"{"sub_goals":[{"description":"Slice A","done_criterion":"A done"},{"description":"Slice B","done_criterion":"B done","depends_on":[0]}]}"#,
        )
        .unwrap();

        let output = output_with(NOISY_RECIPE_STDOUT.as_bytes(), 0);
        let subs = harvest_subgoals_file(&output, &path)
            .expect("noisy stdout must NOT block reading the clean sub-goals file");
        assert_eq!(subs.len(), 2, "sub-goals come from the file, not stdout");
        assert_eq!(subs[0].description, "Slice A");
        assert_eq!(subs[1].depends_on, vec![0]);
    }

    /// A clean exit-0 run reads the sub-goals verbatim from the file.
    #[cfg(unix)]
    #[test]
    fn clean_file_reads_subgoals_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        std::fs::write(
            &path,
            r#"[{"description":"A","done_criterion":"x"},{"description":"B","done_criterion":"y"},{"description":"C","done_criterion":"z"}]"#,
        )
        .unwrap();
        let output = output_with(b"", 0);
        let subs = harvest_subgoals_file(&output, &path).expect("clean run reads the file");
        assert_eq!(subs.len(), 3);
    }

    /// No silent fallback: a missing result file is a LOUD `sub_goals` error even
    /// when stdout carries a perfectly well-formed sub-goals object — proving
    /// stdout is never scraped as a fallback result channel.
    #[cfg(unix)]
    #[test]
    fn missing_subgoals_file_is_loud_error_never_stdout_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json"); // deliberately never written

        let tempting_stdout =
            br#"{"sub_goals":[{"description":"IGNORE ME","done_criterion":"from stdout"},{"description":"B","done_criterion":"y"}]}"#;
        let output = output_with(tempting_stdout, 0);
        let err = harvest_subgoals_file(&output, &path)
            .expect_err("a missing result file must be a loud error, never a stdout fallback");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    /// The agent created the file but wrote nothing (or only whitespace): a loud
    /// `sub_goals` error, never a hollow empty decomposition.
    #[cfg(unix)]
    #[test]
    fn empty_subgoals_file_is_loud_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        std::fs::write(&path, "   \n\t  \n").unwrap();
        let output = output_with(b"", 0);
        let err = harvest_subgoals_file(&output, &path)
            .expect_err("an empty/whitespace-only file must surface loudly");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    /// The agent wrote garbage to the file: a loud `sub_goals` parse error.
    #[cfg(unix)]
    #[test]
    fn malformed_subgoals_file_is_loud_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        std::fs::write(&path, "{ this is not valid json ]").unwrap();
        let output = output_with(b"", 0);
        let err = harvest_subgoals_file(&output, &path)
            .expect_err("a malformed file must surface loudly");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    /// A runaway agent that writes an oversized file (> 1 MiB) is rejected by the
    /// size guard BEFORE the read/parse — a loud `sub_goals` error (R-SEC-3),
    /// never an OOM.
    #[cfg(unix)]
    #[test]
    fn oversized_subgoals_file_is_loud_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        let oversized = vec![b'a'; 1024 * 1024 + 1];
        std::fs::write(&path, &oversized).unwrap();
        let output = output_with(b"", 0);
        let err = harvest_subgoals_file(&output, &path)
            .expect_err("an oversized result file must be rejected loudly");
        match err {
            SimardError::InvalidGoalRecord { field, .. } => assert_eq!(field, "sub_goals"),
            other => panic!("expected InvalidGoalRecord{{field:\"sub_goals\"}}, got {other:?}"),
        }
    }

    /// A non-zero recipe exit is a LOUD terminal decomposer failure that carries
    /// context — never a silent success and never confused with a transport
    /// (`sub_goals`) failure.
    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_loud_decomposer_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub_goals.json");
        let output = output_with(b"boom on stdout", 3);
        let err = harvest_subgoals_file(&output, &path)
            .expect_err("a non-zero exit must surface an explicit error");
        match err {
            SimardError::InvalidGoalRecord { field, reason } => {
                assert_eq!(field, "decomposer");
                assert!(
                    reason.contains('3') || reason.to_lowercase().contains("exit"),
                    "the error must carry the exit context: {reason}"
                );
            }
            other => panic!("expected InvalidGoalRecord{{field:\"decomposer\"}}, got {other:?}"),
        }
    }

    // ── Group C: `decompose_goal` must never panic on an out-of-range or
    //    self-referential `depends_on` index the agent supplied — every index is
    //    bounds-checked before a DependsOn edge is written (R-SEC-4). ───────────

    struct FixedDecomposer {
        proposals: Vec<SubGoalProposal>,
    }

    impl GoalDecomposer for FixedDecomposer {
        fn propose_subgoals(
            &self,
            _parent: &ActiveGoal,
            _max_children: usize,
        ) -> SimardResult<Vec<SubGoalProposal>> {
            Ok(self.proposals.clone())
        }
    }

    #[test]
    fn decompose_drops_out_of_range_and_self_depends_on_without_panic() {
        let m = LibraryCognitiveMemory::in_memory().expect("in-memory cognitive memory");
        let mut board = GoalBoard::new();
        board.active.push(ActiveGoal::new("goal-p", "Umbrella", 1));

        // child c2 (index 1) claims depends_on = [5 (out of range), 1 (self)];
        // both are invalid and must be dropped, not indexed into a panic.
        let decomposer = FixedDecomposer {
            proposals: vec![
                SubGoalProposal {
                    description: "A".to_string(),
                    done_criterion: "x".to_string(),
                    depends_on: vec![],
                },
                SubGoalProposal {
                    description: "B".to_string(),
                    done_criterion: "y".to_string(),
                    depends_on: vec![5, 1],
                },
            ],
        };

        let outcome = decompose_goal(&m, &mut board, "goal-p", &decomposer, 6)
            .expect("an out-of-range depends_on must not fail or panic the decomposition");
        assert_eq!(outcome.child_ids.len(), 2);
        assert_eq!(
            children_of(&m, "goal-p").unwrap().len(),
            2,
            "both children are still written"
        );
        assert!(
            edges_of_type(&m, GoalEdgeType::DependsOn, "goal-p-c2")
                .unwrap()
                .is_empty(),
            "no DependsOn edge may be forged from an out-of-range or self index"
        );
    }
}
