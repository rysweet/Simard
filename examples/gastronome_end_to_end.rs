//! End-to-end demonstration of the Gastronome identity as an EXTERNAL consumer
//! of the `simard` crate's public API.
//!
//! It designs a menu from a free-text brief, scaffolds the kitchen app, scales
//! the menu to the guest count, costs it, analyzes nutrition, schedules prep,
//! and prints the verified outcome.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example gastronome_end_to_end
//! cargo run --example gastronome_end_to_end -- "Aurora Tasting menu for a gala of 90 guests, fine dining"
//! ```
//!
//! Exit code 0 = the plan ran and verified end-to-end; non-zero = failure.

use simard::gastronome::render_report;
use simard::{MenuBrief, run_gastronome};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let brief_text = std::env::args().nth(1).unwrap_or_else(|| {
        "Harvest Feast menu for a wedding of 120 guests, elegant plated".to_string()
    });

    let brief = MenuBrief::from_prompt(&brief_text);
    let outcome = run_gastronome(&brief)?;

    print!("{}", render_report(&outcome));

    if !outcome.verified {
        return Err("gastronome plan failed verification".into());
    }
    Ok(())
}
