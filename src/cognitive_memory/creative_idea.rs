//! `CreativeIdea` — the prospective-memory representation of one candidate
//! self-improvement idea (design spike #2419).
//!
//! A [`CreativeIdea`] is a typed struct that **round-trips to/from** a single
//! [`CognitiveProspective`] node (Decision 1 of the design doc) with **no schema
//! change** to prospective memory:
//!
//! | `CognitiveProspective` field | `CreativeIdea` mapping |
//! |------------------------------|------------------------|
//! | `description` | [`CreativeIdea::idea`] (the idea text) |
//! | `trigger_condition` | the sentinel [`CREATIVE_IDEA_TRIGGER`] (retrieval key) |
//! | `action_on_trigger` | JSON payload (versioned) with status/context/links/reviews/metric |
//! | `priority` | derived from portfolio/risk |
//!
//! The subsystem is a **spike**: this file is real, tested typed foundation but
//! nothing here is wired into the daemon and the generator is gated OFF (see
//! [`crate::creative_ideas`]). `status` changes **only** through
//! [`CreativeIdea::try_transition`], which validates every edge.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::creative_ideas::reviewers::{Review, ReviewFlags, ReviewVerdict, reviewer_id_from_str};
use crate::creative_ideas::synthesis::SuccessMetric;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::CognitiveProspective;

// The creative-idea *memory-model* types — the lifecycle state machine and the
// typed memory-link taxonomy — are owned upstream by `amplihack-memory-lib`
// (engineering guideline G2: memory-architecture work belongs in the library,
// not forked into Simard). Simard re-exports them and orchestrates around them:
// it persists the idea payload through the existing prospective primitive, runs
// the reviewer pipeline, and routes accepted ideas — all Simard-domain concerns.
pub use amplihack_memory::creative_idea::{
    CREATIVE_IDEA_PAYLOAD_VERSION, CREATIVE_IDEA_TRIGGER, CreativeIdeaStatus as IdeaStatus,
    MemoryLink, MemoryLinkKind,
};

/// Parse a persisted idea-status string into an [`IdeaStatus`], mapping the
/// library's fail-closed parse error into [`SimardError::InvalidCreativeIdeaRecord`].
///
/// **Fail-closed**: an unknown value is a hard error, never a silent default.
pub fn parse_idea_status(s: &str) -> SimardResult<IdeaStatus> {
    s.parse::<IdeaStatus>()
        .map_err(|e| SimardError::InvalidCreativeIdeaRecord {
            field: "status".to_string(),
            reason: e.to_string(),
        })
}

/// Provenance + situational context captured at generation time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IdeaContext {
    /// e.g. `"creative-ideas-thread"`.
    pub source: String,
    /// The active/proposed goals when the idea was generated.
    pub goals_snapshot: Vec<String>,
    /// Hash/summary of the >=24h observation window used.
    pub observation_digest: String,
    /// Free-text rationale for why this idea surfaced.
    pub rationale: String,
}

/// A candidate self-improvement idea, stored as a prospective-memory node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreativeIdea {
    /// Stable identity that survives status updates. Prospective memory is
    /// append-only (no in-place UPDATE), so an update appends a new node with
    /// the **same** `idea_id`; [`CreativeIdeaStore::list`] then keeps the latest
    /// node per `idea_id`. Mirrors the goal-store append+dedupe pattern.
    pub idea_id: String,
    /// Monotonic revision, bumped by [`CreativeIdeaStore::update`]. Makes
    /// "keep the latest revision" robust independent of node-id ordering.
    pub revision: u64,
    /// Prospective `node_id` of the most-recent revision (`""` until stored).
    pub node_id: String,
    /// The idea text (-> prospective `description`).
    pub idea: String,
    /// Lifecycle status; mutate only via [`CreativeIdea::try_transition`].
    pub status: IdeaStatus,
    /// Provenance/situational context (-> payload).
    pub context: IdeaContext,
    /// Supporting semantic/episodic/procedural/goal nodes (-> payload).
    pub links: Vec<MemoryLink>,
    /// Accumulated reviewer output (-> payload).
    pub reviews: Vec<Review>,
    /// Set by the measurability reviewer; the ONLY thing that can later move the
    /// idea to `ImplementationCompleted` (-> payload).
    pub success_metric: Option<SuccessMetric>,
    /// Injected unix-epoch (seconds) at generation time (-> payload).
    pub created_epoch: u64,
}

