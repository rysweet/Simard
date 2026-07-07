//! Unit tests for the Creative Ideas subsystem (design spike #2419).
//!
//! Hermetic: no network, injected clock (`now_epoch`), and reused in-memory
//! backends ([`LibraryCognitiveMemory::in_memory`], [`InMemoryGoalStore`]) plus
//! deterministic fakes for the reviewers and the `gh` seam. These tests pin the
//! data model, the state machine, the round-trip, the pipeline, the routing,
//! the safety gates, and the OFF-by-default contract.

use std::cell::RefCell;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::cognitive_memory::creative_idea::{
    CREATIVE_IDEA_PAYLOAD_VERSION, CREATIVE_IDEA_TRIGGER, CreativeIdea, CreativeIdeaStore,
    IdeaContext, IdeaStatus, MemoryLink, MemoryLinkKind, ProspectiveCreativeIdeaStore,
};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::cognitive_threads::threads::creative_ideas::{
    CreativeIdeasThread, FakeIdeaSource, GenerationInputs, RawIdea,
};
use crate::cognitive_threads::{CognitiveThread, ThreadContext, ThreadHealth, ThreadKind};
use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalStatus, GoalStore, InMemoryGoalStore, goal_slug};
use crate::memory_cognitive::CognitiveProspective;
use crate::stewardship::gh_client::GhIssue;

use super::dedup::{self, is_near_duplicate, reject_duplicates};
use super::reviewers::{
    CRUSTY_OLD_ENGINEER_ID, MEASURABILITY_ID, PHILOSOPHY_GUARDIAN_ID, Review, ReviewFlags,
    ReviewVerdict, Reviewer, run_review_pipeline,
};
use super::routing::{
    IdeaGhClient, gh_pr_add_label_argv, gh_pr_add_reviewer_argv, gh_pr_draft_argv, mark_completed,
    mark_idea_pr, route_idea_to_goal, route_idea_to_issue,
};
use super::synthesis::{DefaultSynthesizer, SuccessMetric};
use super::{
    CREATIVE_IDEA_ISSUE_LABEL, CREATIVE_IDEA_OWNER, CREATIVE_IDEA_PR_LABEL, CreativeIdeasConfig,
};

// ---------------------------------------------------------------------------
// Fixtures & fakes
// ---------------------------------------------------------------------------

const ALL_STATUSES: [IdeaStatus; 8] = [
    IdeaStatus::New,
    IdeaStatus::NeedsRevision,
    IdeaStatus::NeedsHumanReview,
    IdeaStatus::AcceptedForImplementation,
    IdeaStatus::Rejected,
    IdeaStatus::Deferred,
    IdeaStatus::ImplementationStarted,
    IdeaStatus::ImplementationCompleted,
];

/// The allowed edges of the state machine (must mirror `can_transition_to`).
const ALLOWED_EDGES: &[(IdeaStatus, IdeaStatus)] = &[
    (IdeaStatus::New, IdeaStatus::AcceptedForImplementation),
    (IdeaStatus::New, IdeaStatus::Rejected),
    (IdeaStatus::New, IdeaStatus::Deferred),
    (IdeaStatus::New, IdeaStatus::NeedsRevision),
    (IdeaStatus::New, IdeaStatus::NeedsHumanReview),
    (IdeaStatus::NeedsRevision, IdeaStatus::New),
    (IdeaStatus::NeedsRevision, IdeaStatus::Rejected),
    (IdeaStatus::NeedsRevision, IdeaStatus::Deferred),
    (
        IdeaStatus::NeedsHumanReview,
        IdeaStatus::AcceptedForImplementation,
    ),
    (IdeaStatus::NeedsHumanReview, IdeaStatus::Rejected),
    (IdeaStatus::NeedsHumanReview, IdeaStatus::Deferred),
    (IdeaStatus::Deferred, IdeaStatus::New),
    (IdeaStatus::Deferred, IdeaStatus::Rejected),
    (
        IdeaStatus::AcceptedForImplementation,
        IdeaStatus::ImplementationStarted,
    ),
    (IdeaStatus::AcceptedForImplementation, IdeaStatus::Deferred),
    (IdeaStatus::AcceptedForImplementation, IdeaStatus::Rejected),
    (
        IdeaStatus::ImplementationStarted,
        IdeaStatus::ImplementationCompleted,
    ),
    (IdeaStatus::ImplementationStarted, IdeaStatus::NeedsRevision),
    (IdeaStatus::ImplementationStarted, IdeaStatus::Rejected),
];

fn sample_context() -> IdeaContext {
    IdeaContext {
        source: "creative-ideas-thread".to_string(),
        goals_snapshot: vec!["improve recall".to_string()],
        observation_digest: "digest-abc".to_string(),
        rationale: "recall precision has plateaued for 3 days".to_string(),
    }
}

fn sample_metric() -> SuccessMetric {
    SuccessMetric {
        name: "recall_precision_at_k".to_string(),
        baseline: Some(0.71),
        target: ">= +0.05 over 7-day baseline".to_string(),
        how_measured: "nightly recall eval".to_string(),
    }
}

fn support_review(reviewer: &'static str, metric: Option<SuccessMetric>) -> Review {
    Review {
        reviewer,
        verdict: ReviewVerdict::Support,
        notes: "looks reasonable".to_string(),
        flags: ReviewFlags::default(),
        proposed_metric: metric,
    }
}

/// A deterministic [`Reviewer`] that returns a canned [`Review`].
struct StubReviewer {
    id: &'static str,
    review: Review,
}

impl Reviewer for StubReviewer {
    fn id(&self) -> &'static str {
        self.id
    }
    fn review(&self, _ctx: &super::reviewers::ReviewContext<'_>) -> SimardResult<Review> {
        Ok(self.review.clone())
    }
}

/// A recording fake for the [`IdeaGhClient`] seam.
#[derive(Default)]
struct FakeIdeaGhClient {
    issues: RefCell<Vec<RecordedIssue>>,
    pr_ops: RefCell<Vec<String>>,
}

