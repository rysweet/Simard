//! Concierge — the pluggable hospitality-design + operations identity.
//!
//! The Concierge is a Simard identity that both **designs hotels** and
//! **scaffolds the software to run them**. This module is its deterministic
//! backbone, exposed to the operator through `simard concierge`:
//!
//! - [`concept`] — design a [`HotelConcept`](concept::HotelConcept) (property
//!   layout, guest experience, brand) from a compact brief.
//! - [`scaffold`] — materialise the concept plus a runnable reservations/PMS
//!   prototype seed into an output directory.
//! - [`pms`] — the runnable in-memory reservations / PMS / housekeeping /
//!   channel-management engine.
//!
//! [`run_prototype`] and [`run_end_to_end`] drive the prototype through a full
//! operational cycle, producing a verifiable operations report. The agentic
//! recipes and prompts under `prompt_assets/simard/` compose on top of this
//! backbone; the backbone itself needs no LLM, so the whole end-to-end path is
//! deterministic and CI-testable.

pub mod concept;
pub mod pms;
pub mod scaffold;

use concept::{HotelBrief, HotelConcept};
use pms::PmsEngine;
use scaffold::{BookingRequest, PrototypeSeed};

/// The outcome of running the reservations/PMS prototype end-to-end.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationsReport {
    pub property: String,
    /// Human-readable trace of every operation performed, in order.
    pub trace: Vec<String>,
    pub bookings_made: u32,
    pub check_ins: u32,
    pub check_outs: u32,
    pub housekeeping_rooms_advanced: u32,
    pub occupied_after: u32,
    /// Final availability per category (category, available, total).
    pub availability: Vec<(String, u32, u32)>,
}

impl OperationsReport {
    /// Render the report as plain text suitable for CLI output.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Reservations/PMS prototype run — {}\n",
            self.property
        ));
        out.push_str("\nOperations trace:\n");
        for line in &self.trace {
            out.push_str(&format!("  {line}\n"));
        }
        out.push_str("\nSummary:\n");
        out.push_str(&format!(
            "  bookings made:              {}\n",
            self.bookings_made
        ));
        out.push_str(&format!(
            "  check-ins:                  {}\n",
            self.check_ins
        ));
        out.push_str(&format!(
            "  check-outs:                 {}\n",
            self.check_outs
        ));
        out.push_str(&format!(
            "  housekeeping rooms advanced: {}\n",
            self.housekeeping_rooms_advanced
        ));
        out.push_str(&format!(
            "  occupied after run:         {}\n",
            self.occupied_after
        ));
        out.push_str("\nChannel availability (pushed to distribution channels):\n");
        for (category, available, total) in &self.availability {
            out.push_str(&format!("  {category}: {available}/{total} sellable\n"));
        }
        out
    }
}

/// Execute the prototype seed through a full operational cycle:
/// book every seed reservation, check each guest in, check every other guest
/// out, run one housekeeping cycle, then push channel availability.
pub fn run_prototype(seed: PrototypeSeed) -> OperationsReport {
    let PrototypeSeed {
        concept,
        mut engine,
        bookings,
    } = seed;
    let mut trace = Vec::new();
    let mut booked_ids = Vec::new();
    let mut bookings_made = 0;
    let mut check_ins = 0;
    let mut check_outs = 0;

    for BookingRequest {
        guest,
        category,
        nights,
        channel,
    } in bookings
    {
        match engine.book(&guest, &category, nights, &channel) {
            Ok(id) => {
                bookings_made += 1;
                trace.push(format!(
                    "BOOK   {id} — {guest}, {category}, {nights}n via {channel}"
                ));
                booked_ids.push(id);
            }
            Err(e) => trace.push(format!("BOOK   FAILED — {guest}, {category}: {e}")),
        }
    }

    // Check everyone in.
    for id in &booked_ids {
        match engine.check_in(id) {
            Ok(room) => {
                check_ins += 1;
                trace.push(format!("CHECKIN {id} → room {room}"));
            }
            Err(e) => trace.push(format!("CHECKIN {id} FAILED — {e}")),
        }
    }

    // Check out every other guest so housekeeping and channel effects are
    // observable while some rooms stay occupied.
    for (i, id) in booked_ids.iter().enumerate() {
        if i % 2 == 0 {
            match engine.check_out(id) {
                Ok(()) => {
                    check_outs += 1;
                    trace.push(format!("CHECKOUT {id}"));
                }
                Err(e) => trace.push(format!("CHECKOUT {id} FAILED — {e}")),
            }
        }
    }

    let board = engine.housekeeping_board();
    trace.push(format!("HOUSEKEEPING board: {} task(s)", board.len()));
    let advanced = engine.run_housekeeping();
    trace.push(format!("HOUSEKEEPING cycle: {advanced} room(s) advanced"));

    let availability: Vec<(String, u32, u32)> = engine
        .channel_availability()
        .into_iter()
        .map(|c| (c.category, c.available, c.total))
        .collect();
    trace.push(format!(
        "CHANNEL SYNC: pushed availability for {} categor{}",
        availability.len(),
        if availability.len() == 1 { "y" } else { "ies" }
    ));

    OperationsReport {
        property: concept.brief.name.clone(),
        trace,
        bookings_made,
        check_ins,
        check_outs,
        housekeeping_rooms_advanced: advanced,
        occupied_after: engine.occupied_count(),
        availability,
    }
}

/// One-shot design → seed → run. Returns the concept and the operations report,
/// proving the Concierge can produce a hotel concept plus a runnable
/// reservations/PMS prototype end-to-end without touching disk.
pub fn run_end_to_end(brief: HotelBrief) -> Result<(HotelConcept, OperationsReport), String> {
    let concept = HotelConcept::design(brief)?;
    let seed = PrototypeSeed::from_concept(concept.clone());
    let report = run_prototype(seed);
    Ok((concept, report))
}

/// Build a fresh PMS engine directly from a concept (convenience re-export path).
pub fn engine_from_concept(concept: &HotelConcept) -> PmsEngine {
    PmsEngine::from_concept(concept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_demo_produces_concept_and_report() {
        let (concept, report) = run_end_to_end(HotelBrief::demo()).unwrap();
        assert_eq!(concept.brief.name, "The Cedar & Fern");
        assert!(report.bookings_made > 0, "should make bookings");
        assert_eq!(report.check_ins, report.bookings_made);
        assert!(report.check_outs > 0);
        assert!(!report.availability.is_empty());
        // Trace must show every operational stage.
        let text = report.to_text();
        assert!(text.contains("BOOK"));
        assert!(text.contains("CHECKIN"));
        assert!(text.contains("CHECKOUT"));
        assert!(text.contains("HOUSEKEEPING"));
        assert!(text.contains("CHANNEL SYNC"));
    }

    #[test]
    fn end_to_end_is_deterministic() {
        let a = run_end_to_end(HotelBrief::demo()).unwrap().1;
        let b = run_end_to_end(HotelBrief::demo()).unwrap().1;
        assert_eq!(a, b);
    }

    #[test]
    fn run_prototype_occupancy_reflects_partial_checkout() {
        let (_c, report) = run_end_to_end(HotelBrief::demo()).unwrap();
        // Half of check-ins remain occupied (odd indices never check out).
        assert!(report.occupied_after > 0);
        assert!(report.occupied_after <= report.check_ins);
    }

    #[test]
    fn end_to_end_rejects_invalid_brief() {
        let mut brief = HotelBrief::demo();
        brief.rooms = 0;
        assert!(run_end_to_end(brief).is_err());
    }
}
