//! Core types for the goal board: goals, backlog items, and board state.

use std::fmt::{self, Display, Formatter};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of concurrently active goals.
///
/// Raised from 7 to 20 (operator directive, issue #6) so umbrella
/// decompositions can fan out fully — a 15-repo supply-chain umbrella spawns
/// many distinct per-repo goals. This governs only how many distinct goals may
/// *exist* on the board; actual concurrent *execution* stays bounded by the
/// separate AIMD engineer concurrency cap
/// (`crate::ooda_loop::OodaConfig::max_concurrent_actions`).
pub const MAX_ACTIVE_GOALS: usize = 20;

/// Progress state for an active goal.
///
/// Variants align with the spec-mandated lifecycle statuses in
/// [`crate::goals::GoalStatus`] (`Proposed`, `Active`/progress variants,
/// `Paused`, `Completed`) so the operator-facing goal curation path can
/// distinguish proposed-vs-active goals and pause goals (issue #2098).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GoalProgress {
    /// Goal has been proposed but not yet accepted onto the active board.
    Proposed,
    NotStarted,
    InProgress {
        percent: u32,
    },
    Blocked(String),
    /// Goal is temporarily paused — not blocked, but deliberately on hold.
    Paused,
    Completed,
}

impl Display for GoalProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proposed => f.write_str("proposed"),
            Self::NotStarted => f.write_str("not-started"),
            Self::InProgress { percent } => write!(f, "in-progress({percent}%)"),
            Self::Blocked(reason) => write!(f, "blocked: {reason}"),
            Self::Paused => f.write_str("paused"),
            Self::Completed => f.write_str("completed"),
        }
    }
}

/// Canonical marker that `simard goal add --standing` prepends to a goal's
/// description so the standing/perpetual nature is durable in the persisted
/// board without adding a new field to every `ActiveGoal` construction site
/// (issue #2580). Detection is by description marker rather than a bool field
/// so pre-existing live standing goals — whose descriptions already read
/// "STANDING PERPETUAL goal" / "Standing goal" — are reconciled automatically
/// without touching `~/.simard`.
pub const STANDING_MARKER_PREFIX: &str = "[standing] ";

/// Whole-phrase, case-insensitive markers in a goal description that make it a
/// standing/perpetual goal. Matched with a leading word boundary so ordinary
/// words that merely *contain* one of these substrings (e.g. "understanding",
/// "outstanding") never trigger a false positive.
const STANDING_DESCRIPTION_MARKERS: &[&str] = &[
    "standing perpetual",
    "perpetual/standing",
    "standing/perpetual",
    "standing goal",
    "perpetual goal",
];

/// The `[standing]` sentinel written by `--standing`. Detected verbatim
/// (bracket-delimited, so no word-boundary check is needed).
const STANDING_SENTINEL: &str = "[standing]";

/// True when `description` durably marks the goal as standing/perpetual.
///
/// A standing goal has no terminal done-state: it is never marked
/// `Completed` by goal-curation or the completion gate, is never tombstoned,
/// and when its current unit of work finishes it rolls to a fresh cycle
/// (see [`ActiveGoal::roll_to_new_cycle`]).
pub fn description_marks_standing(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    if lower.contains(STANDING_SENTINEL) {
        return true;
    }
    STANDING_DESCRIPTION_MARKERS
        .iter()
        .any(|phrase| contains_phrase_on_word_boundary(&lower, phrase))
}

/// `haystack_lower.contains(phrase)` but only where the match begins on a word
/// boundary (start-of-string or a non-alphanumeric char before it). Both
/// arguments must already be lowercase. Keeps "understanding goal" from
/// matching "standing goal".
fn contains_phrase_on_word_boundary(haystack_lower: &str, phrase: &str) -> bool {
    let bytes = haystack_lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack_lower[from..].find(phrase) {
        let idx = from + rel;
        let prev_is_word = idx > 0 && bytes[idx - 1].is_ascii_alphanumeric();
        if !prev_is_word {
            return true;
        }
        from = idx + 1;
    }
    false
}

