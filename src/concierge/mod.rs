//! The **Concierge** capability: design a hotel concept and scaffold the
//! software that runs it, then prove the scaffold works by driving a booking
//! end-to-end.
//!
//! This module is the runnable core behind the `simard-concierge` identity. It
//! has two halves:
//!
//! - [`design`] turns a (possibly untrusted, free-text) brief into a structured
//!   [`HotelConcept`](design::HotelConcept) covering property layout,
//!   guest-experience journey, and brand identity.
//! - [`pms`] is a small in-memory reservations / property-management engine
//!   (rooms, reservations, housekeeping, channel management) that can be
//!   scaffolded straight from a concept.
//!
//! [`run_concierge`] wires the two together: design → scaffold → a demonstrated
//! reservation lifecycle → invariant verification, returning a
//! [`ConciergeOutcome`] that is both machine-readable (serde) and renderable as
//! an operator report via [`render_report`].

pub mod design;
pub mod pms;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};

pub use design::{
    BrandIdentity, DesignVerification, ExperienceStage, GuestExperience, HotelBrief, HotelConcept,
    Positioning, PropertyLayout, RoomTypePlan, design_hotel,
};
pub use pms::{
    Channel, ChannelAvailability, Housekeeping, PmsEngine, Reservation, ReservationStatus, Room,
    RoomType,
};

/// Errors produced while designing or operating a hotel concept.
///
/// Self-contained (not folded into `SimardError`) so the concierge stays a
/// modular brick with its own contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConciergeError {
    /// The brief could not be turned into a buildable concept.
    InvalidBrief { reason: String },
    /// A booking referenced a room type that is not registered.
    UnknownRoomType { code: String },
    /// No room of the requested type is free for the window.
    NoAvailability { code: String },
    /// The requested stay dates are not valid.
    InvalidStay { reason: String },
    /// A reservation id was not found.
    UnknownReservation { id: String },
    /// A lifecycle transition was not allowed from the current state.
    InvalidTransition {
        id: String,
        from: String,
        to: String,
    },
    /// The end-to-end run failed its own verification invariants.
    VerificationFailed { reason: String },
}

impl Display for ConciergeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBrief { reason } => write!(f, "invalid hotel brief: {reason}"),
            Self::UnknownRoomType { code } => write!(f, "unknown room type: {code}"),
            Self::NoAvailability { code } => {
                write!(f, "no availability for room type: {code}")
            }
            Self::InvalidStay { reason } => write!(f, "invalid stay: {reason}"),
            Self::UnknownReservation { id } => write!(f, "unknown reservation: {id}"),
            Self::InvalidTransition { id, from, to } => {
                write!(f, "invalid transition for {id}: {from} -> {to}")
            }
            Self::VerificationFailed { reason } => {
                write!(f, "concierge verification failed: {reason}")
            }
        }
    }
}

impl Error for ConciergeError {}

/// A single demonstrated reservation, captured for the outcome report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaySummary {
    pub reservation_id: String,
    pub guest: String,
    pub room_number: String,
    pub type_code: String,
    pub arrival: NaiveDate,
    pub departure: NaiveDate,
    pub nights: i64,
    pub total_cents: u32,
    pub final_status: ReservationStatus,
}

/// The full result of an end-to-end concierge run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConciergeOutcome {
    pub concept: HotelConcept,
    pub total_rooms: u32,
    pub room_type_count: usize,
    pub sample_stay: StaySummary,
    pub direct_availability_on_arrival: u32,
    /// Whether the designed concept satisfied every hospitality design
    /// invariant (the measurable done-criteria for the design half).
    pub concept_verified: bool,
    /// One `ok: …` / `FAIL: …` line per checked design invariant.
    pub design_verification_notes: Vec<String>,
    /// Whether every post-run operational invariant held.
    pub verified: bool,
    pub verification_notes: Vec<String>,
}

/// Default arrival used for the demonstration stay when scaffolding a concept.
fn demo_arrival() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 6, 15).expect("static demo date is valid")
}

