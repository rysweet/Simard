//! M1 — the Overseer packaged as an **optional, read-only `CognitiveThread`
//! sensor**, plus the real Observe adapter over the shipped `StatusSnapshot`.
//!
//! This is the concrete, runnable M1 surface described in the design doc's
//! "Embedding seam": a least-authority `impl CognitiveThread` that Observes →
//! Orients → Reports → files **deduplicated** issues, and takes **no write
//! action beyond issue-filing** (no launches, merges, conflict-resolution,
//! deploys, or goal transfers). Because it needs none of the guarded
//! capabilities, it fits the least-authority `ThreadContext` and can be
//! registered next to `MaintenanceThread` / `EngineerLogAnalysisThread` behind
//! the `SIMARD_OVERSEER_ENABLED` flag (default OFF) with no default behaviour
//! change.
//!
//! Reuse map (see `docs/design/overseer.md` §capability table / grounding ledger):
//! - Observe: `status::assemble` → `StatusSnapshot` (`src/status/provider.rs:58`).
//! - Report: `status::render::to_terminal` / `status::json::to_string_pretty`.
//! - Dedup vs in-flight: `goal_curation::load_goal_board` (read via `ThreadContext.memory`).
//! - File deduped issues: `stewardship::process_orchestrator_run` (via
//!   [`StewardshipIssueFiler`](crate::overseer::observer::StewardshipIssueFiler)).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::cognitive_threads::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
use crate::goal_curation::{GoalBoard, load_goal_board};
use crate::status::{AssembleOptions, StatusSnapshot, assemble};
use crate::stewardship::RealGhClient;

use crate::overseer::capabilities::{
    InFlightItem, IssueFiler, IssueOutcome, ObservedState, OverseerError, StatusReader,
};
use crate::overseer::config;
use crate::overseer::intervention::Intervention;
use crate::overseer::observer::{StewardshipIssueFiler, decide_read_only, is_m1_permitted};
use crate::overseer::orient;
use crate::overseer::signal::{Problem, Signal, signals_from};

// ─────────────────────────── Observe adapter ───────────────────────────────

/// Source of the full `StatusSnapshot`. Kept distinct from the design's
/// [`StatusReader`] (which yields the flattened [`ObservedState`]) because the
/// M1 Report renders the *whole* snapshot via `status::render`.
pub trait SnapshotSource {
    fn assemble_snapshot(&self) -> Result<StatusSnapshot, OverseerError>;
}

/// The real Observe adapter. Wraps `crate::status::assemble` — the exact value
/// `simard status` renders — and is read-only over `~/.simard` telemetry. It
/// satisfies BOTH capability traits: [`SnapshotSource`] (full snapshot, for the
/// M1 Report) and [`StatusReader`] (flattened [`ObservedState`], reused by the
/// M2 `Overseer::run_cycle`). `assemble` is infallible (degraded sections come
/// back as `None`), so both methods always `Ok`.
#[derive(Clone, Debug)]
pub struct SnapshotStatusReader {
    opts: AssembleOptions,
}

impl SnapshotStatusReader {
    pub fn new(opts: AssembleOptions) -> Self {
        Self { opts }
    }

    /// Construct from the process environment defaults (`~/.simard` state root,
    /// `simard.service` unit).
    pub fn from_env() -> Self {
        Self {
            opts: AssembleOptions::default(),
        }
    }
}

impl Default for SnapshotStatusReader {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SnapshotSource for SnapshotStatusReader {
    fn assemble_snapshot(&self) -> Result<StatusSnapshot, OverseerError> {
        Ok(assemble(&self.opts))
    }
}

impl StatusReader for SnapshotStatusReader {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        Ok(observed_from_snapshot(&assemble(&self.opts)))
    }
}

