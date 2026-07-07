//! Semantic dedup + enhance gate for Creative Ideas (issue #2925).
//!
//! # The problem
//!
//! The Creative Ideas board accumulated ~104 ideas with heavy **semantic**
//! duplication — the same handful of suggestions restated in different words. A
//! word-set similarity check (`dedup.rs`) cannot catch that: two ideas can share
//! almost no words and still be the same idea, and it can never *strengthen* an
//! existing idea with a candidate's new angle.
//!
//! # Where the intelligence lives
//!
//! The decision is a **structured-reasoning brain step**
//! ([`OodaBrain::decide_idea_dedup`]) driven by a hot-reloadable recipe
//! (`creative-idea-dedup.yaml`) — NOT Rust cosine/Jaccard/threshold code. This
//! module is the THIN, fail-closed Rust rail that mirrors the resource-admission
//! gate ([`crate::ooda_actions::advance_goal::resource_admission`]): assemble a
//! bounded shortlist → call the reasoner → apply the structured decision.
//!
//! The word-set Jaccard in [`crate::creative_ideas::dedup`] survives only as a
//! **coarse pre-filter** (Stage-1 shortlist ranking) and as the **fail-closed
//! backstop** when the semantic layer is switched off — never as the semantic
//! authority.
//!
//! # The rails (fail-CLOSED)
//!
//! | Rail | Guard | Result |
//! | --- | --- | --- |
//! | **Empty pool** | `pool` empty | [`PlannedAction::Create`]; brain not consulted. |
//! | **Kill-switch** | `enabled == false` | Deterministic Jaccard `Skip`-or-`Create`; brain not consulted, never `Enhance`. |
//! | **Empty shortlist** | nothing lexically near | `Create`; brain not consulted (v1 limitation of the lexical pre-filter). |
//! | **Brain error** | `decide_idea_dedup` returns `Err` | [`PlannedAction::FailClosed`] — the candidate is **dropped this cycle** (never a silent duplicate), the error is surfaced, and it is retried next run. |
//! | **Bad ENHANCE target** | `target_node_id` ∉ shortlist | `FailClosed` — never a wrong-node write, never a silent duplicate. |
//! | **Valid brain result** | otherwise | `CreateNew → Create`, `Skip → Skip`, `EnhanceExisting → Enhance`. |
//!
//! Observability (telemetry counts) is applied by the **caller** (the tick), not
//! inside the pure [`plan_candidate`], so the pure-rail tests write no files.
#![allow(dead_code)]

use crate::cognitive_memory::creative_idea::{CreativeIdea, CreativeIdeaStore, IdeaStatus};
use crate::cognitive_threads::threads::creative_ideas::RawIdea;
use crate::creative_ideas::dedup;
use crate::error::SimardResult;
use crate::ooda_brain::{
    ExistingIdeaView, IdeaConsolidationCtx, IdeaDedupCtx, IdeaDedupDecision, OodaBrain,
};

/// Kill-switch env var for the **agentic (semantic)** dedup layer. Only the
/// exact value `off` (case-insensitive) disables it; any other value keeps it
/// ON. Read once at daemon start. Disabling never disables deduplication — it
/// reverts to the deterministic Jaccard backstop; only the reasoning and the
/// `enhance` capability are switched off.
pub const SEMANTIC_DEDUP_ENV: &str = "SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP";

/// Stage-1 coarse-shortlist size env var — how many nearest existing ideas the
/// reasoner sees per candidate.
pub const SHORTLIST_K_ENV: &str = "SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K";

/// Default Stage-1 shortlist size.
pub const DEFAULT_SHORTLIST_K: usize = 12;
const SHORTLIST_K_MIN: usize = 1;
const SHORTLIST_K_MAX: usize = 64;

/// Cap on the merged-rationale length written back on an ENHANCE / consolidation
/// merge (bounds unbounded growth across repeated merges).
const MAX_MERGED_RATIONALE_CHARS: usize = 4000;

/// Whether the semantic (agentic) dedup layer is enabled, read from the process
/// environment. Secure default is **ON**; only the exact value `off`
/// (case-insensitive) disables it.
#[must_use]
pub fn semantic_dedup_enabled() -> bool {
    semantic_dedup_enabled_from(std::env::var(SEMANTIC_DEDUP_ENV).ok().as_deref())
}

/// Pure kill-switch classifier (testable without touching the process env).
fn semantic_dedup_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !v.trim().eq_ignore_ascii_case("off"),
        None => true,
    }
}

