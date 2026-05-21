//! Integration tests for the hallucinated-progress meta-bug fix
//! (issue #1967, design.md §5.2 I1–I5).
//!
//! These tests exercise the full path from a `GoalBoard` mutation request
//! through the new gatekeeper façade
//! `update_goal_progress_with_evidence`, asserting on the observable
//! contract: board mutation only happens with evidence, and rejected
//! claims surface as `"brain hallucination detected: …"` episodes that
//! the dashboard can find by content-substring search.
//!
//! Unlike `src/goal_curation/tests_progress_evidence.rs` (which mocks
//! the runner traits and tests the checker in isolation), these tests
//! exercise the public re-exports from `simard::goal_curation` so they
//! pin the end-to-end contract used by the OODA loop in production.
//!
//! These tests MUST FAIL on `f4cd5d69` — the symbols don't exist yet —
//! and are the integration-level acceptance gate for the fix.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use simard::cognitive_memory::CognitiveMemoryOps;
use simard::error::SimardResult;
use simard::goal_curation::progress_evidence::{
    DefaultProgressEvidenceChecker, EvidenceDecision, GhPr, GhRunner, GitRunner,
    NoopProgressEvidenceChecker,
};
use simard::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, WipRef, add_active_goal, update_goal_progress,
    update_goal_progress_with_evidence,
};
use simard::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

// ─── shared helpers (duplicated from the unit-test file because tests/
//     compiles as a separate crate and can't reach into pub(crate) items) ──

const GOAL_ID: &str = "enhance-simard-meeting-experience";

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 21, 4, 0, 0).unwrap()
}

fn seed_goal(board: &mut GoalBoard, status: GoalProgress, last_update: Option<DateTime<Utc>>) {
    add_active_goal(
        board,
        ActiveGoal {
            id: GOAL_ID.to_string(),
            description: "integration-test goal".to_string(),
            priority: 1,
            status,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![WipRef {
                kind: "issue".to_string(),
                ref_id: "1951".to_string(),
                label: "meeting-experience epic".to_string(),
                url: None,
            }],
            last_progress_update_at: last_update,
        },
    )
    .expect("seed goal");
}

// ─── RecordingMemory — mirrors the unit-test fake but lives in tests/ crate ──

#[derive(Default)]
struct RecordingMemory {
    episodes: Mutex<Vec<RecordedEpisode>>,
}

#[derive(Clone, Debug)]
struct RecordedEpisode {
    content: String,
    #[allow(dead_code)]
    source_label: String,
}

impl RecordingMemory {
    fn new() -> Self {
        Self::default()
    }
    fn episodes(&self) -> Vec<RecordedEpisode> {
        self.episodes.lock().unwrap().clone()
    }
    fn contents(&self) -> Vec<String> {
        self.episodes().into_iter().map(|e| e.content).collect()
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
        _metadata: Option<&Value>,
    ) -> SimardResult<String> {
        self.episodes.lock().unwrap().push(RecordedEpisode {
            content: content.to_string(),
            source_label: source_label.to_string(),
        });
        Ok("epi_x".into())
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
}

// ─── Fake runners ───────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeGit {
    branches: Mutex<Vec<String>>,
    commits: Mutex<Vec<(String, DateTime<Utc>, String)>>,
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
}
impl GitRunner for FakeGit {
    fn list_branches(&self, _root: &Path, pattern: &str) -> std::io::Result<Vec<String>> {
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
#[allow(dead_code)]
impl FakeGh {
    fn new() -> Self {
        Self::default()
    }
    fn add_pr(&self, pr: GhPr) {
        self.prs.lock().unwrap().push(pr);
    }
}
impl GhRunner for FakeGh {
    fn search_prs(&self, _slug: &str, _q: &str) -> std::io::Result<Vec<GhPr>> {
        Ok(self.prs.lock().unwrap().clone())
    }
}

fn checker(git: Arc<FakeGit>, gh: Arc<FakeGh>) -> DefaultProgressEvidenceChecker {
    DefaultProgressEvidenceChecker {
        repo_root: PathBuf::from("/tmp/fake-repo"),
        remote_slug: "rysweet/Simard".to_string(),
        git,
        gh,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// I1 — no engineer activity ⇒ percent stays, alert visible
//
// Models the live-production observation from issue #1967: the OODA loop
// proposes a 35→75 jump on `enhance-simard-meeting-experience` with zero
// commits, zero PRs since the last update. The gate must reject and
// record one hallucination episode.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i1_no_engineer_activity_rejects_progress_and_emits_alert() {
    let mut board = GoalBoard::new();
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 35 },
        Some(fixed_now() - Duration::hours(28)),
    );
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let gh = Arc::new(FakeGh::new()); // empty — no PRs at all
    let ch = checker(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 75 },
        &ch,
        &memory,
        fixed_now(),
    )
    .expect("Reject must surface as Ok, not Err");

