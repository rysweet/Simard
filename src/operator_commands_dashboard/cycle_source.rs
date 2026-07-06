//! Single authoritative source for the dashboard's "Cycle #N" counter.
//!
//! `daemon_health.json` carries a `cycle_number` field. As of issue #1 the
//! OODA daemon writes it from the *durable* brain counter
//! (`OodaState::cycle_count`, seeded at startup from
//! `PersistentGoalState.cycle_count`), so it is monotonic across restarts and
//! no longer resets to `1` on every deploy. The persisted cycle-report files
//! (`cycle_<N>.json`) carry the same cumulative cycle number that the Thinking
//! tab and the Recent Actions feed display.
//!
//! Every dashboard panel that renders "Cycle #N" must read from one source.
//! This module computes that single value: the maximum of the health counter
//! and the highest persisted cycle-report number. Taking the maximum is a
//! belt-and-braces safety net — the two now share the durable, monotonic brain
//! counter, but the `max` also covers the narrow window right after a restart,
//! before the first post-restart cycle has re-stamped `daemon_health.json`, so
//! the historic "#1 after restart" self-contradiction (issue #1680) can never
//! reappear.

use std::path::Path;

use serde_json::{Value, json};

/// Scan the persisted cycle-report directories for the highest cycle number.
///
/// The daemon writes reports to either `<state_root>/cycle_reports/` or
/// `<state_root>/state/cycle_reports/` (mirrors `read_recent_cycle_reports`).
/// Returns `0` when no reports exist. Only filenames are inspected, so this is
/// cheap even with thousands of report files on disk.
pub(crate) fn latest_persisted_cycle_number(state_root: &Path) -> u64 {
    let candidates = [
        state_root.join("cycle_reports"),
        state_root.join("state").join("cycle_reports"),
    ];

    let mut max_cycle = 0u64;
    for dir in &candidates {
        let Ok(listing) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in listing.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(num) = name
                .strip_prefix("cycle_")
                .and_then(|s| s.strip_suffix(".json"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                max_cycle = max_cycle.max(num);
            }
        }
    }
    max_cycle
}