/// The Stage-1 shortlist size, read from the environment and clamped to
/// `[SHORTLIST_K_MIN, SHORTLIST_K_MAX]`. Out-of-range / unparseable falls back to
/// [`DEFAULT_SHORTLIST_K`].
#[must_use]
pub fn shortlist_k_from_env() -> usize {
    parse_shortlist_k(std::env::var(SHORTLIST_K_ENV).ok().as_deref())
}

/// Pure shortlist-K parser (clamped; testable without the process env).
fn parse_shortlist_k(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|k| (SHORTLIST_K_MIN..=SHORTLIST_K_MAX).contains(k))
        .unwrap_or(DEFAULT_SHORTLIST_K)
}

/// The applied plan the tick executes for one candidate idea.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlannedAction {
    /// Persist a new idea + review/route it (today's byte-for-byte path).
    Create,
    /// Drop the candidate; nothing is persisted (a true semantic duplicate).
    Skip { rationale: String },
    /// Merge the candidate into an existing idea (validated target).
    Enhance {
        target_node_id: String,
        rationale: String,
    },
    /// Fail-CLOSED: the reasoner errored (or named a bad target). The candidate
    /// is **dropped this cycle** — never a silent duplicate — and retried next
    /// run. The `reason` is surfaced by the caller.
    FailClosed { reason: String },
}

/// The **pure** decision core for one candidate — no IO, no store writes, no
/// metric emission — so the rail is trivially testable. Returns the
/// [`PlannedAction`] the caller applies.
///
/// Two-stage to bound prompt cost as the pool grows: Stage 1 ranks the pool by
/// the cheap word-set [`dedup::similarity`] and keeps the top-`shortlist_k`;
/// Stage 2 asks the reasoner to judge SEMANTIC equivalence over that shortlist.
pub(crate) fn plan_candidate(
    candidate: &RawIdea,
    pool: &[CreativeIdea],
    brain: &dyn OodaBrain,
    enabled: bool,
    shortlist_k: usize,
    jaccard_threshold: f64,
) -> PlannedAction {
    // Empty pool — nothing to dedup against.
    if pool.is_empty() {
        return PlannedAction::Create;
    }

    // Kill-switch: deterministic rail only (brain NOT consulted). Deduplication
    // is never disabled — only the reasoning and the `enhance` capability.
    if !enabled {
        return deterministic_backstop(candidate, pool, jaccard_threshold);
    }

    // Stage 1 — coarse shortlist (cheap, deterministic).
    let shortlist = coarse_shortlist(candidate, pool, shortlist_k);
    if shortlist.is_empty() {
        // Nothing lexically near this candidate. v1 limitation: a zero-overlap
        // semantic duplicate is not caught here (embedding shortlist is future
        // work); the honest default is to treat it as novel.
        return PlannedAction::Create;
    }

    // Stage 2 — agentic judge.
    let ctx = IdeaDedupCtx {
        candidate_idea: candidate.idea.clone(),
        candidate_rationale: candidate.rationale.clone(),
        existing_shortlist: shortlist.clone(),
    };
    match brain.decide_idea_dedup(&ctx) {
        Ok(IdeaDedupDecision::CreateNew { .. }) => PlannedAction::Create,
        Ok(IdeaDedupDecision::Skip { rationale }) => PlannedAction::Skip { rationale },
        Ok(IdeaDedupDecision::EnhanceExisting {
            target_node_id,
            rationale,
        }) => {
            if shortlist.iter().any(|v| v.node_id == target_node_id) {
                PlannedAction::Enhance {
                    target_node_id,
                    rationale,
                }
            } else {
                // The reasoner named a node it was not shown — fail closed
                // rather than mutate an unrelated idea or blindly create.
                tracing::error!(
                    target: "creative_ideas",
                    target_node_id = %target_node_id,
                    "[simard] creative-ideas dedup: brain named an out-of-shortlist ENHANCE target — failing closed (#2925)"
                );
                PlannedAction::FailClosed {
                    reason: format!(
                        "enhance target '{target_node_id}' not in the candidate's shortlist"
                    ),
                }
            }
        }
        Err(error) => {
            // Fail-CLOSED: never silently create a duplicate on a broken
            // reasoner. Drop the candidate this cycle; it is regenerated and
            // retried next run.
            tracing::error!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas dedup reasoner FAILED — failing closed (candidate dropped this cycle, retried next run) (#2925)"
            );
            PlannedAction::FailClosed {
                reason: error.to_string(),
            }
        }
    }
}

