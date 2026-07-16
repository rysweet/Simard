//! Operator probe for the Gastronome identity: design a menu and prove a
//! runnable kitchen app takes the brief to a costed, scheduled plan end-to-end.

use crate::gastronome::{MenuBrief, render_report, run_gastronome};

use super::state_root::parse_runtime_topology;

/// Run the Gastronome end-to-end: parse the free-text brief, design the menu,
/// scaffold the kitchen app, compute the costed/scaled/scheduled plan, and print
/// the verified outcome report.
///
/// The `topology` argument is validated for parity with the other probes even
/// though the kitchen prototype runs in-process; the objective is the
/// (untrusted) menu/event brief.
///
/// # Errors
/// Returns an error if `topology` is invalid or if the end-to-end run fails its
/// verification invariants.
pub fn run_gastronome_probe(
    topology: &str,
    brief_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let topology = parse_runtime_topology(topology)?;
    let brief = MenuBrief::from_prompt(brief_text);
    let outcome = run_gastronome(&brief)?;
    let report = render_report(&outcome);
    print!("{report}");
    println!("Topology: {topology}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gastronome_probe_runs_and_verifies() {
        let result = run_gastronome_probe(
            "single-process",
            "Harvest Feast menu for a wedding of 120 guests, elegant plated",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn gastronome_probe_rejects_invalid_topology() {
        let result = run_gastronome_probe("not-a-topology", "A dinner for 40 guests");
        assert!(result.is_err());
    }

    #[test]
    fn gastronome_probe_handles_thin_brief() {
        // Falls back to defaults rather than failing.
        let result = run_gastronome_probe("single-process", "");
        assert!(result.is_ok());
    }
}
