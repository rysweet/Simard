//! Unit tests for the CI-health sweep. These pin the exact distinction that
//! motivated the module: a `disabled` workflow's stale `failure` is **ignored**,
//! while an `active` workflow's `failure` is an **actionable** failure.

use super::cache::GreenShaCache;
use super::classify::{
    IgnoreReason, WorkflowVerdict, build_report, classify_workflow, repo_cacheable,
};
use super::gh::{
    GhWorkflowClient, RawRunRow, RawWorkflowRow, build_repo_snapshot, collect_fleet,
    latest_run_by_workflow, parse_run_rows, snapshot_from_fixture,
};
use super::types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};
use super::{report_to_json, run_sweep, sweep_fixture};
use crate::error::SimardResult;

fn run(status: &str, conclusion: Option<&str>) -> WorkflowRun {
    WorkflowRun {
        status: status.to_string(),
        conclusion: conclusion.map(RunConclusion::parse),
        event: "push".to_string(),
        created_at: "2026-07-06T00:00:00Z".to_string(),
        database_id: 42,
    }
}

fn wf(name: &str, state: WorkflowState, latest: Option<WorkflowRun>) -> WorkflowSnapshot {
    WorkflowSnapshot {
        name: name.to_string(),
        state,
        latest_run: latest,
    }
}

// ── enum parsing ────────────────────────────────────────────────────────────

#[test]
fn workflow_state_parses_and_flags_disabled() {
    assert_eq!(WorkflowState::parse("active"), WorkflowState::Active);
    assert!(!WorkflowState::parse("active").is_disabled());
    assert!(WorkflowState::parse("disabled_manually").is_disabled());
    assert!(WorkflowState::parse("disabled_inactivity").is_disabled());
    assert_eq!(
        WorkflowState::parse("weird_new_state"),
        WorkflowState::Unknown("weird_new_state".to_string())
    );
    assert!(!WorkflowState::parse("weird_new_state").is_disabled());
}

#[test]
fn run_conclusion_actionable_set_is_exactly_failures() {
    for s in ["failure", "timed_out", "startup_failure"] {
        assert!(
            RunConclusion::parse(s).is_actionable_failure(),
            "{s} must be actionable"
        );
    }
    for s in [
        "success",
        "cancelled",
        "skipped",
        "neutral",
        "action_required",
        "stale",
    ] {
        assert!(
            !RunConclusion::parse(s).is_actionable_failure(),
            "{s} must NOT be actionable"
        );
    }
    // Unknown conclusions are never treated as actionable failures.
    assert!(!RunConclusion::parse("brand_new").is_actionable_failure());
}

// ── classify_workflow ───────────────────────────────────────────────────────

#[test]
fn active_success_is_green() {
    let v = classify_workflow(&wf(
        "CI",
        WorkflowState::Active,
        Some(run("completed", Some("success"))),
    ));
    assert_eq!(v, WorkflowVerdict::Green);
}

#[test]
fn active_failure_is_actionable() {
    let v = classify_workflow(&wf(
        "CI",
        WorkflowState::Active,
        Some(run("completed", Some("failure"))),
    ));
    assert_eq!(
        v,
        WorkflowVerdict::ActionableFailure {
            conclusion: RunConclusion::Failure
        }
    );
}

#[test]
fn active_timed_out_is_actionable() {
    let v = classify_workflow(&wf(
        "CI",
        WorkflowState::Active,
        Some(run("completed", Some("timed_out"))),
    ));
    assert!(matches!(v, WorkflowVerdict::ActionableFailure { .. }));
}

#[test]
fn disabled_failure_is_ignored_not_actionable() {
    // The azlin case: a disabled workflow whose last run failed months ago.
    for state in [
        WorkflowState::DisabledManually,
        WorkflowState::DisabledInactivity,
    ] {
        let v = classify_workflow(&wf(
            "Code Quality Tracker",
            state,
            Some(run("completed", Some("failure"))),
        ));
        assert_eq!(
            v,
            WorkflowVerdict::Ignored {
                reason: IgnoreReason::WorkflowDisabled
            }
        );
    }
}

#[test]
fn active_cancelled_is_ignored_non_failure() {
    // The agent-kgpacks "Build Knowledge Pack" case.
    let v = classify_workflow(&wf(
        "Build Knowledge Pack",
        WorkflowState::Active,
        Some(run("completed", Some("cancelled"))),
    ));
    assert_eq!(
        v,
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::NonFailureConclusion(RunConclusion::Cancelled)
        }
    );
}

#[test]
fn active_skipped_is_ignored_non_failure() {
    let v = classify_workflow(&wf(
        "Deploy",
        WorkflowState::Active,
        Some(run("completed", Some("skipped"))),
    ));
    assert!(matches!(
        v,
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::NonFailureConclusion(_)
        }
    ));
}

#[test]
fn active_no_run_is_ignored_no_run() {
    let v = classify_workflow(&wf("Never Ran", WorkflowState::Active, None));
    assert_eq!(
        v,
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::NoRun
        }
    );
}