    assert!(
        matches!(decision, EvidenceDecision::Reject { .. }),
        "I1: zero-activity progress claim must be Rejected; got {decision:?}"
    );

    let goal = board
        .active
        .iter()
        .find(|g| g.id == GOAL_ID)
        .expect("goal still on board after Reject");
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 35 },
        "I1: rejected claim must NOT mutate the percent — it must stay at 35"
    );

    let contents = memory.contents();
    let alerts: Vec<&String> = contents
        .iter()
        .filter(|c| c.starts_with("brain hallucination detected:"))
        .collect();
    assert_eq!(
        alerts.len(),
        1,
        "I1: exactly one hallucination alert must be emitted; got {contents:?}"
    );
    assert!(
        alerts[0].contains("35") && alerts[0].contains("75"),
        "I1: alert must include both old and new percent so operators can triage; got: {}",
        alerts[0]
    );
    assert!(
        alerts[0].contains(GOAL_ID),
        "I1: alert must include the goal id; got: {}",
        alerts[0]
    );
}

// ───────────────────────────────────────────────────────────────────────────
// I2 — real commit on engineer branch ⇒ percent advances, accept episode
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i2_real_commit_on_engineer_branch_accepts_and_advances() {
    let mut board = GoalBoard::new();
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 35 },
        Some(fixed_now() - Duration::hours(2)),
    );
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let branch = format!("engineer/{GOAL_ID}-pid7777");
    git.add_branch(&branch);
    git.add_commit(&branch, fixed_now() - Duration::minutes(10), "deadbeefcafe");
    let gh = Arc::new(FakeGh::new());
    let ch = checker(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 75 },
        &ch,
        &memory,
        fixed_now(),
    )
    .expect("Accept must not error");
    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "I2: real commit must produce Accept; got {decision:?}"
    );

    let goal = board.active.iter().find(|g| g.id == GOAL_ID).unwrap();
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 75 },
        "I2: Accept must persist the new percent"
    );
    assert_eq!(
        goal.last_progress_update_at,
        Some(fixed_now()),
        "I2: Accept must stamp last_progress_update_at"
    );

    let contents = memory.contents();
    assert_eq!(
        contents
            .iter()
            .filter(|c| c.starts_with("goal progress accepted:"))
            .count(),
        1,
        "I2: exactly one 'goal progress accepted:' episode; got {contents:?}"
    );
    assert!(
        contents
            .iter()
            .all(|c| !c.starts_with("brain hallucination detected:")),
        "I2: no hallucination alert should be emitted on Accept; got {contents:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// I3 — subordinate heartbeat with zero commits ⇒ stays at prior percent
//
// Models the `advance_goal/subordinate.rs:56` hallucination site: engineer
// is alive but has produced nothing. The 50% heartbeat bump must be
// rejected if it would be an *increase* over the current percent.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i3_subordinate_heartbeat_without_commits_does_not_bump_percent() {
    let mut board = GoalBoard::new();
    // Engineer was already at 60%; heartbeat would propose 50% (no-op or
    // decrease) OR would propose 50% from a base of 10% (an increase).
    // We exercise the increase case here because that is the hallucination.
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 10 },
        Some(fixed_now() - Duration::hours(1)),
    );
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new()); // empty — engineer hasn't committed
    let gh = Arc::new(FakeGh::new());
    let ch = checker(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &ch,
        &memory,
        fixed_now(),
    )
    .expect("Reject is Ok");

    assert!(
        matches!(decision, EvidenceDecision::Reject { .. }),
        "I3: heartbeat without commits must Reject the bump; got {decision:?}"
    );
    let goal = board.active.iter().find(|g| g.id == GOAL_ID).unwrap();
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 10 },
        "I3: percent must stay at the prior value when engineer has no commits"
    );
    assert!(
        memory
            .contents()
            .iter()
            .any(|c| c.starts_with("brain hallucination detected:")),
        "I3: hallucination alert must be emitted"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// I4 — subordinate heartbeat WITH a commit on its branch ⇒ percent advances
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i4_subordinate_heartbeat_with_commit_advances_to_fifty_percent() {
    let mut board = GoalBoard::new();
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 10 },
        Some(fixed_now() - Duration::hours(1)),
    );
    let memory = RecordingMemory::new();
    let git = Arc::new(FakeGit::new());
    let branch = format!("engineer/{GOAL_ID}-pid4242");
    git.add_branch(&branch);
    git.add_commit(&branch, fixed_now() - Duration::minutes(5), "feedfacef00d");
    let gh = Arc::new(FakeGh::new());
    let ch = checker(git, gh);

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 50 },
        &ch,
        &memory,
        fixed_now(),
    )
    .expect("Accept is Ok");
    assert!(
        matches!(decision, EvidenceDecision::Accept { .. }),
        "I4: heartbeat with engineer-branch commit must Accept; got {decision:?}"
    );

    let goal = board.active.iter().find(|g| g.id == GOAL_ID).unwrap();
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 50 },
        "I4: percent must advance to 50 with evidence"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// I5 — dashboard PUT path bypasses the gate (operator override is intentional)