/// A reference to work-in-progress associated with a goal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WipRef {
    /// Kind of reference: "pr", "issue", "branch", "session", "engineer"
    pub kind: String,
    /// Reference value: PR number, issue number, branch name, etc.
    pub ref_id: String,
    /// Human-readable label
    pub label: String,
    /// URL if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// An active goal on the board. Active goals are limited to `MAX_ACTIVE_GOALS`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveGoal {
    pub id: String,
    pub description: String,
    pub priority: u32,
    pub status: GoalProgress,
    pub assigned_to: Option<String>,
    /// Target repository slug for this goal (issue #2359, BUG 1).
    ///
    /// `None` (the default) routes the goal's engineer to the daemon's own
    /// repo ("Simard"). A slug such as `"amplihack-rs"` routes the engineer's
    /// worktree — and therefore its PRs — to `$HOME/src/amplihack-rs`. See
    /// [`crate::ooda_actions::advance_goal`]'s `repo_resolver`.
    ///
    /// `skip_serializing_if` keeps pre-#2359 goal-board snapshots byte-
    /// identical (the key is omitted entirely for repo-less goals), and
    /// `serde(default)` deserializes legacy JSON that has no `repo` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Current activity summary — what's happening right now toward this goal.
    #[serde(default)]
    pub current_activity: Option<String>,
    /// References to work-in-progress: PRs, issues, branches, sessions.
    #[serde(default)]
    pub wip_refs: Vec<WipRef>,
    /// Wall-clock timestamp of the last accepted progress update.
    ///
    /// `None` for goals created before issue #1967; the progress-evidence
    /// gate (see `progress_evidence` module) falls back to a memory scan,
    /// then to the daemon's process-start timestamp. Set automatically by
    /// `update_goal_progress_with_evidence` on every accepted update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_update_at: Option<DateTime<Utc>>,
    /// Id of the goal this sub-goal decomposes from (issue #2405). `None` for a
    /// top-level goal that was never produced by decomposition. This is the
    /// cheap, always-present back-reference that lets any consumer already
    /// holding the board group children under a parent without a graph query;
    /// the authoritative linkage is the `decomposes_into` edge in the graph
    /// (see `docs/reference/goal-decomposition.md`).
    ///
    /// `#[serde(default, skip_serializing_if)]` keeps pre-#2405 goal-board
    /// snapshots and `goal_records.json` entries byte-identical (the key is
    /// omitted entirely when unset) and lets legacy JSON without the key load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_goal_id: Option<String>,
    /// Provenance flag: `true` only when the operator EXPLICITLY set this goal's
    /// priority (via `simard goal set-priority`), `false` for every other origin
    /// — dashboard/CLI-added defaults, decomposition inheritance, seeded goals,
    /// meeting-derived goals (issue #2695 follow-up). The prioritization pass
    /// ([`super::prioritize::prioritize`]) re-scores only non-explicit goals, so
    /// this flag is what keeps the operator's hand-set priorities intact while
    /// the flat/undifferentiated ones get spread apart.
    ///
    /// `#[serde(default, skip_serializing_if)]` keeps pre-#2695 goal-board
    /// snapshots byte-identical (the key is omitted entirely when `false`) and
    /// lets legacy JSON without the key load as non-explicit (pass-eligible).
    #[serde(default, skip_serializing_if = "is_false")]
    pub priority_explicit: bool,
}

/// `skip_serializing_if` predicate: omit a `bool` field when it is `false` so
/// the serialized form stays additive (no key for the default). Kept as a named
/// free function because serde's `skip_serializing_if` requires a path to a
/// `fn(&T) -> bool`.
fn is_false(b: &bool) -> bool {
    !*b
}

