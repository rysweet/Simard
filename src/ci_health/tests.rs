//! Unit tests for the CI-health sweep. These pin the exact distinction that
//! motivated the module: a `disabled` workflow's stale `failure` is **ignored**,
//! while an `active` workflow's `failure` is an **actionable** failure.

use super::classify::{IgnoreReason, WorkflowVerdict, build_report, classify_workflow};
use super::gh::{
    GhWorkflowClient, RawRunRow, RawWorkflowRow, build_repo_snapshot, collect_fleet,
    latest_run_by_workflow, parse_run_rows, snapshot_from_fixture,
};
use super::types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};
use super::{report_to_json, sweep_fixture};
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

fn run_row(wfname: &str, created: &str, status: &str, conclusion: &str, id: u64) -> RawRunRow {
    RawRunRow {
        workflow_name: wfname.to_string(),
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
        run_row("CI", "2026-06-01T00:00:00Z", "completed", "failure", 1),
        run_row("CI", "2026-07-01T00:00:00Z", "completed", "success", 2),
        run_row("Docs", "2026-05-01T00:00:00Z", "completed", "success", 3),
    ];
    let latest = latest_run_by_workflow(&rows);
    assert_eq!(latest.get("CI").unwrap().database_id, 2);
    assert_eq!(latest.get("Docs").unwrap().database_id, 3);
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
        "2026-07-01T00:00:00Z",
        "completed",
        "success",
        7,
    )];
    let snap = build_repo_snapshot("rysweet/foo", "main", &workflows, &runs);
    assert_eq!(snap.workflows.len(), 2);
    let ci = snap.workflows.iter().find(|w| w.name == "CI").unwrap();
    assert_eq!(ci.latest_run.as_ref().unwrap().database_id, 7);
    let nightly = snap.workflows.iter().find(|w| w.name == "Nightly").unwrap();
    assert!(nightly.latest_run.is_none());
}

#[test]
fn parse_run_rows_maps_empty_conclusion_to_none() {
    let json = br#"[{"workflowName":"CI","status":"in_progress","conclusion":"","event":"push","createdAt":"2026-07-06T00:00:00Z","databaseId":5}]"#;
    let rows = parse_run_rows(json).unwrap();
    let snap = build_repo_snapshot(
        "rysweet/foo",
        "main",
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
            run_row("CI", "2026-07-01T00:00:00Z", "completed", "success", 10),
            run_row("Old", "2026-01-01T00:00:00Z", "completed", "failure", 11),
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
    let snap = collect_fleet(&FakeGh, &["rysweet/a", "rysweet/b"]).unwrap();
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
    let snap = collect_fleet(&gh, &["rysweet/x"]).unwrap();
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
