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
    use super::super::diagnose::{FailedJob, RunDiagnosis, RunDiagnostics};
    use super::super::steward::{UnauthorizedSkip, ci_failure_signature, file_issues_for_report};
    use crate::error::SimardError;
    use crate::stewardship::{GhClient, GhIssue, StewardshipOutcome};

    #[derive(Default)]
    struct FakeGhClient {
        /// Pre-seeded `search_issues` responses, keyed by (repo, signature).
        search: Mutex<HashMap<(String, String), Vec<GhIssue>>>,
        /// When set, every `search_issues` call returns this error.
        search_error: Mutex<Option<String>>,
        /// Repos whose `create_issue` returns the given error (e.g. a cross-repo
        /// authorization denial), keyed by repo slug.
        create_error_for: Mutex<HashMap<String, String>>,
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
        /// Make `create_issue` fail for exactly `repo` with `reason` (used to
        /// simulate a cross-repo `issues:write` denial on one sibling).
        fn fail_create_for(&self, repo: &str, reason: &str) {
            self.create_error_for
                .lock()
                .unwrap()
                .insert(repo.to_string(), reason.to_string());
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
            if let Some(reason) = self.create_error_for.lock().unwrap().get(repo).cloned() {
                return Err(SimardError::StewardshipGhCommandFailed { reason });
            }
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

    /// Fake [`RunDiagnostics`] for the steward filing tests. Records its calls
    /// so tests can assert diagnosis is invoked exactly for newly-filed issues
    /// (and never for matched-existing ones).
    #[derive(Default)]
    struct FakeDiagnostics {
        /// When set, every `diagnose` call returns this error.
        error: Option<String>,
        /// Failing jobs returned on success (empty = "no failing job found").
        jobs: Vec<FailedJob>,
        calls: Mutex<Vec<(String, u64)>>,
    }

    impl FakeDiagnostics {
        /// A canned single failing job+step, so a filed body has a concrete
        /// root cause to assert on.
        fn new() -> Self {
            Self {
                jobs: vec![FailedJob {
                    name: "build".to_string(),
                    conclusion: "failure".to_string(),
                    job_id: Some(101),
                    failed_steps: vec!["compile".to_string()],
                    annotations: vec!["`error[E0432]: unresolved import`".to_string()],
                }],
                ..Self::default()
            }
        }
        fn failing(reason: &str) -> Self {
            Self {
                error: Some(reason.to_string()),
                ..Self::default()
            }
        }
        fn calls(&self) -> Vec<(String, u64)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl RunDiagnostics for FakeDiagnostics {
        fn diagnose(&self, repo: &str, run_id: u64) -> Result<RunDiagnosis, SimardError> {
            self.calls.lock().unwrap().push((repo.to_string(), run_id));
            if let Some(reason) = &self.error {
                return Err(SimardError::CiHealthGhCommandFailed {
                    reason: reason.clone(),
                });
            }
            Ok(RunDiagnosis {
                run_id,
                failed_jobs: self.jobs.clone(),
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

        let outcomes = file_issues_for_report(
            &report(vec![f1.clone(), f2.clone()]),
            &gh,
            &FakeDiagnostics::new(),
        )
        .unwrap()
        .outcomes;

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

        let diag = FakeDiagnostics::new();
        let outcomes = file_issues_for_report(&report(vec![f]), &gh, &diag)
            .unwrap()
            .outcomes;

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
        // …nor spend a diagnosis `gh` call: an already-tracked failure is a
        // zero-network no-op beyond the dedup search.
        assert!(diag.calls().is_empty());
    }

    #[test]
    fn collapses_duplicate_signatures_into_one_issue_per_sweep() {
        // Two workflow files sharing a name / a repeated failure hash to the
        // same signature; only one issue may be filed in a single sweep.
        let f1 = af("rysweet/amplihack-rs", "CI", "failure", 1);
        let f2 = af("rysweet/amplihack-rs", "CI", "timed_out", 2);
        assert_eq!(ci_failure_signature(&f1), ci_failure_signature(&f2));
        let gh = FakeGhClient::new();

        let outcomes = file_issues_for_report(&report(vec![f1, f2]), &gh, &FakeDiagnostics::new())
            .unwrap()
            .outcomes;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(gh.create_calls().len(), 1);
    }

    #[test]
    fn green_report_files_nothing_and_touches_no_gh() {
        let gh = FakeGhClient::new();
        let diag = FakeDiagnostics::new();
        let outcomes = file_issues_for_report(&report(vec![]), &gh, &diag)
            .unwrap()
            .outcomes;
        assert!(outcomes.is_empty());
        assert!(gh.search_calls().is_empty());
        assert!(gh.create_calls().is_empty());
        assert!(diag.calls().is_empty());
    }

    #[test]
    fn a_degraded_search_propagates_and_never_files() {
        let gh = FakeGhClient::new();
        gh.fail_search("`gh issue list` exited 1");
        let diag = FakeDiagnostics::new();
        let err = file_issues_for_report(
            &report(vec![af("rysweet/azlin", "CI", "failure", 9)]),
            &gh,
            &diag,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::StewardshipGhCommandFailed { .. }
        ));
        // Fail-loud: no issue is filed while the search is degraded.
        assert!(gh.create_calls().is_empty());
        // A degraded search short-circuits before diagnosis is even attempted.
        assert!(diag.calls().is_empty());
    }

    #[test]
    fn issue_carries_the_dedup_frontmatter_and_ci_specifics() {
        let f = af("rysweet/amplihack-rs", "Code Atlas", "failure", 42);
        let sig = ci_failure_signature(&f);
        let gh = FakeGhClient::new();

        file_issues_for_report(&report(vec![f]), &gh, &FakeDiagnostics::new()).unwrap();

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
    fn a_new_issue_embeds_the_root_cause_diagnosis() {
        let f = af("rysweet/amplihack-rs", "Code Atlas", "failure", 42);
        let gh = FakeGhClient::new();
        let diag = FakeDiagnostics::new();

        file_issues_for_report(&report(vec![f]), &gh, &diag).unwrap();

        // Diagnosis was fetched for exactly the failing run, in its repo.
        assert_eq!(diag.calls(), vec![("rysweet/amplihack-rs".to_string(), 42)]);
        let (_, _, body) = gh.create_calls().into_iter().next().unwrap();
        assert!(body.contains("## Root cause"), "body was:\n{body}");
        // The canned failing job+step are named so a fixer needn't re-fetch logs.
        assert!(body.contains("job `build`"), "body was:\n{body}");
        assert!(body.contains("step `compile`"), "body was:\n{body}");
        // …and the concrete error text (the job's failure annotation) is embedded
        // so the fixer sees *what* broke, not only *which* step.
        assert!(
            body.contains("error[E0432]: unresolved import"),
            "body was:\n{body}"
        );
    }

    #[test]
    fn a_diagnosis_error_still_files_the_issue_marked_unavailable() {
        // Filing the tracking issue is the correctness-critical act; a diagnosis
        // that cannot be fetched must not abort it — the issue is filed anyway,
        // recording that the root cause is unavailable (no silent degradation).
        let f = af("rysweet/amplihack-rs", "Auto Release", "failure", 7);
        let gh = FakeGhClient::new();
        let diag = FakeDiagnostics::failing("`gh run view` exited 1");

        let outcomes = file_issues_for_report(&report(vec![f]), &gh, &diag)
            .unwrap()
            .outcomes;

        assert!(matches!(
            outcomes.as_slice(),
            [StewardshipOutcome::FiledNew { .. }]
        ));
        let (_, _, body) = gh.create_calls().into_iter().next().unwrap();
        assert!(body.contains("Diagnosis unavailable"), "body was:\n{body}");
        assert!(body.contains("`gh run view` exited 1"), "body was:\n{body}");
        // The run link is still offered for manual investigation.
        assert!(body.contains("https://github.com/rysweet/amplihack-rs/actions/runs/7"));
    }

    #[test]
    fn a_failure_without_a_run_id_records_diagnosis_unavailable() {
        let mut f = af("rysweet/amplihack-rs", "Code Atlas", "failure", 1);
        f.run_id = None;
        f.run_url = None;
        let gh = FakeGhClient::new();
        let diag = FakeDiagnostics::new();

        file_issues_for_report(&report(vec![f]), &gh, &diag).unwrap();

        // With no run id there is nothing to diagnose; the fetch is skipped.
        assert!(diag.calls().is_empty());
        let (_, _, body) = gh.create_calls().into_iter().next().unwrap();
        assert!(body.contains("Diagnosis unavailable"), "body was:\n{body}");
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

    #[test]
    fn a_cross_repo_write_denial_is_skipped_not_fatal_and_other_repos_still_file() {
        // The production failure mode: a governed *sibling* repo is red, but the
        // scheduled sweep's token (the repo-scoped default `GITHUB_TOKEN`, when
        // `STEWARD_GH_TOKEN` is absent) cannot create issues there — GitHub
        // returns "Resource not accessible by integration". This must NOT abort
        // the sweep: the sibling is recorded as an unauthorized skip and every
        // *writable* repo (Simard's own failure included) still gets its issue.
        let writable = af("rysweet/Simard", "verify", "failure", 1);
        let sibling = af(
            "rysweet/amplihack-recipe-runner",
            "Bot Detection",
            "failure",
            2,
        );
        let gh = FakeGhClient::new();
        gh.fail_create_for(
            "rysweet/amplihack-recipe-runner",
            "`gh issue create -R rysweet/amplihack-recipe-runner` exited exit status: 1 \
             with stderr:\nGraphQL: Resource not accessible by integration (createIssue)",
        );

        let filing = file_issues_for_report(
            &report(vec![writable, sibling]),
            &gh,
            &FakeDiagnostics::new(),
        )
        .expect("a cross-repo write denial must not fail the sweep");

        // Simard's own failure is filed; the unwritable sibling is skipped.
        assert_eq!(filing.outcomes.len(), 1, "the writable repo is still filed");
        match &filing.outcomes[0] {
            StewardshipOutcome::FiledNew { repo, .. } => assert_eq!(repo, "rysweet/Simard"),
            other => panic!("expected FiledNew for rysweet/Simard, got {other:?}"),
        }
        assert_eq!(filing.skipped_unauthorized.len(), 1);
        let skip = &filing.skipped_unauthorized[0];
        assert_eq!(skip.repo, "rysweet/amplihack-recipe-runner");
        assert_eq!(skip.workflow.as_deref(), Some("Bot Detection"));
        assert!(
            skip.reason.contains("not accessible by integration"),
            "skip names the underlying gh error: {}",
            skip.reason
        );
        // Both repos' creates were attempted; the sibling's was denied (not
        // silently skipped before the call), the writable one succeeded.
        let created: Vec<String> = gh.create_calls().into_iter().map(|c| c.0).collect();
        assert_eq!(created.len(), 2);
        assert!(created.contains(&"rysweet/Simard".to_string()));
        assert!(created.contains(&"rysweet/amplihack-recipe-runner".to_string()));
    }

    #[test]
    fn a_forbidden_403_write_denial_is_also_treated_as_an_authorization_skip() {
        // A PAT/App variant returns an HTTP 403 rather than the integration
        // phrasing; it is the same "token can't write here" condition and must
        // likewise be a reported skip, not a fatal error.
        let f = af("rysweet/amplihack-rs", "CI", "failure", 3);
        let gh = FakeGhClient::new();
        gh.fail_create_for(
            "rysweet/amplihack-rs",
            "`gh issue create` exited 1 with stderr:\nHTTP 403: Must have admin rights to Repository.",
        );

        let filing = file_issues_for_report(&report(vec![f]), &gh, &FakeDiagnostics::new())
            .expect("a 403 write denial must not fail the sweep");

        assert!(filing.outcomes.is_empty());
        assert_eq!(filing.skipped_unauthorized.len(), 1);
        assert_eq!(filing.skipped_unauthorized[0].repo, "rysweet/amplihack-rs");
    }

    #[test]
    fn a_non_authorization_gh_error_still_fails_loud() {
        // A genuine/transient `gh` failure (not a permission denial) must still
        // propagate — no silent degradation of an unexpected error.
        let f = af("rysweet/amplihack-rs", "CI", "failure", 4);
        let gh = FakeGhClient::new();
        gh.fail_create_for(
            "rysweet/amplihack-rs",
            "`gh issue create` exited 1 with stderr:\nerror connecting to api.github.com",
        );

        let err = file_issues_for_report(&report(vec![f]), &gh, &FakeDiagnostics::new())
            .expect_err("a non-authorization gh error must fail loud");
        assert!(matches!(
            err,
            SimardError::StewardshipGhCommandFailed { .. }
        ));
    }

    #[test]
    fn a_403_secondary_rate_limit_fails_loud_and_is_not_masked_as_an_auth_skip() {
        // GitHub returns secondary/abuse rate limits as HTTP 403 too, but those
        // are transient — NOT a permanent permission denial. A fleet sweep firing
        // bursts of writes is exactly what trips them, so they must fail loud (or
        // be retried), never be downgraded to a skip that leaves the run green
        // with a real failure untracked.
        let f = af("rysweet/amplihack-rs", "CI", "failure", 5);
        let gh = FakeGhClient::new();
        gh.fail_create_for(
            "rysweet/amplihack-rs",
            "`gh issue create` exited 1 with stderr:\nHTTP 403: You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
        );

        let err = file_issues_for_report(&report(vec![f]), &gh, &FakeDiagnostics::new())
            .expect_err("a 403 rate-limit must fail loud, not be masked as an auth skip");
        assert!(matches!(
            err,
            SimardError::StewardshipGhCommandFailed { .. }
        ));
    }

    #[test]
    fn the_unauthorized_skip_type_is_constructible() {
        // Guard the public shape the CLI reporter depends on.
        let skip = UnauthorizedSkip {
            repo: "rysweet/x".to_string(),
            workflow: Some("CI".to_string()),
            reason: "Resource not accessible by integration".to_string(),
        };
        assert_eq!(skip.repo, "rysweet/x");
        assert_eq!(skip.workflow.as_deref(), Some("CI"));
    }
}

mod steward_issue_resolution {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::super::classify::{ActionableFailure, FleetReport, RepoReport, WorkflowReport};
    use super::super::steward::{
        CiIssueResolver, ci_failure_signature, ci_signature_for, is_ci_health_tracking_issue,
        resolve_issues_for_report,
    };
    use crate::error::SimardError;
    use crate::stewardship::GhIssue;

    /// Fake [`CiIssueResolver`]: serves a pre-seeded per-repo list of open
    /// tracking issues and records every list/close call, with optional injected
    /// errors on either operation.
    #[derive(Default)]
    struct FakeResolver {
        /// Open tracking issues to return from `list_open_tracking_issues`, keyed
        /// by repo slug.
        issues: Mutex<HashMap<String, Vec<GhIssue>>>,
        list_error: Mutex<Option<String>>,
        close_error: Mutex<Option<String>>,
        list_calls: Mutex<Vec<String>>,
        close_calls: Mutex<Vec<(String, u64, String)>>,
    }

    impl FakeResolver {
        fn new() -> Self {
            Self::default()
        }
        /// Seed an open tracking issue carrying `sig` in its body, so
        /// `list_open_tracking_issues`+`find_existing` resolves it.
        fn seed_issue(&self, repo: &str, sig: &str, number: u64) {
            self.issues
                .lock()
                .unwrap()
                .entry(repo.to_string())
                .or_default()
                .push(GhIssue {
                    number,
                    url: format!("https://github.com/{repo}/issues/{number}"),
                    title: format!("[ci-health] failing on {repo}"),
                    body: format!("filed-by: simard-stewardship\nstewardship-signature: {sig}\n"),
                });
        }
        fn fail_list(&self, reason: &str) {
            *self.list_error.lock().unwrap() = Some(reason.to_string());
        }
        fn fail_close(reason: &str) -> Self {
            Self {
                close_error: Mutex::new(Some(reason.to_string())),
                ..Self::default()
            }
        }
        fn list_calls(&self) -> Vec<String> {
            self.list_calls.lock().unwrap().clone()
        }
        fn close_calls(&self) -> Vec<(String, u64, String)> {
            self.close_calls.lock().unwrap().clone()
        }
    }

    impl CiIssueResolver for FakeResolver {
        fn list_open_tracking_issues(&self, repo: &str) -> Result<Vec<GhIssue>, SimardError> {
            self.list_calls.lock().unwrap().push(repo.to_string());
            if let Some(reason) = self.list_error.lock().unwrap().clone() {
                return Err(SimardError::CiHealthGhCommandFailed { reason });
            }
            Ok(self
                .issues
                .lock()
                .unwrap()
                .get(repo)
                .cloned()
                .unwrap_or_default())
        }

        fn close_issue(&self, repo: &str, number: u64, comment: &str) -> Result<(), SimardError> {
            self.close_calls
                .lock()
                .unwrap()
                .push((repo.to_string(), number, comment.to_string()));
            if let Some(reason) = self.close_error.lock().unwrap().clone() {
                return Err(SimardError::CiHealthGhCommandFailed { reason });
            }
            Ok(())
        }
    }

    fn wf(name: &str, verdict: &str, run_id: Option<u64>) -> WorkflowReport {
        WorkflowReport {
            name: name.to_string(),
            verdict: verdict.to_string(),
            conclusion: None,
            reason: None,
            run_id,
        }
    }

    /// A one-repo fresh report with the given workflows.
    fn report(slug: &str, workflows: Vec<WorkflowReport>) -> FleetReport {
        FleetReport {
            green: !workflows.iter().any(|w| w.verdict == "actionable_failure"),
            repos_checked: 1,
            repos_from_cache: 0,
            workflows_checked: workflows.len(),
            actionable_failures: Vec::new(),
            repos: vec![RepoReport {
                slug: slug.to_string(),
                default_branch: "main".to_string(),
                green_from_cache: false,
                workflows,
            }],
        }
    }

    #[test]
    fn closes_the_tracking_issue_of_a_now_green_workflow() {
        let repo = "rysweet/amplihack-rs";
        let sig = ci_signature_for(repo, "Code Atlas");
        let resolver = FakeResolver::new();
        resolver.seed_issue(repo, &sig, 938);

        let outcomes = resolve_issues_for_report(
            &report(repo, vec![wf("Code Atlas", "green", Some(42))]),
            &resolver,
        )
        .unwrap()
        .closed;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].repo, repo);
        assert_eq!(outcomes[0].workflow, "Code Atlas");
        assert_eq!(outcomes[0].issue_number, 938);
        assert_eq!(outcomes[0].signature, sig);
        // The issue was actually closed, with a green-evidence comment linking
        // the now-green run.
        let calls = resolver.close_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, repo);
        assert_eq!(calls[0].1, 938);
        assert!(calls[0].2.contains("now green"), "comment: {}", calls[0].2);
        assert!(
            calls[0]
                .2
                .contains("https://github.com/rysweet/amplihack-rs/actions/runs/42"),
            "comment: {}",
            calls[0].2
        );
        assert!(calls[0].2.contains("Code Atlas"), "comment: {}", calls[0].2);
        // The repo's open issues were listed exactly once (O(repos), not
        // O(green-workflows)).
        assert_eq!(resolver.list_calls(), vec![repo.to_string()]);
    }

    #[test]
    fn the_resolution_signature_matches_the_filing_signature() {
        // The signature resolution keys on must equal the one filing used, or a
        // filed issue would never be found to close.
        let f = ActionableFailure {
            repo: "rysweet/azlin".to_string(),
            default_branch: "main".to_string(),
            workflow: "CI".to_string(),
            conclusion: "failure".to_string(),
            run_id: Some(1),
            run_url: Some("https://github.com/rysweet/azlin/actions/runs/1".to_string()),
        };
        assert_eq!(
            ci_signature_for("rysweet/azlin", "CI"),
            ci_failure_signature(&f)
        );
    }

    #[test]
    fn a_green_workflow_without_a_tracking_issue_closes_nothing() {
        let repo = "rysweet/Simard";
        let resolver = FakeResolver::new(); // no seeded issue → empty list

        let outcomes = resolve_issues_for_report(
            &report(repo, vec![wf("verify", "green", Some(9))]),
            &resolver,
        )
        .unwrap()
        .closed;

        assert!(outcomes.is_empty());
        assert!(resolver.close_calls().is_empty());
        // It still *listed* the repo (that is how it learns there is no issue),
        // but closed nothing.
        assert_eq!(resolver.list_calls(), vec![repo.to_string()]);
    }

    #[test]
    fn a_repo_with_no_green_workflow_is_not_even_listed() {
        // The cheap pre-check skips the list call for a repo that has no green
        // workflow to resolve.
        let repo = "rysweet/amplihack-rs";
        let resolver = FakeResolver::new();

        let outcomes = resolve_issues_for_report(
            &report(
                repo,
                vec![
                    wf("CI", "actionable_failure", Some(8)),
                    wf("Auto Release", "ignored", None),
                ],
            ),
            &resolver,
        )
        .unwrap()
        .closed;

        assert!(outcomes.is_empty());
        assert!(resolver.list_calls().is_empty());
        assert!(resolver.close_calls().is_empty());
    }

    #[test]
    fn only_green_workflows_are_considered_for_resolution() {
        // A failing and an ignored (in-progress) workflow must never close an
        // issue — only the green one is a resolution candidate.
        let repo = "rysweet/amplihack-rs";
        let resolver = FakeResolver::new();
        let green_sig = ci_signature_for(repo, "Publish Snapshot Release");
        resolver.seed_issue(repo, &green_sig, 940);
        // Seed an issue for the still-failing workflow too — it must NOT be closed.
        resolver.seed_issue(repo, &ci_signature_for(repo, "CI"), 100);

        let outcomes = resolve_issues_for_report(
            &report(
                repo,
                vec![
                    wf("Publish Snapshot Release", "green", Some(7)),
                    wf("CI", "actionable_failure", Some(8)),
                    wf("Auto Release", "ignored", None),
                ],
            ),
            &resolver,
        )
        .unwrap()
        .closed;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].issue_number, 940);
        // Exactly one list (the repo, once); no per-workflow calls.
        assert_eq!(resolver.list_calls(), vec![repo.to_string()]);
        // Only the green workflow's issue is closed; #100 stays open.
        let closed: Vec<u64> = resolver.close_calls().into_iter().map(|c| c.1).collect();
        assert_eq!(closed, vec![940]);
    }

    #[test]
    fn cache_served_repos_are_skipped() {
        let resolver = FakeResolver::new();
        let mut r = report("rysweet/RustyClawd", Vec::new());
        r.repos[0].green_from_cache = true;
        r.repos_from_cache = 1;

        let outcomes = resolve_issues_for_report(&r, &resolver).unwrap().closed;

        assert!(outcomes.is_empty());
        assert!(resolver.list_calls().is_empty());
        assert!(resolver.close_calls().is_empty());
    }

    #[test]
    fn a_degraded_list_propagates_and_closes_nothing() {
        let resolver = FakeResolver::new();
        resolver.fail_list("`gh issue list` exited 1");

        let err = resolve_issues_for_report(
            &report("rysweet/azlin", vec![wf("CI", "green", Some(1))]),
            &resolver,
        )
        .unwrap_err();

        assert!(matches!(err, SimardError::CiHealthGhCommandFailed { .. }));
        // Fail-loud: nothing is closed while the list is degraded.
        assert!(resolver.close_calls().is_empty());
    }

    #[test]
    fn a_close_failure_propagates() {
        let repo = "rysweet/azlin";
        let sig = ci_signature_for(repo, "CI");
        let resolver = FakeResolver::fail_close("`gh issue close` exited 1");
        resolver.seed_issue(repo, &sig, 55);

        let err =
            resolve_issues_for_report(&report(repo, vec![wf("CI", "green", Some(2))]), &resolver)
                .unwrap_err();

        assert!(matches!(err, SimardError::CiHealthGhCommandFailed { .. }));
    }

    #[test]
    fn comment_without_a_run_id_names_the_run_generically() {
        let repo = "rysweet/azlin";
        let sig = ci_signature_for(repo, "Rust CI");
        let resolver = FakeResolver::new();
        resolver.seed_issue(repo, &sig, 77);

        resolve_issues_for_report(&report(repo, vec![wf("Rust CI", "green", None)]), &resolver)
            .unwrap();

        let calls = resolver.close_calls();
        assert_eq!(calls.len(), 1);
        // No fabricated run URL when the id was not captured.
        assert!(
            !calls[0].2.contains("/actions/runs/"),
            "comment: {}",
            calls[0].2
        );
        assert!(
            calls[0].2.contains("its latest default-branch run"),
            "comment: {}",
            calls[0].2
        );
    }

    #[test]
    fn two_green_workflows_sharing_a_name_close_the_shared_issue_once() {
        // Two workflow files sharing a `name:` hash to one signature, so filing
        // opened a single issue for the pair. Resolution must close that one
        // issue exactly once — never double-close it (which would post a second
        // comment and, on an already-closed issue, could fail-loud).
        let repo = "rysweet/amplihack-rs";
        let sig = ci_signature_for(repo, "CI");
        let resolver = FakeResolver::new();
        resolver.seed_issue(repo, &sig, 500);

        let outcomes = resolve_issues_for_report(
            &report(
                repo,
                vec![wf("CI", "green", Some(1)), wf("CI", "green", Some(2))],
            ),
            &resolver,
        )
        .unwrap()
        .closed;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].issue_number, 500);
        let closed: Vec<u64> = resolver.close_calls().into_iter().map(|c| c.1).collect();
        assert_eq!(closed, vec![500]);
    }

    #[test]
    fn a_green_workflow_does_not_close_an_issue_a_same_name_sibling_still_fails() {
        // Two workflow *files* share the name `CI` (they classify independently
        // but collapse to one signature/issue): file A is green, file B is an
        // actionable failure. Filing (re-)files/matches the shared issue for the
        // still-broken B, so resolution must NOT close it off the green A —
        // otherwise the operator sees a close/re-open flap and the real failure
        // goes untracked.
        let repo = "rysweet/amplihack-rs";
        let sig = ci_signature_for(repo, "CI");
        let resolver = FakeResolver::new();
        resolver.seed_issue(repo, &sig, 600);

        let mut r = report(
            repo,
            vec![
                wf("CI", "green", Some(1)),
                wf("CI", "actionable_failure", Some(2)),
            ],
        );
        // Mirror what build_report would emit: the failing sibling is a live
        // actionable failure this sweep, keyed on the same signature.
        r.actionable_failures = vec![ActionableFailure {
            repo: repo.to_string(),
            default_branch: "main".to_string(),
            workflow: "CI".to_string(),
            conclusion: "failure".to_string(),
            run_id: Some(2),
            run_url: Some("https://github.com/rysweet/amplihack-rs/actions/runs/2".to_string()),
        }];
        r.green = false;

        let outcomes = resolve_issues_for_report(&r, &resolver).unwrap().closed;

        // Nothing closed: the shared issue is still tracking the failing sibling.
        assert!(outcomes.is_empty(), "outcomes: {outcomes:?}");
        assert!(
            resolver.close_calls().is_empty(),
            "must not close an issue whose signature still has a live failure"
        );
    }

    #[test]
    fn the_local_tracking_issue_filter_selects_only_ci_health_issues() {
        // The production list step over-fetches a repo's open issues (GitHub's
        // tokenizing search cannot select tracking issues reliably) and filters
        // locally by the unique `ci-health-workflow:` marker. That marker must be
        // exactly the one filing embeds, and must not fire on look-alike text
        // that merely mentions the words "ci", "health", or "workflow".
        // A real filed body (marker present) is selected.
        let real = "filed-by: simard-stewardship\nstewardship-signature: abcd1234\n\
                    ci-health-repo: rysweet/azlin\nci-health-workflow: CI\n";
        assert!(is_ci_health_tracking_issue(real));

        // A backlog issue that merely tokenizes to the same search words is NOT
        // selected (this is exactly the false-positive class GitHub search
        // returns, which the local filter must reject).
        let decoy = "Steward CI / GitHub Actions health across all governance repos; \
                     this workflow gap keeps recurring.";
        assert!(!is_ci_health_tracking_issue(decoy));

        // The bare hyphenated word without the trailing `:` field is not a match.
        assert!(!is_ci_health_tracking_issue("see ci-health-workflow docs"));
    }

    #[test]
    fn a_cross_repo_read_denial_during_resolution_is_skipped_not_fatal() {
        // Symmetric to filing: if the token cannot list/close a governed
        // sibling's issues, resolution records an unauthorized skip and moves on
        // rather than aborting the whole sweep (which would turn the scheduled
        // run red on an unwritable sibling).
        let resolver = FakeResolver::new();
        resolver.fail_list(
            "`gh issue list -R rysweet/amplihack-recipe-runner` exited 1: \
             GraphQL: Resource not accessible by integration",
        );

        let resolution = resolve_issues_for_report(
            &report(
                "rysweet/amplihack-recipe-runner",
                vec![wf("CI", "green", Some(1))],
            ),
            &resolver,
        )
        .expect("a cross-repo read denial must not fail resolution");

        assert!(resolution.closed.is_empty());
        assert_eq!(resolution.skipped_unauthorized.len(), 1);
        let skip = &resolution.skipped_unauthorized[0];
        assert_eq!(skip.repo, "rysweet/amplihack-recipe-runner");
        // A repo-level resolution skip carries no single workflow.
        assert!(skip.workflow.is_none());
        assert!(
            skip.reason.contains("not accessible by integration"),
            "skip names the underlying gh error: {}",
            skip.reason
        );
        // Nothing was closed while the repo was unreadable.
        assert!(resolver.close_calls().is_empty());
    }
}

