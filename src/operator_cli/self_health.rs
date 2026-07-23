//! `simard self-health` — run the post-deploy health probe and print a report.
//!
//! A sibling of `self-test`. Reports the six self-deploy health probes
//! (version, memory, goal board, brains LLM-backed, no quarantine, entrypoint
//! parity) against the live store, opened through the canonical reader memory
//! (daemon socket when up, else a direct on-disk open). Exit code 0 when every
//! probe is healthy, non-zero otherwise. The self-deploy orchestrator runs the
//! same probe internally; this is the operator-facing entry point.
//!
//! See `docs/reference/self-deploy-api.md#simard-self-health`.

use crate::memory_ipc::open_reader_client;
use std::path::Path;

pub(super) const SELF_HEALTH_HELP: &str = "\
Simard self-health subcommand

Usage: simard self-health [--json] [--pre-deploy-facts=N] [--acknowledge-quarantine]

  --json                    Emit the SelfHealthReport as JSON (default: human table).
  --pre-deploy-facts=N      Baseline cognitive-memory fact count to compare against
                            (the orchestrator passes the count captured before the
                            swap). When omitted, the memory probe reports the live
                            count only.
  --acknowledge-quarantine  Acknowledge every present cognitive-memory quarantine
                            artifact under the state root AND the live-store subdir
                            `<state_root>/state/`, writing a durable `.ack` sidecar
                            next to each so the `no_quarantine` probe stops
                            counting it (issue #4469). Idempotent and NON-destructive:
                            no artifact is deleted — the #2550 recovery asset is
                            retained. Use this to clear a genuinely-stuck quarantine
                            that freezes self-deploy. The probe is then re-run.

Exit code: 0 when every probe is healthy; non-zero when any probe fails.
";

/// Parse `--json`, `--pre-deploy-facts=N`, and `--acknowledge-quarantine` from
/// the remaining args.
fn parse_flags(
    args: impl Iterator<Item = String>,
) -> Result<(bool, Option<u64>, bool), Box<dyn std::error::Error>> {
    let mut json = false;
    let mut pre_deploy_facts = None;
    let mut acknowledge_quarantine = false;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if arg == "--acknowledge-quarantine" {
            acknowledge_quarantine = true;
        } else if let Some(n) = arg.strip_prefix("--pre-deploy-facts=") {
            pre_deploy_facts = Some(
                n.parse::<u64>()
                    .map_err(|e| format!("invalid --pre-deploy-facts '{n}': {e}"))?,
            );
        } else {
            return Err(
                format!("unexpected argument '{arg}' (see `simard self-health --help`)").into(),
            );
        }
    }
    Ok((json, pre_deploy_facts, acknowledge_quarantine))
}

/// Acknowledge every present cognitive-memory quarantine artifact under
/// `state_root` (issue #4469), writing a durable `.ack` sidecar next to each so
/// the `no_quarantine` probe stops counting it. Idempotent and non-destructive:
/// no artifact is deleted (the #2550 recovery asset is retained). Returns the
/// number of artifacts acknowledged. Best-effort: a per-artifact failure is
/// logged and skipped so one hostile entry cannot block clearing the rest.
///
/// Scans the SAME directory set the `no_quarantine` probe and the cleanup sweep
/// scan — the top-level state root AND the live-store subdir
/// `<state_root>/state/` (where the de-forked backend actually drops corrupt
/// snapshots) — single-sourced via
/// [`crate::state_root::quarantine_scan_dirs_under`]. Otherwise this operator
/// remediation would silently miss a stuck quarantine in `state/` (the primary
/// location) that still reddens the probe, leaving self-deploy frozen despite a
/// "success" from this command.
fn acknowledge_all_present_quarantines(state_root: &Path) -> usize {
    let mut acknowledged = 0;
    for dir in crate::state_root::quarantine_scan_dirs_under(state_root) {
        for name in crate::self_deploy::present_quarantine_artifacts(&dir) {
            match crate::self_deploy::acknowledge(&dir, &name) {
                Ok(_) => acknowledged += 1,
                // `?name` (Debug) escapes control chars in the untrusted quarantine
                // basename to prevent log-line forgery (#4469 security review).
                Err(e) => tracing::warn!(
                    artifact = ?name,
                    dir = ?dir,
                    error = %e,
                    "self_health.acknowledge_quarantine_failed: skipping one artifact (#4469)"
                ),
            }
        }
    }
    acknowledged
}