#[derive(Clone)]
struct RecordedIssue {
    labels: Vec<String>,
    assignees: Vec<String>,
    body: String,
}

impl IdeaGhClient for FakeIdeaGhClient {
    fn create_labeled_issue(
        &self,
        _repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
        assignees: &[&str],
    ) -> SimardResult<GhIssue> {
        self.issues.borrow_mut().push(RecordedIssue {
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
            assignees: assignees.iter().map(|s| (*s).to_string()).collect(),
            body: body.to_string(),
        });
        Ok(GhIssue {
            number: 4242,
            url: "https://example.test/issues/4242".to_string(),
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    fn set_pr_draft(&self, _repo: &str, pr: u64, draft: bool) -> SimardResult<()> {
        self.pr_ops
            .borrow_mut()
            .push(format!("set_pr_draft:{pr}:{draft}"));
        Ok(())
    }

    fn add_pr_label(&self, _repo: &str, pr: u64, label: &str) -> SimardResult<()> {
        self.pr_ops
            .borrow_mut()
            .push(format!("add_pr_label:{pr}:{label}"));
        Ok(())
    }

    fn request_pr_review(&self, _repo: &str, pr: u64, reviewer: &str) -> SimardResult<()> {
        self.pr_ops
            .borrow_mut()
            .push(format!("request_pr_review:{pr}:{reviewer}"));
        Ok(())
    }
}

/// Owns the borrowed resources a [`ThreadContext`] needs.
struct TickEnv {
    rt: tokio::runtime::Runtime,
    mem: LibraryCognitiveMemory,
    shutdown: AtomicBool,
    tmp: tempfile::TempDir,
}

impl TickEnv {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let mem = LibraryCognitiveMemory::in_memory().expect("in-memory cognitive store");
        let tmp = tempfile::tempdir().expect("tempdir");
        Self {
            rt,
            mem,
            shutdown: AtomicBool::new(false),
            tmp,
        }
    }

    fn ctx(&self, now_epoch: u64, dry_run: bool) -> ThreadContext<'_> {
        ThreadContext {
            state_root: self.tmp.path(),
            repo_root: self.tmp.path(),
            memory: &self.mem as &dyn CognitiveMemoryOps,
            runtime: self.rt.handle().clone(),
            shutdown: &self.shutdown,
            now_epoch,
            dry_run,
        }
    }
}

// ---------------------------------------------------------------------------
// 1. State machine — only valid transitions
// ---------------------------------------------------------------------------

#[test]
fn state_machine_allows_only_valid_transitions() {
    for &from in &ALL_STATUSES {
        for &to in &ALL_STATUSES {
            let expected = ALLOWED_EDGES.contains(&(from, to));
            assert_eq!(
                from.can_transition_to(to),
                expected,
                "can_transition_to mismatch for {from} -> {to}"
            );

            let mut idea = CreativeIdea::new("x", sample_context(), 100);
            idea.status = from;
            let result = idea.try_transition(to);
            if expected {
                assert!(result.is_ok(), "expected {from} -> {to} to be allowed");
                assert_eq!(idea.status, to);
            } else {
                assert!(
                    matches!(result, Err(SimardError::InvalidIdeaTransition { from: f, to: t }) if f == from && t == to),
                    "expected InvalidIdeaTransition for {from} -> {to}"
                );
                assert_eq!(
                    idea.status, from,
                    "status must not change on a rejected edge"
                );
            }
        }
    }
}

#[test]
fn terminal_states_have_no_outgoing_edges() {
    for &terminal in &[IdeaStatus::Rejected, IdeaStatus::ImplementationCompleted] {
        assert!(terminal.is_terminal());
        for &to in &ALL_STATUSES {
            assert!(
                !terminal.can_transition_to(to),
                "{terminal} is terminal but allowed -> {to}"
            );
        }
    }
}

#[test]
fn implementation_completed_only_from_implementation_started() {
    for &from in &ALL_STATUSES {
        let allowed = from.can_transition_to(IdeaStatus::ImplementationCompleted);
        assert_eq!(
            allowed,
            from == IdeaStatus::ImplementationStarted,
            "only ImplementationStarted may reach ImplementationCompleted (offender: {from})"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Persistence / retrieval — round-trip through prospective memory
// ---------------------------------------------------------------------------

#[test]
fn generated_idea_persists_as_prospective_node_and_round_trips() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    let store = ProspectiveCreativeIdeaStore::new(&mem);

    let mut idea = CreativeIdea::new(
        "add a nightly recall regression eval",
        sample_context(),
        1_700_000_000,
    );
    idea.links = vec![
        MemoryLink {
            kind: MemoryLinkKind::Semantic,
            node_id: "sem_1".to_string(),
        },
        MemoryLink {
            kind: MemoryLinkKind::Episodic,
            node_id: "epi_9".to_string(),
        },
    ];
    idea.success_metric = Some(sample_metric());
    idea.reviews = vec![support_review(MEASURABILITY_ID, Some(sample_metric()))];

    let node_id = store.store(&idea).expect("store idea");
    assert!(!node_id.is_empty());

    // The raw prospective node carries the retrieval sentinel.
    let raw = mem.list_all_prospective(u32::MAX).expect("list raw");
    let node = raw
        .iter()
        .find(|n| n.trigger_condition == CREATIVE_IDEA_TRIGGER)
        .expect("a creative-idea node exists");
    assert_eq!(
        node.description, idea.idea,
        "description mirrors the idea text"
    );

    // Round-trips with payload fidelity.
    let listed = store.list(u32::MAX).expect("list ideas");
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.idea, idea.idea);
    assert_eq!(got.status, IdeaStatus::New);
    assert_eq!(got.links, idea.links);
    assert_eq!(got.context, idea.context);
    assert_eq!(got.success_metric, idea.success_metric);
    assert_eq!(got.reviews, idea.reviews);

    // `get` by node_id resolves the same idea.
    let by_id = store.get(&got.node_id).expect("get").expect("some");
    assert_eq!(by_id.idea, idea.idea);
    assert_eq!(by_id.node_id, got.node_id);
}

// ---------------------------------------------------------------------------
// 3. Pipeline — all four reviewers + synthesis sets a legal status
// ---------------------------------------------------------------------------

#[test]
fn pipeline_runs_all_reviewers_and_synthesis_sets_status() {
    let inputs = GenerationInputs::default();
    let mut idea = CreativeIdea::new("cache distilled facts by concept", sample_context(), 200);

    let crusty = StubReviewer {
        id: CRUSTY_OLD_ENGINEER_ID,
        review: support_review(CRUSTY_OLD_ENGINEER_ID, None),
    };
    let philosophy = StubReviewer {
        id: PHILOSOPHY_GUARDIAN_ID,
        review: support_review(PHILOSOPHY_GUARDIAN_ID, None),
    };
    let measurability = StubReviewer {
        id: MEASURABILITY_ID,
        review: support_review(MEASURABILITY_ID, Some(sample_metric())),
    };
    let reviewers: [&dyn Reviewer; 3] = [&crusty, &philosophy, &measurability];

    let outcome = run_review_pipeline(&mut idea, &inputs, &reviewers, &DefaultSynthesizer)
        .expect("pipeline runs");

    // All three vetting reviewers ran (the synthesizer is the fourth step).
    assert_eq!(idea.reviews.len(), 3);
    // Synthesis set a status that is a legal transition from `New`.
    assert!(IdeaStatus::New.can_transition_to(outcome.next_status));
    assert_eq!(idea.status, outcome.next_status);
    // Support + a metric with no risk flags => accepted, metric attached.
    assert_eq!(idea.status, IdeaStatus::AcceptedForImplementation);
    assert_eq!(idea.success_metric, Some(sample_metric()));
}

#[test]
fn synthesis_routes_high_risk_idea_to_human_review() {
    let inputs = GenerationInputs::default();
    let mut idea = CreativeIdea::new("auto-delete stale worktrees", sample_context(), 200);

    let mut risky = support_review(CRUSTY_OLD_ENGINEER_ID, None);
    risky.verdict = ReviewVerdict::Concern;
    risky.flags.high_risk = true;
    risky.flags.irreversible = true;

    let crusty = StubReviewer {
        id: CRUSTY_OLD_ENGINEER_ID,
        review: risky,
    };
    let measurability = StubReviewer {
        id: MEASURABILITY_ID,
        review: support_review(MEASURABILITY_ID, Some(sample_metric())),
    };
    let reviewers: [&dyn Reviewer; 2] = [&crusty, &measurability];

    run_review_pipeline(&mut idea, &inputs, &reviewers, &DefaultSynthesizer).expect("pipeline");
    assert_eq!(idea.status, IdeaStatus::NeedsHumanReview);
}

#[test]
fn synthesis_needs_revision_when_no_metric() {
    let inputs = GenerationInputs::default();
    let mut idea = CreativeIdea::new("some idea with no measurable outcome", sample_context(), 1);
    let crusty = StubReviewer {
        id: CRUSTY_OLD_ENGINEER_ID,
        review: support_review(CRUSTY_OLD_ENGINEER_ID, None),
    };
    let reviewers: [&dyn Reviewer; 1] = [&crusty];
    run_review_pipeline(&mut idea, &inputs, &reviewers, &DefaultSynthesizer).expect("pipeline");
    assert_eq!(idea.status, IdeaStatus::NeedsRevision);
}

// ---------------------------------------------------------------------------
// 4. Routing — accepted, not flagged => a proposed goal
// ---------------------------------------------------------------------------

#[test]
fn route_accepted_idea_produces_proposed_goal() {
    let goals = InMemoryGoalStore::try_default().expect("goal store");
    let mut idea = CreativeIdea::new(
        "distill meeting transcripts into facts",
        sample_context(),
        5,
    );
    idea.node_id = "pro_idea_1".to_string();
    idea.status = IdeaStatus::AcceptedForImplementation;

    let record = route_idea_to_goal(&idea, &goals, 1_700_000_500).expect("route to goal");
    assert_eq!(record.status, GoalStatus::Proposed);
    assert_eq!(record.slug, goal_slug(&idea.idea));

    // Traceability: the originating node id is in the goal evidence.
    let ev = record
        .evidence
        .iter()
        .map(|e| e.to_persisted_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        ev.contains("pro_idea_1"),
        "evidence should tag the idea node id: {ev}"
    );

    // Persisted in the store.
    let stored = goals.list().expect("list goals");
    assert!(
        stored
            .iter()
            .any(|g| g.slug == record.slug && g.status == GoalStatus::Proposed)
    );
}

#[test]
fn route_accepted_idea_stamps_source_creative_ideas_provenance() {
    // Issue #2743 headline case: a goal promoted from a creative idea is
    // stamped `source:creative-ideas` at creation, so "which goals came from
    // creative ideas?" is answerable by an exact-tag filter.
    let goals = InMemoryGoalStore::try_default().expect("goal store");
    let mut idea = CreativeIdea::new("ship the live tag filter", sample_context(), 7);
    idea.node_id = "pro_idea_prov".to_string();
    idea.status = IdeaStatus::AcceptedForImplementation;

    let record = route_idea_to_goal(&idea, &goals, 1_700_000_777).expect("route to goal");
    assert_eq!(
        record.labels,
        vec![crate::goal_curation::labels::SOURCE_CREATIVE_IDEAS.to_string()],
        "creative-idea goal must carry exactly source:creative-ideas at birth",
    );

    // The provenance survives persistence into the store.
    let stored = goals.list().expect("list goals");
    let persisted = stored
        .iter()
        .find(|g| g.slug == record.slug)
        .expect("goal persisted");
    assert!(
        persisted
            .labels
            .iter()
            .any(|l| l == crate::goal_curation::labels::SOURCE_CREATIVE_IDEAS),
        "persisted creative-idea goal must remain queryable by source:creative-ideas",
    );
}

#[test]
fn route_non_accepted_idea_to_goal_is_rejected() {
    let goals = InMemoryGoalStore::try_default().expect("goal store");
    let mut idea = CreativeIdea::new("premature idea", sample_context(), 5);
    idea.status = IdeaStatus::New;
    let result = route_idea_to_goal(&idea, &goals, 1);
    assert!(matches!(
        result,
        Err(SimardError::InvalidIdeaTransition {
            from: IdeaStatus::New,
            to: IdeaStatus::ImplementationStarted
        })
    ));
    assert!(goals.list().expect("list").is_empty());
}

// ---------------------------------------------------------------------------
// 5. Routing — NeedsHumanReview => issue tagging the owner
// ---------------------------------------------------------------------------

#[test]
fn route_needs_human_idea_files_issue_tagging_owner() {
    let gh = FakeIdeaGhClient::default();
    let mut idea = CreativeIdea::new("risky exploratory refactor", sample_context(), 9);
    idea.node_id = "pro_idea_hr".to_string();
    idea.status = IdeaStatus::NeedsHumanReview;

    let issue = route_idea_to_issue(&idea, &gh, "rysweet/Simard").expect("file issue");
    assert!(issue.body.contains("pro_idea_hr"));

    let recorded = gh.issues.borrow();
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0]
            .labels
            .iter()
            .any(|l| l == CREATIVE_IDEA_ISSUE_LABEL)
    );
    assert!(
        recorded[0]
            .assignees
            .iter()
            .any(|a| a == CREATIVE_IDEA_OWNER)
    );
    assert!(recorded[0].body.contains("pro_idea_hr"));
}

