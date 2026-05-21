//! TDD-first unit tests for the hallucinated-progress meta-bug fix
//! (issue #1967).
//!
//! These tests pin the contract of the new
//! `goal_curation::progress_evidence` module and the gatekeeper façade
//! `goal_curation::operations::update_goal_progress_with_evidence` as
//! specified in `design.md` §2 and §5.1 (U1–U12). They MUST FAIL on
//! `f4cd5d69` because neither symbol exists yet — failure is the
//! definition of done for this TDD step.
//!
//! Pattern: each scenario builds a goal board with one active goal,
//! injects fake `GitRunner` / `GhRunner` impls into a
//! `DefaultProgressEvidenceChecker` (or uses `NoopProgressEvidenceChecker`
//! for the bypass cases), routes a proposed progress mutation through
//! `update_goal_progress_with_evidence`, and asserts on the returned
//! `EvidenceDecision` plus the board / memory side-effects.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::operations::{add_active_goal, update_goal_progress_with_evidence};
use super::progress_evidence::{
    DefaultProgressEvidenceChecker, EvidenceDecision, GhPr, GhRunner, GitRunner,
    NoopProgressEvidenceChecker, ProgressEvidenceChecker,
};
use super::types::{ActiveGoal, GoalBoard, GoalProgress, WipRef};

// ─── helpers ────────────────────────────────────────────────────────────────

const GOAL_ID: &str = "improve-cognitive-memory-persistence";

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 21, 3, 30, 0).unwrap()
}

fn since_one_hour_ago() -> DateTime<Utc> {
    fixed_now() - Duration::hours(1)
}

fn make_goal_with_status(status: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        id: GOAL_ID.to_string(),
        description: "test goal".to_string(),
        priority: 1,
        status,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![WipRef {
            kind: "issue".to_string(),
            ref_id: "1967".to_string(),
            label: "meta-bug umbrella".to_string(),
            url: None,
        }],
        last_progress_update_at: Some(since_one_hour_ago()),
    }
}

fn make_board_with_goal(status: GoalProgress) -> GoalBoard {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal_with_status(status)).expect("seed goal");
    board
}

fn lookup_goal<'b>(board: &'b GoalBoard, id: &str) -> &'b ActiveGoal {
    board
        .active
        .iter()
        .find(|g| g.id == id)
        .expect("goal must exist on board after operation")
}

// ─── fake CognitiveMemoryOps that records every store_episode call ──────────

#[derive(Default)]
struct RecordingMemory {
    episodes: Mutex<Vec<RecordedEpisode>>,
    /// Optional pre-seeded "goal progress accepted:" episodes — drives the
    /// `since`-fallback test (U11).
    seeded_search_hits: Mutex<Vec<(String, DateTime<Utc>)>>,
}

#[derive(Clone, Debug)]
struct RecordedEpisode {
    content: String,
    #[allow(dead_code)]
    source_label: String,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

impl RecordingMemory {
    fn new() -> Self {
        Self::default()
    }

    fn seed_prior_accept(&self, goal_id: &str, at: DateTime<Utc>) {
        self.seeded_search_hits
            .lock()
            .unwrap()
            .push((goal_id.to_string(), at));
    }

    fn episodes(&self) -> Vec<RecordedEpisode> {
        self.episodes.lock().unwrap().clone()
    }
}

impl CognitiveMemoryOps for RecordingMemory {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen_0".into())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("w_0".into())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&Value>,
    ) -> SimardResult<String> {
        self.episodes.lock().unwrap().push(RecordedEpisode {
            content: content.to_string(),
            source_label: source_label.to_string(),
            metadata: metadata.cloned(),
        });
        Ok(format!("epi_{}", self.episodes.lock().unwrap().len()))
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _c: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        Ok("f_0".into())
    }
    fn search_facts(
        &self,
        _q: &str,
        _limit: u32,
        _min_conf: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("p_0".into())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pr_0".into())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics {
            sensory_count: 0,
            working_count: 0,
            episodic_count: self.episodes.lock().unwrap().len() as u64,
            semantic_count: 0,
            procedural_count: 0,
            prospective_count: 0,
        })
    }
    fn search_episodes_starting_with(
        &self,
        _prefix: &str,
        _limit: u32,
    ) -> SimardResult<Vec<(String, DateTime<Utc>)>> {
        // Tests seed `(goal_id, at)` pairs via `seed_prior_accept`. The
        // façade later filters by `content.contains(goal_id)` so we return
        // the seeded set verbatim.
        Ok(self.seeded_search_hits.lock().unwrap().clone())
    }
}