impl CreativeIdea {
    /// Build a fresh `New` idea with no reviews/metric yet and a freshly-minted
    /// stable [`Self::idea_id`].
    #[must_use]
    pub fn new(idea: impl Into<String>, context: IdeaContext, created_epoch: u64) -> Self {
        Self {
            idea_id: uuid::Uuid::new_v4().to_string(),
            revision: 0,
            node_id: String::new(),
            idea: idea.into(),
            status: IdeaStatus::New,
            context,
            links: Vec::new(),
            reviews: Vec::new(),
            success_metric: None,
            created_epoch,
        }
    }

    /// Validate and apply a status transition. The **only** way `status`
    /// changes. Returns [`SimardError::InvalidIdeaTransition`] on any edge not
    /// in the allowed table.
    pub fn try_transition(&mut self, to: IdeaStatus) -> SimardResult<()> {
        if self.status.can_transition_to(to) {
            self.status = to;
            Ok(())
        } else {
            Err(SimardError::InvalidIdeaTransition {
                from: self.status,
                to,
            })
        }
    }

    /// Priority written to the prospective node (higher = more urgent to
    /// review). Derived from risk flags on the accumulated reviews.
    #[must_use]
    pub fn priority(&self) -> i64 {
        let flagged = self
            .reviews
            .iter()
            .any(|r| r.flags.high_risk || r.flags.irreversible || r.flags.needs_human);
        if flagged { 5 } else { 3 }
    }

    /// Serialize the versioned JSON payload written to `action_on_trigger`.
    pub fn to_action_payload(&self) -> SimardResult<String> {
        let payload = StoredPayload {
            payload_version: CREATIVE_IDEA_PAYLOAD_VERSION,
            idea_id: self.idea_id.clone(),
            revision: self.revision,
            status: self.status,
            context: self.context.clone(),
            links: self.links.clone(),
            reviews: self
                .reviews
                .iter()
                .map(PersistedReview::from_review)
                .collect(),
            success_metric: self.success_metric.clone(),
            created_epoch: self.created_epoch,
        };
        serde_json::to_string(&payload).map_err(|e| SimardError::InvalidCreativeIdeaRecord {
            field: "action_on_trigger".to_string(),
            reason: format!("failed to serialize payload: {e}"),
        })
    }

    /// Reconstruct a `CreativeIdea` from a stored [`CognitiveProspective`] node.
    ///
    /// Fail-closed: a wrong sentinel, unparseable payload, too-new
    /// `payload_version`, or unknown enum string is a hard
    /// [`SimardError::InvalidCreativeIdeaRecord`].
    pub fn from_prospective(node: &CognitiveProspective) -> SimardResult<Self> {
        if node.trigger_condition != CREATIVE_IDEA_TRIGGER {
            return Err(SimardError::InvalidCreativeIdeaRecord {
                field: "trigger_condition".to_string(),
                reason: format!(
                    "expected sentinel '{CREATIVE_IDEA_TRIGGER}', got '{}'",
                    node.trigger_condition
                ),
            });
        }
        let payload: StoredPayload =
            serde_json::from_str(&node.action_on_trigger).map_err(|e| {
                SimardError::InvalidCreativeIdeaRecord {
                    field: "action_on_trigger".to_string(),
                    reason: format!("failed to parse payload: {e}"),
                }
            })?;
        if payload.payload_version > CREATIVE_IDEA_PAYLOAD_VERSION {
            return Err(SimardError::InvalidCreativeIdeaRecord {
                field: "payload_version".to_string(),
                reason: format!(
                    "on-disk row version {} is newer than reader version {CREATIVE_IDEA_PAYLOAD_VERSION}",
                    payload.payload_version
                ),
            });
        }
        let reviews = payload
            .reviews
            .into_iter()
            .map(PersistedReview::into_review)
            .collect::<SimardResult<Vec<_>>>()?;
        // Legacy rows written before `idea_id` existed fall back to the node_id
        // as a stable-enough identity (each such row is its own idea).
        let idea_id = if payload.idea_id.is_empty() {
            node.node_id.clone()
        } else {
            payload.idea_id
        };
        Ok(Self {
            idea_id,
            revision: payload.revision,
            node_id: node.node_id.clone(),
            idea: node.description.clone(),
            status: payload.status,
            context: payload.context,
            links: payload.links,
            reviews,
            success_metric: payload.success_metric,
            created_epoch: payload.created_epoch,
        })
    }
}

