//! Episode-ingestion classifier (issue #2327).
//!
//! The deterministic policy that runs before every `store_episode` intake
//! site. It is a **pure decision function** ([`classify`]) plus a thin IO
//! seam ([`store_episode_classified`]) that performs the store and bumps the
//! per-cycle intake counters.
//!
//! ## Why
//!
//! Simard's cognitive memory was filling with operational noise — session
//! start/complete/persist markers, "flushing working memory" bookkeeping, and
//! `continue_skipping` brain chatter — crowding out the meaningful episodics
//! (action failures, durable completions, handoffs, goal-board transitions,
//! user decisions) that distillation actually wants to promote into facts and
//! procedures. The classifier drops the noise, down-scopes low-value
//! bookkeeping, and stores the meaningful events at full importance with the
//! `{importance, event_kind, goal_id, cycle, is_operational}` metadata the
//! rest of the cognitive-memory stack reads.
//!
//! ## Decision rules (strict priority — first match wins)
//!
//! 1. **Failure override.** Content carrying a failure/error signal (`error`,
//!    `failed`, `failure`, `panic`, `exception`) is ALWAYS stored at full
//!    importance, even if it also matches a drop marker (A7). Recipe-context
//!    failures classify as [`EventKind::RecipeFailure`], everything else as
//!    [`EventKind::ActionFailure`].
//! 2. **Known-noise markers → Drop.** `started with objective`,
//!    `completed and persisted`, `flushing working memory`,
//!    `continue_skipping`, `no decision keyword`.
//! 3. **Meaningful content → Store.** handoffs, goal promotions/archival,
//!    user decisions, durable action completions.
//! 4. **Operational bookkeeping → DownScope.** Cross-session hydration
//!    summaries and any otherwise-unmatched content are persisted at low
//!    importance with `is_operational = true` (never dropped — only
//!    de-prioritised).

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

/// Coarse taxonomy of *why* an episode is being stored. Serializes as
/// `snake_case`, so the string lands in episode metadata exactly as the
/// taxonomy enumerates it (`action_failure`, `goal_promotion`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ActionFailure,
    ActionCompleted,
    Handoff,
    GoalArchival,
    GoalPromotion,
    UserDecision,
    RecipeFailure,
    Operational,
}

/// Structured metadata attached to every **stored** and **down-scoped**
/// episode. Serialized to JSON and passed as the `metadata` argument of
/// `store_episode`. `Drop` writes nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeMetadata {
    /// Importance in `0.0..=1.0`. Failures highest (0.9), durable completions
    /// 0.7, down-scoped operational 0.1.
    pub importance: f64,
    pub event_kind: EventKind,
    /// Threaded from the call context; serializes to JSON `null` when absent.
    pub goal_id: Option<String>,
    /// Threaded from the call context; serializes to JSON `null` when absent.
    pub cycle: Option<u32>,
    /// `true` only for down-scoped operational episodes.
    pub is_operational: bool,
}

impl EpisodeMetadata {
    /// Serialize to a JSON object with EXACTLY the five documented keys,
    /// ready to hand to `store_episode(.., Some(&json))`. `goal_id` / `cycle`
    /// render as JSON `null` when absent so the object shape is stable.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("EpisodeMetadata serializes to JSON")
    }
}

/// Caller-supplied context that the `content` / `source_label` strings alone
/// cannot convey. Both fields are optional; [`Default`] yields an all-`None`
/// context.
#[derive(Clone, Debug, Default)]
pub struct IntakeContext {
    pub goal_id: Option<String>,
    pub cycle: Option<u32>,
}

/// The result of classifying one episode at its intake site.
#[derive(Clone, Debug, PartialEq)]
pub enum IntakeDecision {
    /// Do not store; operational noise.
    Drop,
    /// Store, but flagged operational/low-importance.
    DownScope(EpisodeMetadata),
    /// Store with durable, full-importance metadata.
    Store(EpisodeMetadata),
}

impl IntakeDecision {
    /// `true` when the episode is dropped (never stored).
    pub fn is_dropped(&self) -> bool {
        matches!(self, IntakeDecision::Drop)
    }

    /// `true` when the episode is stored at full importance.
    pub fn is_store(&self) -> bool {
        matches!(self, IntakeDecision::Store(_))
    }

    /// `true` when the episode is stored down-scoped (operational).
    pub fn is_downscoped(&self) -> bool {
        matches!(self, IntakeDecision::DownScope(_))
    }

