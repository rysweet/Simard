/// Persist cycle report to `<state_root>/cycle_reports/cycle_<N>.json`.
pub(super) fn persist_cycle_report(
    state_root: &std::path::Path,
    report: &crate::ooda_loop::CycleReport,
) {
    let dir = state_root.join("cycle_reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("cycle_{}.json", report.cycle_number));
    let summary = crate::ooda_loop::summarize_cycle_report(report);
    // Write a lightweight summary rather than serializing the full report.
    let _ = std::fs::write(&path, summary);
}

/// Persist cycle results to cognitive memory as an episodic record.
///
/// Records the cycle summary and outcome counts so that future OODA cycles
/// and goal curation sessions can recall what happened. Best-effort: failures
/// are logged but do not abort the daemon.
pub(super) fn persist_cycle_to_memory(
    bridges: &crate::ooda_loop::OodaBridges,
    report: &crate::ooda_loop::CycleReport,
) {
    use serde_json::json;

    let summary = crate::ooda_loop::summarize_cycle_report(report);
    let succeeded = report.outcomes.iter().filter(|o| o.success).count();
    let failed = report.outcomes.len() - succeeded;

    let metadata = json!({
        "cycle_number": report.cycle_number,
        "actions_succeeded": succeeded,
        "actions_failed": failed,
        "goal_count": report.observation.goal_statuses.len(),
        "open_issues": report.observation.environment.open_issues.len(),
    });

    if let Err(e) = bridges
        .memory
        .store_episode(&summary, "ooda-daemon", Some(&metadata))
    {
        eprintln!("[simard] OODA persist: failed to store episode: {e}");
    }
}