// ---------------------------------------------------------------------------
// 6. Routing — PR human-review gate (draft + label + owner review, no bypass)
// ---------------------------------------------------------------------------

#[test]
fn mark_idea_pr_applies_full_gate_and_never_bypasses() {
    let gh = FakeIdeaGhClient::default();
    let mut idea = CreativeIdea::new("idea behind a PR", sample_context(), 11);
    idea.node_id = "pro_idea_pr".to_string();

    let gate = mark_idea_pr(77, &idea, &gh, "rysweet/Simard").expect("mark pr");
    assert!(gate.draft);
    assert_eq!(gate.blocking_label, CREATIVE_IDEA_PR_LABEL);
    assert_eq!(
        gate.review_requested_from,
        vec![CREATIVE_IDEA_OWNER.to_string()]
    );
    assert_eq!(gate.originating_idea, "pro_idea_pr");

    let ops = gh.pr_ops.borrow();
    assert!(ops.contains(&"set_pr_draft:77:true".to_string()));
    assert!(ops.contains(&format!("add_pr_label:77:{CREATIVE_IDEA_PR_LABEL}")));
    assert!(ops.contains(&format!("request_pr_review:77:{CREATIVE_IDEA_OWNER}")));
}

#[test]
fn constructed_gh_argv_never_contain_admin_or_no_verify() {
    let argvs: Vec<Vec<String>> = vec![
        gh_pr_draft_argv("rysweet/Simard", 77),
        gh_pr_add_label_argv("rysweet/Simard", 77, CREATIVE_IDEA_PR_LABEL),
        gh_pr_add_reviewer_argv("rysweet/Simard", 77, CREATIVE_IDEA_OWNER),
        super::routing::gh_issue_create_argv(
            "rysweet/Simard",
            "t",
            "b",
            &[CREATIVE_IDEA_ISSUE_LABEL],
            &[CREATIVE_IDEA_OWNER],
        ),
    ];

    for argv in &argvs {
        assert!(
            !argv.iter().any(|a| a == "--admin"),
            "no --admin in {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--no-verify"),
            "no --no-verify in {argv:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. Dedup — rejects a near-duplicate of a prior idea
// ---------------------------------------------------------------------------

#[test]
fn dedup_rejects_near_duplicate_of_prior_idea() {
    let prior = CreativeIdea::new(
        "add a nightly recall regression eval for cognitive memory",
        sample_context(),
        1,
    );

    assert!(is_near_duplicate(
        "add a nightly recall regression eval for cognitive memory please",
        &prior.idea,
        dedup::DEFAULT_DEDUP_THRESHOLD
    ));
    assert!(!is_near_duplicate(
        "rewrite the dashboard styling",
        &prior.idea,
        dedup::DEFAULT_DEDUP_THRESHOLD
    ));

    let candidates = vec![
        RawIdea {
            idea: "add a nightly recall regression eval for cognitive memory".to_string(),
            links: vec![],
            rationale: String::new(),
        },
        RawIdea {
            idea: "introduce a brand new sensory pre-processing thread".to_string(),
            links: vec![],
            rationale: String::new(),
        },
    ];
    let kept = reject_duplicates(
        candidates,
        std::slice::from_ref(&prior),
        dedup::DEFAULT_DEDUP_THRESHOLD,
    );
    assert_eq!(kept.len(), 1);
    assert!(kept[0].idea.contains("sensory"));
}

// ---------------------------------------------------------------------------
// 8. Default-ON, opt-out
// ---------------------------------------------------------------------------

#[test]
fn subsystem_is_on_by_default_opt_out() {
    // Default-ON, consistent with the Overseer/Journal cognitive threads.
    assert!(CreativeIdeasConfig::default().enabled());
    // An empty environment leaves it ON.
    assert!(CreativeIdeasConfig::from_lookup(|_| None).enabled());
    // Only an explicit falsey value opts out.
    assert!(
        !CreativeIdeasConfig::from_lookup(|k| (k == super::ENABLED_ENV).then(|| "0".to_string()))
            .enabled()
    );
    assert!(
        !CreativeIdeasConfig::from_lookup(
            |k| (k == super::ENABLED_ENV).then(|| "false".to_string())
        )
        .enabled()
    );
    // A truthy value keeps it ON.
    assert!(
        CreativeIdeasConfig::from_lookup(|k| (k == super::ENABLED_ENV).then(|| "true".to_string()))
            .enabled()
    );

    // The thread reports enabled by default and reuses the BackgroundThought kind.
    let thread = CreativeIdeasThread::new(
        CreativeIdeasConfig::default(),
        Box::new(FakeIdeaSource::default()),
    );
    assert!(thread.enabled());
    assert_eq!(thread.kind(), ThreadKind::BackgroundThought);
}

// ---------------------------------------------------------------------------
// 9. Error contracts — fail-closed
// ---------------------------------------------------------------------------

#[test]
fn illegal_transition_returns_invalid_idea_transition() {
    let mut idea = CreativeIdea::new("x", sample_context(), 1);
    idea.status = IdeaStatus::Rejected; // terminal
    let err = idea.try_transition(IdeaStatus::New).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidIdeaTransition {
            from: IdeaStatus::Rejected,
            to: IdeaStatus::New
        }
    ));
}