#[test]
fn in_progress_run_is_ignored() {
    let v = classify_workflow(&wf(
        "CI",
        WorkflowState::Active,
        Some(run("in_progress", None)),
    ));
    assert_eq!(
        v,
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::InProgress
        }
    );
}

#[test]
fn completed_null_conclusion_is_ignored_not_failure() {
    let v = classify_workflow(&wf(
        "CI",
        WorkflowState::Active,
        Some(run("completed", None)),
    ));
    assert_eq!(
        v,
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::InProgress
        }
    );
}

// ── build_report ────────────────────────────────────────────────────────────

fn mixed_fleet() -> FleetSnapshot {
    FleetSnapshot {
        repos: vec![
            RepoSnapshot {
                slug: "rysweet/azlin".to_string(),
                default_branch: "main".to_string(),
                head_sha: "azlin-sha".to_string(),
                green_from_cache: false,
                workflows: vec![
                    wf(
                        "CI",
                        WorkflowState::Active,
                        Some(run("completed", Some("success"))),
                    ),
                    wf(
                        "Code Quality Tracker",
                        WorkflowState::DisabledManually,
                        Some(run("completed", Some("failure"))),
                    ),
                ],
            },
            RepoSnapshot {
                slug: "rysweet/agent-kgpacks".to_string(),
                default_branch: "main".to_string(),
                head_sha: "kgpacks-sha".to_string(),
                green_from_cache: false,
                workflows: vec![wf(
                    "Build Knowledge Pack",
                    WorkflowState::Active,
                    Some(run("completed", Some("cancelled"))),
                )],
            },
        ],
    }
}

#[test]
fn report_is_green_when_only_disabled_and_cancelled_non_green() {
    let report = build_report(&mixed_fleet());
    assert!(report.green, "disabled+cancelled must not fail the fleet");
    assert_eq!(report.repos_checked, 2);
    assert_eq!(report.workflows_checked, 3);
    assert!(report.actionable_failures.is_empty());
}

#[test]
fn report_flags_active_failure_with_run_url() {
    let mut fleet = mixed_fleet();
    fleet.repos.push(RepoSnapshot {
        slug: "rysweet/RustyClawd".to_string(),
        default_branch: "main".to_string(),
        head_sha: "rusty-sha".to_string(),
        green_from_cache: false,
        workflows: vec![WorkflowSnapshot {
            name: "CI".to_string(),
            state: WorkflowState::Active,
            latest_run: Some(WorkflowRun {
                status: "completed".to_string(),
                conclusion: Some(RunConclusion::Failure),
                event: "push".to_string(),
                created_at: "2026-07-06T00:00:00Z".to_string(),
                database_id: 999,
            }),
        }],
    });
    let report = build_report(&fleet);
    assert!(!report.green);
    assert_eq!(report.actionable_failures.len(), 1);
    let af = &report.actionable_failures[0];
    assert_eq!(af.repo, "rysweet/RustyClawd");
    assert_eq!(af.workflow, "CI");
    assert_eq!(af.conclusion, "failure");
    assert_eq!(
        af.run_url.as_deref(),
        Some("https://github.com/rysweet/RustyClawd/actions/runs/999")
    );
}

#[test]
fn report_serializes_to_json_with_stable_keys() {
    let json = report_to_json(&build_report(&mixed_fleet())).unwrap();
    assert!(json.contains("\"green\": true"));
    assert!(json.contains("\"workflow_disabled\""));
    assert!(json.contains("non_failure_conclusion:cancelled"));
}

// ── gh join helpers (pure) ──────────────────────────────────────────────────

fn run_row(
    wfname: &str,
    wf_id: u64,
    created: &str,
    status: &str,
    conclusion: &str,
    id: u64,
) -> RawRunRow {
    RawRunRow {
        workflow_name: wfname.to_string(),
        workflow_database_id: wf_id,
        status: status.to_string(),
        conclusion: conclusion.to_string(),
        event: "push".to_string(),
        created_at: created.to_string(),
        database_id: id,
    }
}

#[test]
fn latest_run_by_workflow_picks_newest() {
    let rows = vec![
        run_row("CI", 1, "2026-06-01T00:00:00Z", "completed", "failure", 1),
        run_row("CI", 1, "2026-07-01T00:00:00Z", "completed", "success", 2),
        run_row("Docs", 2, "2026-05-01T00:00:00Z", "completed", "success", 3),
    ];
    let latest = latest_run_by_workflow(&rows);
    assert_eq!(latest.get(&1).unwrap().database_id, 2);
    assert_eq!(latest.get(&2).unwrap().database_id, 3);
}