mod diagnosis {
    use super::super::diagnose::{
        FailedJob, RunDiagnosis, parse_failure_annotations, parse_run_diagnosis,
    };

    const RUN_URL: &str = "https://github.com/rysweet/amplihack-rs/actions/runs/42";

    #[test]
    fn parses_only_failing_jobs_and_their_failing_steps() {
        // One failing job with a mix of step conclusions, plus a fully-green
        // job that must be dropped entirely.
        let json = br#"{
          "jobs": [
            {
              "name": "build-atlas",
              "databaseId": 555,
              "conclusion": "failure",
              "steps": [
                {"name": "Set up job", "conclusion": "success"},
                {"name": "Install Graphviz", "conclusion": "success"},
                {"name": "Render DOT diagrams", "conclusion": "failure"},
                {"name": "Upload atlas artifact", "conclusion": "skipped"}
              ]
            },
            {
              "name": "lint",
              "conclusion": "success",
              "steps": [{"name": "clippy", "conclusion": "success"}]
            }
          ]
        }"#;
        let d = parse_run_diagnosis(42, json).unwrap();
        assert_eq!(
            d,
            RunDiagnosis {
                run_id: 42,
                failed_jobs: vec![FailedJob {
                    name: "build-atlas".to_string(),
                    conclusion: "failure".to_string(),
                    job_id: Some(555),
                    failed_steps: vec!["Render DOT diagrams".to_string()],
                    annotations: vec![],
                }],
            }
        );
        assert!(!d.is_empty());
    }

    #[test]
    fn treats_timed_out_and_startup_failure_as_failing_but_not_cancelled() {
        let json = br#"{
          "jobs": [
            {"name": "a", "conclusion": "timed_out", "steps": []},
            {"name": "b", "conclusion": "startup_failure", "steps": []},
            {"name": "c", "conclusion": "cancelled", "steps": []},
            {"name": "d", "conclusion": "", "steps": []}
          ]
        }"#;
        let d = parse_run_diagnosis(1, json).unwrap();
        let names: Vec<_> = d.failed_jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn empty_jobs_is_a_diagnosable_empty_result() {
        let d = parse_run_diagnosis(9, br#"{"jobs": []}"#).unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn a_missing_jobs_key_is_an_error_not_a_false_empty() {
        // `gh run view --json jobs` always includes `jobs`; a response without
        // it is an unexpected shape and must surface as an error (→ diagnosis
        // unavailable), never a confident "no failing job".
        assert!(parse_run_diagnosis(1, br#"{"unexpected": "shape"}"#).is_err());
    }

    #[test]
    fn malformed_json_is_an_error_not_a_silent_empty() {
        assert!(parse_run_diagnosis(1, b"not json").is_err());
    }

    #[test]
    fn render_names_each_failing_job_and_step_with_the_run_link() {
        let d = RunDiagnosis {
            run_id: 42,
            failed_jobs: vec![
                FailedJob {
                    name: "build-atlas".to_string(),
                    conclusion: "failure".to_string(),
                    job_id: Some(1),
                    failed_steps: vec!["Render DOT diagrams".to_string()],
                    annotations: vec![],
                },
                FailedJob {
                    name: "Build aarch64".to_string(),
                    conclusion: "failure".to_string(),
                    job_id: Some(2),
                    failed_steps: vec!["Build (native)".to_string(), "Package".to_string()],
                    annotations: vec![],
                },
            ],
        };
        let out = d.render(RUN_URL);
        assert!(out.starts_with("## Root cause\n"));
        assert!(out.contains(&format!("[run 42]({RUN_URL})")));
        assert!(out.contains("- job `build-atlas` \u{2192} step `Render DOT diagrams`"));
        assert!(out.contains("- job `Build aarch64` \u{2192} step `Build (native)`"));
        assert!(out.contains("- job `Build aarch64` \u{2192} step `Package`"));
    }

    #[test]
    fn render_embeds_a_failing_jobs_error_annotations_as_nested_bullets() {
        // Beyond naming the failing step, the job's failure annotations (the
        // concrete error text) are embedded so a fixer sees *what* broke.
        let d = RunDiagnosis {
            run_id: 42,
            failed_jobs: vec![FailedJob {
                name: "test".to_string(),
                conclusion: "failure".to_string(),
                job_id: Some(9),
                failed_steps: vec!["cargo test".to_string()],
                annotations: vec![
                    "`Process completed with exit code 101.`".to_string(),
                    "`src/lib.rs:12`: `error[E0432]: unresolved import`".to_string(),
                ],
            }],
        };
        let out = d.render(RUN_URL);
        // The step is named, and the error text follows as indented sub-bullets.
        assert!(out.contains("- job `test` \u{2192} step `cargo test`"));
        assert!(out.contains("    - `Process completed with exit code 101.`"));
        assert!(out.contains("    - `src/lib.rs:12`: `error[E0432]: unresolved import`"));
    }

    #[test]
    fn render_of_a_stepless_failing_job_still_shows_its_annotations() {
        // A job that failed without any step failing (e.g. timed_out) still
        // surfaces its error annotations under its conclusion line.
        let d = RunDiagnosis {
            run_id: 5,
            failed_jobs: vec![FailedJob {
                name: "deploy".to_string(),
                conclusion: "failure".to_string(),
                job_id: Some(3),
                failed_steps: vec![],
                annotations: vec!["`The self-hosted runner lost communication`".to_string()],
            }],
        };
        let out = d.render(RUN_URL);
        assert!(out.contains("job `deploy` concluded `failure`"));
        assert!(out.contains("    - `The self-hosted runner lost communication`"));
    }

    #[test]
    fn render_of_a_failing_job_with_no_failing_step_reports_its_conclusion() {
        // A stepless failing job (e.g. a timed-out job) is described by its own
        // reported conclusion, not a guessed cause.
        let d = RunDiagnosis {
            run_id: 5,
            failed_jobs: vec![FailedJob {
                name: "deploy".to_string(),
                conclusion: "timed_out".to_string(),
                job_id: Some(4),
                failed_steps: vec![],
                annotations: vec![],
            }],
        };
        let out = d.render(RUN_URL);
        assert!(out.contains("job `deploy` concluded `timed_out`"));
        assert!(out.contains("no individual step reported failing"));
        // No speculation about the cause.
        assert!(!out.contains("setup/teardown"));
    }

    #[test]
    fn render_of_an_empty_diagnosis_points_at_the_run() {
        let d = RunDiagnosis {
            run_id: 7,
            failed_jobs: vec![],
        };
        let out = d.render(RUN_URL);
        assert!(out.starts_with("## Root cause\n"));
        assert!(out.contains("No failing job/step was identified"));
        assert!(out.contains(&format!("[run 7]({RUN_URL})")));
    }

    // ── Failure-annotation parsing ──────────────────────────────────────────

    #[test]
    fn parse_annotations_keeps_only_failure_level_and_formats_message() {
        // A `warning` (deprecation) and a `failure` annotation; only the failure
        // survives. The `.github` placeholder path adds no locus, so the message
        // is rendered plainly.
        let json = br#"[
          {"annotation_level": "warning", "message": "Node.js 20 is deprecated.", "path": ".github", "start_line": 2},
          {"annotation_level": "failure", "message": "Process completed with exit code 101.", "path": ".github", "start_line": 915}
        ]"#;
        let lines = parse_failure_annotations(json).unwrap();
        assert_eq!(
            lines,
            vec!["Process completed with exit code 101.".to_string()]
        );
    }

    #[test]
    fn parse_annotations_prefixes_a_real_path_and_line_locus() {
        // A real source path + line becomes a backticked `path:line` locus; the
        // message — which itself contains backticks — is rendered plainly so its
        // inline code is not double-wrapped into broken markdown.
        let json = br#"[
          {"annotation_level": "failure", "message": "error[E0432]: unresolved import `foo`", "path": "src/lib.rs", "start_line": 12}
        ]"#;
        let lines = parse_failure_annotations(json).unwrap();
        assert_eq!(
            lines,
            vec!["`src/lib.rs:12`: error[E0432]: unresolved import `foo`".to_string()]
        );
    }

    #[test]
    fn parse_annotations_collapses_multiline_and_truncates_long_messages() {
        let long = "x".repeat(500);
        let json = format!(
            r#"[{{"annotation_level": "failure", "message": "line one\n   line two\n{long}", "path": "", "start_line": null}}]"#
        );
        let lines = parse_failure_annotations(json.as_bytes()).unwrap();
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // Newlines/indentation collapsed to single spaces.
        assert!(line.starts_with("line one line two"));
        // Truncated with an ellipsis (bounded length, not the full 500 chars).
        assert!(line.contains('…'));
        assert!(line.chars().count() < 500);
    }

    #[test]
    fn parse_annotations_caps_count_and_notes_the_remainder() {
        // Eight failure annotations → at most five embedded, plus an explicit
        // "(+3 more …)" marker so the truncation is visible, never silent.
        let mut items = String::from("[");
        for i in 0..8 {
            if i > 0 {
                items.push(',');
            }
            items.push_str(&format!(
                r#"{{"annotation_level": "failure", "message": "err {i}", "path": "", "start_line": null}}"#
            ));
        }
        items.push(']');
        let lines = parse_failure_annotations(items.as_bytes()).unwrap();
        assert_eq!(lines.len(), 6, "5 annotations + 1 remainder marker");
        assert_eq!(lines[5], "(+3 more failure annotation(s))");
    }

    #[test]
    fn parse_annotations_of_an_empty_array_is_empty_not_an_error() {
        assert!(parse_failure_annotations(b"[]").unwrap().is_empty());
    }

    #[test]
    fn parse_annotations_of_malformed_json_is_an_error() {
        // So a caller can treat annotations as unavailable, never a false
        // "no error text".
        assert!(parse_failure_annotations(b"not json").is_err());
    }
}