impl ActiveGoal {
    /// Construct a new active goal in the `NotStarted` state with no assignee,
    /// targeting the daemon's own repo (`repo = None`).
    pub fn new(id: impl Into<String>, description: impl Into<String>, priority: u32) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            repo: None,
            current_activity: None,
            wip_refs: Vec::new(),
            last_progress_update_at: None,
            parent_goal_id: None,
            priority_explicit: false,
        }
    }

    /// Builder: set (or clear, with `None`) the target-repo slug.
    #[must_use]
    pub fn with_repo(mut self, repo: Option<String>) -> Self {
        self.repo = repo;
        self
    }

    /// Builder: set (or clear, with `None`) the decomposition parent linkage
    /// (issue #2405).
    #[must_use]
    pub fn with_parent(mut self, parent_goal_id: Option<String>) -> Self {
        self.parent_goal_id = parent_goal_id;
        self
    }

    /// Builder: mark (or clear) this goal's priority as operator-set provenance
    /// (issue #2695 follow-up). Only the operator `simard goal set-priority`
    /// path sets this `true`; the prioritization pass leaves such goals' exact
    /// priorities untouched.
    #[must_use]
    pub fn with_priority_explicit(mut self, explicit: bool) -> Self {
        self.priority_explicit = explicit;
        self
    }

    /// Short label for display.
    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.description)
    }

    /// True when this is a standing/perpetual goal — one with no terminal
    /// done-state (issue #2580). Backed by a durable description marker so the
    /// live research + CI-stewardship standing goals are recognised without a
    /// data migration. A standing goal must never be marked `Completed` by
    /// curation or the completion gate, and must never be tombstoned.
    pub fn is_perpetual(&self) -> bool {
        description_marks_standing(&self.description)
    }

    /// Builder: durably mark this goal as standing/perpetual by prepending the
    /// [`STANDING_MARKER_PREFIX`] to its description (idempotent — a goal that
    /// already reads as standing is returned unchanged). Set by
    /// `simard goal add --standing`.
    #[must_use]
    pub fn mark_standing(mut self) -> Self {
        if !self.is_perpetual() {
            self.description = format!("{STANDING_MARKER_PREFIX}{}", self.description);
        }
        self
    }

    /// Roll a standing/perpetual goal into a fresh cycle after its current unit
    /// of work finishes, instead of terminating it (issue #2580). Resets the
    /// goal to an actionable, re-dispatchable state: status back to
    /// `NotStarted`, assignment cleared, and stale work-in-progress refs
    /// dropped so the next OODA cycle re-enters the spawn path. The
    /// standing-goal description marker (and thus [`is_perpetual`]) is
    /// preserved.
    ///
    /// [`is_perpetual`]: ActiveGoal::is_perpetual
    pub fn roll_to_new_cycle(&mut self) {
        self.status = GoalProgress::NotStarted;
        self.assigned_to = None;
        self.wip_refs.clear();
        self.current_activity =
            Some("standing goal — finished a unit of work; rolled to a fresh cycle".to_string());
    }
}

/// A backlog item scored for future promotion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacklogItem {
    pub id: String,
    pub description: String,
    pub source: String,
    pub score: f64,
}

/// The goal board: active goals + scored backlog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalBoard {
    pub active: Vec<ActiveGoal>,
    pub backlog: Vec<BacklogItem>,
}

impl GoalBoard {
    /// Create an empty goal board.
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            backlog: Vec::new(),
        }
    }

    /// How many active goal slots remain.
    pub fn active_slots_remaining(&self) -> usize {
        MAX_ACTIVE_GOALS.saturating_sub(self.active.len())
    }

    /// Render a durable summary of the board state.
    pub fn durable_summary(&self) -> String {
        let active_labels: Vec<String> = self.active.iter().map(|g| g.concise_label()).collect();
        let active_text = if active_labels.is_empty() {
            "none".to_string()
        } else {
            active_labels.join("; ")
        };
        let backlog_text = if self.backlog.is_empty() {
            "none".to_string()
        } else {
            format!("{} items", self.backlog.len())
        };
        format!("active=[{active_text}]; backlog={backlog_text}")
    }
}