/// Read the process-local `cycle_number` from a parsed `daemon_health.json`.
///
/// Returns `0` when the value is missing or unparseable.
pub(crate) fn health_cycle_number(daemon_health: Option<&Value>) -> u64 {
    daemon_health
        .and_then(|h| h.get("cycle_number"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// The single authoritative cycle number for every dashboard panel.
///
/// Returns the larger of the process-local health counter and the highest
/// persisted cycle-report number so that Overview, Whiteboard, and System
/// Status always agree with the Thinking tab and the Recent Actions feed
/// (issue #1680).
pub(crate) fn authoritative_cycle_number(state_root: &Path, daemon_health: Option<&Value>) -> u64 {
    health_cycle_number(daemon_health).max(latest_persisted_cycle_number(state_root))
}

/// How many of the newest persisted cycle reports the shared cycle-report reader
/// loads before collapsing. Bounded so the unbounded `cycle_reports/` directory
/// (one file per cycle, never pruned) can never flood a hot, repeatedly-polled
/// dashboard endpoint — mirrors the `/api/ooda-cycles` limit (#21).
const MAX_CYCLE_REPORTS: usize = 50;

/// Read the newest persisted cycle reports as RAW report objects (the shape the
/// Thinking tab renders), newest cycle first.
///
/// Reuses [`read_recent_cycle_reports`], which already unions both the top-level
/// `<state_root>/cycle_reports/` and the live `<state_root>/state/cycle_reports/`
/// directories, orders newest-first, and keeps only the newest `n` cycles. Here
/// we unwrap its `{cycle_number, report}` wrapper into the raw report and stamp
/// the report's `cycle_number` with the AUTHORITATIVE FILENAME index — the
/// persisted cumulative number — over the in-body counter, which resets to `1`
/// on every daemon restart (#1680) and is the frozen "#1" the operator saw.
fn read_raw_cycle_reports(state_root: &Path, n: usize) -> Vec<Value> {
    super::current_work::read_recent_cycle_reports(state_root, n)
        .into_iter()
        .map(|entry| {
            let filename_cycle = entry
                .get("cycle_number")
                .cloned()
                .unwrap_or_else(|| json!(0));
            match entry.get("report") {
                // Parsed JSON report: promote it to the top level and overwrite
                // the in-body cycle number with the authoritative filename index.
                Some(report) => {
                    let mut raw = report.clone();
                    if let Value::Object(map) = &mut raw {
                        map.insert("cycle_number".to_string(), filename_cycle);
                    }
                    raw
                }
                // Legacy one-line plain-text summary (`{cycle_number, summary}`).
                // A documented shape branch, not a silent fallback: flag it so the
                // shared renderer shows the "legacy" badge instead of OODA phases.
                None => {
                    let mut legacy = entry;
                    if let Value::Object(map) = &mut legacy {
                        map.insert("legacy".to_string(), json!(true));
                    }
                    legacy
                }
            }
        })
        .collect()
}

/// The single shared reader behind BOTH the Activity tab's "Cycle Reports" card
/// (`/api/logs` → `cycle_reports`) and the Thinking tab's "Agent Internal
/// Reasoning" view (`/api/ooda-thinking` → `reports`).
///
/// Reads the newest cycle reports from the union of both persisted directories,
/// newest-first, with each report carrying its authoritative filename cycle
/// number, then collapses runs of equivalent cycles via
/// [`super::thinking_collapse::collapse_reports`]. Because both endpoints call
/// this one function they can no longer diverge on a stale copy (#26): the
/// Activity card is now the same correct, deduplicated, detail-carrying view the
/// Thinking tab already renders.
pub(crate) fn read_cycle_reports_collapsed(state_root: &Path) -> Vec<Value> {
    let raw = read_raw_cycle_reports(state_root, MAX_CYCLE_REPORTS);
    super::thinking_collapse::collapse_reports(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_cycle(dir: &Path, n: u64) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("cycle_{n}.json")), "{}").unwrap();
    }

    #[test]
    fn latest_persisted_is_zero_when_no_reports() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(latest_persisted_cycle_number(tmp.path()), 0);
    }

    #[test]
    fn latest_persisted_finds_max_in_top_level_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cycle_reports");
        write_cycle(&dir, 3);
        write_cycle(&dir, 1159);
        write_cycle(&dir, 42);
        assert_eq!(latest_persisted_cycle_number(tmp.path()), 1159);
    }

    #[test]
    fn latest_persisted_finds_max_in_state_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state").join("cycle_reports");
        write_cycle(&dir, 7);
        write_cycle(&dir, 84);
        assert_eq!(latest_persisted_cycle_number(tmp.path()), 84);
    }

    #[test]
    fn latest_persisted_takes_max_across_both_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write_cycle(&tmp.path().join("cycle_reports"), 1159);
        write_cycle(&tmp.path().join("state").join("cycle_reports"), 84);
        assert_eq!(latest_persisted_cycle_number(tmp.path()), 1159);
    }

    #[test]
    fn latest_persisted_ignores_non_cycle_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cycle_reports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        std::fs::write(dir.join("cycle_abc.json"), "{}").unwrap();
        write_cycle(&dir, 5);
        assert_eq!(latest_persisted_cycle_number(tmp.path()), 5);
    }

    #[test]
    fn health_cycle_number_reads_field() {
        let h = json!({ "cycle_number": 1 });
        assert_eq!(health_cycle_number(Some(&h)), 1);
    }

    #[test]
    fn health_cycle_number_defaults_to_zero() {
        assert_eq!(health_cycle_number(None), 0);
        assert_eq!(health_cycle_number(Some(&json!({}))), 0);
    }

    #[test]
    fn authoritative_prefers_persisted_after_restart() {
        // Daemon restarted: health counter reset to 1, but 1159 cycles are
        // recorded on disk. The dashboard must show 1159 everywhere.
        let tmp = tempfile::tempdir().unwrap();
        write_cycle(&tmp.path().join("cycle_reports"), 1159);
        let health = json!({ "cycle_number": 1 });
        assert_eq!(
            authoritative_cycle_number(tmp.path(), Some(&health)),
            1159,
            "after restart the persisted cumulative count must win over the process-local #1"
        );
    }

    #[test]
    fn authoritative_uses_health_for_in_flight_cycle() {
        // No restart: health counter (in-flight cycle 1160) is ahead of the
        // last persisted report (1159, written at the previous cycle's end).
        let tmp = tempfile::tempdir().unwrap();
        write_cycle(&tmp.path().join("cycle_reports"), 1159);
        let health = json!({ "cycle_number": 1160 });
        assert_eq!(authoritative_cycle_number(tmp.path(), Some(&health)), 1160);
    }

    #[test]
    fn authoritative_handles_missing_health_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(authoritative_cycle_number(tmp.path(), None), 0);
    }

    #[test]
    fn durable_brain_counter_is_the_dashboards_cycle_source_after_restart() {
        // Issue #1: on a daemon restart the process-local counter resets, but
        // the durable brain counter (`PersistentGoalState.cycle_count`) is
        // monotonic. The daemon now seeds `daemon_health.json`'s `cycle_number`
        // from that durable value, so the dashboard's single authoritative
        // cycle number renders the brain-relative count — never the reset "#1".
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // The brain has durably lived 1159 cycles.
        crate::goal_board_store::mutate(root, |s| {
            s.cycle_count = 1159;
        })
        .unwrap();
        let durable = crate::goal_board_store::load(root).cycle_count as u64;
        assert_eq!(durable, 1159);

        // A freshly restarted process, before any new cycle report is written
        // to disk: health carries the durable brain count (not `cycles_run + 1`
        // == 1). The authoritative dashboard number must be the brain-relative
        // value.
        let health = json!({ "cycle_number": durable });
        assert_eq!(
            authoritative_cycle_number(root, Some(&health)),
            1159,
            "the dashboard must render the durable brain-relative cycle, never the process-reset #1",
        );
    }
}