/// The versioned JSON payload persisted in `action_on_trigger`.
#[derive(Serialize, Deserialize)]
struct StoredPayload {
    payload_version: u16,
    #[serde(default)]
    idea_id: String,
    #[serde(default)]
    revision: u64,
    status: IdeaStatus,
    context: IdeaContext,
    links: Vec<MemoryLink>,
    reviews: Vec<PersistedReview>,
    success_metric: Option<SuccessMetric>,
    created_epoch: u64,
}

/// On-disk form of a [`Review`]. [`Review::reviewer`] is a `&'static str`
/// (a stable telemetry id) which cannot be `Deserialize`d directly, so the
/// payload stores the id as a `String` and [`Self::into_review`] maps it back
/// to a known static id — **fail-closed** on an unknown reviewer.
#[derive(Serialize, Deserialize)]
struct PersistedReview {
    reviewer: String,
    verdict: ReviewVerdict,
    notes: String,
    flags: ReviewFlags,
    proposed_metric: Option<SuccessMetric>,
}

impl PersistedReview {
    fn from_review(review: &Review) -> Self {
        Self {
            reviewer: review.reviewer.to_string(),
            verdict: review.verdict,
            notes: review.notes.clone(),
            flags: review.flags,
            proposed_metric: review.proposed_metric.clone(),
        }
    }

    fn into_review(self) -> SimardResult<Review> {
        let reviewer = reviewer_id_from_str(&self.reviewer).ok_or_else(|| {
            SimardError::InvalidCreativeIdeaRecord {
                field: "reviews.reviewer".to_string(),
                reason: format!("unknown reviewer id '{}'", self.reviewer),
            }
        })?;
        Ok(Review {
            reviewer,
            verdict: self.verdict,
            notes: self.notes,
            flags: self.flags,
            proposed_metric: self.proposed_metric,
        })
    }
}

