//! Operator-probe surface for the LOCAL COIN Gym harness done-gate.
//!
//! The LOCAL COIN Gym harness (`src/coin_gym/`) already ships its own
//! acceptance self-check via the standalone `coin-gym verify` binary. That
//! self-check is the machine-checkable *done-criteria* for the LOCAL harness
//! goal (issue #2713): it exercises every harness component — target loader,
//! baseline runner, team runner, scorer, leaderboard comparator, the
//! skwaq-style self-improvement loop, and the `coin evaluate`/`coin verify`
//! contract wiring — offline against the built-in sample snapshot.
//!
//! This module re-exposes that same repo-grounded done-gate through the
//! operator-probe dispatcher (`simard_operator_probe coin-gym-verify`), so the
//! LOCAL COIN harness is reachable from the same surface that exposes the other
//! repo-grounded engineer surfaces (engineer-loop-run, terminal-run, …) rather
//! than only from a separate compatibility binary. The probe is hermetic and
//! offline: it isolates the self-improvement tactic memory under a throwaway
//! temp directory and exits non-zero if any LOCAL acceptance criterion fails.
//!
//! Live VM grading (`coin evaluate`/`coin verify`, Phase 3, issue #2823) is
//! externally gated on a provisioned Docker host and is intentionally out of
//! this gate's scope — the probe never posts results anywhere and never reaches
//! the network.

use crate::coin_gym::{LOCAL_ACCEPTANCE_SCOPE_NOTE, run_acceptance_checks};

use super::format::print_text;

/// Run the LOCAL COIN Gym acceptance self-check and render it in the
/// operator-probe style.
///
/// This is the operator-surface equivalent of `coin-gym verify`: it runs the
/// same [`run_acceptance_checks`] done-gate against the built-in sample
/// snapshot, isolating the self-improvement tactic memory under a throwaway
/// temp directory so the probe never touches a user's real profiles.
///
/// # Errors
/// Returns an error when the temp home cannot be created, or when one or more
/// of the LOCAL acceptance criteria fail (so the probe exits non-zero and the
/// done-gate stays honest).
pub fn run_coin_gym_verify_probe() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()
        .map_err(|e| format!("coin-gym-verify: cannot create temp home: {e}"))?;
    let report = run_acceptance_checks(tmp.path());

    println!("Probe mode: coin-gym-verify");
    print_text("Snapshot", "built-in sample (offline mock oracle)");
    for check in &report.checks {
        let status = if check.passed { "PASS" } else { "FAIL" };
        print_text(&format!("[{status}] {}", check.criterion), &check.detail);
    }
    println!(
        "Result: {}/{} LOCAL acceptance criteria passed",
        report.passed_count(),
        report.total()
    );
    print_text("Scope", LOCAL_ACCEPTANCE_SCOPE_NOTE);

    if report.all_passed() {
        Ok(())
    } else {
        Err(format!(
            "{} of {} LOCAL COIN acceptance criteria failed; see the FAIL rows above",
            report.total() - report.passed_count(),
            report.total()
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_passes_on_the_built_in_sample_snapshot() {
        // The shipped LOCAL harness sample is expected to satisfy every
        // done-criterion, so the operator probe must succeed and exit zero.
        let result = run_coin_gym_verify_probe();
        assert!(
            result.is_ok(),
            "coin-gym-verify probe should pass on the built-in sample: {result:?}"
        );
    }

    #[test]
    fn probe_is_deterministic_and_hermetic() {
        // Two back-to-back runs must agree: the probe isolates tactic memory
        // under a throwaway temp dir, so repeated invocations never diverge or
        // leak state into a user's real profiles.
        let first = run_coin_gym_verify_probe();
        let second = run_coin_gym_verify_probe();
        assert_eq!(
            first.is_ok(),
            second.is_ok(),
            "coin-gym-verify probe must be deterministic across runs"
        );
        assert!(first.is_ok() && second.is_ok());
    }

    #[test]
    fn scope_note_stays_local_only() {
        // Guard the LOCAL-only framing: the operator surface must never imply
        // it performs live VM grading or posts results externally. The note is
        // the same constant the CLI `coin-gym verify` gate renders, so this
        // also guards the two surfaces against drifting apart.
        assert!(LOCAL_ACCEPTANCE_SCOPE_NOTE.contains("LOCAL offline harness only"));
        assert!(LOCAL_ACCEPTANCE_SCOPE_NOTE.contains("Phase 3"));
    }
}