/// Dispatch `simard self-health`.
pub(super) fn dispatch_self_health_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (json, pre_deploy_facts, acknowledge_quarantine) = parse_flags(args)?;

    let state_root = crate::state_root::simard_state_root();

    // #4469: acknowledge present quarantines FIRST (writing durable `.ack`
    // sidecars) so the re-run probe below sees a cleared `no_quarantine`. The
    // artifacts themselves are retained; acknowledgement only silences the probe.
    if acknowledge_quarantine {
        let n = acknowledge_all_present_quarantines(&state_root);
        tracing::info!(
            acknowledged = n,
            "self_health.acknowledge_quarantine: wrote durable .ack sidecars; \
             artifacts retained (#4469)"
        );
    }

    let reader = open_reader_client(&state_root)?;

    // A manual self-health checks THIS running binary against itself, so the
    // target commit is the running build commit; the version probe confirms the
    // expected binary is live. Count brain parse-failures over a recent window.
    let target_commit = env!("SIMARD_GIT_HASH");
    let window_start = chrono::Utc::now() - chrono::Duration::minutes(5);

    let report = crate::self_deploy::run_self_health_probe(
        reader.ops(),
        target_commit,
        pre_deploy_facts,
        0,
        window_start,
    )?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_table(&report);
    }

    if report.healthy {
        Ok(())
    } else {
        Err("self-health: one or more probes are UNHEALTHY".into())
    }
}

