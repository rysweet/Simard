//! End-to-end demonstration of the Bursar identity as an EXTERNAL consumer of
//! the `simard` crate's public API.
//!
//! It designs a target allocation from a free-text brief, backtests it, computes
//! risk metrics, produces a rebalancing recommendation, and prints the verified
//! outcome. It is **research/advisory only** — it never executes an order.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example bursar_end_to_end
//! cargo run --example bursar_end_to_end -- "Aggressive growth portfolio, $1,000,000, 30 years"
//! ```
//!
//! Exit code 0 = the analysis ran and verified end-to-end; non-zero = failure.

use simard::InvestmentBrief;
use simard::bursar::{render_report, run_bursar};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let brief_text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Balanced growth portfolio for a 20 year horizon, $250,000".to_string());

    let brief = InvestmentBrief::from_prompt(&brief_text);
    let outcome = run_bursar(&brief)?;

    print!("{}", render_report(&outcome));

    if outcome.order_execution_performed {
        return Err("bursar must never execute orders".into());
    }
    if !outcome.verified {
        return Err("bursar analysis failed verification".into());
    }
    Ok(())
}