// ─── fake GitRunner / GhRunner ──────────────────────────────────────────────

#[derive(Default)]
struct FakeGit {
    branches: Mutex<Vec<String>>,
    commits: Mutex<Vec<(String, DateTime<Utc>, String)>>, // (branch, when, sha)
    /// Captures the `since` argument the checker passed to `commits_since`
    /// — drives U11 / U12 (sourcing of `since`).
    last_since: Mutex<Option<DateTime<Utc>>>,
}

impl FakeGit {
    fn new() -> Self {
        Self::default()
    }
    fn add_branch(&self, name: &str) {
        self.branches.lock().unwrap().push(name.to_string());
    }
    fn add_commit(&self, branch: &str, at: DateTime<Utc>, sha: &str) {
        self.commits
            .lock()
            .unwrap()
            .push((branch.to_string(), at, sha.to_string()));
    }
    fn last_since(&self) -> Option<DateTime<Utc>> {
        *self.last_since.lock().unwrap()
    }
}

impl GitRunner for FakeGit {
    fn list_branches(&self, _root: &Path, pattern: &str) -> std::io::Result<Vec<String>> {
        // Strip trailing `*` glob and match by prefix.
        let prefix = pattern.trim_end_matches('*');
        Ok(self
            .branches
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.starts_with(prefix))
            .cloned()
            .collect())
    }
    fn commits_since(
        &self,
        _root: &Path,
        branch: &str,
        since: DateTime<Utc>,
    ) -> std::io::Result<Vec<String>> {
        *self.last_since.lock().unwrap() = Some(since);
        Ok(self
            .commits
            .lock()
            .unwrap()
            .iter()
            .filter(|(b, when, _)| b == branch && *when >= since)
            .map(|(_, _, sha)| sha.clone())
            .collect())
    }
}

#[derive(Default)]
struct FakeGh {
    prs: Mutex<Vec<GhPr>>,
}

impl FakeGh {
    fn new() -> Self {
        Self::default()
    }
    fn add_pr(&self, pr: GhPr) {
        self.prs.lock().unwrap().push(pr);
    }
}

impl GhRunner for FakeGh {
    fn search_prs(&self, _repo_slug: &str, _query: &str) -> std::io::Result<Vec<GhPr>> {
        // Tests construct the FakeGh with the PR set the checker is meant
        // to see; the checker filters by title/body match itself, so we
        // return the full set here.
        Ok(self.prs.lock().unwrap().clone())
    }
}

fn make_pr(
    number: u64,
    title: &str,
    body: &str,
    state: &str,
    created_at: DateTime<Utc>,
    merged_at: Option<DateTime<Utc>>,
) -> GhPr {
    GhPr {
        number,
        title: title.to_string(),
        body: Some(body.to_string()),
        state: state.to_string(),
        created_at,
        merged_at,
    }
}

fn checker_with(git: Arc<FakeGit>, gh: Arc<FakeGh>) -> DefaultProgressEvidenceChecker {
    DefaultProgressEvidenceChecker {
        repo_root: PathBuf::from("/tmp/fake-repo"),
        remote_slug: "rysweet/Simard".to_string(),
        git,
        gh,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// U1 — Non-increase bypass (decrease)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u1_non_increase_decrease_bypasses_checker_and_writes_board() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 50 });
    let memory = RecordingMemory::new();
    let checker = NoopProgressEvidenceChecker; // bypass means checker is irrelevant

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 30 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("bypass call must not error");

    match decision {
        EvidenceDecision::Accept { reason } => assert!(
            reason.contains("bypass"),
            "decrease must be reported as bypass; reason={reason}"
        ),
        other => panic!("expected Accept(bypass), got {other:?}"),
    }

    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::InProgress { percent: 30 },
        "decrease must still be persisted to the board"
    );
    assert!(
        memory.episodes().is_empty(),
        "bypass path must NOT write any audit episode; got {:?}",
        memory.episodes()
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U2 — Blocked bypass
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u2_blocked_proposal_bypasses_checker() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 40 });
    let memory = RecordingMemory::new();
    let checker = NoopProgressEvidenceChecker;

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::Blocked("waiting on upstream".to_string()),
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("blocked is a non-progress transition; must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { ref reason } if reason.contains("bypass")),
        "Blocked must be Accept(bypass); got {decision:?}"
    );
    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::Blocked("waiting on upstream".to_string()),
        "Blocked status must be persisted"
    );
    assert!(memory.episodes().is_empty(), "no audit episode on bypass");
}

