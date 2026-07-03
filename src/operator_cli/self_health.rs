//! `simard self-health` — run the post-deploy health probe and print a report.
//!
//! A sibling of `self-test`. Reports the five self-deploy health probes
//! (version, memory, goal board, reasoners LLM-backed, no quarantine) against the
//! live store, opened through the canonical reader adapter (daemon socket when
//! up, else a direct on-disk open). Exit code 0 when every probe is healthy,
//! non-zero otherwise. The self-deploy orchestrator runs the same probe
//! internally; this is the operator-facing entry point.
//!
//! See `docs/reference/self-deploy-api.md#simard-self-health`.

use crate::memory_ipc::open_reader_adapter;

pub(super) const SELF_HEALTH_HELP: &str = "\
Simard self-health subcommand

Usage: simard self-health [--json] [--pre-deploy-facts=N]

  --json                 Emit the SelfHealthReport as JSON (default: human table).
  --pre-deploy-facts=N   Baseline cognitive-memory fact count to compare against
                         (the orchestrator passes the count captured before the
                         swap). When omitted, the memory probe reports the live
                         count only.

Exit code: 0 when every probe is healthy; non-zero when any probe fails.
";

/// Parse `--json` and `--pre-deploy-facts=N` from the remaining args.
fn parse_flags(
    args: impl Iterator<Item = String>,
) -> Result<(bool, Option<u64>), Box<dyn std::error::Error>> {
    let mut json = false;
    let mut pre_deploy_facts = None;
    for arg in args {
        if arg == "--json" {
            json = true;
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
    Ok((json, pre_deploy_facts))
}

/// Dispatch `simard self-health`.
pub(super) fn dispatch_self_health_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (json, pre_deploy_facts) = parse_flags(args)?;

    let state_root = crate::state_root::simard_state_root();
    let reader = open_reader_adapter(&state_root)?;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flags_defaults() {
        let (json, baseline) = parse_flags(Vec::<String>::new().into_iter()).unwrap();
        assert!(!json);
        assert_eq!(baseline, None);
    }

    #[test]
    fn parse_flags_json_and_baseline() {
        let args = vec!["--json".to_string(), "--pre-deploy-facts=1206".to_string()];
        let (json, baseline) = parse_flags(args.into_iter()).unwrap();
        assert!(json);
        assert_eq!(baseline, Some(1206));
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
}
