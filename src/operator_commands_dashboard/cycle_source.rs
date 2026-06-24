//! Single authoritative source for the dashboard's "Cycle #N" counter.
//!
//! `daemon_health.json` carries a `cycle_number` field that is a
//! *process-local* counter: the OODA daemon writes it as `cycles_run + 1`,
//! and both `cycles_run` and the in-memory `OodaState::cycle_count` reset to
//! `0` every time the daemon process restarts. The persisted cycle-report
//! files (`cycle_<N>.json`) carry the *cumulative* cycle number that the
//! Thinking tab and the Recent Actions feed already display. After a daemon
//! restart the two disagree — health says `#1` while the reports (and
//! Thinking/Recent Actions) say `#1159` — which is the self-contradiction
//! reported in issue #1680.
//!
//! Every dashboard panel that renders "Cycle #N" must read from one source.
//! This module computes that single value: the maximum of the process-local
//! health counter and the highest persisted cycle-report number. The maximum
//! is correct because the persisted count is monotonic across restarts, and
//! when no restart has occurred the in-flight cycle is reflected by the health
//! counter (which is then `>=` the latest persisted report).

use std::path::Path;

use serde_json::Value;

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
}
