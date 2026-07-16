//! Runnable reservations / PMS prototype engine.
//!
//! This is the "software to run the hotel" half of the Concierge deliverable: a
//! small but genuinely runnable in-memory property-management engine covering
//! the four operational services named in the brief:
//!
//! - **Reservations** — book, hold, and cancel stays.
//! - **PMS front desk** — assign rooms, check guests in and out.
//! - **Housekeeping** — track dirty/clean/inspected room status and generate a
//!   daily task list.
//! - **Channel management** — compute and "push" live availability to
//!   distribution channels.
//!
//! The engine is deterministic and fully in-process, so `simard concierge run`
//! can execute a booking → check-in → housekeeping → check-out → channel-sync
//! cycle end-to-end and print a verifiable operations trace with no external
//! dependencies.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::concept::HotelConcept;

/// Housekeeping status of a room.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomStatus {
    /// Ready to sell.
    Clean,
    /// Occupied by a checked-in guest.
    Occupied,
    /// Vacated, awaiting housekeeping.
    Dirty,
    /// Cleaned, awaiting inspection before it can be sold again.
    Inspected,
    /// Removed from inventory (maintenance).
    OutOfOrder,
}

impl RoomStatus {
    /// Whether a room in this status can be assigned to a new reservation.
    pub fn is_sellable(self) -> bool {
        matches!(self, Self::Clean | Self::Inspected)
    }
}

/// A physical room in the property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub number: String,
    pub category: String,
    pub status: RoomStatus,
}

/// Reservation lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservationStatus {
    Booked,
    CheckedIn,
    CheckedOut,
    Cancelled,
}

/// A guest reservation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reservation {
    pub id: String,
    pub guest: String,
    pub category: String,
    pub nights: u32,
    pub status: ReservationStatus,
    /// Assigned room number, once the front desk assigns one.
    pub room: Option<String>,
    /// Booking channel (e.g. "direct", "ota-expedia").
    pub channel: String,
}

/// A single housekeeping task for the daily board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HousekeepingTask {
    pub room: String,
    pub action: String,
}

/// Per-category availability snapshot pushed to distribution channels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelAvailability {
    pub category: String,
    pub available: u32,
    pub total: u32,
}

/// Errors the engine can return. Kept as a small, self-describing enum so the
/// CLI can surface actionable messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PmsError {
    UnknownReservation(String),
    UnknownCategory(String),
    NoRoomAvailable(String),
    InvalidTransition { id: String, reason: String },
    EmptyProperty,
}

impl std::fmt::Display for PmsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownReservation(id) => write!(f, "unknown reservation '{id}'"),
            Self::UnknownCategory(c) => write!(f, "unknown room category '{c}'"),
            Self::NoRoomAvailable(c) => {
                write!(f, "no sellable room available in category '{c}'")
            }
            Self::InvalidTransition { id, reason } => {
                write!(f, "invalid transition for reservation '{id}': {reason}")
            }
            Self::EmptyProperty => write!(f, "property has no rooms"),
        }
    }
}

impl std::error::Error for PmsError {}

/// The in-memory property-management engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PmsEngine {
    property: String,
    rooms: Vec<Room>,
    reservations: Vec<Reservation>,
    #[serde(default)]
    next_res_seq: u32,
}

impl PmsEngine {
    /// Build an engine from a hotel concept: materialises the room mix into
    /// individual, numbered, clean rooms ready to sell.
    pub fn from_concept(concept: &HotelConcept) -> Self {
        let mut rooms = Vec::new();
        let mut floor = 1u32;
        let mut on_floor = 0u32;
        let per_floor = concept.layout.rooms_per_floor.max(1);
        for cat in &concept.layout.room_mix {
            for _ in 0..cat.count {
                if on_floor >= per_floor {
                    floor += 1;
                    on_floor = 0;
                }
                on_floor += 1;
                rooms.push(Room {
                    number: format!("{floor}{:02}", on_floor),
                    category: cat.name.clone(),
                    status: RoomStatus::Clean,
                });
            }
        }
        Self {
            property: concept.brief.name.clone(),
            rooms,
            reservations: Vec::new(),
            next_res_seq: 0,
        }
    }

    pub fn property(&self) -> &str {
        &self.property
    }

    pub fn rooms(&self) -> &[Room] {
        &self.rooms
    }

    pub fn reservations(&self) -> &[Reservation] {
        &self.reservations
    }

    /// Distinct sellable room categories (from the physical inventory).
    fn has_category(&self, category: &str) -> bool {
        self.rooms.iter().any(|r| r.category == category)
    }

    /// Book a reservation in a category. Does not yet assign a physical room.
    pub fn book(
        &mut self,
        guest: &str,
        category: &str,
        nights: u32,
        channel: &str,
    ) -> Result<String, PmsError> {
        if self.rooms.is_empty() {
            return Err(PmsError::EmptyProperty);
        }
        if !self.has_category(category) {
            return Err(PmsError::UnknownCategory(category.to_string()));
        }
        self.next_res_seq += 1;
        let id = format!("R{:04}", self.next_res_seq);
        self.reservations.push(Reservation {
            id: id.clone(),
            guest: guest.to_string(),
            category: category.to_string(),
            nights: nights.max(1),
            status: ReservationStatus::Booked,
            room: None,
            channel: channel.to_string(),
        });
        Ok(id)
    }