// ───────────────────────────────────────────────────────────────────────────
// U3 — NotStarted bypass (reset path)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u3_notstarted_proposal_bypasses_checker() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 60 });
    let memory = RecordingMemory::new();
    let checker = NoopProgressEvidenceChecker;

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::NotStarted,
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("NotStarted reset must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { ref reason } if reason.contains("bypass")),
        "NotStarted must be Accept(bypass); got {decision:?}"
    );
    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::NotStarted,
        "reset to NotStarted must be persisted"
    );
    assert!(memory.episodes().is_empty(), "no audit episode on bypass");
}

// ───────────────────────────────────────────────────────────────────────────
// U4 — Reject when no commits and no PRs exist
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u4_reject_when_no_git_evidence_and_no_pr_evidence() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    let checker = checker_with(git.clone(), gh.clone());

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Reject must surface as Ok(Reject), not Err");

    match decision {
        EvidenceDecision::Reject { reason } => {
            assert!(
                !reason.is_empty(),
                "reject reason must be non-empty so operators can triage"
            );
        }
        other => panic!("expected Reject, got {other:?}"),
    }

    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::InProgress { percent: 20 },
        "board must NOT be mutated on Reject — prior percent stays"
    );

    let eps = memory.episodes();
    assert_eq!(
        eps.len(),
        1,
        "exactly one audit episode must be emitted on Reject; got {eps:?}"
    );
    let content = &eps[0].content;
    assert!(
        content.starts_with("brain hallucination detected:"),
        "episode must start with the exact-match alert prefix; got: {content}"
    );
    assert!(
        content.contains("20") && content.contains("50"),
        "alert must mention both the old and new percent; got: {content}"
    );
    assert!(
        content.contains(GOAL_ID),
        "alert must mention the goal id; got: {content}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U5 — Accept on engineer-branch commit (rule 1)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u5_accept_on_engineer_branch_commit_after_since() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let branch = format!("engineer/{GOAL_ID}-pid1234");
    git.add_branch(&branch);
    git.add_commit(
        &branch,
        fixed_now() - Duration::minutes(10), // strictly after since (=1h ago)
        "abcdef1234567",
    );
    let gh = Arc::new(FakeGh::new());
    let checker = checker_with(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Accept path must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "expected Accept on engineer-branch commit; got {decision:?}"
    );

    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::InProgress { percent: 50 },
        "Accept must persist the new percent"
    );

    let eps = memory.episodes();
    assert_eq!(eps.len(), 1, "exactly one audit episode on Accept");
    assert!(
        eps[0].content.starts_with("goal progress accepted:"),
        "Accept audit episode must use the 'goal progress accepted:' prefix; got: {}",
        eps[0].content
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U6 — Accept on PR with goal slug in title (rule 2)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u6_accept_on_pr_with_goal_slug_in_title() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    gh.add_pr(make_pr(
        1969,
        &format!("WIP: {GOAL_ID} — checkpoint progress"),
        "no body",
        "OPEN",
        fixed_now() - Duration::minutes(15),
        None,
    ));
    let checker = checker_with(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Accept path must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "expected Accept on PR title containing goal slug; got {decision:?}"
    );
    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::InProgress { percent: 50 },
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U7 — Accept on PR body referencing a wip_refs issue (rule 2)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u7_accept_on_pr_body_referencing_wip_ref_issue() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    gh.add_pr(make_pr(
        2001,
        "unrelated title",
        "This PR addresses some of the work in #1967 by ...",
        "OPEN",
        fixed_now() - Duration::minutes(5),
        None,
    ));
    let checker = checker_with(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Accept path must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "expected Accept when PR body references wip_refs issue #1967; got {decision:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U8 — Accept on merged PR that closes a wip_refs issue (rule 3)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u8_accept_on_merged_pr_closing_wip_ref_issue() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    gh.add_pr(make_pr(
        2050,
        "Fix the meta-bug",
        "Fixes #1967\n\nDetails: gates progress on git evidence.",
        "MERGED",
        fixed_now() - Duration::hours(2),
        Some(fixed_now() - Duration::minutes(20)),
    ));
    let checker = checker_with(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::Completed,
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Accept path must not error");

    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "expected Accept when merged PR closes wip_refs issue #1967; got {decision:?}"
    );
    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::Completed,
        "Completed must be persisted on Accept"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U9 — Reject when an old PR exists (createdAt < since, no commits)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u9_reject_when_only_pr_evidence_is_older_than_since() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());

    // PR mentions the wip_ref issue but was created BEFORE `since` and
    // never merged → does not count as evidence for the current update.
    gh.add_pr(make_pr(
        1000,
        "old WIP work",
        "touches #1967",
        "OPEN",
        fixed_now() - Duration::hours(48),
        None,
    ));
    let checker = checker_with(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    )
    .expect("Reject is Ok(Reject), not Err");

    assert!(
        matches!(decision, EvidenceDecision::Reject { .. }),
        "expected Reject when only stale PR evidence exists; got {decision:?}"
    );
    assert_eq!(
        lookup_goal(&board, GOAL_ID).status,
        GoalProgress::InProgress { percent: 20 },
        "board must not mutate on Reject"
    );
    let eps = memory.episodes();
    assert_eq!(eps.len(), 1);
    assert!(eps[0].content.starts_with("brain hallucination detected:"));
}