/// Pure projection of a `StatusSnapshot` onto the Overseer's [`ObservedState`].
/// Every field cites the exact snapshot path; a **degraded** section
/// (`SectionEnvelope.data == None`) maps to `None`, never a panic. PR/CI reads
/// (`ready_prs` / `ci_failures`) are an M2 capability, so they stay empty at M1.
pub fn observed_from_snapshot(snap: &StatusSnapshot) -> ObservedState {
    let telemetry = snap.telemetry.data.as_ref();
    let daemon = snap.daemon.data.as_ref();
    let resources = snap.resources.data.as_ref();
    let llm = snap.llm.data.as_ref();
    let memory = snap.memory.data.as_ref();
    let gym = snap.gym.data.as_ref();

    ObservedState {
        distill_fail_pct: telemetry.and_then(|t| t.distill_fail_pct),
        restart_churn: telemetry
            .and_then(|t| t.restart_churn)
            .or_else(|| daemon.and_then(|d| d.n_restarts)),
        ladder_exhausted: memory.and_then(|m| m.decide_ladder_exhausted),
        spent_today_usd: llm
            .and_then(|l| l.ledger_today.as_ref())
            .map(|w| w.cost_usd),
        daily_budget_usd: llm.and_then(|l| l.daily_budget_usd),
        live_engineers: resources.and_then(|r| r.live_engineers),
        memory_nodes: memory.and_then(|m| m.nodes_total),
        gym_skipped: gym.map(|g| g.skip_gym).unwrap_or(false)
            || telemetry.map(|t| t.gym_skipped).unwrap_or(false),
        anomalies: telemetry.map(|t| t.anomalies.clone()).unwrap_or_default(),
        ready_prs: Vec::new(),
        ci_failures: Vec::new(),
        // Loop/drift are surfaced from the OODA no-progress tracker, which the
        // read-only status snapshot does not yet expose. Left `None` here so the
        // adapter stays additive; a follow-up enriches this from the goal board's
        // progress state to activate production whispers (the daemon is not
        // redeployed by this change).
        consecutive_no_action: None,
        active_goal_id: None,
        drift_detail: None,
    }
}

/// Map Simard's in-flight goal board onto the dedup [`InFlightItem`]s Orient
/// checks against, so the Overseer never fights an engineer already on a case.
/// Read-only. Each active/backlog goal contributes its id and any WIP refs
/// (PR/issue/branch/session ids). Overseer-launched workstreams (M2+) stamp
/// their `dedup_key` here, which is where board-dedup becomes load-bearing.
pub fn in_flight_from_board(board: &GoalBoard) -> Vec<InFlightItem> {
    let mut items = Vec::with_capacity(board.active.len() + board.backlog.len());
    for g in &board.active {
        let mut refs: Vec<String> = g
            .wip_refs
            .iter()
            .map(|w| format!("{}:{}", w.kind, w.ref_id))
            .collect();
        refs.push(g.id.clone());
        items.push(InFlightItem {
            id: g.id.clone(),
            source: g.assigned_to.clone().unwrap_or_else(|| "ooda".to_string()),
            refs,
        });
    }
    for b in &board.backlog {
        items.push(InFlightItem {
            id: b.id.clone(),
            source: b.source.clone(),
            refs: vec![b.id.clone()],
        });
    }
    items
}

// ─────────────────────────── Read-only cycle ───────────────────────────────

/// The result of one read-only observer pass. Side-effect free apart from the
/// deduplicated issues recorded in `issues_filed` (suppressed under `dry_run`).
#[derive(Clone, Debug)]
pub struct ObserverReport {
    pub observed: ObservedState,
    pub signals: Vec<Signal>,
    pub problems: Vec<Problem>,
    /// Issues actually filed this pass (empty under `dry_run`).
    pub issues_filed: Vec<IssueOutcome>,
    /// Per-issue capability errors — a failed file degrades one finding, never
    /// the whole cycle (failure isolation).
    pub file_errors: Vec<String>,
    /// How many `FileIssue` interventions were planned (filed or dry-run).
    pub planned_file_count: usize,
    pub report_terminal: String,
    pub report_json: String,
}

