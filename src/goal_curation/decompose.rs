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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::edges::{write_edge, write_node};
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
            for (child_id, proposal) in child_ids.iter().zip(proposals.iter()) {
                board.active.push(
                    ActiveGoal::new(
                        child_id.clone(),
                        proposal.description.clone(),
                        parent.priority,
                    )
                    .with_repo(parent.repo.clone())
                    .with_parent(Some(goal_id.to_string())),
                );
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
/// `goal-decomposition.yaml` recipe, then reads the agent's proposed sub-goals
/// back from a dedicated result **file** (issue #2708) — never by scraping the
/// shared, noisy `recipe-runner-rs` stdout. This is the production path the
/// operator CLI (`simard goal decompose`) uses.
pub struct RecipeGoalDecomposer {
    recipe_path: PathBuf,
    agent_binary: &'static str,
    /// Repository root, stored so each decomposition's result file lands in-tree
    /// under `<repo_root>/target/`, where a workspace-sandboxed agent can
    /// reliably write it (a `/tmp` path can be refused by copilot/claude).
    repo_root: PathBuf,
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
            repo_root: repo_root.to_path_buf(),
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

        // Dedicated clean result channel (#2708): the agent writes ONLY its
        // {"sub_goals":[…]} JSON to a file we own, so a successful decomposition
        // can never be discarded by ANSI/log/banner noise on the shared
        // recipe-runner-rs stdout. The result dir lives in-tree under target/
        // (git-ignored and reliably writable by a workspace-sandboxed agent,
        // unlike a /tmp path) and is an unpredictable O_EXCL TempDir (0700) to
        // defeat symlink/TOCTOU races. The handle is held alive across the child
        // run and the read, then dropped (RAII) to remove the file on every exit.
        let target_dir = self.repo_root.join("target");
        fs::create_dir_all(&target_dir).map_err(|e| SimardError::InvalidGoalRecord {
            field: "decomposer".to_string(),
            reason: format!(
                "could not create result-channel dir '{}': {e}",
                target_dir.display()
            ),
        })?;
        let result_dir = tempfile::Builder::new()
            .prefix("simard-subgoals-")
            .tempdir_in(&target_dir)
            .map_err(|e| SimardError::InvalidGoalRecord {
                field: "decomposer".to_string(),
                reason: format!("could not allocate result-channel dir: {e}"),
            })?;
        // Pass an ABSOLUTE path so the agent resolves it against its own cwd
        // correctly. We do NOT pre-create the file — some agents are create-only
        // and refuse to overwrite — so the agent creates it in this owned dir.
        let result_path = {
            let p = result_dir.path().join("sub_goals.json");
            std::path::absolute(&p).unwrap_or(p)
        };

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("goal_id={}", parent.id))
            .arg("-c")
            .arg(format!("goal_description={}", parent.description))
            .arg("-c")
            .arg(format!("plan={plan}"))
            .arg("-c")
            .arg(format!("max_children={max_children}"))
            .arg("-c")
            .arg(format!("sub_goals_output={}", result_path.display()))
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

        // The payload arrives ONLY through the result file; stdout is never
        // parsed for sub-goals (that stdout brace-scan was the #2708 root cause).
        // `result_dir` is still in scope here, so the file is not yet cleaned up.
        read_subgoals_from_file(&result_path)
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

/// Strict-parse the decomposition result text into sub-goal proposals.
///
/// A single-shot `serde_json::from_str::<SubGoalsPayload>` on the trimmed input:
/// either the wrapped `{"sub_goals": […]}` object or a bare `[…]` array. Any
/// prose, log noise, or markdown fence around the JSON is **rejected** — that
/// outermost-brace tolerance was the #2708 root cause, where noise on the shared
/// `recipe-runner-rs` stdout discarded successful decompositions. Malformed
/// input surfaces a loud `InvalidGoalRecord { field: "sub_goals" }`.
pub fn parse_subgoals_json(text: &str) -> SimardResult<Vec<SubGoalProposal>> {
    let trimmed = text.trim();
    serde_json::from_str::<SubGoalsPayload>(trimmed)
        .map(SubGoalsPayload::into_subgoals)
        .map_err(|e| SimardError::InvalidGoalRecord {
            field: "sub_goals".to_string(),
            reason: format!(
                "could not parse sub-goals as JSON ({e}): {}",
                truncate(trimmed, 200)
            ),
        })
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

/// Maximum size (in bytes) Simard will read from the result-channel file. A
/// runaway or hostile agent must not be able to OOM Simard with an unbounded
/// write; content past this ~1 MiB cap is rejected loudly rather than buffered.
const MAX_SUBGOALS_FILE_BYTES: u64 = 1024 * 1024;

/// Read the decomposition agent's structured sub-goal proposals from the
/// dedicated result-channel file (issue #2708).
///
/// This is the clean-transport replacement for scraping `recipe-runner-rs`
/// stdout: the agent writes **only** its `{"sub_goals": [...]}` JSON to this
/// file, and Simard reads it back with a **strict**, single-shot parse (no
/// outermost-brace scanning, no prose tolerance). A missing, empty, oversized,
/// or malformed file surfaces a **loud** `InvalidGoalRecord { field:
/// "sub_goals" }` and never an empty `Vec`.
fn read_subgoals_from_file(path: &Path) -> SimardResult<Vec<SubGoalProposal>> {
    let meta = fs::metadata(path).map_err(|e| SimardError::InvalidGoalRecord {
        field: "sub_goals".to_string(),
        reason: format!(
            "decomposition wrote no result file at '{}': {e}",
            path.display()
        ),
    })?;
    if meta.len() > MAX_SUBGOALS_FILE_BYTES {
        return Err(SimardError::InvalidGoalRecord {
            field: "sub_goals".to_string(),
            reason: format!(
                "decomposition result file '{}' is {} bytes, over the {MAX_SUBGOALS_FILE_BYTES}-byte cap",
                path.display(),
                meta.len()
            ),
        });
    }
    let raw = fs::read_to_string(path).map_err(|e| SimardError::InvalidGoalRecord {
        field: "sub_goals".to_string(),
        reason: format!(
            "decomposition result file '{}' is unreadable: {e}",
            path.display()
        ),
    })?;
    if raw.trim().is_empty() {
        return Err(SimardError::InvalidGoalRecord {
            field: "sub_goals".to_string(),
            reason: format!(
                "decomposition result file '{}' is empty; the agent wrote no sub-goals",
                path.display()
            ),
        });
    }
    parse_subgoals_json(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{SimardError, SimardResult};
    use std::fs;
    use tempfile::{TempDir, tempdir};

    // ── Strict parser (`parse_subgoals_json`): clean JSON only, no scraping ──

    #[test]
    fn parse_wrapped_object() {
        let text = r#"{"sub_goals":[
            {"description":"A","done_criterion":"a done"},
            {"description":"B","done_criterion":"b done","depends_on":[0]}
        ]}"#;
        let subs = parse_subgoals_json(text).expect("clean wrapped JSON parses");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].description, "A");
        assert_eq!(subs[1].depends_on, vec![0]);
    }

    #[test]
    fn parse_bare_array() {
        let text = r#"[{"description":"A","done_criterion":"x"},{"description":"B","done_criterion":"y"}]"#;
        let subs = parse_subgoals_json(text).expect("clean bare array parses");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].description, "B");
        assert!(subs[0].depends_on.is_empty());
    }

    /// #2708: the strict parser must **reject** prose/markdown-fence-wrapped
    /// JSON. The old tolerant brace-scan "rescued" exactly this shape — the very
    /// fragility that discarded successful decompositions when the payload was
    /// surrounded by noise. Rejecting it here proves the scraping transport is
    /// gone. (Replaces the former `parse_tolerates_prose_and_fences`, which
    /// asserted the now-deleted behavior.)
    #[test]
    fn parse_rejects_prose_and_fences() {
        let text = "Here is the decomposition:\n```json\n{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\n```\nDone.";
        assert!(
            parse_subgoals_json(text).is_err(),
            "strict parser must not scrape JSON out of prose/fences (#2708)"
        );
    }

    #[test]
    fn parse_rejects_non_json() {
        assert!(parse_subgoals_json("no json here").is_err());
    }

    // ── File result-channel (`read_subgoals_from_file`): the clean handoff ──

    fn write_channel_file(dir: &TempDir, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("write channel file");
        path
    }

    /// A missing / empty / oversized / malformed structured result must be a
    /// **loud** error (never a silent empty decomposition), tagged to the
    /// `sub_goals` field so the caller can surface a clear message.
    fn assert_loud_subgoals_error(res: SimardResult<Vec<SubGoalProposal>>) {
        match res {
            Err(SimardError::InvalidGoalRecord { field, .. }) => assert_eq!(
                field, "sub_goals",
                "a bad structured result must be a loud `sub_goals` error"
            ),
            Err(other) => {
                panic!("expected InvalidGoalRecord{{ field: \"sub_goals\" }}, got {other:?}")
            }
            Ok(v) => panic!(
                "a bad structured result must surface a loud error, got Ok({} proposals)",
                v.len()
            ),
        }
    }

    /// The decomposition is delivered via a clean file the agent writes — the
    /// wrapped `{"sub_goals":[…]}` shape round-trips into typed proposals with
    /// their `depends_on` ordering intact.
    #[test]
    fn file_channel_parses_clean_payload() {
        let dir = tempdir().expect("tempdir");
        let path = write_channel_file(
            &dir,
            "subgoals.json",
            r#"{"sub_goals":[
                {"description":"Slice A","done_criterion":"A merged"},
                {"description":"Slice B","done_criterion":"B merged","depends_on":[0]}
            ]}"#,
        );

        let subs = read_subgoals_from_file(&path).expect("clean channel file parses");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].description, "Slice A");
        assert_eq!(subs[0].done_criterion, "A merged");
        assert_eq!(
            subs[1].depends_on,
            vec![0],
            "sibling ordering must survive the file handoff"
        );
    }

    /// The bare-array shape is equally accepted from the file channel.
    #[test]
    fn file_channel_parses_bare_array() {
        let dir = tempdir().expect("tempdir");
        let path = write_channel_file(
            &dir,
            "subgoals.json",
            r#"[{"description":"A","done_criterion":"x"},{"description":"B","done_criterion":"y"}]"#,
        );

        let subs = read_subgoals_from_file(&path).expect("bare array channel file parses");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].description, "B");
    }

    /// #2708 core guarantee — two halves that together prove the stdout
    /// brace-scan transport is gone:
    ///
    ///  (a) a clean file parses regardless of any noise a run would have emitted
    ///      to stdout (stdout is not part of the transport at all); and
    ///  (b) the *same* class of ANSI + tracing + banner noise that used to
    ///      surround the JSON on stdout, when it lands in the file, is now
    ///      **rejected** — the pre-#2708 outermost-brace scanner would have
    ///      found and (wrongly) "rescued" the embedded object.
    #[test]
    fn stdout_noise_does_not_rescue_parse() {
        let dir = tempdir().expect("tempdir");

        // (a) positive: a clean file is all that matters.
        let clean = write_channel_file(
            &dir,
            "clean.json",
            r#"{"sub_goals":[{"description":"A","done_criterion":"x"},{"description":"B","done_criterion":"y"}]}"#,
        );
        assert_eq!(
            read_subgoals_from_file(&clean)
                .expect("clean file parses")
                .len(),
            2
        );

        // (b) proof-of-no-scraping: real recipe-runner-style noise (ANSI colour
        // codes, tracing lines, a banner, a timing footer) wrapped around a
        // valid JSON object. The strict file parser must reject the whole thing.
        let noisy = concat!(
            "\u{1b}[2m2026-07-06T19:33:15.148Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m recipe_runner: starting goal-decomposition.yaml\n",
            "\u{1b}[1m========== recipe-runner-rs ==========\u{1b}[0m\n",
            "\u{1b}[2mDEBUG\u{1b}[0m agent: emitting result\n",
            "{\"sub_goals\":[{\"description\":\"A\",\"done_criterion\":\"x\"},{\"description\":\"B\",\"done_criterion\":\"y\"}]}\n",
            "\u{1b}[2m2026-07-06T19:34:03.900Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m recipe_runner: done in 48.7s\n",
        );
        let noisy_path = write_channel_file(&dir, "noisy.json", noisy);
        assert_loud_subgoals_error(read_subgoals_from_file(&noisy_path));
    }

    // ── Loud errors: missing / empty / malformed / oversized ──────────────────

    #[test]
    fn missing_file_is_loud() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.json");
        assert!(!path.exists());
        assert_loud_subgoals_error(read_subgoals_from_file(&path));
    }

    #[test]
    fn empty_file_is_loud() {
        let dir = tempdir().expect("tempdir");
        let path = write_channel_file(&dir, "empty.json", "   \n\t  ");
        assert_loud_subgoals_error(read_subgoals_from_file(&path));
    }

    #[test]
    fn malformed_json_file_is_loud() {
        let dir = tempdir().expect("tempdir");
        let path = write_channel_file(
            &dir,
            "malformed.json",
            r#"{"sub_goals": [ {"description": "A", "#,
        );
        assert_loud_subgoals_error(read_subgoals_from_file(&path));
    }

    /// A runaway or hostile agent must not be able to OOM Simard: content past
    /// the ~1 MiB size cap is rejected loudly even when it is otherwise valid
    /// JSON — the size guard, not the parser, is what stops it.
    #[test]
    fn oversized_file_is_loud() {
        let dir = tempdir().expect("tempdir");
        // Valid wrapped JSON whose description blows well past the ~1 MiB cap.
        let huge_desc = "A".repeat(2 * 1024 * 1024);
        let payload = format!(
            r#"{{"sub_goals":[{{"description":"{huge_desc}","done_criterion":"x"}},{{"description":"B","done_criterion":"y"}}]}}"#
        );
        assert!(payload.len() > 1024 * 1024, "payload must exceed the cap");
        let path = write_channel_file(&dir, "oversized.json", &payload);
        assert_loud_subgoals_error(read_subgoals_from_file(&path));
    }
}
