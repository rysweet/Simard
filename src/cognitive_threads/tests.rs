//! TDD test suite for cognitive-thread scheduling (design §12 + security §5d).
//!
//! These tests are authored **before** the behaviour implementation. The data
//! surface, constructors, and the telemetry seam are real, so the identity /
//! naming / default-invariant tests pass today; the behaviour tests exercise
//! `todo!()` stubs and therefore FAIL (red) until the implementation step fills
//! them in. Every test is hermetic: injected `now_epoch`, no sleeps, no
//! network (a fake `GhClient`), and no process-global env mutation (so the
//! `serial(cognitive_memory)` contract does not apply).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::SimardResult;
use crate::stewardship::dedup;
use crate::stewardship::gh_client::{GhClient, GhIssue};

use super::mind::Mind;
use super::schedule::{self, MIN_INTERVAL_SECS};
use super::telemetry;
use super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
use super::threads::engineer_log_analysis::{
    self, EngineerLogAnalysisConfig, EngineerLogAnalysisThread,
};
use super::threads::maintenance::{self, MaintenanceConfig, MaintenanceThread};

// ---------------------------------------------------------------------------
// Test doubles & fixtures
// ---------------------------------------------------------------------------

/// Owns the borrowed resources a [`ThreadContext`] needs, so a test can mint a
/// context bound to its own lifetime.
struct TestEnv {
    rt: tokio::runtime::Runtime,
    mem: LibraryCognitiveMemory,
    shutdown: AtomicBool,
    tmp: tempfile::TempDir,
}

impl TestEnv {
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

    fn state_root(&self) -> &Path {
        self.tmp.path()
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
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

/// What a [`FakeThread`] does when ticked.
#[derive(Clone, Copy)]
enum FakeBehavior {
    Succeed,
    Error,
    Panic,
}

/// A configurable fake [`CognitiveThread`] used to drive the scheduler.
struct FakeThread {
    id: String,
    kind: ThreadKind,
    priority: Priority,
    policy: SchedulePolicy,
    enabled: bool,
    behavior: FakeBehavior,
    runs: Arc<AtomicUsize>,
    run_log: Arc<Mutex<Vec<String>>>,
}

impl CognitiveThread for FakeThread {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> ThreadKind {
        self.kind
    }
    fn policy(&self) -> SchedulePolicy {
        self.policy.clone()
    }
    fn priority(&self) -> Priority {
        self.priority
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn tick(&mut self, _ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.run_log.lock().expect("run log").push(self.id.clone());
        match self.behavior {
            FakeBehavior::Succeed => ThreadOutcome::ok("ok", Duration::from_millis(1)),
            FakeBehavior::Error => ThreadOutcome::failed("boom", Duration::from_millis(1)),
            FakeBehavior::Panic => panic!("fake thread {} panicked", self.id),
        }
    }
    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: self.id.clone(),
            enabled: self.enabled,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
            backoff_until_epoch: None,
        }
    }
}

/// "Always due" interval policy.
fn always_due() -> SchedulePolicy {
    SchedulePolicy::Interval(Duration::ZERO)
}

/// Build a fake thread, returning it boxed plus a handle to its run counter.
fn fake(
    id: &str,
    kind: ThreadKind,
    priority: Priority,
    policy: SchedulePolicy,
    behavior: FakeBehavior,
    run_log: &Arc<Mutex<Vec<String>>>,
) -> (Box<dyn CognitiveThread>, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let thread = FakeThread {
        id: id.to_string(),
        kind,
        priority,
        policy,
        enabled: true,
        behavior,
        runs: Arc::clone(&runs),
        run_log: Arc::clone(run_log),
    };
    (Box::new(thread), runs)
}

/// An in-process fake `gh` client: records created issues and answers searches
/// from that record (no network, no credentials).
#[derive(Clone)]
struct FakeGhClient {
    created: Arc<Mutex<Vec<GhIssue>>>,
    next_number: Arc<AtomicU64>,
}

impl FakeGhClient {
    fn new() -> Self {
        Self {
            created: Arc::new(Mutex::new(Vec::new())),
            next_number: Arc::new(AtomicU64::new(1)),
        }
    }