/// Run one read-only Observe → Orient → Report(+file deduped issues) pass.
///
/// The **hard M1 invariant** — no write action beyond deduplicated issue-filing
/// — is enforced two ways: a `debug_assert!` on [`is_m1_permitted`], and the
/// match arm that only ever dispatches `FileIssue`. A `Report`/`Escalate` plan
/// carries no side effect; no other `Intervention` variant is reachable from
/// [`decide_read_only`].
pub fn run_observer_cycle(
    snapshot: &StatusSnapshot,
    in_flight: &[InFlightItem],
    issues: &dyn IssueFiler,
    dry_run: bool,
) -> ObserverReport {
    let observed = observed_from_snapshot(snapshot);
    let signals = signals_from(&observed);
    let problems = orient(&signals, in_flight);

    let mut issues_filed = Vec::new();
    let mut file_errors = Vec::new();
    let mut planned_file_count = 0usize;

    for p in &problems {
        let iv = decide_read_only(p);
        debug_assert!(
            is_m1_permitted(&iv),
            "M1 observer planned a non-read-only action: {iv:?}"
        );
        if let Intervention::FileIssue { run } = iv {
            planned_file_count += 1;
            if !dry_run {
                match issues.file(&run) {
                    Ok(outcome) => issues_filed.push(outcome),
                    Err(e) => file_errors.push(e.to_string()),
                }
            }
        }
    }

    let report_terminal = crate::status::render::to_terminal(snapshot);
    let report_json = crate::status::json::to_string_pretty(snapshot)
        .unwrap_or_else(|e| format!("{{\"error\":\"status json render failed: {e}\"}}"));

    ObserverReport {
        observed,
        signals,
        problems,
        issues_filed,
        file_errors,
        planned_file_count,
        report_terminal,
        report_json,
    }
}

// ─────────────────────────── CognitiveThread sensor ────────────────────────

/// Stable thread id for the M1 observer sensor.
pub const OVERSEER_SENSOR_ID: &str = "overseer-observer";

/// The M1 Overseer as a least-authority background sensor. Holds only two
/// injectable capabilities — an Observe source and a deduplicated issue filer —
/// so it can take **no** high-risk action. Constructed via [`from_env`] in the
/// daemon (behind the flag) or with fakes in tests.
///
/// [`from_env`]: OverseerSensorThread::from_env
pub struct OverseerSensorThread {
    snapshots: Box<dyn SnapshotSource + Send>,
    issues: Box<dyn IssueFiler + Send>,
    interval_secs: u64,
    enabled: bool,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl OverseerSensorThread {
    /// Construct with injected capabilities (tests inject fakes).
    pub fn new(
        snapshots: Box<dyn SnapshotSource + Send>,
        issues: Box<dyn IssueFiler + Send>,
        interval_secs: u64,
        enabled: bool,
    ) -> Self {
        Self {
            snapshots,
            issues,
            interval_secs,
            enabled,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }

    /// Construct the real daemon sensor: Observe over `~/.simard` telemetry,
    /// file issues through the real `gh` client. Enablement + cadence come from
    /// the `SIMARD_OVERSEER_*` env knobs (default OFF, clamped cadence).
    pub fn from_env() -> Self {
        Self::new(
            Box::new(SnapshotStatusReader::from_env()),
            Box::new(StewardshipIssueFiler::new(Arc::new(RealGhClient::new()))),
            config::overseer_interval_secs(),
            config::overseer_enabled(),
        )
    }
}

impl CognitiveThread for OverseerSensorThread {
    fn id(&self) -> &str {
        OVERSEER_SENSOR_ID
    }

    fn kind(&self) -> ThreadKind {
        // A read-only observer over Simard's own telemetry — a sensory faculty.
        ThreadKind::SensoryProcessing
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(self.interval_secs))
    }

    fn priority(&self) -> Priority {
        // Background, never-critical: it must never steal budget from OODA.
        Priority::Low
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();

        if !self.enabled {
            return ThreadOutcome::skipped();
        }

        // Observe — a snapshot failure aborts THIS cycle cleanly (no partial act).
        let snapshot = match self.snapshots.assemble_snapshot() {
            Ok(s) => s,
            Err(e) => {
                self.last_run_epoch = Some(ctx.now_epoch);
                self.next_run_epoch = Some(ctx.now_epoch.saturating_add(self.interval_secs));
                self.last_success = Some(false);
                self.consecutive_errors = self.consecutive_errors.saturating_add(1);
                return ThreadOutcome::failed(
                    format!("overseer observe failed: {e}"),
                    start.elapsed(),
                );
            }
        };

        // Orient dedup input: Simard's in-flight goals (read-only; a board-read
        // failure degrades to "no dedup", never a crash).
        let in_flight = load_goal_board(ctx.memory)
            .map(|b| in_flight_from_board(&b))
            .unwrap_or_default();

        // Under dry_run the observer plans but files nothing (its only write).
        let report = run_observer_cycle(&snapshot, &in_flight, self.issues.as_ref(), ctx.dry_run);

        self.last_run_epoch = Some(ctx.now_epoch);
        self.next_run_epoch = Some(ctx.now_epoch.saturating_add(self.interval_secs));

        let summary = format!(
            "overseer observer: {} signal(s), {} problem(s), {} issue(s) filed{}",
            report.signals.len(),
            report.problems.len(),
            report.issues_filed.len(),
            if report.file_errors.is_empty() {
                String::new()
            } else {
                format!(", {} file error(s)", report.file_errors.len())
            },
        );

        let detail = json!({
            "signals": report.signals.len(),
            "problems": report
                .problems
                .iter()
                .map(|p| json!({
                    "kind": format!("{:?}", p.kind),
                    "priority": format!("{:?}", p.priority),
                    "dedup_key": p.dedup_key,
                }))
                .collect::<Vec<_>>(),
            "issues_filed": report
                .issues_filed
                .iter()
                .map(issue_outcome_json)
                .collect::<Vec<_>>(),
            "planned_file_count": report.planned_file_count,
            "file_errors": report.file_errors,
            "dry_run": ctx.dry_run,
            "read_only": true,
        });

        if report.file_errors.is_empty() {
            self.last_success = Some(true);
            self.consecutive_errors = 0;
            ThreadOutcome::ok(summary, start.elapsed()).with_detail(detail)
        } else {
            self.last_success = Some(false);
            self.consecutive_errors = self.consecutive_errors.saturating_add(1);
            ThreadOutcome::failed(summary, start.elapsed()).with_detail(detail)
        }
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: OVERSEER_SENSOR_ID.to_string(),
            enabled: self.enabled,
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,
        }
    }
}