#[test]
fn build_repo_snapshot_keys_by_id_so_same_named_workflows_dont_collapse() {
    // Two DISTINCT workflows share the display name "CI" (allowed by GitHub).
    let workflows = vec![
        RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 100,
        },
        RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 200,
        },
    ];
    // id 100's latest is a newer success; id 200's latest is an older failure.
    let runs = vec![
        run_row("CI", 100, "2026-07-06T00:00:00Z", "completed", "success", 1),
        run_row("CI", 200, "2026-07-05T00:00:00Z", "completed", "failure", 2),
    ];
    let snap = build_repo_snapshot("rysweet/foo", "main", "foo-sha", &workflows, &runs);
    // Keying by id must NOT collapse them onto the newer success.
    assert_eq!(
        classify_workflow(&snap.workflows[0]),
        WorkflowVerdict::Green
    );
    assert!(matches!(
        classify_workflow(&snap.workflows[1]),
        WorkflowVerdict::ActionableFailure { .. }
    ));
    let report = build_report(&FleetSnapshot { repos: vec![snap] });
    assert!(!report.green);
    assert_eq!(report.actionable_failures.len(), 1);
}

#[test]
fn build_repo_snapshot_joins_and_marks_missing_runs() {
    let workflows = vec![
        RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 100,
        },
        RawWorkflowRow {
            name: "Nightly".to_string(),
            state: "active".to_string(),
            id: 200,
        },
    ];
    let runs = vec![run_row(
        "CI",
        100,
        "2026-07-01T00:00:00Z",
        "completed",
        "success",
        7,
    )];
    let snap = build_repo_snapshot("rysweet/foo", "main", "foo-sha", &workflows, &runs);
    assert_eq!(snap.workflows.len(), 2);
    let ci = snap.workflows.iter().find(|w| w.name == "CI").unwrap();
    assert_eq!(ci.latest_run.as_ref().unwrap().database_id, 7);
    let nightly = snap.workflows.iter().find(|w| w.name == "Nightly").unwrap();
    assert!(nightly.latest_run.is_none());
}

#[test]
fn parse_run_rows_maps_empty_conclusion_to_none() {
    let json = br#"[{"workflowName":"CI","workflowDatabaseId":100,"status":"in_progress","conclusion":"","event":"push","createdAt":"2026-07-06T00:00:00Z","databaseId":5}]"#;
    let rows = parse_run_rows(json).unwrap();
    let snap = build_repo_snapshot(
        "rysweet/foo",
        "main",
        "foo-sha",
        &[RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 100,
        }],
        &rows,
    );
    let ci = &snap.workflows[0];
    assert!(ci.latest_run.as_ref().unwrap().conclusion.is_none());
    assert_eq!(classify_workflow(ci), {
        WorkflowVerdict::Ignored {
            reason: IgnoreReason::InProgress,
        }
    });
}

// ── collect_fleet against a fake client ─────────────────────────────────────

struct FakeGh;

impl GhWorkflowClient for FakeGh {
    fn default_branch(&self, _repo: &str) -> SimardResult<String> {
        Ok("main".to_string())
    }
    fn head_sha(&self, _repo: &str, _branch: &str) -> SimardResult<String> {
        Ok("fake-sha".to_string())
    }
    fn list_workflows(&self, _repo: &str) -> SimardResult<Vec<RawWorkflowRow>> {
        Ok(vec![
            RawWorkflowRow {
                name: "CI".to_string(),
                state: "active".to_string(),
                id: 100,
            },
            RawWorkflowRow {
                name: "Old".to_string(),
                state: "disabled_manually".to_string(),
                id: 200,
            },
        ])
    }
    fn list_runs(&self, _repo: &str, _branch: &str) -> SimardResult<Vec<RawRunRow>> {
        Ok(vec![
            run_row(
                "CI",
                100,
                "2026-07-01T00:00:00Z",
                "completed",
                "success",
                10,
            ),
            run_row(
                "Old",
                200,
                "2026-01-01T00:00:00Z",
                "completed",
                "failure",
                11,
            ),
        ])
    }
    fn latest_run(
        &self,
        _repo: &str,
        _branch: &str,
        _workflow_id: u64,
    ) -> SimardResult<Option<RawRunRow>> {
        Ok(None)
    }
}

#[test]
fn collect_fleet_builds_green_snapshot_from_fake() {
    let snap = collect_fleet(
        &FakeGh,
        &["rysweet/a", "rysweet/b"],
        &GreenShaCache::empty(),
    )
    .unwrap();
    assert_eq!(snap.repos.len(), 2);
    let report = build_report(&snap);
    assert!(report.green);
    assert_eq!(report.workflows_checked, 4);
}

// ── collect_fleet fallback: a workflow whose latest run fell outside the
//    branch-wide window must not be silently reported as NoRun/green ─────────

struct WindowGapGh {
    queried: std::cell::RefCell<Vec<u64>>,
}