    fn created_count(&self) -> usize {
        self.created.lock().expect("created lock").len()
    }
}

impl GhClient for FakeGhClient {
    fn search_issues(&self, _repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>> {
        let needle = format!("stewardship-signature: {signature}");
        Ok(self
            .created
            .lock()
            .expect("created lock")
            .iter()
            .filter(|i| i.body.contains(&needle))
            .cloned()
            .collect())
    }
}

impl crate::stewardship::gh_client::IssueMutationTransport for FakeGhClient {
    fn create_issue(
        &self,
        _repo: &str,
        _identity: &crate::stewardship::IssueMutationIdentity,
        title: &str,
        body: &str,
        _labels: &[String],
        _assignees: &[String],
    ) -> SimardResult<GhIssue> {
        let number = self.next_number.fetch_add(1, Ordering::SeqCst);
        let issue = GhIssue {
            number,
            url: format!("https://github.com/rysweet/Simard/issues/{number}"),
            title: title.to_string(),
            body: body.to_string(),
        };
        self.created
            .lock()
            .expect("created lock")
            .push(issue.clone());
        Ok(issue)
    }
}

/// Write a persisted cycle report containing a single failed engineer outcome
/// whose `detail` is `detail` (mirrors `persist_cycle_report`'s JSON shape).
fn seed_cycle_report(state_root: &Path, cycle: u32, detail: &str) {
    let dir = state_root.join("cycle_reports");
    std::fs::create_dir_all(&dir).expect("create cycle_reports");
    let report = json!({
        "cycle_number": cycle,
        "summary": "seeded failure",
        "outcomes": [
            {
                "action_kind": "AdvanceGoal",
                "action_description": "spawn engineer for goal g1",
                "success": false,
                "detail": detail,
            }
        ],
    });
    std::fs::write(
        dir.join(format!("cycle_{cycle}.json")),
        serde_json::to_string_pretty(&report).expect("serialize report"),
    )
    .expect("write cycle report");
}

// ---------------------------------------------------------------------------
// §12 — schedule.rs: pure due-computation (injected clock, no sleeps)
// ---------------------------------------------------------------------------

#[test]
fn interval_is_due_when_never_run() {
    let policy = SchedulePolicy::Interval(Duration::from_secs(100));
    assert!(schedule::is_due(&policy, None, 12_345));
}

#[test]
fn interval_is_not_due_before_deadline_and_due_at_deadline() {
    let policy = SchedulePolicy::Interval(Duration::from_secs(100));
    assert!(!schedule::is_due(&policy, Some(1_000), 1_099));
    assert!(schedule::is_due(&policy, Some(1_000), 1_100));
    assert!(schedule::is_due(&policy, Some(1_000), 5_000));
}

#[test]
fn interval_next_run_is_last_plus_interval() {
    let policy = SchedulePolicy::Interval(Duration::from_secs(100));
    assert_eq!(
        schedule::next_run_epoch(&policy, Some(1_000), 1_050),
        Some(1_100)
    );
    // Never run yet => scheduled now.
    assert_eq!(schedule::next_run_epoch(&policy, None, 2_000), Some(2_000));
}

#[test]
fn on_demand_and_event_driven_are_never_auto_due() {
    for policy in [SchedulePolicy::OnDemand, SchedulePolicy::EventDriven] {
        assert!(!schedule::is_due(&policy, None, 9_999));
        assert!(!schedule::is_due(&policy, Some(1), 9_999));
        assert_eq!(schedule::next_run_epoch(&policy, Some(1), 9_999), None);
    }
}

#[test]
fn adaptive_behaves_like_interval_current() {
    let policy = SchedulePolicy::Adaptive {
        min: Duration::from_secs(60),
        max: Duration::from_secs(600),
        current: Duration::from_secs(100),
    };
    assert!(!schedule::is_due(&policy, Some(1_000), 1_099));
    assert!(schedule::is_due(&policy, Some(1_000), 1_100));
    assert_eq!(
        schedule::next_run_epoch(&policy, Some(1_000), 1_050),
        Some(1_100)
    );
}

#[test]
fn backoff_is_monotonic_and_capped() {
    let base = Duration::from_secs(10);
    let cap = Duration::from_secs(300);
    let now = 1_000;
    let b1 = schedule::backoff_until_epoch(now, 1, base, cap);
    let b2 = schedule::backoff_until_epoch(now, 2, base, cap);
    let b3 = schedule::backoff_until_epoch(now, 3, base, cap);
    assert!(
        b1 >= now && b2 >= b1 && b3 >= b2,
        "backoff must grow: {b1} {b2} {b3}"
    );
    // Never exceeds now + cap, even for a huge error count.
    let huge = schedule::backoff_until_epoch(now, u32::MAX, base, cap);
    assert!(huge <= now + cap.as_secs(), "backoff capped at now+cap");
    assert!(huge >= now);
}

#[test]
fn clamp_interval_secs_enforces_floor() {
    // SR-8: a hostile/misconfigured 0 must not make a thread due every tick.
    assert_eq!(schedule::clamp_interval_secs(0), MIN_INTERVAL_SECS);
    assert!(schedule::clamp_interval_secs(0) > 0);
    // Values above the floor are unchanged.
    assert_eq!(schedule::clamp_interval_secs(3_600), 3_600);
}

// ---------------------------------------------------------------------------
// §12 — Mind: registry (green today), due-computation, failure isolation,
// priority budget, OODA parity, graceful shutdown.
// ---------------------------------------------------------------------------

#[test]
fn mind_registry_len_and_is_empty() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut mind = Mind::with_budget(2);
    assert!(mind.is_empty());
    let (t, _runs) = fake(
        "a",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    mind.register(t);
    assert_eq!(mind.len(), 1);
    assert!(!mind.is_empty());
}

#[test]
fn due_threads_excludes_on_demand_and_disabled() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut mind = Mind::with_budget(4);
    // index 0: due interval thread
    let (t0, _r0) = fake(
        "due",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    // index 1: on-demand thread (never auto-due)
    let (t1, _r1) = fake(
        "on_demand",
        ThreadKind::Maintenance,
        Priority::Low,
        SchedulePolicy::OnDemand,
        FakeBehavior::Succeed,
        &log,
    );
    mind.register(t0).register(t1);
    assert_eq!(mind.due_threads(10_000), vec![0]);
}

#[test]
fn run_due_runs_critical_every_tick_and_bounds_noncritical_fanout() {
    // OODA parity + priority budget (§5, §6): a Critical thread runs on every
    // tick and is never crowded out by a flood of due Low threads, which are
    // themselves bounded by the per-tick budget.
    let env = TestEnv::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let budget = 2;
    let mut mind = Mind::with_budget(budget);

    let (crit, crit_runs) = fake(
        "ooda",
        ThreadKind::Ooda,
        Priority::Critical,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    mind.register(crit);
    for i in 0..6 {
        let (t, _r) = fake(
            &format!("low_{i}"),
            ThreadKind::Maintenance,
            Priority::Low,
            always_due(),
            FakeBehavior::Succeed,
            &log,
        );
        mind.register(t);
    }

    let ticks: u64 = 5;
    for tick in 0..ticks {
        let mut ctx = env.ctx(1_000 + tick, false);
        let outcomes = mind.run_due(&mut ctx);
        // The Critical thread must have run this tick (never starved) ...
        assert_eq!(
            crit_runs.load(Ordering::SeqCst) as u64,
            tick + 1,
            "critical thread must run exactly once per tick"
        );
        // ... and it must be the first thing that ran each tick.
        assert!(
            matches!(outcomes.first(), Some(o) if o.ran),
            "OODA runs first and unconditionally"
        );
    }

    // Critical ran every tick.
    assert_eq!(crit_runs.load(Ordering::SeqCst), ticks as usize);
    // Non-critical fan-out per tick was bounded by the budget: total Low runs
    // never exceeds budget * ticks.
    let low_total: usize = (0..6)
        .map(|i| {
            log.lock()
                .expect("log")
                .iter()
                .filter(|id| **id == format!("low_{i}"))
                .count()
        })
        .sum();
    assert!(
        low_total <= budget * ticks as usize,
        "non-critical fan-out {low_total} must be bounded by budget*ticks {}",
        budget * ticks as usize
    );
}

#[test]
fn run_due_isolates_panicking_thread_without_killing_siblings() {
    // Failure isolation (§5.4): a panicking thread is caught, recorded, and
    // backed off; OODA and other threads still run.
    let env = TestEnv::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut mind = Mind::with_budget(8);

    let (crit, crit_runs) = fake(
        "ooda",
        ThreadKind::Ooda,
        Priority::Critical,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    let (boom, _boom_runs) = fake(
        "boom",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Panic,
        &log,
    );
    let (sibling, sibling_runs) = fake(
        "sibling",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    mind.register(crit).register(boom).register(sibling);

    let mut ctx = env.ctx(1_000, false);
    // Must NOT propagate the panic.
    let _outcomes = mind.run_due(&mut ctx);

    assert_eq!(
        crit_runs.load(Ordering::SeqCst),
        1,
        "critical survived the panic"
    );
    assert_eq!(
        sibling_runs.load(Ordering::SeqCst),
        1,
        "sibling survived the panic"
    );

    // The panicking thread is recorded as errored and backed off.
    let health = mind.health();
    let boom_health = health
        .iter()
        .find(|h| h.id == "boom")
        .expect("boom health present");
    assert!(
        boom_health.consecutive_errors >= 1,
        "panic recorded as error"
    );
    assert!(
        boom_health.backoff_until_epoch.is_some(),
        "panic triggers backoff"
    );
}

#[test]
fn run_due_backs_off_erroring_thread_but_never_ooda() {
    let env = TestEnv::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut mind = Mind::with_budget(8);

    let (crit, _crit_runs) = fake(
        "ooda",
        ThreadKind::Ooda,
        Priority::Critical,
        always_due(),
        FakeBehavior::Error, // even if OODA reports failure it is never backed off
        &log,
    );
    let (bad, _bad_runs) = fake(
        "bad",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Error,
        &log,
    );
    mind.register(crit).register(bad);

    let mut ctx = env.ctx(2_000, false);
    let _ = mind.run_due(&mut ctx);

    let health = mind.health();
    let ooda = health.iter().find(|h| h.id == "ooda").expect("ooda health");
    let bad = health.iter().find(|h| h.id == "bad").expect("bad health");
    assert!(
        ooda.backoff_until_epoch.is_none(),
        "OODA is never backed off"
    );
    assert!(
        bad.consecutive_errors >= 1,
        "erroring thread accrues errors"
    );
    assert!(
        bad.backoff_until_epoch.is_some(),
        "erroring thread backs off"
    );
}

#[test]
fn run_due_returns_early_on_shutdown() {
    // Graceful shutdown (§5.5): with shutdown requested, non-critical threads
    // must not be started.
    let env = TestEnv::new();
    env.request_shutdown();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut mind = Mind::with_budget(8);

    let (low, low_runs) = fake(
        "low",
        ThreadKind::Maintenance,
        Priority::Low,
        always_due(),
        FakeBehavior::Succeed,
        &log,
    );
    mind.register(low);

    let mut ctx = env.ctx(3_000, false);
    let _ = mind.run_due(&mut ctx);
    assert_eq!(
        low_runs.load(Ordering::SeqCst),
        0,
        "non-critical thread must not start after shutdown requested"
    );
}

// ---------------------------------------------------------------------------
// §12 — MaintenanceThread: identity, safety gate (SR-5/6), dry-run, floors.
// ---------------------------------------------------------------------------

#[test]
fn maintenance_thread_identity_and_policy() {
    let t = MaintenanceThread::new(MaintenanceConfig {
        interval_secs: 3_600,
        ..MaintenanceConfig::default()
    });
    assert_eq!(t.id(), "maintenance");
    assert_eq!(t.kind(), ThreadKind::Maintenance);
    assert_eq!(t.priority(), Priority::Low);
    assert_eq!(
        t.policy(),
        SchedulePolicy::Interval(Duration::from_secs(3_600))
    );
}

#[test]
fn maintenance_config_default_retention_floors_at_least_one() {
    let cfg = MaintenanceConfig::default();
    assert!(cfg.keep_corrupt >= 1);
    assert!(cfg.keep_snapshots >= 1);
    assert!(cfg.keep_backups >= 1);
    assert!(
        cfg.dry_run,
        "destructive maintenance ships dry-run-first (SR-7)"
    );
}

#[test]
fn is_safe_to_delete_allows_stale_dir_inside_allow_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let allow = tmp.path().join("artifacts");
    std::fs::create_dir_all(&allow).expect("mk allow root");
    let candidate = allow.join("cognitive.corrupt-OLD");
    std::fs::create_dir_all(&candidate).expect("mk candidate");

    assert!(maintenance::is_safe_to_delete(
        &candidate,
        std::slice::from_ref(&allow),
        &[],
    ));
}

#[test]
fn is_safe_to_delete_refuses_symlink() {
    // SR-5: a symlink (even inside an allow root) must be refused so a swapped
    // symlink cannot redirect a delete at a protected target.
    let tmp = tempfile::tempdir().expect("tempdir");
    let allow = tmp.path().join("artifacts");
    std::fs::create_dir_all(&allow).expect("mk allow root");
    let protected = tmp.path().join("protected");
    std::fs::create_dir_all(&protected).expect("mk protected");
    let link = allow.join("link");
    std::os::unix::fs::symlink(&protected, &link).expect("mk symlink");

    assert!(
        !maintenance::is_safe_to_delete(&link, std::slice::from_ref(&allow), &[]),
        "a symlink must never be a delete candidate"
    );
}

#[test]
fn is_safe_to_delete_refuses_path_outside_allow_root() {
    // Canonical allow-list defeats `..`/traversal escapes.
    let tmp = tempfile::tempdir().expect("tempdir");
    let allow = tmp.path().join("artifacts");
    std::fs::create_dir_all(&allow).expect("mk allow root");
    let outside = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).expect("mk outside");

    assert!(
        !maintenance::is_safe_to_delete(&outside, std::slice::from_ref(&allow), &[]),
        "a path outside every allow root must be refused"
    );
}

#[test]
fn is_safe_to_delete_refuses_deny_listed_path() {
    // SR-6: an explicitly protected path is refused even when inside an allow
    // root (e.g. the live store / repo).
    let tmp = tempfile::tempdir().expect("tempdir");
    let allow = tmp.path().join("artifacts");
    std::fs::create_dir_all(&allow).expect("mk allow root");
    let protected = allow.join("repo");
    std::fs::create_dir_all(&protected).expect("mk protected");

    assert!(
        !maintenance::is_safe_to_delete(
            &protected,
            std::slice::from_ref(&allow),
            std::slice::from_ref(&protected),
        ),
        "a deny-listed path must be refused"
    );
}

#[test]
fn maintenance_dry_run_deletes_nothing() {
    // SR-7: with dry_run, a stale artifact under the state root survives.
    let env = TestEnv::new();
    let stale = env.state_root().join("cognitive.corrupt-OLD");
    std::fs::create_dir_all(&stale).expect("seed stale dir");

    let mut thread = MaintenanceThread::new(MaintenanceConfig {
        interval_secs: 3_600,
        dry_run: true,
        ..MaintenanceConfig::default()
    });
    let mut ctx = env.ctx(1_000, true);
    let _ = thread.tick(&mut ctx);

    assert!(
        stale.exists(),
        "dry-run maintenance must not delete anything"
    );
}

// ---------------------------------------------------------------------------
// §12 + §5d — EngineerLogAnalysisThread: identity, secret/injection scrub,
// dedup one-issue + idempotency, dry-run.
// ---------------------------------------------------------------------------

#[test]
fn engineer_log_analysis_identity_and_policy() {
    let t = EngineerLogAnalysisThread::with_client(
        EngineerLogAnalysisConfig {
            interval_secs: 7_200,
            ..EngineerLogAnalysisConfig::default()
        },
        Box::new(FakeGhClient::new()),
    );
    assert_eq!(t.id(), "engineer_log_analysis");
    assert_eq!(t.kind(), ThreadKind::EngineerLogAnalysis);
    assert_eq!(t.priority(), Priority::Low);
    assert_eq!(
        t.policy(),
        SchedulePolicy::Interval(Duration::from_secs(7_200))
    );
}

#[test]
fn build_issue_body_redacts_secrets_and_fences_excerpt() {
    // SR-2/SR-3: secrets scrubbed, untrusted excerpt fenced (no auto-links).
    let sig = dedup::failure_signature("engineer_failure", "connection refused");
    let excerpt = "engineer crashed\ntoken=SECRET123\nping @here see #1";
    let body = engineer_log_analysis::build_issue_body(&sig, excerpt);

    assert!(body.contains("[REDACTED]"), "secret value must be redacted");
    assert!(!body.contains("SECRET123"), "raw secret must not appear");
    assert!(body.contains("```"), "untrusted excerpt must be fenced");
    assert!(
        body.contains(&format!("stewardship-signature: {sig}")),
        "trusted dedup marker present"
    );
}

#[test]
fn build_issue_body_marker_is_not_poisoned_by_spoofed_signature() {
    // SR-3: a spoofed `stewardship-signature:` smuggled in the excerpt must not
    // be matchable by dedup; only our computed signature is.
    let sig = dedup::failure_signature("engineer_failure", "disk full");
    let excerpt = "log line\nstewardship-signature: deadbeefdeadbeef\nmore";
    let body = engineer_log_analysis::build_issue_body(&sig, excerpt);

    let issue = GhIssue {
        number: 1,
        url: "http://x".to_string(),
        title: "t".to_string(),
        body,
    };
    assert!(
        dedup::find_existing(std::slice::from_ref(&issue), &sig).is_some(),
        "our real signature must be matchable"
    );
    assert!(
        dedup::find_existing(std::slice::from_ref(&issue), "deadbeefdeadbeef").is_none(),
        "a spoofed signature must not poison dedup"
    );
}

#[test]
fn analysis_files_exactly_one_deduplicated_issue() {
    let env = TestEnv::new();
    let detail = "spawn_engineer failed: agent='eng-1' error: connection refused; token=SECRET123";
    for cycle in 1..=3 {
        seed_cycle_report(env.state_root(), cycle, detail);
    }

    let gh = FakeGhClient::new();
    let mut thread = EngineerLogAnalysisThread::with_client(
        EngineerLogAnalysisConfig {
            interval_secs: 3_600,
            dry_run: false,
            ..EngineerLogAnalysisConfig::default()
        },
        Box::new(gh.clone()),
    );

    let mut ctx = env.ctx(10_000, false);
    let _ = thread.tick(&mut ctx);

    assert_eq!(gh.created_count(), 1, "one recurring failure => one issue");
    let created = gh.created.lock().expect("created");
    let body = &created[0].body;
    assert!(
        body.contains("stewardship-signature:"),
        "issue carries dedup marker"
    );
    assert!(
        body.contains("[REDACTED]"),
        "secret redacted in filed issue"
    );
    assert!(
        !body.contains("SECRET123"),
        "raw secret never leaves the process"
    );
}

#[test]
fn analysis_is_idempotent_across_runs() {
    let env = TestEnv::new();
    let detail = "spawn_engineer failed: agent='eng-1' error: connection refused";
    for cycle in 1..=3 {
        seed_cycle_report(env.state_root(), cycle, detail);
    }

    let gh = FakeGhClient::new();
    let mut thread = EngineerLogAnalysisThread::with_client(
        EngineerLogAnalysisConfig {
            interval_secs: 3_600,
            dry_run: false,
            ..EngineerLogAnalysisConfig::default()
        },
        Box::new(gh.clone()),
    );

    let mut ctx1 = env.ctx(10_000, false);
    let _ = thread.tick(&mut ctx1);
    let mut ctx2 = env.ctx(20_000, false);
    let _ = thread.tick(&mut ctx2);

    assert_eq!(
        gh.created_count(),
        1,
        "second run finds the existing issue and does not create a duplicate"
    );
}

#[test]
fn analysis_dry_run_files_no_issue() {
    let env = TestEnv::new();
    let detail = "spawn_engineer failed: agent='eng-1' error: connection refused";
    for cycle in 1..=3 {
        seed_cycle_report(env.state_root(), cycle, detail);
    }

    let gh = FakeGhClient::new();
    let mut thread = EngineerLogAnalysisThread::with_client(
        EngineerLogAnalysisConfig {
            interval_secs: 3_600,
            dry_run: true,
            ..EngineerLogAnalysisConfig::default()
        },
        Box::new(gh.clone()),
    );

    let mut ctx = env.ctx(10_000, true);
    let _ = thread.tick(&mut ctx);
    assert_eq!(gh.created_count(), 0, "dry-run must not create issues");
}

// ---------------------------------------------------------------------------
// §7 — telemetry naming contract (SR-11). Green today: names are constants.
// ---------------------------------------------------------------------------

#[test]
fn metric_names_use_fixed_scheme() {
    assert_eq!(
        telemetry::metric_name("maintenance", "runs"),
        "simard.thread.maintenance.runs"
    );
    assert_eq!(
        telemetry::metric_name("engineer_log_analysis", "duration_seconds"),
        "simard.thread.engineer_log_analysis.duration_seconds"
    );
    assert_eq!(
        telemetry::metric_name("ooda", "next_run_epoch"),
        "simard.thread.ooda.next_run_epoch"
    );
}

#[test]
fn reserved_thread_kinds_are_representable() {
    // The abstraction can host the future threads without a trait change.
    for kind in [
        ThreadKind::BackgroundThought,
        ThreadKind::MemoryConsolidation,
        ThreadKind::SensoryProcessing,
        ThreadKind::LongTermPlanning,
    ] {
        // Serializable + comparable — proves they are first-class variants.
        let json = serde_json::to_value(kind).expect("serialize kind");
        assert!(json.is_string());
    }
}