/// A thin persistence seam for creative ideas over prospective memory.
///
/// The production adapter is [`ProspectiveCreativeIdeaStore`]; tests use an
/// in-memory fake. No new storage backend is introduced.
///
/// Prospective memory is append-only, so [`Self::update`] appends a new node
/// carrying the same [`CreativeIdea::idea_id`]; [`Self::list`] then collapses
/// each `idea_id` to its most-recent node so an idea appears **once** at its
/// current status.
pub trait CreativeIdeaStore {
    /// Persist a new idea; returns its prospective `node_id`.
    fn store(&self, idea: &CreativeIdea) -> SimardResult<String>;
    /// Append an updated revision of an existing idea (same `idea_id`).
    fn update(&self, idea: &CreativeIdea) -> SimardResult<()>;
    /// List up to `limit` current creative ideas (latest revision per `idea_id`).
    fn list(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>>;
    /// Fetch one idea by its current `node_id`.
    fn get(&self, node_id: &str) -> SimardResult<Option<CreativeIdea>>;
    /// List current ideas whose status equals `status` (enumerable-by-status).
    fn list_by_status(&self, status: IdeaStatus, limit: u32) -> SimardResult<Vec<CreativeIdea>> {
        Ok(self
            .list(limit)?
            .into_iter()
            .filter(|idea| idea.status == status)
            .collect())
    }
}

/// Collapse an append-only stream of creative-idea revisions to the current
/// state: for each `idea_id`, keep the row with the greatest `(revision,
/// node_id)`. The monotonic `revision` (bumped by [`CreativeIdeaStore::update`])
/// makes this robust independent of `node_id` ordering. Public so read-only
/// consumers (the dashboard, the TUI) collapse revisions identically.
#[must_use]
pub fn latest_revision_per_idea(mut ideas: Vec<CreativeIdea>) -> Vec<CreativeIdea> {
    use std::collections::HashMap;
    fn rank(i: &CreativeIdea) -> (u64, &str) {
        (i.revision, i.node_id.as_str())
    }
    let mut latest: HashMap<String, CreativeIdea> = HashMap::new();
    for idea in ideas.drain(..) {
        match latest.get(&idea.idea_id) {
            Some(existing) if rank(existing) >= rank(&idea) => {}
            _ => {
                latest.insert(idea.idea_id.clone(), idea);
            }
        }
    }
    let mut out: Vec<CreativeIdea> = latest.into_values().collect();
    // Deterministic order: highest revision (newest) first.
    out.sort_by(|a, b| rank(b).cmp(&rank(a)));
    out
}

/// Production [`CreativeIdeaStore`] over [`CognitiveMemoryOps`] — thin, no new
/// backend. `list` keeps only rows whose `trigger_condition` is the sentinel.
pub struct ProspectiveCreativeIdeaStore<'a> {
    mem: &'a dyn CognitiveMemoryOps,
}

impl<'a> ProspectiveCreativeIdeaStore<'a> {
    /// Wrap a live cognitive-memory handle.
    #[must_use]
    pub fn new(mem: &'a dyn CognitiveMemoryOps) -> Self {
        Self { mem }
    }

    /// Every stored revision (no dedupe) filtered by the sentinel — the raw
    /// append-only stream. Used internally by [`Self::list`] and by tests.
    fn all_revisions(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>> {
        let nodes = self.mem.list_all_prospective(limit)?;
        nodes
            .iter()
            .filter(|n| n.trigger_condition == CREATIVE_IDEA_TRIGGER)
            .map(CreativeIdea::from_prospective)
            .collect()
    }
}

impl CreativeIdeaStore for ProspectiveCreativeIdeaStore<'_> {
    fn store(&self, idea: &CreativeIdea) -> SimardResult<String> {
        let action = idea.to_action_payload()?;
        self.mem
            .store_prospective(&idea.idea, CREATIVE_IDEA_TRIGGER, &action, idea.priority())
    }

    fn update(&self, idea: &CreativeIdea) -> SimardResult<()> {
        // Append-only backend: re-persist under the same `idea_id` at the next
        // revision. Computing the revision from the current max makes "latest
        // wins" robust without any caller bookkeeping.
        let next_revision = self
            .all_revisions(u32::MAX)?
            .iter()
            .filter(|i| i.idea_id == idea.idea_id)
            .map(|i| i.revision)
            .max()
            .map_or(1, |m| m.saturating_add(1));
        let mut to_store = idea.clone();
        to_store.revision = next_revision;
        let action = to_store.to_action_payload()?;
        self.mem
            .store_prospective(&idea.idea, CREATIVE_IDEA_TRIGGER, &action, idea.priority())?;
        Ok(())
    }

    fn list(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>> {
        Ok(latest_revision_per_idea(self.all_revisions(limit)?))
    }

    fn get(&self, node_id: &str) -> SimardResult<Option<CreativeIdea>> {
        Ok(self
            .list(u32::MAX)?
            .into_iter()
            .find(|idea| idea.node_id == node_id))
    }
}