#[test]
fn unknown_status_string_is_rejected_not_defaulted() {
    use crate::cognitive_memory::creative_idea::parse_idea_status;
    let err = parse_idea_status("Bogus").unwrap_err();
    assert!(matches!(err, SimardError::InvalidCreativeIdeaRecord { .. }));
    // A known one round-trips.
    assert_eq!(
        parse_idea_status("NeedsHumanReview").expect("known"),
        IdeaStatus::NeedsHumanReview
    );
}

#[test]
fn bad_payload_and_wrong_sentinel_and_new_version_fail_closed() {
    // Wrong sentinel.
    let wrong = CognitiveProspective {
        node_id: "pro_x".to_string(),
        description: "d".to_string(),
        trigger_condition: "not-a-creative-idea".to_string(),
        action_on_trigger: "{}".to_string(),
        status: "pending".to_string(),
        priority: 3,
    };
    assert!(matches!(
        CreativeIdea::from_prospective(&wrong),
        Err(SimardError::InvalidCreativeIdeaRecord { .. })
    ));

    // Unparseable payload.
    let bad_json = CognitiveProspective {
        trigger_condition: CREATIVE_IDEA_TRIGGER.to_string(),
        action_on_trigger: "{ not valid json".to_string(),
        ..wrong.clone()
    };
    assert!(matches!(
        CreativeIdea::from_prospective(&bad_json),
        Err(SimardError::InvalidCreativeIdeaRecord { .. })
    ));

    // A too-new payload_version.
    let mut idea = CreativeIdea::new("versioned", sample_context(), 1);
    idea.reviews = vec![support_review(MEASURABILITY_ID, Some(sample_metric()))];
    let payload = idea.to_action_payload().expect("payload");
    let mut value: Value = serde_json::from_str(&payload).expect("json");
    value["payload_version"] = Value::from(CREATIVE_IDEA_PAYLOAD_VERSION as u64 + 1);
    let too_new = CognitiveProspective {
        trigger_condition: CREATIVE_IDEA_TRIGGER.to_string(),
        action_on_trigger: value.to_string(),
        ..wrong.clone()
    };
    assert!(matches!(
        CreativeIdea::from_prospective(&too_new),
        Err(SimardError::InvalidCreativeIdeaRecord { .. })
    ));

    // An unknown reviewer id in the payload is fail-closed.
    let mut value2: Value = serde_json::from_str(&payload).expect("json");
    value2["reviews"][0]["reviewer"] = Value::from("totally_unknown_reviewer");
    let unknown_reviewer = CognitiveProspective {
        trigger_condition: CREATIVE_IDEA_TRIGGER.to_string(),
        action_on_trigger: value2.to_string(),
        ..wrong
    };
    assert!(matches!(
        CreativeIdea::from_prospective(&unknown_reviewer),
        Err(SimardError::InvalidCreativeIdeaRecord { .. })
    ));
}

