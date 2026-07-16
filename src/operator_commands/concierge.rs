//! Operator probe for the Concierge identity: design a hotel and prove a
//! runnable reservations/PMS prototype end-to-end.

use crate::concierge::{HotelBrief, render_report, run_concierge};

use super::state_root::parse_runtime_topology;

/// Run the Concierge end-to-end: parse the free-text brief, design the hotel,
/// scaffold and drive the reservations/PMS prototype, and print the verified
/// outcome report.
///
/// The `topology` argument is validated for parity with the other probes even
/// though the concierge prototype runs in-process; the objective is the
/// (untrusted) hotel brief.
///
/// # Errors
/// Returns an error if `topology` is invalid or if the end-to-end run fails its
/// verification invariants.
pub fn run_concierge_probe(
    topology: &str,
    brief_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let topology = parse_runtime_topology(topology)?;
    let brief = HotelBrief::from_prompt(brief_text);
    let outcome = run_concierge(&brief)?;
    let report = render_report(&outcome);
    print!("{report}");
    println!("Topology: {topology}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concierge_probe_runs_and_verifies() {
        let result = run_concierge_probe(
            "single-process",
            "Harbor Light in Lisbon, a 120-room boutique waterfront hotel",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn concierge_probe_rejects_invalid_topology() {
        let result = run_concierge_probe("not-a-topology", "Some hotel in Nowhere, 40 rooms");
        assert!(result.is_err());
    }

    #[test]
    fn concierge_probe_handles_thin_brief() {
        // Falls back to defaults rather than failing.
        let result = run_concierge_probe("single-process", "");
        assert!(result.is_ok());
    }
}