impl GhWorkflowClient for WindowGapGh {
    fn default_branch(&self, _repo: &str) -> SimardResult<String> {
        Ok("main".to_string())
    }
    fn head_sha(&self, _repo: &str, _branch: &str) -> SimardResult<String> {
        Ok("windowgap-sha".to_string())
    }
    fn list_workflows(&self, _repo: &str) -> SimardResult<Vec<RawWorkflowRow>> {
        Ok(vec![
            RawWorkflowRow {
                name: "CI".to_string(),
                state: "active".to_string(),
                id: 100,
            },
            // An infrequently-triggered active workflow whose last (failing)
            // run was pushed out of the branch-wide window by CI's volume.
            RawWorkflowRow {
                name: "Nightly".to_string(),
                state: "active".to_string(),
                id: 200,
            },
            // A disabled workflow also absent from the window; must NOT be
            // queried and must stay ignored.
            RawWorkflowRow {
                name: "OldDisabled".to_string(),
                state: "disabled_manually".to_string(),
                id: 300,
            },
        ])
    }
    fn list_runs(&self, _repo: &str, _branch: &str) -> SimardResult<Vec<RawRunRow>> {
        // Only CI appears in the window; Nightly and OldDisabled are absent.
        Ok(vec![run_row(
            "CI",
            100,
            "2026-07-06T00:00:00Z",
            "completed",
            "success",
            10,
        )])
    }
    fn latest_run(
        &self,
        _repo: &str,
        _branch: &str,
        workflow_id: u64,
    ) -> SimardResult<Option<RawRunRow>> {
        self.queried.borrow_mut().push(workflow_id);
        Ok(if workflow_id == 200 {
            Some(run_row(
                "Nightly",
                200,
                "2026-06-01T00:00:00Z",
                "completed",
                "failure",
                20,
            ))
        } else {
            None
        })
    }
}

#[test]
fn collect_fleet_falls_back_for_active_workflow_missing_from_window() {
    let gh = WindowGapGh {
        queried: std::cell::RefCell::new(Vec::new()),
    };
    let snap = collect_fleet(&gh, &["rysweet/x"], &GreenShaCache::empty()).unwrap();
    let report = build_report(&snap);

    // The out-of-window active failure must surface as actionable, not be
    // silently ignored as NoRun (which would falsely report the fleet green).
    assert!(!report.green);
    assert_eq!(report.actionable_failures.len(), 1);
    assert_eq!(report.actionable_failures[0].workflow, "Nightly");

    let queried = gh.queried.borrow();
    // Nightly (absent from window) was queried directly...
    assert!(queried.contains(&200));
    // ...CI (present in window) was not re-queried...
    assert!(!queried.contains(&100));
    // ...and the disabled workflow was skipped entirely.
    assert!(!queried.contains(&300));
}

// ── fixture path (drives the gadugi scenario) ───────────────────────────────

#[test]
fn fixture_round_trips_real_scenario() {
    let fixture = br#"{
      "repos": [
        {"slug":"rysweet/azlin","default_branch":"main","workflows":[
          {"name":"CI","state":"active","latest_run":{"status":"completed","conclusion":"success","event":"push","created_at":"2026-07-04T00:00:00Z","database_id":1}},
          {"name":"Code Quality Tracker","state":"disabled_manually","latest_run":{"status":"completed","conclusion":"failure","event":"schedule","created_at":"2026-06-15T00:00:00Z","database_id":2}}
        ]},
        {"slug":"rysweet/agent-kgpacks","default_branch":"main","workflows":[
          {"name":"Build Knowledge Pack","state":"active","latest_run":{"status":"completed","conclusion":"cancelled","event":"issues","created_at":"2026-03-06T00:00:00Z","database_id":3}}
        ]}
      ]
    }"#;
    let report = sweep_fixture(fixture).unwrap();
    assert!(report.green);
    assert!(report.actionable_failures.is_empty());
    // And a genuine active failure in the same fixture shape flips it red.
    let failing = br#"{"repos":[{"slug":"rysweet/x","default_branch":"main","workflows":[
      {"name":"CI","state":"active","latest_run":{"status":"completed","conclusion":"failure","event":"push","created_at":"2026-07-06T00:00:00Z","database_id":9}}]}]}"#;
    let snap: FleetSnapshot = snapshot_from_fixture(failing).unwrap();
    assert!(!build_report(&snap).green);
}

// ── last-known-green head-SHA cache ─────────────────────────────────────────

/// A configurable fake that records how many times the expensive collection
/// calls (`list_workflows` / `list_runs`) were made, so a cache *skip* can be
/// proven by asserting they were never called.
struct CountingGh {
    head: String,
    workflows: Vec<RawWorkflowRow>,
    runs: Vec<RawRunRow>,
    workflows_calls: std::cell::RefCell<usize>,
    runs_calls: std::cell::RefCell<usize>,
    head_calls: std::cell::RefCell<usize>,
}