impl Default for GoalBoard {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Goal carryover record (issue #2092)
// ---------------------------------------------------------------------------

/// Well-known cognitive-memory concept key for carryover records.
pub const CARRYOVER_CONCEPT: &str = "goal-board:carryover";

/// Records that a meeting wrote goal updates to the board and expects an
/// engineer session to consume them. Written by the meeting close pipeline,
/// verified by the engineer loop on startup.
///
/// Without this record the handoff is implicit — if the state root diverges
/// between meeting and engineer, goals silently vanish. With it, the
/// engineer loop can detect a stale or missing carryover and surface a
/// clear error instead of silent data loss (spec line 665).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalCarryoverRecord {
    /// Stable meeting id that produced the goal updates (e.g. from
    /// `derive_meeting_id`).
    pub meeting_id: String,
    /// RFC 3339 timestamp of when the carryover was written.
    pub handed_off_at: DateTime<Utc>,
    /// SHA-256 hex digest of the serialized `GoalBoard` at the moment it
    /// was persisted by the meeting close pipeline. The engineer loop
    /// re-hashes its loaded board and compares; a mismatch means the board
    /// drifted since the meeting wrote it.
    pub board_snapshot_hash: String,
    /// Number of active goals on the board at handoff time.
    pub active_goal_count: usize,
    /// Ids of active goals at handoff time — lets the engineer loop
    /// enumerate exactly which goals it should have received.
    pub active_goal_ids: Vec<String>,
    /// Whether the engineer loop has acknowledged this carryover.
    #[serde(default)]
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// Goal graph: nodes and typed edges (issue #2405)
// ---------------------------------------------------------------------------

/// A goal projected into the cognitive-memory graph as a stable node keyed by
/// the goal `id` (issue #2405).
///
/// It is the durable **anchor** that decomposition edges point at, so edges
/// keep valid endpoints even after a goal leaves the active board (for example
/// a parent demoted to the backlog once its children are on the board). The
/// node carries the goal id, description, and an optional `done_criterion`.
/// See `docs/reference/goal-decomposition.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalNode {
    pub id: String,
    pub description: String,
    /// The explicit, independently-verifiable done-criterion for this goal.
    /// `None` for nodes (e.g. a parent umbrella) whose completion is a roll-up
    /// of its children rather than a single criterion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_criterion: Option<String>,
}

impl GoalNode {
    /// Construct a goal node. `done_criterion` is `Option<impl Into<String>>`
    /// so call sites can pass `Some("…")`, `Some(string)`, or `None`.
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        done_criterion: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            done_criterion: done_criterion.map(Into::into),
        }
    }
}

/// The two typed edges that connect goals in the decomposition graph
/// (issue #2405).
///
/// The string form ([`as_str`](Self::as_str)) — and the serde representation —
/// are a **durable, cross-system contract**: they appear verbatim in the
/// `goal-edge:{type}` concept key, the `goal-edge:{type}:{from}->{to}` caller
/// key, and the edge tags, so they must never drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEdgeType {
    /// parent → child: the parent goal decomposes into this child. Its inverse
    /// is read as `parent_of`.
    DecomposesInto,
    /// child → child: this sub-goal is gated on a sibling completing first
    /// (ordering / dependency). Optional.
    DependsOn,
}

impl GoalEdgeType {
    /// The stable snake_case token used in concept keys, caller keys, and tags.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DecomposesInto => "decomposes_into",
            Self::DependsOn => "depends_on",
        }
    }
}

/// A typed relationship edge between two goals.
///
/// Edges are persisted as **typed relationship facts** through
/// [`CognitiveMemoryOps`](crate::cognitive_memory::CognitiveMemoryOps) (design
/// choice (b) from issue #2405): one fact per edge under a stable caller key so
/// re-writing the same edge dedups instead of accumulating, and querying back
/// is the ordinary `search_facts` path. The graph-write/read helpers live in
/// [`super::edges`]; this type owns the durable concept/caller-key/tag/content
/// **format**.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEdge {
    pub from: String,
    pub to: String,
    pub edge_type: GoalEdgeType,
}

