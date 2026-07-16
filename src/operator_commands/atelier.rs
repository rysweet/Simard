//! Operator probe for the Atelier identity: design a furniture / physical
//! product and prove a runnable fabrication prototype end-to-end, driving the
//! brief all the way to fabrication-ready exports.

use crate::atelier::{ProductBrief, render_report, run_atelier};

use super::state_root::parse_runtime_topology;

/// Run the Atelier end-to-end: parse the free-text brief, design the product,
/// fabricate it (cut list, BOM, and STEP/STL/OpenSCAD/SVG exports), verify the
/// invariants, and print the resulting outcome report.
///
/// The `topology` argument is validated for parity with the other probes even
/// though the fabrication prototype runs in-process; the objective is the
/// (untrusted) product brief.
///
/// # Errors
/// Returns an error if `topology` is invalid or if the end-to-end run fails its
/// verification invariants.
pub fn run_atelier_probe(
    topology: &str,
    brief_text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let topology = parse_runtime_topology(topology)?;
    let brief = ProductBrief::from_prompt(brief_text);
    let outcome = run_atelier(&brief)?;
    let report = render_report(&outcome);
    print!("{report}");
    println!("Topology: {topology}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atelier_probe_runs_and_verifies() {
        let result = run_atelier_probe(
            "single-process",
            "Larch dining table in solid oak, 1800x900x740mm",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn atelier_probe_rejects_invalid_topology() {
        let result = run_atelier_probe("not-a-topology", "A walnut stool 360x360x650");
        assert!(result.is_err());
    }

    #[test]
    fn atelier_probe_handles_thin_brief() {
        let result = run_atelier_probe("single-process", "");
        assert!(result.is_ok());
    }
}