impl CountingGh {
    fn new(head: &str, workflows: Vec<RawWorkflowRow>, runs: Vec<RawRunRow>) -> Self {
        Self {
            head: head.to_string(),
            workflows,
            runs,
            workflows_calls: std::cell::RefCell::new(0),
            runs_calls: std::cell::RefCell::new(0),
            head_calls: std::cell::RefCell::new(0),
        }
    }
    /// One active, commit-driven (`push`), successful CI workflow.
    fn green_push() -> Self {
        Self::new(
            "sha-B",
            vec![RawWorkflowRow {
                name: "CI".to_string(),
                state: "active".to_string(),
                id: 100,
            }],
            vec![run_row(
                "CI",
                100,
                "2026-07-06T00:00:00Z",
                "completed",
                "success",
                10,
            )],
        )
    }
}

impl GhWorkflowClient for CountingGh {
    fn default_branch(&self, _repo: &str) -> SimardResult<String> {
        Ok("main".to_string())
    }
    fn head_sha(&self, _repo: &str, _branch: &str) -> SimardResult<String> {
        *self.head_calls.borrow_mut() += 1;
        Ok(self.head.clone())
    }
    fn list_workflows(&self, _repo: &str) -> SimardResult<Vec<RawWorkflowRow>> {
        *self.workflows_calls.borrow_mut() += 1;
        Ok(self.workflows.clone())
    }
    fn list_runs(&self, _repo: &str, _branch: &str) -> SimardResult<Vec<RawRunRow>> {
        *self.runs_calls.borrow_mut() += 1;
        Ok(self.runs.clone())
    }
    fn latest_run(
        &self,
        _repo: &str,
        _branch: &str,
        _workflow_id: u64,
    ) -> SimardResult<Option<RawRunRow>> {
        Ok(None)
    }
}

#[test]
fn cache_hit_skips_collection_and_reports_green_by_cache() {
    // The repo's head SHA matches the cached green SHA, so the expensive
    // collection must be skipped entirely. The fake would return a *failing*
    // workflow if it were ever asked — proving the skip.
    let gh = CountingGh::new(
        "sha-A",
        vec![RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 100,
        }],
        vec![run_row(
            "CI",
            100,
            "2026-07-06T00:00:00Z",
            "completed",
            "failure",
            10,
        )],
    );
    let mut cache = GreenShaCache::empty();
    cache.record_green("rysweet/x", "sha-A");

    let report = run_sweep(&gh, &["rysweet/x"], &mut cache).unwrap();

    assert!(report.green, "cache-served repo keeps the fleet green");
    assert_eq!(report.repos_from_cache, 1);
    assert_eq!(report.repos_checked, 1);
    assert_eq!(report.workflows_checked, 0, "no workflows re-collected");
    assert!(report.repos[0].green_from_cache);
    // The expensive collection calls were never made.
    assert_eq!(*gh.workflows_calls.borrow(), 0);
    assert_eq!(*gh.runs_calls.borrow(), 0);
    // The cache entry is preserved unchanged.
    assert_eq!(cache.get("rysweet/x"), Some("sha-A"));
}

#[test]
fn cache_miss_on_sha_change_recollects_and_updates_entry() {
    // Cached at sha-A, but the branch head is now sha-B -> cache miss -> full
    // collection -> still green -> cache advances to sha-B.
    let gh = CountingGh::green_push();
    let mut cache = GreenShaCache::empty();
    cache.record_green("rysweet/x", "sha-A");

    let report = run_sweep(&gh, &["rysweet/x"], &mut cache).unwrap();

    assert!(report.green);
    assert_eq!(report.repos_from_cache, 0, "SHA changed -> not from cache");
    assert!(!report.repos[0].green_from_cache);
    assert_eq!(report.workflows_checked, 1);
    // The expensive collection ran exactly once...
    assert_eq!(*gh.workflows_calls.borrow(), 1);
    assert_eq!(*gh.runs_calls.borrow(), 1);
    // ...and the cache now pins the new green SHA.
    assert_eq!(cache.get("rysweet/x"), Some("sha-B"));
}

#[test]
fn cache_invalidation_on_non_green_drops_entry() {
    // Cached at sha-A; head moved to sha-B and CI now fails -> cache miss ->
    // collect -> red -> the stale green entry is invalidated.
    let gh = CountingGh::new(
        "sha-B",
        vec![RawWorkflowRow {
            name: "CI".to_string(),
            state: "active".to_string(),
            id: 100,
        }],
        vec![run_row(
            "CI",
            100,
            "2026-07-06T00:00:00Z",
            "completed",
            "failure",
            10,
        )],
    );
    let mut cache = GreenShaCache::empty();
    cache.record_green("rysweet/x", "sha-A");

    let report = run_sweep(&gh, &["rysweet/x"], &mut cache).unwrap();

    assert!(!report.green);
    assert_eq!(report.actionable_failures.len(), 1);
    assert_eq!(*gh.workflows_calls.borrow(), 1, "red repo was re-collected");
    assert_eq!(cache.get("rysweet/x"), None, "stale green SHA invalidated");
}

