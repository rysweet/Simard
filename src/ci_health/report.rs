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