    fn reservation_index(&self, id: &str) -> Result<usize, PmsError> {
        self.reservations
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| PmsError::UnknownReservation(id.to_string()))
    }

    /// Cancel a booked reservation.
    pub fn cancel(&mut self, id: &str) -> Result<(), PmsError> {
        let idx = self.reservation_index(id)?;
        match self.reservations[idx].status {
            ReservationStatus::Booked => {
                self.reservations[idx].status = ReservationStatus::Cancelled;
                Ok(())
            }
            other => Err(PmsError::InvalidTransition {
                id: id.to_string(),
                reason: format!("cannot cancel a {other:?} reservation"),
            }),
        }
    }

    /// Assign the first sellable room in the reservation's category and check
    /// the guest in. Marks the room `Occupied`.
    pub fn check_in(&mut self, id: &str) -> Result<String, PmsError> {
        let idx = self.reservation_index(id)?;
        if self.reservations[idx].status != ReservationStatus::Booked {
            return Err(PmsError::InvalidTransition {
                id: id.to_string(),
                reason: "only Booked reservations can check in".to_string(),
            });
        }
        let category = self.reservations[idx].category.clone();
        let room_pos = self
            .rooms
            .iter()
            .position(|r| r.category == category && r.status.is_sellable())
            .ok_or_else(|| PmsError::NoRoomAvailable(category.clone()))?;
        self.rooms[room_pos].status = RoomStatus::Occupied;
        let room_number = self.rooms[room_pos].number.clone();
        self.reservations[idx].status = ReservationStatus::CheckedIn;
        self.reservations[idx].room = Some(room_number.clone());
        Ok(room_number)
    }

    /// Check a guest out. Marks the room `Dirty` for housekeeping.
    pub fn check_out(&mut self, id: &str) -> Result<(), PmsError> {
        let idx = self.reservation_index(id)?;
        if self.reservations[idx].status != ReservationStatus::CheckedIn {
            return Err(PmsError::InvalidTransition {
                id: id.to_string(),
                reason: "only CheckedIn reservations can check out".to_string(),
            });
        }
        if let Some(room_number) = self.reservations[idx].room.clone()
            && let Some(room) = self.rooms.iter_mut().find(|r| r.number == room_number)
        {
            room.status = RoomStatus::Dirty;
        }
        self.reservations[idx].status = ReservationStatus::CheckedOut;
        Ok(())
    }

    /// The housekeeping board: one task per room that is not ready to sell.
    pub fn housekeeping_board(&self) -> Vec<HousekeepingTask> {
        self.rooms
            .iter()
            .filter_map(|r| {
                let action = match r.status {
                    RoomStatus::Dirty => "clean",
                    RoomStatus::Inspected => "inspect",
                    RoomStatus::OutOfOrder => "repair",
                    RoomStatus::Clean | RoomStatus::Occupied => return None,
                };
                Some(HousekeepingTask {
                    room: r.number.clone(),
                    action: action.to_string(),
                })
            })
            .collect()
    }

    /// Run one housekeeping cycle: dirty rooms become inspected, inspected
    /// rooms become clean (ready to sell). Returns the number of rooms advanced.
    pub fn run_housekeeping(&mut self) -> u32 {
        let mut advanced = 0;
        for room in &mut self.rooms {
            match room.status {
                RoomStatus::Dirty => {
                    room.status = RoomStatus::Inspected;
                    advanced += 1;
                }
                RoomStatus::Inspected => {
                    room.status = RoomStatus::Clean;
                    advanced += 1;
                }
                _ => {}
            }
        }
        advanced
    }

    /// Channel-management availability: sellable rooms per category, pushed to
    /// distribution channels. Deterministically ordered by category name.
    pub fn channel_availability(&self) -> Vec<ChannelAvailability> {
        let mut totals: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for room in &self.rooms {
            let entry = totals.entry(room.category.clone()).or_insert((0, 0));
            entry.1 += 1;
            if room.status.is_sellable() {
                entry.0 += 1;
            }
        }
        totals
            .into_iter()
            .map(|(category, (available, total))| ChannelAvailability {
                category,
                available,
                total,
            })
            .collect()
    }

    /// Total rooms currently occupied.
    pub fn occupied_count(&self) -> u32 {
        self.rooms
            .iter()
            .filter(|r| r.status == RoomStatus::Occupied)
            .count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concierge::concept::{HotelBrief, HotelConcept};

    fn engine() -> PmsEngine {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        PmsEngine::from_concept(&concept)
    }

    #[test]
    fn from_concept_materialises_every_room() {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        let eng = PmsEngine::from_concept(&concept);
        let expected: u32 = concept.layout.room_mix.iter().map(|c| c.count).sum();
        assert_eq!(eng.rooms().len() as u32, expected);
        assert!(eng.rooms().iter().all(|r| r.status == RoomStatus::Clean));
        // Room numbers are unique.
        let mut numbers: Vec<_> = eng.rooms().iter().map(|r| r.number.clone()).collect();
        numbers.sort();
        numbers.dedup();
        assert_eq!(numbers.len(), eng.rooms().len());
    }

    #[test]
    fn full_lifecycle_end_to_end() {
        let mut eng = engine();
        let category = eng.rooms()[0].category.clone();
        let id = eng.book("Ada Lovelace", &category, 2, "direct").unwrap();
        let room = eng.check_in(&id).unwrap();
        assert_eq!(eng.occupied_count(), 1);
        assert!(!room.is_empty());

        eng.check_out(&id).unwrap();
        assert_eq!(eng.occupied_count(), 0);
        // The vacated room shows up on the housekeeping board.
        assert!(eng.housekeeping_board().iter().any(|t| t.room == room));

        // Two housekeeping cycles return it to sellable (dirty→inspected→clean).
        assert!(eng.run_housekeeping() >= 1);
        assert!(eng.run_housekeeping() >= 1);
        assert!(eng.housekeeping_board().is_empty());
    }

    #[test]
    fn book_rejects_unknown_category_and_empty_property() {
        let mut eng = engine();
        assert_eq!(
            eng.book("Guest", "Penthouse Palace", 1, "direct"),
            Err(PmsError::UnknownCategory("Penthouse Palace".to_string()))
        );

        let mut empty = PmsEngine {
            property: "Empty".to_string(),
            rooms: vec![],
            reservations: vec![],
            next_res_seq: 0,
        };
        assert_eq!(
            empty.book("Guest", "Standard", 1, "direct"),
            Err(PmsError::EmptyProperty)
        );
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let mut eng = engine();
        let category = eng.rooms()[0].category.clone();
        let id = eng.book("Guest", &category, 1, "direct").unwrap();
        // Cannot check out before check-in.
        assert!(matches!(
            eng.check_out(&id),
            Err(PmsError::InvalidTransition { .. })
        ));
        eng.check_in(&id).unwrap();
        // Cannot cancel a checked-in reservation.
        assert!(matches!(
            eng.cancel(&id),
            Err(PmsError::InvalidTransition { .. })
        ));
        // Unknown reservation id.
        assert_eq!(
            eng.check_in("R9999"),
            Err(PmsError::UnknownReservation("R9999".to_string()))
        );
    }

    #[test]
    fn cancel_frees_nothing_but_marks_cancelled() {
        let mut eng = engine();
        let category = eng.rooms()[0].category.clone();
        let id = eng.book("Guest", &category, 1, "ota-expedia").unwrap();
        eng.cancel(&id).unwrap();
        let res = eng.reservations().iter().find(|r| r.id == id).unwrap();
        assert_eq!(res.status, ReservationStatus::Cancelled);
    }

    #[test]
    fn channel_availability_drops_when_room_occupied() {
        let mut eng = engine();
        let category = eng.rooms()[0].category.clone();
        let before = eng
            .channel_availability()
            .into_iter()
            .find(|c| c.category == category)
            .unwrap()
            .available;
        let id = eng.book("Guest", &category, 1, "direct").unwrap();
        eng.check_in(&id).unwrap();
        let after = eng
            .channel_availability()
            .into_iter()
            .find(|c| c.category == category)
            .unwrap()
            .available;
        assert_eq!(after, before - 1);
    }

    #[test]
    fn no_room_available_when_category_sold_out() {
        // Tiny property so we can exhaust a category deterministically.
        let brief = HotelBrief {
            name: "Tiny Inn".to_string(),
            location: "Nowhere".to_string(),
            rooms: 4,
            theme: "minimal".to_string(),
            positioning: super::super::concept::Positioning::Select,
        };
        let concept = HotelConcept::design(brief).unwrap();
        let mut eng = PmsEngine::from_concept(&concept);
        let category = eng.rooms()[0].category.clone();
        let in_category = eng
            .rooms()
            .iter()
            .filter(|r| r.category == category)
            .count();
        let mut ids = Vec::new();
        for _ in 0..in_category {
            let id = eng.book("Guest", &category, 1, "direct").unwrap();
            eng.check_in(&id).unwrap();
            ids.push(id);
        }
        // One more booking in the category cannot be assigned a room.
        let overflow = eng.book("Guest", &category, 1, "direct").unwrap();
        assert_eq!(
            eng.check_in(&overflow),
            Err(PmsError::NoRoomAvailable(category))
        );
    }

    #[test]
    fn engine_roundtrips_through_json() {
        let eng = engine();
        let json = serde_json::to_string(&eng).unwrap();
        let back: PmsEngine = serde_json::from_str(&json).unwrap();
        assert_eq!(eng, back);
    }
}