/// Design a hotel from a free-text brief, scaffold its PMS, and drive a booking
/// end-to-end, verifying operational invariants along the way.
///
/// # Errors
/// Propagates [`ConciergeError`] from design, booking, or lifecycle steps, and
/// returns [`ConciergeError::VerificationFailed`] if a post-run invariant is
/// violated.
pub fn run_concierge(brief: &HotelBrief) -> Result<ConciergeOutcome, ConciergeError> {
    let concept = design_hotel(brief)?;

    // Certify the design half against the hospitality design invariants before
    // scaffolding — the measurable done-criteria for "design a hotel concept".
    // Fail closed so a malformed concept never reaches the PMS scaffold.
    let design_verification = concept.verify_design();
    if !design_verification.ok {
        return Err(ConciergeError::VerificationFailed {
            reason: design_verification.notes.join("; "),
        });
    }

    let mut engine = PmsEngine::from_concept(&concept);

    let total_rooms = u32::try_from(engine.room_count()).unwrap_or(u32::MAX);
    let room_type_count = engine.room_types().len();

    // Pick the first designed room type as the demonstration category.
    let type_code = concept
        .layout
        .room_mix
        .first()
        .map(|plan| plan.code.clone())
        .ok_or_else(|| ConciergeError::InvalidBrief {
            reason: "concept produced no room types".to_string(),
        })?;

    let arrival = demo_arrival();
    let nights = 2_u32;
    let night_end = arrival + Duration::days(1);

    let availability_before = engine.channel_availability(arrival);
    let direct_availability_on_arrival = availability_before
        .iter()
        .find(|c| c.channel == Channel::Direct)
        .map_or(0, ChannelAvailability::total);

    let reservation = engine.book("Simard Demo Guest", &type_code, arrival, nights)?;
    let held_room = reservation.room_number.clone();

    // Availability must drop by exactly one for the booked night.
    let after_book = channel_total(&engine.channel_availability(arrival), Channel::Direct);

    engine.check_in(&reservation.id)?;
    engine.check_out(&reservation.id)?;
    let serviced = engine.run_housekeeping();

    let final_reservation = engine
        .reservation(&reservation.id)
        .cloned()
        .ok_or_else(|| ConciergeError::UnknownReservation {
            id: reservation.id.clone(),
        })?;

    // --- Verify invariants ---
    let mut notes = Vec::new();
    let mut verified = true;
    let check = |condition: bool, ok: &str, fail: &str, notes: &mut Vec<String>| {
        if condition {
            notes.push(format!("ok: {ok}"));
        } else {
            notes.push(format!("FAIL: {fail}"));
        }
        condition
    };

    verified &= check(
        after_book + 1 == direct_availability_on_arrival,
        "booking reduced direct availability by one",
        "booking did not reduce direct availability by one",
        &mut notes,
    );
    verified &= check(
        final_reservation.status == ReservationStatus::CheckedOut,
        "reservation reached checked-out",
        "reservation did not reach checked-out",
        &mut notes,
    );
    verified &= check(
        serviced.contains(&held_room),
        "housekeeping serviced the vacated room",
        "housekeeping did not service the vacated room",
        &mut notes,
    );
    let released = channel_total(&engine.channel_availability(arrival), Channel::Direct);
    verified &= check(
        released == direct_availability_on_arrival,
        "availability fully restored after checkout",
        "availability not restored after checkout",
        &mut notes,
    );
    verified &= check(
        engine
            .available_rooms(&type_code, arrival, night_end)
            .iter()
            .all(|room| room.housekeeping.is_sellable()),
        "all sellable rooms are clean/inspected",
        "a sellable room was left dirty",
        &mut notes,
    );

    if !verified {
        return Err(ConciergeError::VerificationFailed {
            reason: notes.join("; "),
        });
    }

    let nights_stayed = final_reservation.nights();
    let sample_stay = StaySummary {
        reservation_id: final_reservation.id,
        guest: final_reservation.guest,
        room_number: final_reservation.room_number,
        type_code: final_reservation.type_code,
        arrival: final_reservation.arrival,
        departure: final_reservation.departure,
        nights: nights_stayed,
        total_cents: final_reservation.total_cents,
        final_status: final_reservation.status,
    };

    Ok(ConciergeOutcome {
        concept,
        total_rooms,
        room_type_count,
        sample_stay,
        direct_availability_on_arrival,
        concept_verified: design_verification.ok,
        design_verification_notes: design_verification.notes,
        verified,
        verification_notes: notes,
    })
}

fn channel_total(snapshot: &[ChannelAvailability], channel: Channel) -> u32 {
    snapshot
        .iter()
        .find(|c| c.channel == channel)
        .map_or(0, ChannelAvailability::total)
}

