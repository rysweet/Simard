//! End-to-end demonstration of the Atelier identity as an EXTERNAL consumer of
//! the `simard` crate's public API.
//!
//! It designs a furniture / physical product from a free-text brief, fabricates
//! the cut list, BOM, and exports (OpenSCAD/STL/STEP/SVG render), verifies the
//! invariants, and prints the outcome.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example atelier_end_to_end
//! cargo run --example atelier_end_to_end -- "Standing desk in birch plywood, 1400x700x1050mm, batch of 6"
//! ```
//!
//! Exit code 0 = the prototype ran and verified end-to-end; non-zero = failure.

use simard::ProductBrief;
use simard::atelier::render_report;
use simard::run_atelier;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let brief_text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Larch dining table in solid oak, 1800x900x740mm".to_string());

    let brief = ProductBrief::from_prompt(&brief_text);
    let outcome = run_atelier(&brief)?;

    print!("{}", render_report(&outcome));

    if !outcome.verified {
        return Err("atelier prototype failed verification".into());
    }
    Ok(())
}