fn print_table(report: &crate::self_deploy::SelfHealthReport) {
    let p = &report.probes;
    let mark = |ok: bool| if ok { "ok  " } else { "FAIL" };
    println!(
        "simard self-health: {}",
        if report.healthy {
            "HEALTHY"
        } else {
            "UNHEALTHY"
        }
    );
    println!(
        "  [{}] version_advanced   running={} target={}",
        mark(p.version_advanced.healthy),
        p.version_advanced.running,
        p.version_advanced.target
    );
    println!(
        "  [{}] memory_intact      live_facts={} baseline={}",
        mark(p.memory_intact.healthy),
        p.memory_intact.live_facts,
        p.memory_intact
            .baseline_facts
            .map(|n| n.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    );
    println!(
        "  [{}] goal_board_intact  active_goals={}",
        mark(p.goal_board_intact.healthy),
        p.goal_board_intact.active_goals
    );
    println!(
        "  [{}] brains_llm_backed  fallback_records={}",
        mark(p.brains_llm_backed.healthy),
        p.brains_llm_backed.fallback_records
    );
    println!(
        "  [{}] no_quarantine      quarantined={}",
        mark(p.no_quarantine.healthy),
        p.no_quarantine.quarantined
    );
    println!(
        "  [{}] entrypoint_parity  path={} version={} mismatch={} foreign={}",
        mark(p.entrypoint_parity.healthy),
        if p.entrypoint_parity.resolved_path.is_empty() {
            "<unresolved>"
        } else {
            &p.entrypoint_parity.resolved_path
        },
        if p.entrypoint_parity.path_version.is_empty() {
            "<none>"
        } else {
            &p.entrypoint_parity.path_version
        },
        p.entrypoint_parity.path_mismatch,
        p.entrypoint_parity.foreign_shadow
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flags_defaults() {
        let (json, baseline, ack) = parse_flags(Vec::<String>::new().into_iter()).unwrap();
        assert!(!json);
        assert_eq!(baseline, None);
        assert!(!ack);
    }

    #[test]
    fn parse_flags_json_and_baseline() {
        let args = vec!["--json".to_string(), "--pre-deploy-facts=1206".to_string()];
        let (json, baseline, ack) = parse_flags(args.into_iter()).unwrap();
        assert!(json);
        assert_eq!(baseline, Some(1206));
        assert!(!ack);
    }

    #[test]
    fn parse_flags_acknowledge_quarantine() {
        let args = vec!["--acknowledge-quarantine".to_string()];
        let (json, baseline, ack) = parse_flags(args.into_iter()).unwrap();
        assert!(!json);
        assert_eq!(baseline, None);
        assert!(ack, "--acknowledge-quarantine must set the ack flag");
    }

    #[test]
    fn parse_flags_rejects_unknown() {
        let args = vec!["--bogus".to_string()];
        assert!(parse_flags(args.into_iter()).is_err());
    }

    #[test]
    fn parse_flags_rejects_bad_baseline() {
        let args = vec!["--pre-deploy-facts=notanumber".to_string()];
        assert!(parse_flags(args.into_iter()).is_err());
    }

    #[test]
    fn acknowledge_all_writes_sidecars_and_retains_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Two quarantines + the live store + an unrelated file.
        std::fs::write(root.join("cognitive.corrupt-20260101120000"), b"a").unwrap();
        std::fs::write(root.join("cognitive.wal.corrupt-20260101120000"), b"b").unwrap();
        std::fs::write(root.join("cognitive"), b"live").unwrap();
        std::fs::write(root.join("unrelated.txt"), b"x").unwrap();

        let n = acknowledge_all_present_quarantines(root);
        assert_eq!(n, 2, "both quarantines acknowledged; live store excluded");

        // Sidecars written; artifacts and the live store retained.
        assert!(root.join("cognitive.corrupt-20260101120000.ack").is_file());
        assert!(
            root.join("cognitive.wal.corrupt-20260101120000.ack")
                .is_file()
        );
        assert!(root.join("cognitive.corrupt-20260101120000").is_file());
        assert!(root.join("cognitive").is_file());
        assert!(
            !root.join("cognitive.ack").exists(),
            "the live store must never be acknowledged"
        );

        // Idempotent: a second pass re-acknowledges without error or accumulation.
        assert_eq!(acknowledge_all_present_quarantines(root), 2);
        let markers = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter(|e| crate::self_deploy::is_ack_marker_name(&e.file_name().to_string_lossy()))
            .count();
        assert_eq!(markers, 2, "exactly one sidecar per quarantine");
    }

    /// #4469 regression: the operator remediation MUST cover the live-store
    /// subdir `<state_root>/state/` too — the primary location the de-forked
    /// backend drops corrupt snapshots, which the `no_quarantine` probe and the
    /// cleanup sweep both scan. Before the fix this scanned only the top level,
    /// so a stuck quarantine under `state/` could never be cleared manually and
    /// self-deploy stayed frozen despite a "success" from this command.
    #[test]
    fn acknowledge_all_covers_state_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();

        // One quarantine at the top level, one under state/.
        std::fs::write(root.join("cognitive.corrupt-20260101120000"), b"a").unwrap();
        std::fs::write(state.join("cognitive.corrupt-20260202120000"), b"b").unwrap();

        let n = acknowledge_all_present_quarantines(root);
        assert_eq!(
            n, 2,
            "quarantines under BOTH the state root and <state_root>/state/ must be acknowledged"
        );
        assert!(root.join("cognitive.corrupt-20260101120000.ack").is_file());
        assert!(
            state.join("cognitive.corrupt-20260202120000.ack").is_file(),
            "the state/ quarantine's sidecar must be written next to it"
        );
        // Non-destructive: both artifacts retained.
        assert!(root.join("cognitive.corrupt-20260101120000").is_file());
        assert!(state.join("cognitive.corrupt-20260202120000").is_file());
    }
}