    /// The metadata that will be written, or `None` for a dropped episode.
    pub fn metadata(&self) -> Option<&EpisodeMetadata> {
        match self {
            IntakeDecision::Drop => None,
            IntakeDecision::DownScope(m) | IntakeDecision::Store(m) => Some(m),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Marker token sets
// ───────────────────────────────────────────────────────────────────────────

/// Case-insensitive failure/error signal tokens (A7 override).
const FAILURE_TOKENS: &[&str] = &["error", "failed", "failure", "panic", "exception"];

/// Case-insensitive operational-noise markers → drop.
const NOISE_MARKERS: &[&str] = &[
    "started with objective",
    "completed and persisted",
    "flushing working memory",
    "continue_skipping",
    "no decision keyword",
];

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// `true` when content/source carries a failure or error signal.
fn has_failure_signal(content_lc: &str) -> bool {
    contains_any(content_lc, FAILURE_TOKENS)
}

/// Build the durable metadata for a stored event.
fn store_meta(importance: f64, event_kind: EventKind, ctx: &IntakeContext) -> EpisodeMetadata {
    EpisodeMetadata {
        importance,
        event_kind,
        goal_id: ctx.goal_id.clone(),
        cycle: ctx.cycle,
        is_operational: false,
    }
}

/// Build the low-importance metadata for a down-scoped operational event.
fn downscope_meta(ctx: &IntakeContext) -> EpisodeMetadata {
    EpisodeMetadata {
        importance: 0.1,
        event_kind: EventKind::Operational,
        goal_id: ctx.goal_id.clone(),
        cycle: ctx.cycle,
        is_operational: true,
    }
}

/// Classify one episode into [`IntakeDecision::Drop`],
/// [`IntakeDecision::DownScope`], or [`IntakeDecision::Store`].
///
/// Pure, no IO — the fully unit-testable core. Evaluates the rules in strict
/// priority order (see the module docs) and returns on the first match.
pub fn classify(content: &str, source_label: &str, ctx: &IntakeContext) -> IntakeDecision {
    let content_lc = content.to_lowercase();
    let source_lc = source_label.to_lowercase();

    // Rule 1 — failure override (highest priority, beats drop markers).
    if has_failure_signal(&content_lc) {
        let kind = if content_lc.contains("recipe") || source_lc.contains("recipe") {
            EventKind::RecipeFailure
        } else {
            EventKind::ActionFailure
        };
        return IntakeDecision::Store(store_meta(0.9, kind, ctx));
    }

    // Rule 2 — known-noise markers → drop.
    if contains_any(&content_lc, NOISE_MARKERS) {
        return IntakeDecision::Drop;
    }

    // Rule 3 — meaningful content → store (content-driven so call sites do
    // not have to pre-tag every event).
    //
    // User decision.
    if content_lc.contains("user decided")
        || content_lc.contains("user decision")
        || source_lc.contains("user-decision")
    {
        return IntakeDecision::Store(store_meta(0.85, EventKind::UserDecision, ctx));
    }
    // Goal-board promotion / archival.
    if content_lc.contains("promoted goal") || content_lc.contains("from backlog to active") {
        return IntakeDecision::Store(store_meta(0.8, EventKind::GoalPromotion, ctx));
    }
    if content_lc.contains("archived goal") || content_lc.contains("goal archival") {
        return IntakeDecision::Store(store_meta(0.8, EventKind::GoalArchival, ctx));
    }
    // Handoff between sessions / worktrees.
    if content_lc.contains("handoff") || source_lc.contains("handoff") {
        return IntakeDecision::Store(store_meta(0.8, EventKind::Handoff, ctx));
    }
    // Durable action completion (opened/merged PR, etc.).
    if content_lc.contains("opened pr")
        || content_lc.contains("pull request")
        || content_lc.contains("merged")
    {
        return IntakeDecision::Store(store_meta(0.7, EventKind::ActionCompleted, ctx));
    }
    // Goal-curator board summaries are durable goal-state transitions worth
    // keeping even when the free text does not name the move explicitly.
    if source_lc == "goal-curator" {
        return IntakeDecision::Store(store_meta(0.8, EventKind::GoalArchival, ctx));
    }

    // Rule 4 — operational bookkeeping / unmatched → down-scope (never drop).
    IntakeDecision::DownScope(downscope_meta(ctx))
}

/// Strip noise lines from a concatenated reflection transcript so the
/// transcript episode can still be stored (its id is needed for fact
/// provenance) without carrying `continue_skipping` chatter.
///
/// - Splits on `\n`; drops lines containing `continue_skipping` or
///   `no decision keyword`.
/// - If the **original** transcript carries a failure signal, returns the
///   original text **unchanged** — a transcript that records a failure is
///   never stripped.
/// - Otherwise returns the joined survivors.
/// - Returns `None` when the survivor set is empty or whitespace-only (pure
///   noise, no failure) — the caller then drops the episode unless it still
///   needs the id for provenance.
pub fn sanitize_transcript(transcript: &str) -> Option<String> {
    if has_failure_signal(&transcript.to_lowercase()) {
        return Some(transcript.to_string());
    }
    let kept: Vec<&str> = transcript
        .lines()
        .filter(|line| {
            let lc = line.to_lowercase();
            !lc.contains("continue_skipping") && !lc.contains("no decision keyword")
        })
        .collect();
    let joined = kept.join("\n");
    if joined.trim().is_empty() {
        None
    } else {
        Some(joined)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// IO seam + per-cycle metrics
// ───────────────────────────────────────────────────────────────────────────

/// Per-cycle intake counters (dropped / stored / down-scoped). A single
/// process-global instance ([`global_intake_counters`]) aggregates every
/// intake site within a cycle; [`IntakeCounters::log_summary`] emits one line
/// and resets at cycle end.
#[derive(Default)]
pub struct IntakeCounters {
    dropped: AtomicU32,
    stored: AtomicU32,
    downscoped: AtomicU32,
}

impl IntakeCounters {
    /// Record a classification decision into the per-cycle counters.
    pub fn record(&self, decision: &IntakeDecision) {
        match decision {
            IntakeDecision::Drop => &self.dropped,
            IntakeDecision::Store(_) => &self.stored,
            IntakeDecision::DownScope(_) => &self.downscoped,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    /// `(dropped, stored, downscoped)` snapshot.
    pub fn snapshot(&self) -> (u32, u32, u32) {
        (
            self.dropped.load(Ordering::Relaxed),
            self.stored.load(Ordering::Relaxed),
            self.downscoped.load(Ordering::Relaxed),
        )
    }

    fn reset(&self) {
        self.dropped.store(0, Ordering::Relaxed);
        self.stored.store(0, Ordering::Relaxed);
        self.downscoped.store(0, Ordering::Relaxed);
    }

    /// Emit one aggregated intake-hygiene line per cycle and reset the
    /// counters. No-op (silent) when nothing was classified this cycle.
    pub fn log_summary(&self) {
        let (dropped, stored, downscoped) = self.snapshot();
        if dropped == 0 && stored == 0 && downscoped == 0 {
            return;
        }
        tracing::info!(
            target: "simard::intake",
            dropped,
            stored,
            downscoped,
            "episode-intake dropped={dropped} stored={stored} downscoped={downscoped}"
        );
        eprintln!(
            "[simard] episode-intake dropped={dropped} stored={stored} downscoped={downscoped}"
        );
        self.reset();
    }
}

/// The process-global intake counters shared by every `store_episode`
/// chokepoint within a cycle.
pub fn global_intake_counters() -> &'static IntakeCounters {
    static COUNTERS: std::sync::OnceLock<IntakeCounters> = std::sync::OnceLock::new();
    COUNTERS.get_or_init(IntakeCounters::default)
}

/// Classify-then-store IO seam every intake site calls instead of
/// `bridge.store_episode`.
///
/// 1. [`classify`] the episode.
/// 2. On [`IntakeDecision::Drop`]: bump the dropped counter, return `Ok(None)`
///    — the bridge is never touched.
/// 3. On `Store` / `DownScope`: write the metadata JSON via `store_episode`,
///    bump the matching counter, and return `Ok(Some(episode_id))`.
///
/// The returned id is the same id used downstream for
/// `store_fact_with_provenance` / `store_procedure_with_provenance`.
pub fn store_episode_classified(
    bridge: &dyn CognitiveMemoryOps,
    content: &str,
    source_label: &str,
    ctx: &IntakeContext,
) -> SimardResult<Option<String>> {
    let decision = classify(content, source_label, ctx);
    global_intake_counters().record(&decision);
    match decision.metadata() {
        None => Ok(None),
        Some(meta) => {
            let json = meta.to_json();
            let id = bridge.store_episode(content, source_label, Some(&json))?;
            Ok(Some(id))
        }
    }
}

/// Like [`store_episode_classified`] but ALWAYS stores the episode and returns
/// its id, because a downstream write links provenance to it (issue #2327, A2
/// — a reflection transcript that derives facts must keep an id even when the
/// classifier would otherwise drop it as noise). A `Drop` decision is
/// converted to a down-scoped store so the id always exists.
pub fn store_episode_for_provenance(
    bridge: &dyn CognitiveMemoryOps,
    content: &str,
    source_label: &str,
    ctx: &IntakeContext,
) -> SimardResult<String> {
    let decision = classify(content, source_label, ctx);
    let decision = match decision {
        IntakeDecision::Drop => IntakeDecision::DownScope(downscope_meta(ctx)),
        keep => keep,
    };
    global_intake_counters().record(&decision);
    let meta = decision
        .metadata()
        .expect("non-Drop decision always carries metadata");
    let json = meta.to_json();
    bridge.store_episode(content, source_label, Some(&json))
}