#[test]
fn green_but_scheduled_repo_is_not_cached() {
    // A green fleet whose latest run is `schedule` (not commit-driven) must NOT
    // be cached: a future scheduled run could fail without a new commit, so
    // skipping on an unchanged SHA would be unsound.
    let gh = CountingGh::new(
        "sha-B",
        vec![RawWorkflowRow {
            name: "Nightly".to_string(),
            state: "active".to_string(),
            id: 100,
        }],
        vec![RawRunRow {
            event: "schedule".to_string(),
            ..run_row(
                "Nightly",
                100,
                "2026-07-06T00:00:00Z",
                "completed",
                "success",
                10,
            )
        }],
    );
    let mut cache = GreenShaCache::empty();

    let report = run_sweep(&gh, &["rysweet/x"], &mut cache).unwrap();

    assert!(report.green);
    assert!(
        cache.is_empty(),
        "scheduled-workflow repo must not be cached"
    );
}

#[test]
fn repo_cacheable_only_for_completed_commit_driven_non_failing_active_workflows() {
    let commit_driven_green = RepoSnapshot {
        slug: "rysweet/x".to_string(),
        default_branch: "main".to_string(),
        head_sha: "sha".to_string(),
        green_from_cache: false,
        workflows: vec![wf(
            "CI",
            WorkflowState::Active,
            Some(run("completed", Some("success"))),
        )],
    };
    assert!(repo_cacheable(&commit_driven_green));

    // A disabled-only repo is vacuously cacheable (nothing can run).
    let disabled_only = RepoSnapshot {
        workflows: vec![wf(
            "Old",
            WorkflowState::DisabledManually,
            Some(run("completed", Some("failure"))),
        )],
        ..commit_driven_green.clone()
    };
    assert!(repo_cacheable(&disabled_only));

    // Scheduled latest run -> not commit-driven -> not cacheable.
    let scheduled = RepoSnapshot {
        workflows: vec![WorkflowSnapshot {
            name: "Nightly".to_string(),
            state: WorkflowState::Active,
            latest_run: Some(WorkflowRun {
                event: "schedule".to_string(),
                ..run("completed", Some("success"))
            }),
        }],
        ..commit_driven_green.clone()
    };
    assert!(!repo_cacheable(&scheduled));

    // A repo whose only active workflow has never run is cacheable: a scheduled
    // trigger would already have produced runs, so a never-run workflow cannot
    // fire on an unchanged default branch.
    let never_ran = RepoSnapshot {
        workflows: vec![wf("New", WorkflowState::Active, None)],
        ..commit_driven_green.clone()
    };
    assert!(repo_cacheable(&never_ran));

    // A push-success workflow alongside a never-run workflow stays cacheable.
    let push_plus_norun = RepoSnapshot {
        workflows: vec![
            wf(
                "CI",
                WorkflowState::Active,
                Some(run("completed", Some("success"))),
            ),
            wf("Release", WorkflowState::Active, None),
        ],
        ..commit_driven_green.clone()
    };
    assert!(repo_cacheable(&push_plus_norun));

    // But a push-success workflow alongside a *completed scheduled* run is NOT
    // cacheable — the schedule can fire again without a commit.
    let push_plus_scheduled = RepoSnapshot {
        workflows: vec![
            wf(
                "CI",
                WorkflowState::Active,
                Some(run("completed", Some("success"))),
            ),
            WorkflowSnapshot {
                name: "advisory-scan".to_string(),
                state: WorkflowState::Active,
                latest_run: Some(WorkflowRun {
                    event: "schedule".to_string(),
                    ..run("completed", Some("success"))
                }),
            },
        ],
        ..commit_driven_green.clone()
    };
    assert!(!repo_cacheable(&push_plus_scheduled));

    // In-progress latest run -> not cacheable.
    let in_progress = RepoSnapshot {
        workflows: vec![wf(
            "CI",
            WorkflowState::Active,
            Some(run("in_progress", None)),
        )],
        ..commit_driven_green.clone()
    };
    assert!(!repo_cacheable(&in_progress));

    // A repo already served from cache is trivially still cacheable.
    let from_cache = RepoSnapshot {
        green_from_cache: true,
        workflows: vec![],
        ..commit_driven_green.clone()
    };
    assert!(repo_cacheable(&from_cache));
}

#[test]
fn cache_is_green_requires_non_empty_matching_sha() {
    let mut cache = GreenShaCache::empty();
    cache.record_green("rysweet/x", "sha-A");
    assert!(cache.is_green("rysweet/x", "sha-A"));
    assert!(!cache.is_green("rysweet/x", "sha-B"));
    assert!(!cache.is_green("rysweet/y", "sha-A"));
    // An empty head SHA never matches, even against an (impossible) empty entry.
    assert!(!cache.is_green("rysweet/x", ""));
    // record_green ignores an empty SHA (never caches a blank).
    cache.record_green("rysweet/z", "");
    assert_eq!(cache.get("rysweet/z"), None);
}