impl GoalEdge {
    /// Construct an edge `from -> to` of `edge_type`.
    pub fn new(from: impl Into<String>, to: impl Into<String>, edge_type: GoalEdgeType) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            edge_type,
        }
    }

    /// Concept key the edge fact is filed under: `goal-edge:{type}`.
    pub fn concept(&self) -> String {
        format!("goal-edge:{}", self.edge_type.as_str())
    }

    /// Stable caller key so re-writing the same edge dedups:
    /// `goal-edge:{type}:{from}->{to}`.
    pub fn caller_key(&self) -> String {
        format!(
            "goal-edge:{}:{}->{}",
            self.edge_type.as_str(),
            self.from,
            self.to
        )
    }

    /// Discrete tags so keyword recall surfaces the edge by parent id, child
    /// id, or edge type: `["goal-edge", type, "from:X", "to:Y"]`.
    pub fn tags(&self) -> Vec<String> {
        vec![
            "goal-edge".to_string(),
            self.edge_type.as_str().to_string(),
            format!("from:{}", self.from),
            format!("to:{}", self.to),
        ]
    }

    /// Canonical compact JSON content:
    /// `{"from":..,"to":..,"edge_type":..}` (field order = declaration order).
    ///
    /// Serialization is infallible — every field is a `String` or a `Copy`
    /// enum — so a failure here is a real invariant violation that must surface
    /// loudly rather than be papered over with a hand-built (and unescaped)
    /// fallback string.
    pub fn content(&self) -> String {
        serde_json::to_string(self).expect("GoalEdge (String/enum fields) always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── GoalProgress Display ────────────────────────────────────────

    #[test]
    fn goal_progress_display_proposed() {
        assert_eq!(GoalProgress::Proposed.to_string(), "proposed");
    }

    #[test]
    fn goal_progress_display_not_started() {
        assert_eq!(GoalProgress::NotStarted.to_string(), "not-started");
    }

    #[test]
    fn goal_progress_display_in_progress() {
        let p = GoalProgress::InProgress { percent: 42 };
        assert_eq!(p.to_string(), "in-progress(42%)");
    }

    #[test]
    fn goal_progress_display_in_progress_zero() {
        let p = GoalProgress::InProgress { percent: 0 };
        assert_eq!(p.to_string(), "in-progress(0%)");
    }

    #[test]
    fn goal_progress_display_in_progress_hundred() {
        let p = GoalProgress::InProgress { percent: 100 };
        assert_eq!(p.to_string(), "in-progress(100%)");
    }

    #[test]
    fn goal_progress_display_blocked() {
        let p = GoalProgress::Blocked("waiting on review".to_string());
        assert_eq!(p.to_string(), "blocked: waiting on review");
    }

    #[test]
    fn goal_progress_display_paused() {
        assert_eq!(GoalProgress::Paused.to_string(), "paused");
    }

    #[test]
    fn goal_progress_display_completed() {
        assert_eq!(GoalProgress::Completed.to_string(), "completed");
    }

    // ── GoalProgress Serde ──────────────────────────────────────────

    #[test]
    fn goal_progress_serde_all_variants() {
        let variants = vec![
            GoalProgress::Proposed,
            GoalProgress::NotStarted,
            GoalProgress::InProgress { percent: 50 },
            GoalProgress::Blocked("reason".to_string()),
            GoalProgress::Paused,
            GoalProgress::Completed,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let v2: GoalProgress = serde_json::from_str(&json).unwrap();
            assert_eq!(v, v2);
        }
    }

    // ── ActiveGoal ──────────────────────────────────────────────────

    fn sample_goal() -> ActiveGoal {
        ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: "g-1".to_string(),
            description: "Ship MVP".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 75 },
            assigned_to: Some("team-a".to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    #[test]
    fn active_goal_serde_round_trip() {
        let g = sample_goal();
        let json = serde_json::to_string(&g).unwrap();
        let g2: ActiveGoal = serde_json::from_str(&json).unwrap();
        assert_eq!(g, g2);
    }

    #[test]
    fn active_goal_concise_label() {
        let g = sample_goal();
        let label = g.concise_label();
        assert!(label.contains("p1"));
        assert!(label.contains("in-progress(75%)"));
        assert!(label.contains("Ship MVP"));
    }

    #[test]
    fn active_goal_assigned_to_none() {
        let g = ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: "g-2".to_string(),
            description: "Unassigned".to_string(),
            priority: 3,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        };
        let json = serde_json::to_string(&g).unwrap();
        let g2: ActiveGoal = serde_json::from_str(&json).unwrap();
        assert_eq!(g2.assigned_to, None);
    }

    // ── Standing / perpetual goals (issue #2580) ────────────────────

    #[test]
    fn description_marks_standing_recognizes_live_markers() {
        // The exact phrasings on the live board must be recognised so they are
        // reconciled without a data migration or touching ~/.simard.
        assert!(description_marks_standing("STANDING PERPETUAL goal"));
        assert!(description_marks_standing("Standing goal"));
        assert!(description_marks_standing(
            "Continuously research and improve your own cognition. STANDING PERPETUAL goal."
        ));
        assert!(description_marks_standing(&format!(
            "{STANDING_MARKER_PREFIX}watch CI health"
        )));
    }

    #[test]
    fn description_marks_standing_rejects_false_positives() {
        // Ordinary words that merely contain "standing" must not match.
        assert!(!description_marks_standing(
            "Improve understanding of goals"
        ));
        assert!(!description_marks_standing(
            "Fix an outstanding goal-board bug"
        ));
        assert!(!description_marks_standing("Ship the MVP"));
        assert!(!description_marks_standing(""));
    }

    #[test]
    fn is_perpetual_tracks_description() {
        let normal = ActiveGoal::new("g", "Ship the MVP", 1);
        assert!(!normal.is_perpetual());
        let standing = ActiveGoal::new("g", "Steward CI health. Standing goal.", 1);
        assert!(standing.is_perpetual());
    }

    #[test]
    fn mark_standing_makes_goal_perpetual_and_is_idempotent() {
        let g = ActiveGoal::new("g", "watch CI", 1).mark_standing();
        assert!(g.is_perpetual());
        assert!(g.description.starts_with(STANDING_MARKER_PREFIX));
        // Idempotent: a goal already read as standing is not double-marked.
        let again = g.clone().mark_standing();
        assert_eq!(again.description, g.description);
    }

    #[test]
    fn roll_to_new_cycle_resets_to_actionable_and_stays_perpetual() {
        let mut g = sample_goal();
        g.description = "Research cognition. STANDING PERPETUAL goal.".to_string();
        g.status = GoalProgress::Completed;
        g.assigned_to = Some("engineer-x".to_string());
        g.wip_refs = vec![WipRef {
            kind: "pr".to_string(),
            ref_id: "1".to_string(),
            label: "old".to_string(),
            url: None,
        }];
        assert!(g.is_perpetual());

        g.roll_to_new_cycle();
        assert_eq!(g.status, GoalProgress::NotStarted);
        assert_eq!(g.assigned_to, None);
        assert!(g.wip_refs.is_empty());
        assert!(
            g.is_perpetual(),
            "must remain a standing goal after rolling to a new cycle"
        );
    }

    // ── BacklogItem ─────────────────────────────────────────────────

    #[test]
    fn backlog_item_serde_round_trip() {
        let b = BacklogItem {
            id: "b-1".to_string(),
            description: "Refactor auth".to_string(),
            source: "review".to_string(),
            score: 0.85,
        };
        let json = serde_json::to_string(&b).unwrap();
        let b2: BacklogItem = serde_json::from_str(&json).unwrap();
        assert_eq!(b, b2);
    }

    #[test]
    fn backlog_item_zero_score() {
        let b = BacklogItem {
            id: "b-0".to_string(),
            description: "Low priority".to_string(),
            source: "auto".to_string(),
            score: 0.0,
        };
        assert_eq!(b.score, 0.0);
    }

    // ── GoalBoard ───────────────────────────────────────────────────

    #[test]
    fn goal_board_new_is_empty() {
        let board = GoalBoard::new();
        assert!(board.active.is_empty());
        assert!(board.backlog.is_empty());
    }

    #[test]
    fn goal_board_default_equals_new() {
        assert_eq!(GoalBoard::default(), GoalBoard::new());
    }

    #[test]
    fn goal_board_active_slots_remaining_empty() {
        let board = GoalBoard::new();
        assert_eq!(board.active_slots_remaining(), MAX_ACTIVE_GOALS);
    }

    #[test]
    fn goal_board_active_slots_remaining_partial() {
        let board = GoalBoard {
            active: vec![sample_goal(), sample_goal()],
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), MAX_ACTIVE_GOALS - 2);
    }

    #[test]
    fn goal_board_active_slots_remaining_full() {
        let goals: Vec<ActiveGoal> = (0..MAX_ACTIVE_GOALS)
            .map(|i| ActiveGoal {
                parent_goal_id: None,
                priority_explicit: false,
                repo: None,
                id: format!("g-{i}"),
                description: format!("Goal {i}"),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
            })
            .collect();
        let board = GoalBoard {
            active: goals,
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), 0);
    }

    #[test]
    fn goal_board_active_slots_remaining_overflow_saturates() {
        let goals: Vec<ActiveGoal> = (0..MAX_ACTIVE_GOALS + 2)
            .map(|i| ActiveGoal {
                parent_goal_id: None,
                priority_explicit: false,
                repo: None,
                id: format!("g-{i}"),
                description: format!("Goal {i}"),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
            })
            .collect();
        let board = GoalBoard {
            active: goals,
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), 0);
    }

    #[test]
    fn goal_board_serde_round_trip() {
        let board = GoalBoard {
            active: vec![sample_goal()],
            backlog: vec![BacklogItem {
                id: "b-1".to_string(),
                description: "Later".to_string(),
                source: "auto".to_string(),
                score: 0.5,
            }],
        };
        let json = serde_json::to_string(&board).unwrap();
        let b2: GoalBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(board, b2);
    }

    #[test]
    fn durable_summary_empty_board() {
        let board = GoalBoard::new();
        let s = board.durable_summary();
        assert!(s.contains("active=[none]"));
        assert!(s.contains("backlog=none"));
    }

    #[test]
    fn durable_summary_with_goals_and_backlog() {
        let board = GoalBoard {
            active: vec![sample_goal()],
            backlog: vec![
                BacklogItem {
                    id: "b-1".to_string(),
                    description: "X".to_string(),
                    source: "s".to_string(),
                    score: 0.1,
                },
                BacklogItem {
                    id: "b-2".to_string(),
                    description: "Y".to_string(),
                    source: "s".to_string(),
                    score: 0.2,
                },
            ],
        };
        let s = board.durable_summary();
        assert!(s.contains("Ship MVP"));
        assert!(s.contains("2 items"));
    }

    #[test]
    fn max_active_goals_constant() {
        // Raised from 7 to 20 so umbrella decompositions can fan out fully
        // (a 15-repo supply-chain umbrella spawns many per-repo goals). Actual
        // concurrent execution stays bounded by the separate AIMD engineer
        // concurrency cap (`ooda_loop::OodaConfig::max_concurrent_actions`);
        // this constant only governs how many distinct goals may exist.
        assert_eq!(MAX_ACTIVE_GOALS, 20);
    }
}