//
// The dashboard's PUT /api/goals/<id>/progress handler calls the
// low-level `update_goal_progress` directly. That direct call is the
// intentional operator-override escape hatch documented in design.md
// §3.4 and §0.1; this test pins that contract so a future regression
// can't quietly route the dashboard through the façade.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i5_low_level_update_goal_progress_still_works_unchanged() {
    let mut board = GoalBoard::new();
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 20 },
        Some(fixed_now() - Duration::hours(1)),
    );

    // No checker, no memory — the low-level function is the unchanged
    // pre-#1967 writer used by the dashboard.
    update_goal_progress(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 90 },
    )
    .expect("dashboard PUT path must still succeed unconditionally");

    let goal = board.active.iter().find(|g| g.id == GOAL_ID).unwrap();
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 90 },
        "I5: low-level updater (dashboard operator override) must still write \
         without any evidence check — that escape hatch is intentional"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// I6 (kill-switch contract) — NoopProgressEvidenceChecker via the façade
//
// Pins design.md §6 rollout: `SIMARD_PROGRESS_EVIDENCE=off` swaps in the
// NoopChecker; the façade still works but every claim is Accepted with
// a clearly-marked "noop" reason so the audit trail records the bypass.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn i6_kill_switch_noop_checker_accepts_all_via_facade() {
    let mut board = GoalBoard::new();
    seed_goal(
        &mut board,
        GoalProgress::InProgress { percent: 10 },
        Some(fixed_now() - Duration::hours(1)),
    );
    let memory = RecordingMemory::new();
    let ch = NoopProgressEvidenceChecker;

    let decision = update_goal_progress_with_evidence(
        &mut board,
        GOAL_ID,
        GoalProgress::InProgress { percent: 80 },
        &ch,
        &memory,
        fixed_now(),
    )
    .expect("Noop checker must never error");

    match decision {
        EvidenceDecision::Accept { reason } => assert!(
            reason.to_ascii_lowercase().contains("noop"),
            "I6: kill-switch path must mark its Accept reason 'noop' for auditability; got: {reason}"
        ),
        other => panic!("I6: NoopChecker via façade must Accept; got {other:?}"),
    }

    let goal = board.active.iter().find(|g| g.id == GOAL_ID).unwrap();
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 80 },
        "I6: kill-switch must let the percent through"
    );
}
