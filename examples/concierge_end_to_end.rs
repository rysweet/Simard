//! End-to-end demonstration of the Concierge identity as an EXTERNAL consumer
//! of the `simard` crate's public API.
//!
//! It designs a hotel from a free-text brief, scaffolds the reservations/PMS
//! prototype, drives a full booking lifecycle, and prints the verified outcome.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example concierge_end_to_end
//! cargo run --example concierge_end_to_end -- "Aurora Lodge in Reykjavik, 90-room luxury spa resort"
//! ```
//!
//! Exit code 0 = the prototype ran and verified end-to-end; non-zero = failure.

use simard::{HotelBrief, render_report, run_concierge};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let brief_text = std::env::args().nth(1).unwrap_or_else(|| {
        "Harbor Light in Lisbon, a 120-room boutique waterfront hotel".to_string()
    });

    let brief = HotelBrief::from_prompt(&brief_text);
    let outcome = run_concierge(&brief)?;

    print!("{}", render_report(&outcome));

    if !outcome.verified {
        return Err("concierge prototype failed verification".into());
    }
    Ok(())
}