/// Render an operator-facing text report for a concierge outcome.
#[must_use]
pub fn render_report(outcome: &ConciergeOutcome) -> String {
    let concept = &outcome.concept;
    let mut out = String::new();
    let brief = &concept.brief;

    out.push_str("Probe mode: concierge-run\n");
    out.push_str(&format!("Hotel: {}\n", brief.name));
    out.push_str(&format!("Location: {}\n", brief.location));
    out.push_str(&format!("Positioning: {}\n", brief.positioning.label()));
    out.push_str(&format!("Brand tagline: {}\n", concept.brand.tagline));
    out.push_str(&format!("Brand voice: {}\n", concept.brand.voice));
    out.push_str(&format!("Floors: {}\n", concept.layout.floors));
    out.push_str(&format!("Total rooms: {}\n", outcome.total_rooms));
    out.push_str(&format!("Room types: {}\n", outcome.room_type_count));
    for plan in &concept.layout.room_mix {
        out.push_str(&format!(
            "  Room type {} ({}): {} rooms @ {} cents/night, capacity {}\n",
            plan.code, plan.name, plan.count, plan.base_rate_cents, plan.capacity
        ));
    }
    out.push_str(&format!(
        "Public spaces: {}\n",
        concept.layout.public_spaces.join(", ")
    ));
    out.push_str(&format!(
        "Concept verified: {}\n",
        if outcome.concept_verified {
            "yes"
        } else {
            "no"
        }
    ));
    for note in &outcome.design_verification_notes {
        out.push_str(&format!("  - {note}\n"));
    }
    out.push_str("Guest experience:\n");
    for stage in &concept.guest_experience.stages {
        out.push_str(&format!(
            "  {} -> {}\n",
            stage.name,
            stage.touchpoints.join(", ")
        ));
    }
    out.push_str(&format!(
        "Direct availability on arrival: {}\n",
        outcome.direct_availability_on_arrival
    ));
    let stay = &outcome.sample_stay;
    out.push_str(&format!(
        "Sample reservation: {} for {} in room {} ({}), {} -> {} ({} nights), total {} cents, status {:?}\n",
        stay.reservation_id,
        stay.guest,
        stay.room_number,
        stay.type_code,
        stay.arrival,
        stay.departure,
        stay.nights,
        stay.total_cents,
        stay.final_status,
    ));
    out.push_str(&format!(
        "Prototype verified: {}\n",
        if outcome.verified { "yes" } else { "no" }
    ));
    for note in &outcome.verification_notes {
        out.push_str(&format!("  - {note}\n"));
    }
    out.push_str("Session phase: complete\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_run_verifies() {
        let brief =
            HotelBrief::from_prompt("Aurora Lodge in Reykjavik, a 120-room upscale design hotel");
        let outcome = run_concierge(&brief).unwrap();
        assert!(outcome.verified);
        assert!(outcome.concept_verified);
        assert!(
            outcome
                .design_verification_notes
                .iter()
                .all(|n| n.starts_with("ok:")),
            "design invariants must all pass: {:?}",
            outcome.design_verification_notes
        );
        assert_eq!(outcome.total_rooms, 120);
        assert_eq!(
            outcome.sample_stay.final_status,
            ReservationStatus::CheckedOut
        );
        assert_eq!(outcome.sample_stay.nights, 2);
        assert!(outcome.total_rooms >= 1);
        assert!(outcome.room_type_count >= 2);
    }

    #[test]
    fn end_to_end_run_is_deterministic() {
        let brief = HotelBrief::new("Determinism", "Nowhere", Positioning::Midscale, 50, "t");
        let a = run_concierge(&brief).unwrap();
        let b = run_concierge(&brief).unwrap();
        assert_eq!(a.sample_stay, b.sample_stay);
        assert_eq!(a.total_rooms, b.total_rooms);
        assert_eq!(a.concept, b.concept);
    }

    #[test]
    fn report_contains_key_sections() {
        let brief = HotelBrief::new("Reportel", "Metropolis", Positioning::Luxury, 200, "grand");
        let outcome = run_concierge(&brief).unwrap();
        let report = render_report(&outcome);
        assert!(report.contains("Probe mode: concierge-run"));
        assert!(report.contains("Hotel: Reportel"));
        assert!(report.contains("Total rooms: 200"));
        assert!(report.contains("Concept verified: yes"));
        assert!(report.contains("Sample reservation: RES-"));
        assert!(report.contains("Prototype verified: yes"));
        assert!(report.contains("Session phase: complete"));
    }

    #[test]
    fn outcome_serializes_to_json() {
        let brief = HotelBrief::new("JSON Inn", "Seattle", Positioning::Economy, 24, "t");
        let outcome = run_concierge(&brief).unwrap();
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"total_rooms\":24"));
        let round: ConciergeOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(round.total_rooms, 24);
    }

    #[test]
    fn error_display_is_readable() {
        let err = ConciergeError::NoAvailability {
            code: "STD".to_string(),
        };
        assert_eq!(err.to_string(), "no availability for room type: STD");
    }

    #[test]
    fn tiny_property_still_runs_end_to_end() {
        let brief = HotelBrief::new("Tiny", "Village", Positioning::Economy, 8, "cozy");
        let outcome = run_concierge(&brief).unwrap();
        assert!(outcome.verified);
        assert_eq!(outcome.total_rooms, 8);
    }
}
