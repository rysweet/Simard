//! Operator probe for the Bursar identity: design a target allocation and prove
//! a runnable backtest / risk / rebalancing analysis end-to-end.
//!
//! Research/advisory only — the probe never executes an order.

use crate::bursar::{InvestmentBrief, render_report, run_bursar};

use super::state_root::parse_runtime_topology;

/// Run the Bursar end-to-end: parse the free-text brief, design the target
/// allocation, run the backtest / risk / rebalancing analysis, and print the
/// verified outcome report.
///
/// The `topology` argument is validated for parity with the other probes even
/// though the analysis runs in-process; the objective is the (untrusted)
/// investment brief.
///
/// # Errors
/// Returns an error if `topology` is invalid or if the end-to-end run fails its
/// verification invariants.
pub fn run_bursar_probe(
    topology: &str,
    brief_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let topology = parse_runtime_topology(topology)?;
    let brief = InvestmentBrief::from_prompt(brief_text);
    let outcome = run_bursar(&brief)?;
    let report = render_report(&outcome);
    print!("{report}");
    println!("Topology: {topology}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursar_probe_runs_and_verifies() {
        let result = run_bursar_probe(
            "single-process",
            "Balanced growth portfolio for a 20 year horizon, $250,000",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn bursar_probe_rejects_invalid_topology() {
        let result = run_bursar_probe("not-a-topology", "Aggressive portfolio, $100,000, 30 years");
        assert!(result.is_err());
    }

    #[test]
    fn bursar_probe_handles_thin_brief() {
        // Falls back to defaults rather than failing.
        let result = run_bursar_probe("single-process", "");
        assert!(result.is_ok());
    }
}