#[test]
fn cache_round_trips_through_disk_and_tolerates_missing_or_corrupt() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("nested").join("ci_health_green_sha.json");

    // Missing file -> empty cache (first run).
    assert!(GreenShaCache::load(&path).is_empty());

    let mut cache = GreenShaCache::empty();
    cache.record_green("rysweet/Simard", "deadbeef");
    cache.record_green("rysweet/azlin", "cafef00d");
    cache.save(&path).unwrap();

    let loaded = GreenShaCache::load(&path);
    assert_eq!(loaded.get("rysweet/Simard"), Some("deadbeef"));
    assert_eq!(loaded.get("rysweet/azlin"), Some("cafef00d"));
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded, cache);

    // Corrupt file -> empty cache (never blocks a sweep).
    std::fs::write(&path, b"{ this is not json").unwrap();
    assert!(GreenShaCache::load(&path).is_empty());
}

// ────────────────── CI-health → deduplicated-issue steward ───────────────────
//
// These pin the `steward` module that converts an actionable-failure report
// into deduplicated tracking issues (the "one issue per distinct failure" half
// of the standing CI-health goal), reusing the stewardship dedup contract.
mod steward_issue_filing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::super::classify::{ActionableFailure, FleetReport};
    use super::super::steward::{ci_failure_signature, file_issues_for_report};
    use crate::error::SimardError;
    use crate::stewardship::{GhClient, GhIssue, StewardshipOutcome};

    #[derive(Default)]
    struct FakeGhClient {
        /// Pre-seeded `search_issues` responses, keyed by (repo, signature).
        search: Mutex<HashMap<(String, String), Vec<GhIssue>>>,
        /// When set, every `search_issues` call returns this error.
        search_error: Mutex<Option<String>>,
        search_calls: Mutex<Vec<(String, String)>>,
        create_calls: Mutex<Vec<(String, String, String)>>,
        /// Monotonic issue-number source for created issues.
        next_number: Mutex<u64>,
    }

    impl FakeGhClient {
        fn new() -> Self {
            Self {
                next_number: Mutex::new(100),
                ..Self::default()
            }
        }
        fn seed_existing(&self, repo: &str, sig: &str, issues: Vec<GhIssue>) {
            self.search
                .lock()
                .unwrap()
                .insert((repo.to_string(), sig.to_string()), issues);
        }
        fn fail_search(&self, reason: &str) {
            *self.search_error.lock().unwrap() = Some(reason.to_string());
        }
        fn search_calls(&self) -> Vec<(String, String)> {
            self.search_calls.lock().unwrap().clone()
        }
        fn create_calls(&self) -> Vec<(String, String, String)> {
            self.create_calls.lock().unwrap().clone()
        }
    }

    impl GhClient for FakeGhClient {
        fn search_issues(&self, repo: &str, signature: &str) -> Result<Vec<GhIssue>, SimardError> {
            self.search_calls
                .lock()
                .unwrap()
                .push((repo.to_string(), signature.to_string()));
            if let Some(reason) = self.search_error.lock().unwrap().clone() {
                return Err(SimardError::StewardshipGhCommandFailed { reason });
            }
            Ok(self
                .search
                .lock()
                .unwrap()
                .get(&(repo.to_string(), signature.to_string()))
                .cloned()
                .unwrap_or_default())
        }

        fn create_issue(
            &self,
            repo: &str,
            title: &str,
            body: &str,
        ) -> Result<GhIssue, SimardError> {
            self.create_calls.lock().unwrap().push((
                repo.to_string(),
                title.to_string(),
                body.to_string(),
            ));
            let mut n = self.next_number.lock().unwrap();
            let number = *n;
            *n += 1;
            Ok(GhIssue {
                number,
                url: format!("https://github.com/{repo}/issues/{number}"),
                title: title.to_string(),
                body: body.to_string(),
            })
        }
    }

    fn af(repo: &str, workflow: &str, conclusion: &str, run_id: u64) -> ActionableFailure {
        ActionableFailure {
            repo: repo.to_string(),
            default_branch: "main".to_string(),
            workflow: workflow.to_string(),
            conclusion: conclusion.to_string(),
            run_id: Some(run_id),
            run_url: Some(format!("https://github.com/{repo}/actions/runs/{run_id}")),
        }
    }

    fn report(failures: Vec<ActionableFailure>) -> FleetReport {
        FleetReport {
            green: failures.is_empty(),
            repos_checked: 1,
            repos_from_cache: 0,
            workflows_checked: failures.len(),
            actionable_failures: failures,
            repos: Vec::new(),
        }
    }

    #[test]
    fn files_a_new_deduped_issue_per_distinct_failure() {
        let f1 = af("rysweet/amplihack-rs", "Code Atlas", "failure", 1);
        let f2 = af(
            "rysweet/amplihack-rs",
            "Publish Snapshot Release",
            "failure",
            2,
        );
        let gh = FakeGhClient::new();

        let outcomes = file_issues_for_report(&report(vec![f1.clone(), f2.clone()]), &gh).unwrap();

        assert_eq!(outcomes.len(), 2);
        // Every distinct failure is searched (in its own repo, by its signature)
        // and then filed.
        assert_eq!(gh.search_calls().len(), 2);
        assert_eq!(gh.create_calls().len(), 2);
        for outcome in &outcomes {
            match outcome {
                StewardshipOutcome::FiledNew { repo, .. } => {
                    assert_eq!(repo, "rysweet/amplihack-rs")
                }
                other => panic!("expected FiledNew, got {other:?}"),
            }
        }
    }

    #[test]
    fn dedupes_against_an_existing_open_issue_without_filing() {
        let f = af("rysweet/azlin", "CI", "failure", 7);
        let sig = ci_failure_signature(&f);
        let gh = FakeGhClient::new();
        gh.seed_existing(
            "rysweet/azlin",
            &sig,
            vec![GhIssue {
                number: 55,
                url: "https://github.com/rysweet/azlin/issues/55".to_string(),
                title: "[ci-health] CI failing on rysweet/azlin".to_string(),
                body: format!("stewardship-signature: {sig}\n"),
            }],
        );

        let outcomes = file_issues_for_report(&report(vec![f]), &gh).unwrap();

        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            StewardshipOutcome::MatchedExisting {
                repo,
                issue_number,
                signature,
                ..
            } => {
                assert_eq!(repo, "rysweet/azlin");
                assert_eq!(*issue_number, 55);
                assert_eq!(signature, &sig);
            }
            other => panic!("expected MatchedExisting, got {other:?}"),
        }
        // A matched signature must never create a new issue.
        assert!(gh.create_calls().is_empty());
    }

    #[test]
    fn collapses_duplicate_signatures_into_one_issue_per_sweep() {
        // Two workflow files sharing a name / a repeated failure hash to the
        // same signature; only one issue may be filed in a single sweep.
        let f1 = af("rysweet/amplihack-rs", "CI", "failure", 1);
        let f2 = af("rysweet/amplihack-rs", "CI", "timed_out", 2);
        assert_eq!(ci_failure_signature(&f1), ci_failure_signature(&f2));
        let gh = FakeGhClient::new();

        let outcomes = file_issues_for_report(&report(vec![f1, f2]), &gh).unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(gh.create_calls().len(), 1);
    }

    #[test]
    fn green_report_files_nothing_and_touches_no_gh() {
        let gh = FakeGhClient::new();
        let outcomes = file_issues_for_report(&report(vec![]), &gh).unwrap();
        assert!(outcomes.is_empty());
        assert!(gh.search_calls().is_empty());
        assert!(gh.create_calls().is_empty());
    }

    #[test]
    fn a_degraded_search_propagates_and_never_files() {
        let gh = FakeGhClient::new();
        gh.fail_search("`gh issue list` exited 1");
        let err =
            file_issues_for_report(&report(vec![af("rysweet/azlin", "CI", "failure", 9)]), &gh)
                .unwrap_err();
        assert!(matches!(
            err,
            SimardError::StewardshipGhCommandFailed { .. }
        ));
        // Fail-loud: no issue is filed while the search is degraded.
        assert!(gh.create_calls().is_empty());
    }

    #[test]
    fn issue_carries_the_dedup_frontmatter_and_ci_specifics() {
        let f = af("rysweet/amplihack-rs", "Code Atlas", "failure", 42);
        let sig = ci_failure_signature(&f);
        let gh = FakeGhClient::new();

        file_issues_for_report(&report(vec![f]), &gh).unwrap();

        let (repo, title, body) = gh.create_calls().into_iter().next().unwrap();
        assert_eq!(repo, "rysweet/amplihack-rs");
        assert_eq!(
            title,
            "[ci-health] Code Atlas failing on rysweet/amplihack-rs"
        );
        assert!(body.contains("filed-by: simard-stewardship"));
        assert!(body.contains(&format!("stewardship-signature: {sig}")));
        assert!(body.contains("ci-health-repo: rysweet/amplihack-rs"));
        assert!(body.contains("ci-health-workflow: Code Atlas"));
        assert!(body.contains("default-branch: main"));
        assert!(body.contains("latest-conclusion: failure"));
        assert!(body.contains("https://github.com/rysweet/amplihack-rs/actions/runs/42"));
    }

    #[test]
    fn signature_is_stable_across_volatile_run_id_and_conclusion() {
        // Same broken repo+workflow, different run id and a failure/timed_out
        // flap, must hash identically so re-sweeps dedupe.
        let a = af("rysweet/azlin", "CI", "failure", 1);
        let b = af("rysweet/azlin", "CI", "timed_out", 999);
        assert_eq!(ci_failure_signature(&a), ci_failure_signature(&b));
        // A different workflow in the same repo is a distinct failure.
        let c = af("rysweet/azlin", "Rust CI", "failure", 1);
        assert_ne!(ci_failure_signature(&a), ci_failure_signature(&c));
        // The same workflow in a different repo is a distinct failure.
        let d = af("rysweet/Simard", "CI", "failure", 1);
        assert_ne!(ci_failure_signature(&a), ci_failure_signature(&d));
    }
}