// ---------------------------------------------------------------------------
// 10. Tick is total
// ---------------------------------------------------------------------------

#[test]
fn tick_is_total_when_idea_source_errors() {
    let env = TickEnv::new();
    let cfg = CreativeIdeasConfig {
        enabled: true, // turn on so tick reaches the (failing) source
        ..CreativeIdeasConfig::default()
    };
    let mut thread = CreativeIdeasThread::new(cfg, Box::new(FakeIdeaSource::failing()));

    let mut ctx = env.ctx(1_000, /* dry_run */ true);
    let outcome = thread.tick(&mut ctx);

    assert!(outcome.ran, "an enabled tick runs");
    assert!(
        !outcome.success,
        "a failing source yields a failed outcome, not a panic/Err"
    );
    assert!(outcome.summary.contains("creative-ideas tick failed"));

    let health: ThreadHealth = thread.health();
    assert_eq!(health.consecutive_errors, 1);
    assert_eq!(health.last_success, Some(false));
}

#[test]
fn disabled_tick_is_skipped() {
    let env = TickEnv::new();
    let disabled = CreativeIdeasConfig {
        enabled: false,
        ..CreativeIdeasConfig::default()
    };
    let mut thread = CreativeIdeasThread::new(disabled, Box::new(FakeIdeaSource::failing()));
    let mut ctx = env.ctx(1_000, true);
    let outcome = thread.tick(&mut ctx);
    assert!(!outcome.ran, "an explicitly-disabled thread does no work");
    assert!(outcome.success);
}

// ---------------------------------------------------------------------------
// 11. Outcome feedback — mark_completed gated on a met metric
// ---------------------------------------------------------------------------

