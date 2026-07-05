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

/// Prospective node-type sentinel written to `trigger_condition`. This is the
/// retrieval key for every stored creative-idea row, so it is a **stable
/// identifier** — renaming it is a breaking migration, not an edit.
pub const CREATIVE_IDEA_TRIGGER: &str = "creative-idea";

/// On-disk payload schema version. A row whose `payload_version` is **newer**
/// than the reader understands is a hard [`SimardError::InvalidCreativeIdeaRecord`]
/// (fail-closed), never a silent default. Starts at `1`; a future native-links
/// migration bumps it to `2`.
pub const CREATIVE_IDEA_PAYLOAD_VERSION: u16 = 1;

/// The lifecycle status of a creative idea.
///
/// `status` changes only through [`CreativeIdea::try_transition`]; see
/// [`IdeaStatus::can_transition_to`] for the allowed edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IdeaStatus {
    /// Freshly generated, not yet reviewed.
    New,
    /// Synthesis asked for a rewrite before acceptance.
    NeedsRevision,
    /// High-risk / flagged: a human must decide.
    NeedsHumanReview,
    /// Reviewed and accepted; may be promoted to a goal.
    AcceptedForImplementation,
    /// Terminal: rejected.
    Rejected,
    /// Parked; may be reconsidered later.
    Deferred,
    /// A goal/PR is in flight.
    ImplementationStarted,
    /// Terminal: completed — reachable ONLY when the success metric is met.
    ImplementationCompleted,
}

impl IdeaStatus {
    /// Stable string form (matches the serde variant names) used both in the
    /// mirrored prospective `status` field and in the JSON payload.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::NeedsRevision => "NeedsRevision",
            Self::NeedsHumanReview => "NeedsHumanReview",
            Self::AcceptedForImplementation => "AcceptedForImplementation",
            Self::Rejected => "Rejected",
            Self::Deferred => "Deferred",
            Self::ImplementationStarted => "ImplementationStarted",
            Self::ImplementationCompleted => "ImplementationCompleted",
        }
    }

    /// Terminal states have no outgoing transitions.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Rejected | Self::ImplementationCompleted)
    }

    /// Whether `self -> to` is an allowed edge of the state machine.
    ///
    /// The full table (anything not listed is rejected):
    ///
    /// | From | To |
    /// |------|----|
    /// | `New` | `AcceptedForImplementation`, `Rejected`, `Deferred`, `NeedsRevision`, `NeedsHumanReview` |
    /// | `NeedsRevision` | `New`, `Rejected`, `Deferred` |
    /// | `NeedsHumanReview` | `AcceptedForImplementation`, `Rejected`, `Deferred` |
    /// | `Deferred` | `New`, `Rejected` |
    /// | `AcceptedForImplementation` | `ImplementationStarted`, `Deferred`, `Rejected` |
    /// | `ImplementationStarted` | `ImplementationCompleted`, `NeedsRevision`, `Rejected` |
    /// | `Rejected` / `ImplementationCompleted` | *(terminal)* |
    #[must_use]
    pub fn can_transition_to(&self, to: Self) -> bool {
        use IdeaStatus::{
            AcceptedForImplementation, Deferred, ImplementationCompleted, ImplementationStarted,
            NeedsHumanReview, NeedsRevision, New, Rejected,
        };
        matches!(
            (self, to),
            (New, AcceptedForImplementation)
                | (New, Rejected)
                | (New, Deferred)
                | (New, NeedsRevision)
                | (New, NeedsHumanReview)
                | (NeedsRevision, New)
                | (NeedsRevision, Rejected)
                | (NeedsRevision, Deferred)
                | (NeedsHumanReview, AcceptedForImplementation)
                | (NeedsHumanReview, Rejected)
                | (NeedsHumanReview, Deferred)
                | (Deferred, New)
                | (Deferred, Rejected)
                | (AcceptedForImplementation, ImplementationStarted)
                | (AcceptedForImplementation, Deferred)
                | (AcceptedForImplementation, Rejected)
                | (ImplementationStarted, ImplementationCompleted)
                | (ImplementationStarted, NeedsRevision)
                | (ImplementationStarted, Rejected)
        )
    }
}