/// The governed roster is derived from the embedded ecosystem single-source-of-
/// truth (`prompt_assets/simard/ecosystem_repos.toml`) rather than a second
/// hardcoded list, so a repo added there is swept with no code change and the
/// two stewards can never disagree about who is governed.
mod governed_roster {
    use crate::ci_health::governed_repos;

    #[test]
    fn embedded_roster_parses_to_a_nonempty_validated_fleet() {
        let roster = governed_repos().expect("embedded ecosystem roster must parse");
        // A non-empty roster is the whole point: an empty fleet would classify as
        // zero actionable failures and report GREEN — the false-green this sweep
        // exists to prevent. Fail-loud is asserted by the `expect` above.
        assert!(
            !roster.is_empty(),
            "governed roster must not be empty (empty = false-green fleet)"
        );
        // Every entry is a clean `owner/name` slug (the Overseer validator ran).
        for slug in &roster {
            let parts: Vec<&str> = slug.split('/').collect();
            assert_eq!(
                parts.len(),
                2,
                "slug {slug:?} must be a clean owner/name pair"
            );
            assert!(
                !parts[0].is_empty() && !parts[1].is_empty(),
                "slug {slug:?} must have a non-empty owner and name"
            );
        }
    }

    #[test]
    fn governed_roster_includes_simard_itself_and_has_no_duplicates() {
        let roster = governed_repos().expect("embedded ecosystem roster must parse");
        assert!(
            roster.iter().any(|s| s == "rysweet/Simard"),
            "Simard must sweep its own CI, not only its siblings'"
        );
        let unique: std::collections::HashSet<&String> = roster.iter().collect();
        assert_eq!(
            unique.len(),
            roster.len(),
            "a duplicated slug would sweep (and possibly double-file for) one repo twice"
        );
    }

    #[test]
    fn governed_roster_is_exactly_the_embedded_ecosystem_source_of_truth() {
        // The roster the sweep uses must equal what the Overseer parser reads from
        // the *same* embedded bytes — proving there is a single source of truth
        // and no drift between the CI-health fleet and the ecosystem roster.
        let via_sweep = governed_repos().expect("embedded ecosystem roster must parse");
        let embedded = include_str!("../../prompt_assets/simard/ecosystem_repos.toml");
        let via_parser = crate::overseer::ecosystem_observe::parse_ecosystem_roster(embedded)
            .expect("embedded roster must parse via the Overseer parser too");
        assert_eq!(
            via_sweep, via_parser,
            "CI-health roster must equal the ecosystem-observe roster (single source of truth)"
        );
    }
}