/// The deterministic Jaccard backstop: `Skip` if the candidate is a
/// near-duplicate of any pool idea, else `Create`. Used when the semantic layer
/// is switched off. Never `Enhance` (semantic judgement is required for that).
fn deterministic_backstop(
    candidate: &RawIdea,
    pool: &[CreativeIdea],
    threshold: f64,
) -> PlannedAction {
    let dup = pool
        .iter()
        .any(|p| dedup::is_near_duplicate(&candidate.idea, &p.idea, threshold));
    if dup {
        PlannedAction::Skip {
            rationale: "deterministic word-set backstop: near-duplicate of an existing idea"
                .to_string(),
        }
    } else {
        PlannedAction::Create
    }
}

/// Stage-1: rank the pool by the cheap word-set similarity and keep the top-`k`
/// items that have any lexical overlap, as advisory [`ExistingIdeaView`]s.
fn coarse_shortlist(candidate: &RawIdea, pool: &[CreativeIdea], k: usize) -> Vec<ExistingIdeaView> {
    let mut scored: Vec<(f64, &CreativeIdea)> = pool
        .iter()
        .map(|i| (dedup::similarity(&candidate.idea, &i.idea), i))
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k.max(1));
    scored.into_iter().map(|(_, i)| view_of(i)).collect()
}

/// Render one stored idea as the advisory view the reasoner reasons over.
fn view_of(idea: &CreativeIdea) -> ExistingIdeaView {
    ExistingIdeaView {
        node_id: idea.node_id.clone(),
        idea_id: idea.idea_id.clone(),
        idea: idea.idea.clone(),
        rationale: idea.context.rationale.clone(),
    }
}

/// Apply an `Enhance` plan against the store. **Append-only and
/// status-preserving**: loads the target, merges the candidate's rationale and
/// evidence links, and writes back with [`CreativeIdeaStore::update`], which
/// appends a new revision under the same `idea_id` — **no new node**, so the
/// pool count does not grow for a merge. Returns `Ok(false)` if the target node
/// is gone (the caller then degrades to `Create`); `dry_run` writes nothing.
pub(crate) fn apply_enhance(
    store: &dyn CreativeIdeaStore,
    target_node_id: &str,
    candidate: &RawIdea,
    rationale: &str,
    dry_run: bool,
) -> SimardResult<bool> {
    let Some(mut existing) = store.get(target_node_id)? else {
        return Ok(false);
    };
    existing.context.rationale = merge_rationale(
        &existing.context.rationale,
        [candidate.rationale.as_str(), rationale],
    );
    for link in &candidate.links {
        if !existing
            .links
            .iter()
            .any(|l| l.kind == link.kind && l.node_id == link.node_id)
        {
            existing.links.push(link.clone());
        }
    }
    if !dry_run {
        store.update(&existing)?;
    }
    Ok(true)
}

/// Merge additional rationale fragments into an existing rationale: append each
/// non-empty, not-already-present fragment behind an audit marker, length-capped.
fn merge_rationale<'a>(existing: &str, additions: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = existing.trim().to_string();
    for add in additions {
        let add = add.trim();
        if add.is_empty() || out.contains(add) {
            continue;
        }
        if out.is_empty() {
            out.push_str(add);
        } else {
            out.push_str("\n\n[enhanced #2925] ");
            out.push_str(add);
        }
    }
    truncate_chars(&out, MAX_MERGED_RATIONALE_CHARS)
}

/// Truncate to at most `max` characters (char-safe), appending '…' if cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Structured result of a consolidation pass over the existing pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// Semantic-duplication clusters applied (each with ≥1 redundant member).
    pub clusters: usize,
    /// Canonical ideas strengthened.
    pub canonical: usize,
    /// Redundant ideas transitioned `New → Rejected` (no hard deletes).
    pub rejected: usize,
    /// Whether this was a dry-run (nothing written).
    pub dry_run: bool,
}