// ───────────────────────────────────────────────────────────────────────────
// U10 — Accept sets `last_progress_update_at = Some(now)`
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u10_accept_sets_last_progress_update_at_to_now() {
    let mut board = make_board_with_goal(GoalProgress::InProgress { percent: 20 });
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let branch = format!("engineer/{GOAL_ID}-pid9999");
    git.add_branch(&branch);
    git.add_commit(&branch, fixed_now() - Duration::minutes(5), "shashasha");
    let gh = Arc::new(FakeGh::new());
    let checker = checker_with(git, gh);

    let now = fixed_now();
    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        now,
    )
    .expect("Accept must not error");
    assert!(matches!(decision, EvidenceDecision::Accept { .. }));

    let goal = lookup_goal(&board, GOAL_ID);
    assert_eq!(
        goal.last_progress_update_at,
        Some(now),
        "Accept must stamp `last_progress_update_at` to `now`"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U11 — `since` falls back to memory-scan when goal field is None
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u11_since_falls_back_to_memory_scan_for_prior_accept_episode() {
    let mut board = GoalBoard::new();
    let mut goal = make_goal_with_status(GoalProgress::InProgress { percent: 20 });
    // Legacy on-disk board: no `last_progress_update_at` yet.
    goal.last_progress_update_at = None;
    add_active_goal(&mut board, goal).unwrap();

    let memory = RecordingMemory::new();
    let prior_accept = fixed_now() - Duration::hours(6);
    memory.seed_prior_accept(GOAL_ID, prior_accept);

    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    // No evidence; we don't care about the decision, only the `since` value.
    let checker = checker_with(git.clone(), gh);

    let _ = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    );

    let observed = git
        .last_since()
        .expect("checker must have called commits_since at least once");
    assert_eq!(
        observed, prior_accept,
        "with no `last_progress_update_at` and a memory hit, `since` must equal the prior-accept timestamp"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// U12 — `since` falls back to process-start when neither field nor memory exists
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn u12_since_falls_back_to_process_start_when_no_other_signal() {
    let mut board = GoalBoard::new();
    let mut goal = make_goal_with_status(GoalProgress::InProgress { percent: 20 });
    goal.last_progress_update_at = None;
    add_active_goal(&mut board, goal).unwrap();

    let memory = RecordingMemory::new(); // no seeded prior-accept

    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new());
    let checker = checker_with(git.clone(), gh);

    let _ = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &checker,
        &memory,
        fixed_now(),
    );

    let observed = git
        .last_since()
        .expect("checker must have called commits_since");
    // Process start is established at first call to the cached `OnceLock`
    // — `observed` must be a valid timestamp at-or-before `fixed_now`.
    assert!(
        observed <= fixed_now(),
        "process-start fallback must be <= now; observed={observed}"
    );
    // And it must not be a sentinel like the unix epoch.
    let epoch = Utc.timestamp_opt(0, 0).unwrap();
    assert!(
        observed > epoch,
        "process-start fallback must be a real timestamp, not epoch 0; observed={observed}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Extra: NoopProgressEvidenceChecker always accepts (kill-switch contract)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn noop_checker_always_accepts_with_explicit_noop_reason() {
    let goal = make_goal_with_status(GoalProgress::InProgress { percent: 20 });
    let checker = NoopProgressEvidenceChecker;
    let decision = checker.check(&goal, 20, 50, fixed_now());
    match decision {
        EvidenceDecision::Accept { reason } => assert!(
            reason.to_ascii_lowercase().contains("noop"),
            "NoopChecker's Accept reason must mention 'noop' so the audit \
             trail can tell apart real evidence from the kill-switch; got: {reason}"
        ),
        other => panic!("NoopChecker must Accept unconditionally; got {other:?}"),
    }
}
