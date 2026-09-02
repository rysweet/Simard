//! Human-readable rendering of a [`FleetReport`]. The `--json` path serializes
//! the report directly; this module renders the operator-facing table.

use super::classify::FleetReport;

/// Render a fleet report as a plain-text table. The first line is a stable,
/// greppable verdict banner (`CI-HEALTH: GREEN` / `CI-HEALTH: FAILING`).
pub fn render_human(report: &FleetReport) -> String {
    let mut out = String::new();
    let banner = if report.green {
        "CI-HEALTH: GREEN"
    } else {
        "CI-HEALTH: FAILING"
    };
    out.push_str(banner);
    out.push('\n');
    out.push_str(&format!(
        "repos checked: {}   from green cache: {}   workflows checked: {}   actionable failures: {}\n",
        report.repos_checked,
        report.repos_from_cache,
        report.workflows_checked,
        report.actionable_failures.len()
    ));

    if !report.actionable_failures.is_empty() {
        out.push_str(
            "\nActionable failures (active workflow, latest default-branch run failed):\n",
        );
        for af in &report.actionable_failures {
            out.push_str(&format!(
                "  ! {repo} [{branch}] {wf} -> {concl}{url}\n",
                repo = af.repo,
                branch = af.default_branch,
                wf = af.workflow,
                concl = af.conclusion,
                url = af
                    .run_url
                    .as_ref()
                    .map(|u| format!("  {u}"))
                    .unwrap_or_default(),
            ));
        }
    }

    out.push_str("\nPer-repo detail:\n");
    for repo in &report.repos {
        out.push_str(&format!("  {} [{}]\n", repo.slug, repo.default_branch));
        if repo.green_from_cache {
            out.push_str(
                "    [cache] green by cache — head SHA unchanged since last green sweep\n",
            );
            continue;
        }
        for wf in &repo.workflows {
            let marker = match wf.verdict.as_str() {
                "green" => "ok  ",
                "actionable_failure" => "FAIL",
                _ => "skip",
            };
            let detail = match wf.verdict.as_str() {
                "actionable_failure" => {
                    format!(" ({})", wf.conclusion.clone().unwrap_or_default())
                }
                "ignored" => format!(" ({})", wf.reason.clone().unwrap_or_default()),
                _ => String::new(),
            };
            out.push_str(&format!("    [{marker}] {}{detail}\n", wf.name));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::render_human;
    use crate::ci_health::classify::{ActionableFailure, FleetReport, RepoReport, WorkflowReport};

    fn green_workflow(name: &str) -> WorkflowReport {
        WorkflowReport {
            name: name.to_string(),
            verdict: "green".to_string(),
            conclusion: None,
            reason: None,
            run_id: Some(1),
        }
    }

    #[test]
    fn green_report_renders_stable_banner_and_no_failure_section() {
        let report = FleetReport {
            green: true,
            repos_checked: 2,
            repos_from_cache: 1,
            workflows_checked: 3,
            actionable_failures: Vec::new(),
            repos: vec![
                RepoReport {
                    slug: "rysweet/Simard".to_string(),
                    default_branch: "main".to_string(),
                    green_from_cache: false,
                    workflows: vec![green_workflow("verify")],
                },
                RepoReport {
                    slug: "rysweet/Other".to_string(),
                    default_branch: "main".to_string(),
                    green_from_cache: true,
                    workflows: Vec::new(),
                },
            ],
        };

        let rendered = render_human(&report);
        // The banner is the first line and must be greppable.
        assert_eq!(rendered.lines().next(), Some("CI-HEALTH: GREEN"));
        assert!(rendered.contains("actionable failures: 0"));
        // No actionable-failure section for a green fleet.
        assert!(!rendered.contains("Actionable failures ("));
        // Fresh repo lists its workflow as ok; cached repo shows the cache note.
        assert!(rendered.contains("[ok  ] verify"));
        assert!(rendered.contains("green by cache"));
        // A cached repo's workflows are skipped entirely.
        assert!(!rendered.contains("rysweet/Other]\n    [ok"));
    }

    #[test]
    fn failing_report_hoists_actionable_failures_with_url() {
        let report = FleetReport {
            green: false,
            repos_checked: 1,
            repos_from_cache: 0,
            workflows_checked: 2,
            actionable_failures: vec![ActionableFailure {
                repo: "rysweet/Simard".to_string(),
                default_branch: "main".to_string(),
                workflow: "verify".to_string(),
                conclusion: "failure".to_string(),
                run_id: Some(99),
                run_url: Some("https://github.com/rysweet/Simard/actions/runs/99".to_string()),
            }],
            repos: vec![RepoReport {
                slug: "rysweet/Simard".to_string(),
                default_branch: "main".to_string(),
                green_from_cache: false,
                workflows: vec![
                    WorkflowReport {
                        name: "verify".to_string(),
                        verdict: "actionable_failure".to_string(),
                        conclusion: Some("failure".to_string()),
                        reason: None,
                        run_id: Some(99),
                    },
                    WorkflowReport {
                        name: "nightly".to_string(),
                        verdict: "ignored".to_string(),
                        conclusion: None,
                        reason: Some("schedule-only".to_string()),
                        run_id: None,
                    },
                ],
            }],
        };

        let rendered = render_human(&report);
        assert_eq!(rendered.lines().next(), Some("CI-HEALTH: FAILING"));
        assert!(rendered.contains("Actionable failures (active workflow"));
        // The hoisted failure line includes repo, branch, workflow, conclusion, url.
        assert!(rendered
            .contains("! rysweet/Simard [main] verify -> failure  https://github.com/rysweet/Simard/actions/runs/99"));
        // Per-repo detail marks the failing workflow FAIL with its conclusion and
        // the ignored workflow skip with its reason.
        assert!(rendered.contains("[FAIL] verify (failure)"));
        assert!(rendered.contains("[skip] nightly (schedule-only)"));
    }

    #[test]
    fn actionable_failure_without_url_omits_trailing_link() {
        let report = FleetReport {
            green: false,
            repos_checked: 1,
            repos_from_cache: 0,
            workflows_checked: 1,
            actionable_failures: vec![ActionableFailure {
                repo: "rysweet/Simard".to_string(),
                default_branch: "main".to_string(),
                workflow: "verify".to_string(),
                conclusion: "timed_out".to_string(),
                run_id: None,
                run_url: None,
            }],
            repos: Vec::new(),
        };

        let rendered = render_human(&report);
        // Ends the failure line right after the conclusion, with no dangling URL.
        assert!(rendered.contains("! rysweet/Simard [main] verify -> timed_out\n"));
    }
}