impl std::str::FromStr for IdeaStatus {
    type Err = SimardError;

    /// Parse a status string. **Fail-closed**: an unknown value yields
    /// [`SimardError::InvalidCreativeIdeaRecord`] rather than a silent default.
    fn from_str(s: &str) -> SimardResult<Self> {
        match s {
            "New" => Ok(Self::New),
            "NeedsRevision" => Ok(Self::NeedsRevision),
            "NeedsHumanReview" => Ok(Self::NeedsHumanReview),
            "AcceptedForImplementation" => Ok(Self::AcceptedForImplementation),
            "Rejected" => Ok(Self::Rejected),
            "Deferred" => Ok(Self::Deferred),
            "ImplementationStarted" => Ok(Self::ImplementationStarted),
            "ImplementationCompleted" => Ok(Self::ImplementationCompleted),
            other => Err(SimardError::InvalidCreativeIdeaRecord {
                field: "status".to_string(),
                reason: format!("unknown idea status '{other}'"),
            }),
        }
    }
}

impl std::fmt::Display for IdeaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of memory node a [`MemoryLink`] points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MemoryLinkKind {
    /// A distilled semantic fact.
    Semantic,
    /// An autobiographical episode.
    Episodic,
    /// A reusable procedure.
    Procedural,
}

/// A typed edge from a [`CreativeIdea`] to another memory node that
/// supports/resources it.
///
/// FUTURE: promote links to native prospective edges (payload_version 2).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryLink {
    pub kind: MemoryLinkKind,
    pub node_id: String,
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
    /// Prospective `node_id` (`""` until stored).
    pub node_id: String,
    /// The idea text (-> prospective `description`).
    pub idea: String,
    /// Lifecycle status; mutate only via [`CreativeIdea::try_transition`].
    pub status: IdeaStatus,
    /// Provenance/situational context (-> payload).
    pub context: IdeaContext,
    /// Supporting semantic/episodic/procedural nodes (-> payload).
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
    /// Build a fresh `New` idea with no reviews/metric yet.
    #[must_use]
    pub fn new(idea: impl Into<String>, context: IdeaContext, created_epoch: u64) -> Self {
        Self {
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
        Ok(Self {
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
pub trait CreativeIdeaStore {
    /// Persist a new idea; returns its prospective `node_id`.
    fn store(&self, idea: &CreativeIdea) -> SimardResult<String>;
    /// Re-serialize an existing idea's payload/status.
    fn update(&self, idea: &CreativeIdea) -> SimardResult<()>;
    /// List up to `limit` stored creative ideas (filtered by the sentinel).
    fn list(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>>;
    /// Fetch one idea by `node_id`.
    fn get(&self, node_id: &str) -> SimardResult<Option<CreativeIdea>>;
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
}

impl CreativeIdeaStore for ProspectiveCreativeIdeaStore<'_> {
    fn store(&self, idea: &CreativeIdea) -> SimardResult<String> {
        let action = idea.to_action_payload()?;
        self.mem
            .store_prospective(&idea.idea, CREATIVE_IDEA_TRIGGER, &action, idea.priority())
    }

    fn update(&self, idea: &CreativeIdea) -> SimardResult<()> {
        // FUTURE (M2): a real upsert-by-node_id in the memory layer. During the
        // spike `store_prospective` is the only write seam, so `update`
        // re-persists the payload; callers treat ideas as append-only for now.
        let action = idea.to_action_payload()?;
        self.mem
            .store_prospective(&idea.idea, CREATIVE_IDEA_TRIGGER, &action, idea.priority())?;
        Ok(())
    }

    fn list(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>> {
        let nodes = self.mem.list_all_prospective(limit)?;
        nodes
            .iter()
            .filter(|n| n.trigger_condition == CREATIVE_IDEA_TRIGGER)
            .map(CreativeIdea::from_prospective)
            .collect()
    }

    fn get(&self, node_id: &str) -> SimardResult<Option<CreativeIdea>> {
        Ok(self
            .list(u32::MAX)?
            .into_iter()
            .find(|idea| idea.node_id == node_id))
    }
}