#[test]
fn mark_completed_requires_started_and_metric_met() {
    // Happy path.
    let mut idea = CreativeIdea::new("done idea", sample_context(), 1);
    idea.status = IdeaStatus::ImplementationStarted;
    mark_completed(&mut idea, true).expect("complete");
    assert_eq!(idea.status, IdeaStatus::ImplementationCompleted);

    // Refuses when the metric is not met.
    let mut idea2 = CreativeIdea::new("not-yet idea", sample_context(), 1);
    idea2.status = IdeaStatus::ImplementationStarted;
    assert!(matches!(
        mark_completed(&mut idea2, false),
        Err(SimardError::InvalidIdeaTransition { .. })
    ));
    assert_eq!(idea2.status, IdeaStatus::ImplementationStarted);

    // Refuses when not in ImplementationStarted.
    let mut idea3 = CreativeIdea::new("wrong state", sample_context(), 1);
    idea3.status = IdeaStatus::New;
    assert!(matches!(
        mark_completed(&mut idea3, true),
        Err(SimardError::InvalidIdeaTransition { .. })
    ));
}

// ---------------------------------------------------------------------------
// 12. Wired generation + review/route pipeline (hermetic, all fakes)
// ---------------------------------------------------------------------------

use super::pipeline::{AgenticIdeaPipeline, GoalStoreFactory, IdeaPipeline, RouteOutcome};
use super::reviewers::AgentInvoker;
use crate::goals::GoalRecord;

/// A review/route pipeline that leaves ideas untouched — isolates the
/// generation step for the "ten new ideas" assertion.
struct NoopPipeline;

impl IdeaPipeline for NoopPipeline {
    fn review_and_route(
        &self,
        _idea: &mut CreativeIdea,
        _inputs: &GenerationInputs,
        _ctx: &ThreadContext<'_>,
    ) -> SimardResult<RouteOutcome> {
        Ok(RouteOutcome::Parked)
    }
}

/// An [`AgentInvoker`] returning a fixed JSON envelope for every prompt.
struct CannedInvoker {
    response: String,
}

impl AgentInvoker for CannedInvoker {
    fn invoke(&self, _prompt: &str) -> SimardResult<String> {
        Ok(self.response.clone())
    }
}

/// A `gh` fake that records into shared `Arc<Mutex<..>>` handles so the test can
/// inspect it after the pipeline (which owns the boxed client) has run.
#[derive(Clone, Default)]
struct SharedGh {
    issues: std::sync::Arc<std::sync::Mutex<Vec<RecordedIssue>>>,
    pr_ops: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl IdeaGhClient for SharedGh {
    fn create_labeled_issue(
        &self,
        _repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
        assignees: &[&str],
    ) -> SimardResult<GhIssue> {
        self.issues.lock().unwrap().push(RecordedIssue {
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
            assignees: assignees.iter().map(|s| (*s).to_string()).collect(),
            body: body.to_string(),
        });
        Ok(GhIssue {
            number: 999,
            url: "https://example.test/issues/999".to_string(),
            title: title.to_string(),
            body: body.to_string(),
        })
    }
    fn set_pr_draft(&self, _repo: &str, pr: u64, draft: bool) -> SimardResult<()> {
        self.pr_ops
            .lock()
            .unwrap()
            .push(format!("set_pr_draft:{pr}:{draft}"));
        Ok(())
    }
    fn add_pr_label(&self, _repo: &str, pr: u64, label: &str) -> SimardResult<()> {
        self.pr_ops
            .lock()
            .unwrap()
            .push(format!("add_pr_label:{pr}:{label}"));
        Ok(())
    }
    fn request_pr_review(&self, _repo: &str, pr: u64, reviewer: &str) -> SimardResult<()> {
        self.pr_ops
            .lock()
            .unwrap()
            .push(format!("request_pr_review:{pr}:{reviewer}"));
        Ok(())
    }
}

/// A shared in-memory goal store exposed as a [`GoalStore`] and a factory.
struct SharedGoalStore(std::sync::Arc<InMemoryGoalStore>);

impl GoalStore for SharedGoalStore {
    fn descriptor(&self) -> crate::metadata::BackendDescriptor {
        self.0.descriptor()
    }
    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        self.0.put(record)
    }
    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        self.0.list()
    }
    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        self.0.top_goals_by_status(status, limit)
    }
    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        self.0.active_top_goals(limit)
    }
}

struct SharedGoalStoreFactory(std::sync::Arc<InMemoryGoalStore>);

impl GoalStoreFactory for SharedGoalStoreFactory {
    fn open(&self, _state_root: &std::path::Path) -> SimardResult<Box<dyn GoalStore>> {
        Ok(Box::new(SharedGoalStore(std::sync::Arc::clone(&self.0))))
    }
}