fn issue_outcome_json(o: &IssueOutcome) -> serde_json::Value {
    match o {
        IssueOutcome::FiledNew { url } => json!({ "outcome": "filed_new", "url": url }),
        IssueOutcome::MatchedExisting { url } => {
            json!({ "outcome": "matched_existing", "url": url })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::status::{SectionEnvelope, TelemetrySignals};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    // ── Observe mapping ──────────────────────────────────────────────────────

    fn snapshot_with_telemetry(t: TelemetrySignals) -> StatusSnapshot {
        let mut snap = StatusSnapshot::empty();
        snap.telemetry = SectionEnvelope::live(t, None);
        snap
    }

    #[test]
    fn observed_from_degraded_snapshot_is_all_none_no_panic() {
        let observed = observed_from_snapshot(&StatusSnapshot::empty());
        assert_eq!(observed, ObservedState::default());
    }

    #[test]
    fn observed_from_snapshot_maps_telemetry_fields() {
        let observed = observed_from_snapshot(&snapshot_with_telemetry(TelemetrySignals {
            distill_fail_pct: Some(62.0),
            restart_churn: Some(4),
            gym_skipped: true,
            anomalies: vec!["banner pollution".to_string()],
            ..TelemetrySignals::default()
        }));
        assert_eq!(observed.distill_fail_pct, Some(62.0));
        assert_eq!(observed.restart_churn, Some(4));
        assert!(observed.gym_skipped);
        assert_eq!(observed.anomalies, vec!["banner pollution".to_string()]);
    }

    // ── run_observer_cycle: read-only, files deduped issues, renders report ──

    /// A no-network `IssueFiler` fake that just counts and returns FiledNew.
    #[derive(Default)]
    struct CountingFiler {
        filed: Mutex<usize>,
    }
    impl CountingFiler {
        fn count(&self) -> usize {
            *self.filed.lock().unwrap()
        }
    }
    impl IssueFiler for CountingFiler {
        fn file(
            &self,
            _run: &crate::overseer::capabilities::OrchestratorRunBrief,
        ) -> Result<IssueOutcome, OverseerError> {
            let mut n = self.filed.lock().unwrap();
            *n += 1;
            Ok(IssueOutcome::FiledNew {
                url: format!("https://example/issues/{n}"),
            })
        }
    }

    #[test]
    fn quiet_snapshot_files_nothing_but_still_renders_report() {
        let filer = CountingFiler::default();
        let report = run_observer_cycle(&StatusSnapshot::empty(), &[], &filer, false);
        assert!(report.signals.is_empty());
        assert!(report.problems.is_empty());
        assert_eq!(report.planned_file_count, 0);
        assert_eq!(filer.count(), 0);
        assert!(
            !report.report_terminal.is_empty(),
            "Report must always render the snapshot"
        );
    }

    #[test]
    fn process_defect_files_exactly_one_issue_and_reports() {
        let filer = CountingFiler::default();
        let snap = snapshot_with_telemetry(TelemetrySignals {
            distill_fail_pct: Some(62.0),
            ..TelemetrySignals::default()
        });
        let report = run_observer_cycle(&snap, &[], &filer, false);
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.planned_file_count, 1);
        assert_eq!(report.issues_filed.len(), 1);
        assert_eq!(filer.count(), 1);
        assert!(report.file_errors.is_empty());
    }

    #[test]
    fn dry_run_plans_but_files_no_issue() {
        let filer = CountingFiler::default();
        let snap = snapshot_with_telemetry(TelemetrySignals {
            distill_fail_pct: Some(62.0),
            ..TelemetrySignals::default()
        });
        let report = run_observer_cycle(&snap, &[], &filer, true);
        assert_eq!(report.planned_file_count, 1, "the file is planned");
        assert_eq!(filer.count(), 0, "but dry_run files nothing");
        assert!(report.issues_filed.is_empty());
    }

    // ── The CognitiveThread sensor: end-to-end, no network ───────────────────

    struct FakeSnapshots(StatusSnapshot);
    impl SnapshotSource for FakeSnapshots {
        fn assemble_snapshot(&self) -> Result<StatusSnapshot, OverseerError> {
            Ok(self.0.clone())
        }
    }

    /// Stateful, no-network `gh` fake: `create_issue` registers the issue so a
    /// later `search_issues` for the same signature returns it — so the sensor's
    /// dedup is exercised end-to-end across ticks with zero network.
    #[derive(Default)]
    struct StatefulFakeGh {
        issues: Mutex<Vec<crate::stewardship::GhIssue>>,
        next_number: Mutex<u64>,
        create_calls: Mutex<usize>,
    }
    impl StatefulFakeGh {
        fn new() -> Self {
            Self {
                next_number: Mutex::new(1),
                ..Default::default()
            }
        }
        fn create_calls(&self) -> usize {
            *self.create_calls.lock().unwrap()
        }
    }
    impl crate::stewardship::GhClient for StatefulFakeGh {
        fn search_issues(
            &self,
            _repo: &str,
            signature: &str,
        ) -> crate::error::SimardResult<Vec<crate::stewardship::GhIssue>> {
            let needle = format!("stewardship-signature: {signature}");
            Ok(self
                .issues
                .lock()
                .unwrap()
                .iter()
                .filter(|i| i.body.contains(&needle))
                .cloned()
                .collect())
        }
        fn create_issue(
            &self,
            repo: &str,
            title: &str,
            body: &str,
        ) -> crate::error::SimardResult<crate::stewardship::GhIssue> {
            *self.create_calls.lock().unwrap() += 1;
            let number = {
                let mut n = self.next_number.lock().unwrap();
                let cur = *n;
                *n += 1;
                cur
            };
            let issue = crate::stewardship::GhIssue {
                number,
                url: format!("https://github.com/{repo}/issues/{number}"),
                title: title.to_string(),
                body: body.to_string(),
            };
            self.issues.lock().unwrap().push(issue.clone());
            Ok(issue)
        }
    }

    fn sensor_with(
        snap: StatusSnapshot,
        gh: Arc<StatefulFakeGh>,
        enabled: bool,
    ) -> OverseerSensorThread {
        OverseerSensorThread::new(
            Box::new(FakeSnapshots(snap)),
            Box::new(StewardshipIssueFiler::new(gh)),
            config::DEFAULT_OVERSEER_INTERVAL_SECS,
            enabled,
        )
    }

    /// Owns the borrowed resources a `ThreadContext` needs (mirrors the shipped
    /// cognitive-thread test harness): an in-memory store, a current-thread
    /// runtime, and a temp state root — zero network, zero real `~/.simard`.
    struct TestEnv {
        rt: tokio::runtime::Runtime,
        mem: LibraryCognitiveMemory,
        shutdown: AtomicBool,
        tmp: tempfile::TempDir,
    }
    impl TestEnv {
        fn new() -> Self {
            Self {
                rt: tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime"),
                mem: LibraryCognitiveMemory::in_memory().expect("in-memory store"),
                shutdown: AtomicBool::new(false),
                tmp: tempfile::tempdir().expect("tempdir"),
            }
        }
        fn ctx(&self, now_epoch: u64, dry_run: bool) -> ThreadContext<'_> {
            ThreadContext {
                state_root: self.tmp.path() as &Path,
                repo_root: self.tmp.path() as &Path,
                memory: &self.mem as &dyn CognitiveMemoryOps,
                runtime: self.rt.handle().clone(),
                shutdown: &self.shutdown,
                now_epoch,
                dry_run,
            }
        }
    }

    #[test]
    fn disabled_sensor_skips() {
        let env = TestEnv::new();
        let gh = Arc::new(StatefulFakeGh::new());
        let mut sensor = sensor_with(StatusSnapshot::empty(), gh, false);
        let outcome = sensor.tick(&mut env.ctx(100, false));
        assert!(!outcome.ran, "a disabled sensor must not run");
        assert!(!sensor.enabled());
    }

    #[test]
    fn sensor_tick_files_one_deduped_issue_and_is_idempotent() {
        let env = TestEnv::new();
        let gh = Arc::new(StatefulFakeGh::new());
        let snap = snapshot_with_telemetry(TelemetrySignals {
            distill_fail_pct: Some(62.0),
            ..TelemetrySignals::default()
        });
        let mut sensor = sensor_with(snap, gh.clone(), true);

        let first = sensor.tick(&mut env.ctx(100, false));
        assert!(first.ran && first.success, "first tick runs and succeeds");
        assert_eq!(
            gh.create_calls(),
            1,
            "one issue filed for the distill defect"
        );

        // A second cycle over the same recurring problem must NOT duplicate.
        let second = sensor.tick(&mut env.ctx(200, false));
        assert!(second.ran && second.success);
        assert_eq!(gh.create_calls(), 1, "recurring problem stays one issue");

        let health = sensor.health();
        assert_eq!(health.last_run_epoch, Some(200));
        assert_eq!(
            health.next_run_epoch,
            Some(200 + config::DEFAULT_OVERSEER_INTERVAL_SECS)
        );
        assert_eq!(health.consecutive_errors, 0);
    }

    #[test]
    fn sensor_dry_run_files_nothing() {
        let env = TestEnv::new();
        let gh = Arc::new(StatefulFakeGh::new());
        let snap = snapshot_with_telemetry(TelemetrySignals {
            distill_fail_pct: Some(62.0),
            ..TelemetrySignals::default()
        });
        let mut sensor = sensor_with(snap, gh.clone(), true);
        let outcome = sensor.tick(&mut env.ctx(100, true));
        assert!(outcome.ran);
        assert_eq!(gh.create_calls(), 0, "dry_run files no issues");
    }

    #[test]
    fn in_flight_from_empty_board_is_empty() {
        assert!(in_flight_from_board(&GoalBoard::new()).is_empty());
    }
}