/// Cluster the existing pool by semantic duplication (via the consolidation
/// recipe), strengthen each cluster's canonical idea, and transition the
/// redundant ideas to `Rejected`. **Dry-run first**: with `apply == false` it
/// computes and reports the plan and writes nothing. **No hard deletes** — every
/// collapsed idea stays auditable in a terminal `Rejected` state; re-running
/// after apply is idempotent (`Rejected` ideas drop out of the active pool).
pub fn consolidate_existing(
    store: &dyn CreativeIdeaStore,
    brain: &dyn OodaBrain,
    apply: bool,
) -> SimardResult<ConsolidationReport> {
    let pool = store.list(u32::MAX)?;
    let ctx = IdeaConsolidationCtx {
        pool: pool.iter().map(view_of).collect(),
    };
    let clusters = brain.decide_idea_consolidation(&ctx)?;

    let mut report = ConsolidationReport {
        clusters: 0,
        canonical: 0,
        rejected: 0,
        dry_run: !apply,
    };

    for cluster in &clusters {
        let Some(canonical) = pool.iter().find(|i| i.node_id == cluster.canonical_id) else {
            continue;
        };
        // Redundant members that exist, are not the canonical itself, and can
        // still be collapsed (skip anything already terminal — idempotent).
        let redundant: Vec<&CreativeIdea> = cluster
            .redundant_ids
            .iter()
            .filter(|rid| **rid != cluster.canonical_id)
            .filter_map(|rid| pool.iter().find(|i| i.node_id == *rid))
            .filter(|i| i.status.can_transition_to(IdeaStatus::Rejected))
            .collect();
        if redundant.is_empty() {
            continue;
        }
        report.clusters += 1;

        if apply {
            let mut c = canonical.clone();
            c.context.rationale = merge_rationale(
                &c.context.rationale,
                [
                    cluster.merged_rationale.as_str(),
                    cluster.evidence.join("; ").as_str(),
                ],
            );
            store.update(&c)?;
        }
        report.canonical += 1;

        for r in redundant {
            if apply {
                let mut rr = r.clone();
                rr.try_transition(IdeaStatus::Rejected)?;
                store.update(&rr)?;
            }
            report.rejected += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::cognitive_memory::LibraryCognitiveMemory;
    use crate::cognitive_memory::creative_idea::{
        IdeaContext, MemoryLink, MemoryLinkKind, ProspectiveCreativeIdeaStore,
    };
    use crate::creative_ideas::dedup::DEFAULT_DEDUP_THRESHOLD;
    use crate::error::SimardError;
    use crate::ooda_brain::{EngineerLifecycleCtx, EngineerLifecycleDecision, IdeaCluster};

    // --- test doubles ------------------------------------------------------

    /// A stub brain whose dedup decision (or error) and consolidation clusters
    /// are fixed, counting how many times the dedup reasoner was consulted.
    struct StubBrain {
        dedup: Option<IdeaDedupDecision>,
        clusters: Vec<IdeaCluster>,
        dedup_calls: AtomicU32,
    }

    impl StubBrain {
        fn dedup(decision: IdeaDedupDecision) -> Self {
            Self {
                dedup: Some(decision),
                clusters: Vec::new(),
                dedup_calls: AtomicU32::new(0),
            }
        }
        fn erroring() -> Self {
            Self {
                dedup: None,
                clusters: Vec::new(),
                dedup_calls: AtomicU32::new(0),
            }
        }
        fn consolidating(clusters: Vec<IdeaCluster>) -> Self {
            Self {
                dedup: Some(IdeaDedupDecision::CreateNew {
                    rationale: "n/a".into(),
                }),
                clusters,
                dedup_calls: AtomicU32::new(0),
            }
        }
        fn calls(&self) -> u32 {
            self.dedup_calls.load(Ordering::Relaxed)
        }
    }

    impl OodaBrain for StubBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &EngineerLifecycleCtx,
        ) -> SimardResult<EngineerLifecycleDecision> {
            Ok(EngineerLifecycleDecision::ContinueSkipping {
                rationale: "stub".into(),
            })
        }

        fn decide_idea_dedup(&self, _ctx: &IdeaDedupCtx) -> SimardResult<IdeaDedupDecision> {
            self.dedup_calls.fetch_add(1, Ordering::Relaxed);
            match &self.dedup {
                Some(d) => Ok(d.clone()),
                None => Err(SimardError::ReviewUnavailable {
                    reason: "stub dedup brain configured to fail".into(),
                }),
            }
        }

        fn decide_idea_consolidation(
            &self,
            _ctx: &IdeaConsolidationCtx,
        ) -> SimardResult<Vec<IdeaCluster>> {
            Ok(self.clusters.clone())
        }
    }

    fn raw(idea: &str, rationale: &str) -> RawIdea {
        RawIdea {
            idea: idea.to_string(),
            links: Vec::new(),
            rationale: rationale.to_string(),
        }
    }

    /// A stored `New` idea sharing words with the candidate so the coarse
    /// shortlist is non-empty and the reasoner is consulted.
    fn stored(mem: &LibraryCognitiveMemory, idea: &str, rationale: &str) -> CreativeIdea {
        let store = ProspectiveCreativeIdeaStore::new(mem);
        let mut ci = CreativeIdea::new(
            idea,
            IdeaContext {
                source: "test".into(),
                rationale: rationale.into(),
                ..Default::default()
            },
            100,
        );
        ci.node_id = store.store(&ci).expect("store");
        // Re-read so node_id/idea_id are the persisted values.
        store
            .get(&ci.node_id)
            .expect("get")
            .expect("present after store")
    }

    // --- plan_candidate: the fail-closed rails -----------------------------

    #[test]
    fn plan_create_on_empty_pool_without_consulting_brain() {
        let brain = StubBrain::dedup(IdeaDedupDecision::Skip {
            rationale: "would skip".into(),
        });
        let plan = plan_candidate(
            &raw("cache the goal board reads", "hot path"),
            &[],
            &brain,
            true,
            DEFAULT_SHORTLIST_K,
            DEFAULT_DEDUP_THRESHOLD,
        );
        assert_eq!(plan, PlannedAction::Create);
        assert_eq!(brain.calls(), 0, "empty pool must not consult the brain");
    }

    #[test]
    fn plan_kill_switch_off_uses_deterministic_backstop_and_never_calls_brain() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let existing = stored(&mem, "cache the goal board reads each cycle", "perf");
        let brain = StubBrain::dedup(IdeaDedupDecision::EnhanceExisting {
            target_node_id: existing.node_id.clone(),
            rationale: "would enhance".into(),
        });

        // A near-duplicate → deterministic Skip (not Enhance).
        let dup = raw("cache the goal board reads each cycle", "perf");
        let plan = plan_candidate(
            &dup,
            std::slice::from_ref(&existing),
            &brain,
            false,
            DEFAULT_SHORTLIST_K,
            DEFAULT_DEDUP_THRESHOLD,
        );
        assert!(
            matches!(plan, PlannedAction::Skip { .. }),
            "kill-switch off: near-dup ⇒ deterministic Skip, got {plan:?}"
        );

        // A novel candidate → deterministic Create.
        let novel = raw("add a completely unrelated telemetry exporter", "obs");
        let plan = plan_candidate(
            &novel,
            std::slice::from_ref(&existing),
            &brain,
            false,
            DEFAULT_SHORTLIST_K,
            DEFAULT_DEDUP_THRESHOLD,
        );
        assert_eq!(plan, PlannedAction::Create);
        assert_eq!(
            brain.calls(),
            0,
            "kill-switch off must not consult the brain"
        );
    }

    #[test]
    fn plan_maps_each_valid_brain_variant() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let existing = stored(&mem, "cache the goal board reads each cycle", "perf");
        let pool = std::slice::from_ref(&existing);
        let cand = raw("stop re-reading the goal board every cycle", "perf");

        // CreateNew → Create
        let brain = StubBrain::dedup(IdeaDedupDecision::CreateNew {
            rationale: "novel".into(),
        });
        assert_eq!(
            plan_candidate(
                &cand,
                pool,
                &brain,
                true,
                DEFAULT_SHORTLIST_K,
                DEFAULT_DEDUP_THRESHOLD
            ),
            PlannedAction::Create
        );
        assert_eq!(brain.calls(), 1, "shortlist non-empty ⇒ brain consulted");

        // Skip → Skip
        let brain = StubBrain::dedup(IdeaDedupDecision::Skip {
            rationale: "dupe".into(),
        });
        assert!(matches!(
            plan_candidate(
                &cand,
                pool,
                &brain,
                true,
                DEFAULT_SHORTLIST_K,
                DEFAULT_DEDUP_THRESHOLD
            ),
            PlannedAction::Skip { .. }
        ));

        // EnhanceExisting (valid target) → Enhance
        let brain = StubBrain::dedup(IdeaDedupDecision::EnhanceExisting {
            target_node_id: existing.node_id.clone(),
            rationale: "adds evidence".into(),
        });
        assert_eq!(
            plan_candidate(
                &cand,
                pool,
                &brain,
                true,
                DEFAULT_SHORTLIST_K,
                DEFAULT_DEDUP_THRESHOLD
            ),
            PlannedAction::Enhance {
                target_node_id: existing.node_id.clone(),
                rationale: "adds evidence".into(),
            }
        );
    }

    #[test]
    fn plan_brain_error_is_fail_closed_never_create() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let existing = stored(&mem, "cache the goal board reads each cycle", "perf");
        let brain = StubBrain::erroring();
        let cand = raw("stop re-reading the goal board every cycle", "perf");
        let plan = plan_candidate(
            &cand,
            std::slice::from_ref(&existing),
            &brain,
            true,
            DEFAULT_SHORTLIST_K,
            DEFAULT_DEDUP_THRESHOLD,
        );
        assert!(
            matches!(plan, PlannedAction::FailClosed { .. }),
            "a broken reasoner must fail closed (no silent duplicate), got {plan:?}"
        );
    }

    #[test]
    fn plan_bad_enhance_target_is_fail_closed() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let existing = stored(&mem, "cache the goal board reads each cycle", "perf");
        let brain = StubBrain::dedup(IdeaDedupDecision::EnhanceExisting {
            target_node_id: "node-does-not-exist".into(),
            rationale: "adds evidence".into(),
        });
        let cand = raw("stop re-reading the goal board every cycle", "perf");
        let plan = plan_candidate(
            &cand,
            std::slice::from_ref(&existing),
            &brain,
            true,
            DEFAULT_SHORTLIST_K,
            DEFAULT_DEDUP_THRESHOLD,
        );
        assert!(
            matches!(plan, PlannedAction::FailClosed { .. }),
            "an out-of-shortlist ENHANCE target must fail closed, got {plan:?}"
        );
    }

    // --- apply_enhance: append-only, 0 new nodes ---------------------------

    #[test]
    fn apply_enhance_appends_revision_zero_new_nodes_status_preserved() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let store = ProspectiveCreativeIdeaStore::new(&mem);
        let existing = stored(
            &mem,
            "cache the goal board reads each cycle",
            "original rationale",
        );
        let before = store.list(u32::MAX).expect("list").len();
        assert_eq!(before, 1);

        let mut cand = raw(
            "stop re-reading the goal board every cycle",
            "candidate adds a concrete benchmark",
        );
        cand.links.push(MemoryLink {
            kind: MemoryLinkKind::Goal,
            node_id: "g-evidence".into(),
        });

        let applied = apply_enhance(
            &store,
            &existing.node_id,
            &cand,
            "brain: same idea, adds a benchmark",
            /* dry_run */ false,
        )
        .expect("apply_enhance");
        assert!(applied, "target existed ⇒ applied");

        let after = store.list(u32::MAX).expect("list");
        assert_eq!(
            after.len(),
            1,
            "ENHANCE must create 0 new nodes (same idea_id)"
        );
        let merged = &after[0];
        assert_eq!(merged.idea_id, existing.idea_id, "same stable idea_id");
        assert_eq!(merged.status, IdeaStatus::New, "status preserved");
        assert!(
            merged.context.rationale.contains("original rationale")
                && merged.context.rationale.contains("benchmark"),
            "rationale merged: {}",
            merged.context.rationale
        );
        assert!(
            merged.links.iter().any(|l| l.node_id == "g-evidence"),
            "candidate evidence link accreted"
        );
    }

    #[test]
    fn apply_enhance_dry_run_writes_nothing() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let store = ProspectiveCreativeIdeaStore::new(&mem);
        let existing = stored(&mem, "cache the goal board reads", "r");
        let cand = raw("stop re-reading the goal board", "adds angle");
        let applied = apply_enhance(
            &store,
            &existing.node_id,
            &cand,
            "merge",
            /* dry_run */ true,
        )
        .expect("apply_enhance");
        assert!(applied);
        let after = store.list(u32::MAX).expect("list");
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].context.rationale, "r",
            "dry-run must not mutate the stored rationale"
        );
    }

    #[test]
    fn apply_enhance_missing_target_returns_false() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let store = ProspectiveCreativeIdeaStore::new(&mem);
        let cand = raw("x", "y");
        let applied =
            apply_enhance(&store, "no-such-node", &cand, "merge", false).expect("apply_enhance");
        assert!(
            !applied,
            "missing target ⇒ Ok(false) so the caller degrades to Create"
        );
    }

    // --- consolidate_existing ----------------------------------------------

    #[test]
    fn consolidate_dry_run_reports_plan_writes_nothing() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let store = ProspectiveCreativeIdeaStore::new(&mem);
        let canonical = stored(&mem, "cache the goal board reads", "keep me");
        let redundant = stored(&mem, "stop re-reading the goal board", "fold me");

        let brain = StubBrain::consolidating(vec![IdeaCluster {
            canonical_id: canonical.node_id.clone(),
            redundant_ids: vec![redundant.node_id.clone()],
            merged_rationale: "same idea".into(),
            evidence: vec![],
        }]);

        let report = consolidate_existing(&store, &brain, /* apply */ false).expect("consolidate");
        assert_eq!(report.clusters, 1);
        assert_eq!(report.canonical, 1);
        assert_eq!(report.rejected, 1);
        assert!(report.dry_run);

        let after = store.list(u32::MAX).expect("list");
        assert!(
            after.iter().all(|i| i.status == IdeaStatus::New),
            "dry-run must not transition any idea"
        );
    }

    #[test]
    fn consolidate_apply_merges_canonical_and_rejects_redundant_idempotently() {
        let mem = LibraryCognitiveMemory::in_memory().expect("mem");
        let store = ProspectiveCreativeIdeaStore::new(&mem);
        let canonical = stored(&mem, "cache the goal board reads", "keep me");
        let redundant = stored(&mem, "stop re-reading the goal board", "fold me");

        let cluster = IdeaCluster {
            canonical_id: canonical.node_id.clone(),
            redundant_ids: vec![redundant.node_id.clone()],
            merged_rationale: "the same underlying caching idea".into(),
            evidence: vec!["benchmarked 12% fewer reads".into()],
        };
        let brain = StubBrain::consolidating(vec![cluster]);

        let report = consolidate_existing(&store, &brain, /* apply */ true).expect("consolidate");
        assert_eq!(
            (report.clusters, report.canonical, report.rejected),
            (1, 1, 1)
        );
        assert!(!report.dry_run);

        let pool = store.list(u32::MAX).expect("list");
        let canonical_now = pool
            .iter()
            .find(|i| i.idea_id == canonical.idea_id)
            .expect("canonical present");
        assert_eq!(
            canonical_now.status,
            IdeaStatus::New,
            "canonical stays active"
        );
        assert!(
            canonical_now.context.rationale.contains("caching idea"),
            "canonical strengthened: {}",
            canonical_now.context.rationale
        );
        let redundant_now = pool
            .iter()
            .find(|i| i.idea_id == redundant.idea_id)
            .expect("redundant present (no hard delete)");
        assert_eq!(
            redundant_now.status,
            IdeaStatus::Rejected,
            "redundant collapsed to Rejected"
        );

        // Idempotent: a Rejected idea can no longer transition, so a second run
        // finds no collapsible members.
        let report2 = consolidate_existing(&store, &brain, /* apply */ true).expect("consolidate");
        assert_eq!(
            (report2.clusters, report2.rejected),
            (0, 0),
            "second run is idempotent"
        );
    }

    // --- pure classifiers ---------------------------------------------------

    #[test]
    fn kill_switch_classifier_only_off_disables() {
        assert!(!semantic_dedup_enabled_from(Some("off")));
        assert!(!semantic_dedup_enabled_from(Some("  OFF  ")));
        assert!(semantic_dedup_enabled_from(Some("on")));
        assert!(semantic_dedup_enabled_from(Some("")));
        assert!(semantic_dedup_enabled_from(Some("garbage")));
        assert!(semantic_dedup_enabled_from(None));
    }

    #[test]
    fn shortlist_k_parser_clamps_and_defaults() {
        assert_eq!(parse_shortlist_k(Some("20")), 20);
        assert_eq!(parse_shortlist_k(Some("1")), 1);
        assert_eq!(parse_shortlist_k(Some("64")), 64);
        assert_eq!(parse_shortlist_k(Some("0")), DEFAULT_SHORTLIST_K);
        assert_eq!(parse_shortlist_k(Some("999")), DEFAULT_SHORTLIST_K);
        assert_eq!(parse_shortlist_k(Some("nope")), DEFAULT_SHORTLIST_K);
        assert_eq!(parse_shortlist_k(None), DEFAULT_SHORTLIST_K);
    }
}