fn support_metric_response() -> String {
    r#"```json
{"verdict": "Support", "notes": "ok", "metric": {"name": "recall_precision_at_k", "baseline": 0.71, "target": ">= +0.05 over 7-day baseline and a live trend", "how_measured": "nightly recall eval + production self-metric"}}
```"#
    .to_string()
}

#[test]
fn generation_run_produces_ten_new_ideas_with_links() {
    let env = TickEnv::new();
    let mut raws = Vec::new();
    for i in 0..10 {
        raws.push(RawIdea {
            idea: format!("distinct self-improvement idea number {i}"),
            links: vec![MemoryLink {
                kind: MemoryLinkKind::Goal,
                node_id: format!("g{i}"),
            }],
            rationale: format!("grounded rationale {i}"),
        });
    }
    let cfg = CreativeIdeasConfig {
        enabled: true,
        batch: 10,
        ..CreativeIdeasConfig::default()
    };
    let mut thread = CreativeIdeasThread::with_pipeline(
        cfg,
        Box::new(FakeIdeaSource::with_ideas(raws)),
        Box::new(NoopPipeline),
    );
    let mut ctx = env.ctx(1_000, /* dry_run */ false);
    let outcome = thread.tick(&mut ctx);
    assert!(outcome.ran && outcome.success, "{}", outcome.summary);

    let store = ProspectiveCreativeIdeaStore::new(&env.mem);
    let ideas = store.list(u32::MAX).expect("list ideas");
    assert_eq!(ideas.len(), 10, "exactly ten idea prospective-memories");
    for idea in &ideas {
        assert_eq!(idea.status, IdeaStatus::New, "each generated idea is New");
        assert!(!idea.links.is_empty(), "each idea has populated links");
    }
}

#[test]
fn wired_pipeline_accepts_idea_creates_goal_and_persists_reviews() {
    let env = TickEnv::new();
    let goal_store = std::sync::Arc::new(InMemoryGoalStore::try_default().expect("goals"));
    let gh = SharedGh::default();

    let pipeline = AgenticIdeaPipeline::new(
        Box::new(CannedInvoker {
            response: support_metric_response(),
        }),
        Box::new(gh.clone()),
        Box::new(SharedGoalStoreFactory(std::sync::Arc::clone(&goal_store))),
        "rysweet/Simard".to_string(),
    );

    let mut idea = CreativeIdea::new("cache distilled facts by concept", sample_context(), 42);
    let store = ProspectiveCreativeIdeaStore::new(&env.mem);
    idea.node_id = store.store(&idea).expect("persist New");

    let ctx = env.ctx(1234, /* dry_run */ false);
    let outcome = pipeline
        .review_and_route(&mut idea, &GenerationInputs::default(), &ctx)
        .expect("pipeline");

    assert_eq!(outcome, RouteOutcome::Goal);
    assert_eq!(idea.reviews.len(), 3, "all three reviewers contributed");
    assert_eq!(idea.status, IdeaStatus::ImplementationStarted);

    let goals = goal_store.list().expect("goals list");
    assert!(
        goals
            .iter()
            .any(|g| g.status == GoalStatus::Proposed && g.title == idea.idea),
        "an accepted idea creates a Proposed goal on the board"
    );

    let current = store
        .list(u32::MAX)
        .expect("list")
        .into_iter()
        .find(|i| i.idea_id == idea.idea_id)
        .expect("reviewed idea present");
    assert_eq!(current.status, IdeaStatus::ImplementationStarted);
    assert_eq!(current.reviews.len(), 3, "reviews persisted on the idea");
    assert!(
        gh.issues.lock().unwrap().is_empty(),
        "accepted idea files no issue"
    );
}

#[test]
fn wired_pipeline_human_review_files_issue_with_label_and_owner() {
    let env = TickEnv::new();
    let goal_store = std::sync::Arc::new(InMemoryGoalStore::try_default().expect("goals"));
    let gh = SharedGh::default();

    let human_response = "```json\n{\"verdict\": \"NeedsHuman\", \"notes\": \"risky\", \"needs_human\": true, \"high_risk\": true}\n```".to_string();
    let pipeline = AgenticIdeaPipeline::new(
        Box::new(CannedInvoker {
            response: human_response,
        }),
        Box::new(gh.clone()),
        Box::new(SharedGoalStoreFactory(std::sync::Arc::clone(&goal_store))),
        "rysweet/Simard".to_string(),
    );

    let mut idea = CreativeIdea::new(
        "auto-delete stale worktrees on a schedule",
        sample_context(),
        7,
    );
    let store = ProspectiveCreativeIdeaStore::new(&env.mem);
    idea.node_id = store.store(&idea).expect("persist New");

    let ctx = env.ctx(555, /* dry_run */ false);
    let outcome = pipeline
        .review_and_route(&mut idea, &GenerationInputs::default(), &ctx)
        .expect("pipeline");

    assert_eq!(outcome, RouteOutcome::Issue);
    assert_eq!(idea.status, IdeaStatus::NeedsHumanReview);

    let issues = gh.issues.lock().unwrap();
    assert_eq!(
        issues.len(),
        1,
        "a human-review idea files exactly one issue"
    );
    assert!(
        issues[0]
            .labels
            .iter()
            .any(|l| l == CREATIVE_IDEA_ISSUE_LABEL),
        "issue carries the creative-idea label"
    );
    assert!(
        issues[0].assignees.iter().any(|a| a == CREATIVE_IDEA_OWNER),
        "issue tags the repo owner"
    );
    assert!(
        goal_store.list().expect("goals").is_empty(),
        "a human-review idea does not auto-create a goal"
    );
}

#[test]
fn dry_run_reviews_but_writes_and_routes_nothing() {
    let env = TickEnv::new();
    let goal_store = std::sync::Arc::new(InMemoryGoalStore::try_default().expect("goals"));
    let gh = SharedGh::default();
    let pipeline = AgenticIdeaPipeline::new(
        Box::new(CannedInvoker {
            response: support_metric_response(),
        }),
        Box::new(gh.clone()),
        Box::new(SharedGoalStoreFactory(std::sync::Arc::clone(&goal_store))),
        "rysweet/Simard".to_string(),
    );
    let mut idea = CreativeIdea::new("a dry-run idea", sample_context(), 1);
    let ctx = env.ctx(1, /* dry_run */ true);
    let outcome = pipeline
        .review_and_route(&mut idea, &GenerationInputs::default(), &ctx)
        .expect("pipeline");
    assert_eq!(outcome, RouteOutcome::DryRun);
    assert_eq!(idea.reviews.len(), 3, "dry-run still reviews");
    assert!(
        goal_store.list().expect("goals").is_empty(),
        "dry-run writes no goal"
    );
    let store = ProspectiveCreativeIdeaStore::new(&env.mem);
    assert!(
        store.list(u32::MAX).expect("list").is_empty(),
        "dry-run persists nothing"
    );
}

#[test]
fn ideas_are_enumerable_and_searchable_by_status() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    let store = ProspectiveCreativeIdeaStore::new(&mem);

    // One idea in each of several statuses (all legal transitions from New).
    let targets = [
        IdeaStatus::New,
        IdeaStatus::Rejected,
        IdeaStatus::Deferred,
        IdeaStatus::NeedsHumanReview,
    ];
    for (i, &status) in targets.iter().enumerate() {
        let mut idea =
            CreativeIdea::new(format!("idea for status {i}"), sample_context(), i as u64);
        idea.node_id = store.store(&idea).expect("store New");
        if status != IdeaStatus::New {
            idea.try_transition(status).expect("transition");
            store.update(&idea).expect("update");
        }
    }

    for &status in &targets {
        let hits = store.list_by_status(status, u32::MAX).expect("by status");
        assert_eq!(hits.len(), 1, "exactly one idea in status {status}");
        assert_eq!(hits[0].status, status);
    }
    // No duplicate rows leak through despite the append-only updates.
    assert_eq!(store.list(u32::MAX).expect("list").len(), targets.len());
}

#[test]
fn wired_thread_tick_generates_reviews_and_routes() {
    let env = TickEnv::new();
    let goal_store = std::sync::Arc::new(InMemoryGoalStore::try_default().expect("goals"));
    let gh = SharedGh::default();

    let raws: Vec<RawIdea> = (0..10)
        .map(|i| RawIdea {
            idea: format!("wired idea {i}"),
            links: vec![MemoryLink {
                kind: MemoryLinkKind::Semantic,
                node_id: format!("s{i}"),
            }],
            rationale: format!("rationale {i}"),
        })
        .collect();

    let pipeline = AgenticIdeaPipeline::new(
        Box::new(CannedInvoker {
            response: support_metric_response(),
        }),
        Box::new(gh.clone()),
        Box::new(SharedGoalStoreFactory(std::sync::Arc::clone(&goal_store))),
        "rysweet/Simard".to_string(),
    );
    let cfg = CreativeIdeasConfig {
        enabled: true,
        batch: 10,
        ..CreativeIdeasConfig::default()
    };
    let mut thread = CreativeIdeasThread::with_pipeline(
        cfg,
        Box::new(FakeIdeaSource::with_ideas(raws)),
        Box::new(pipeline),
    );

    let mut ctx = env.ctx(2_000, /* dry_run */ false);
    let outcome = thread.tick(&mut ctx);
    assert!(outcome.ran && outcome.success, "{}", outcome.summary);

    // All ten accepted ideas became Proposed goals on the board.
    let goals = goal_store.list().expect("goals");
    assert_eq!(goals.len(), 10, "each accepted idea created a goal");
    assert!(goals.iter().all(|g| g.status == GoalStatus::Proposed));

    // The ideas are now searchable at their post-review status.
    let store = ProspectiveCreativeIdeaStore::new(&env.mem);
    let started = store
        .list_by_status(IdeaStatus::ImplementationStarted, u32::MAX)
        .expect("by status");
    assert_eq!(
        started.len(),
        10,
        "all ten routed to a goal and moved in-flight"
    );
    for idea in &started {
        assert_eq!(idea.reviews.len(), 3);
    }
}

// ---------------------------------------------------------------------------
// 13. Durability — persisted ideas survive a non-graceful restart (#2798)
// ---------------------------------------------------------------------------

/// **Durability regression (#2798).** Disproves the "creative-idea prospective
/// writes are buffer-only, lost on SIGKILL unless the thread checkpoints each
/// batch" hypothesis: a real `CreativeIdeasThread::tick` batch survives a
/// non-graceful restart with no explicit checkpoint, because the pinned engine's
/// WAL is write-through and replayed on open. It drives the tick against an
/// on-disk store, `std::mem::forget`s the handle to skip the graceful-`Drop`
/// checkpoint, cold-reopens from disk, and asserts the ideas are still listable.
/// A RED here means engine WAL replay regressed — an `amplihack-memory-lib` fix
/// (G2), not a Simard-side checkpoint.
#[test]
#[serial_test::serial(cognitive_memory)]
fn tick_persisted_ideas_survive_nongraceful_restart_via_engine_wal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");
    let shutdown = AtomicBool::new(false);

    let raws: Vec<RawIdea> = (0..3)
        .map(|i| RawIdea {
            idea: format!("durable self-improvement idea {i}"),
            links: vec![MemoryLink {
                kind: MemoryLinkKind::Goal,
                node_id: format!("g{i}"),
            }],
            rationale: format!("grounded rationale {i}"),
        })
        .collect();
    let expected = raws.len();

    {
        let mem = LibraryCognitiveMemory::open(&root).expect("open on-disk store");
        let cfg = CreativeIdeasConfig {
            enabled: true,
            batch: expected,
            ..CreativeIdeasConfig::default()
        };
        let mut thread = CreativeIdeasThread::with_pipeline(
            cfg,
            Box::new(FakeIdeaSource::with_ideas(raws)),
            Box::new(NoopPipeline),
        );
        let mut ctx = ThreadContext {
            state_root: &root,
            repo_root: &root,
            memory: &mem as &dyn CognitiveMemoryOps,
            runtime: rt.handle().clone(),
            shutdown: &shutdown,
            now_epoch: 1_000,
            dry_run: false,
        };
        let outcome = thread.tick(&mut ctx);
        assert!(
            outcome.ran && outcome.success,
            "tick must persist the batch: {}",
            outcome.summary
        );

        // Simulate a SIGKILL-during-deploy: skip the graceful `Drop` (and its
        // implicit checkpoint) entirely. Durability must come from the engine's
        // write-through WAL, NOT from any checkpoint.
        std::mem::forget(mem);
    }

    // Force a genuine cold reopen from disk (drop any shared cached handle and
    // reap the stale open-lock left by the forgotten writer).
    crate::memory_ipc::clear_tier2_store_cache();
    let _ = crate::memory_ipc::reap_stale_open_lock(&root);

    let reopened = LibraryCognitiveMemory::open(&root).expect("cold reopen after restart");
    let ideas = ProspectiveCreativeIdeaStore::new(&reopened)
        .list(u32::MAX)
        .expect("list creative ideas after restart");
    assert_eq!(
        ideas.len(),
        expected,
        "every persisted creative idea must survive a non-graceful daemon restart \
         through the engine's write-through WAL (no explicit checkpoint): persist \
         -> SIGKILL -> reopen -> list is non-empty (#2798); got {}",
        ideas.len()
    );
}
