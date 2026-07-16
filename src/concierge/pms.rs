//! A small but genuinely runnable reservations / PMS prototype.
//!
//! The engine models the operational core that runs a hotel:
//! - a room inventory grouped by room type,
//! - reservations with a booking lifecycle (book → check-in → check-out /
//!   cancel),
//! - housekeeping status per room, and
//! - a channel manager that publishes availability to distribution channels.
//!
//! Everything is in-memory and deterministic, so it can be scaffolded from a
//! [`HotelConcept`](super::design::HotelConcept) and exercised end-to-end in a
//! test or example without any external service.

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use super::ConciergeError;
use super::design::HotelConcept;

/// Housekeeping state of a physical room.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Housekeeping {
    Clean,
    Dirty,
    Inspected,
    OutOfOrder,
}

impl Housekeeping {
    /// Whether a room in this state can be assigned to an arriving guest.
    #[must_use]
    pub fn is_sellable(self) -> bool {
        matches!(self, Self::Clean | Self::Inspected)
    }
}

/// A sellable room category.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomType {
    pub code: String,
    pub name: String,
    pub capacity: u32,
    pub base_rate_cents: u32,
}

/// A physical room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Room {
    pub number: String,
    pub floor: u32,
    pub type_code: String,
    pub housekeeping: Housekeeping,
}

/// Lifecycle state of a reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservationStatus {
    Booked,
    CheckedIn,
    CheckedOut,
    Cancelled,
}

impl ReservationStatus {
    /// Whether a reservation in this state still holds room inventory.
    #[must_use]
    pub fn holds_inventory(self) -> bool {
        matches!(self, Self::Booked | Self::CheckedIn)
    }
}

/// A confirmed reservation against a specific room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub id: String,
    pub guest: String,
    pub type_code: String,
    pub room_number: String,
    pub arrival: NaiveDate,
    pub departure: NaiveDate,
    pub status: ReservationStatus,
    pub total_cents: u32,
}

impl Reservation {
    /// Number of nights the reservation spans.
    #[must_use]
    pub fn nights(&self) -> i64 {
        (self.departure - self.arrival).num_days().max(0)
    }
}

/// A distribution channel the property sells through.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Direct,
    BookingCom,
    Expedia,
}

impl Channel {
    /// All channels the channel manager publishes to.
    #[must_use]
    pub fn all() -> [Channel; 3] {
        [Channel::Direct, Channel::BookingCom, Channel::Expedia]
    }

    /// Stable slug for the channel.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Channel::Direct => "direct",
            Channel::BookingCom => "booking.com",
            Channel::Expedia => "expedia",
        }
    }
}

/// Availability published to a single channel for a single night.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelAvailability {
    pub channel: Channel,
    pub date: NaiveDate,
    /// Rooms available per room-type code.
    pub by_type: BTreeMap<String, u32>,
}

impl ChannelAvailability {
    /// Total rooms available across all types on this channel/night.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.by_type.values().copied().sum()
    }
}

/// In-memory reservations / property-management engine.
#[derive(Clone, Debug, Default)]
pub struct PmsEngine {
    room_types: BTreeMap<String, RoomType>,
    rooms: Vec<Room>,
    reservations: Vec<Reservation>,
    next_id: u64,
}

impl PmsEngine {
    /// Create an empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scaffold a running engine from a designed hotel concept: register each
    /// room type and generate numbered rooms per floor.
    #[must_use]
    pub fn from_concept(concept: &HotelConcept) -> Self {
        let mut engine = Self::new();
        for plan in &concept.layout.room_mix {
            engine.register_room_type(RoomType {
                code: plan.code.clone(),
                name: plan.name.clone(),
                capacity: plan.capacity,
                base_rate_cents: plan.base_rate_cents,
            });
        }

        // Distribute rooms across floors, keeping each type contiguous so room
        // numbers stay human-legible (e.g. 101, 102, ... 201, ...).
        let floors = concept.layout.floors.max(1);
        let mut per_floor: BTreeMap<u32, u32> = BTreeMap::new();
        let mut floor_cursor = 1_u32;
        for plan in &concept.layout.room_mix {
            for _ in 0..plan.count {
                let seq = per_floor.entry(floor_cursor).or_insert(0);
                *seq += 1;
                let number = format!("{floor_cursor}{:02}", *seq);
                engine.add_room(Room {
                    number,
                    floor: floor_cursor,
                    type_code: plan.code.clone(),
                    housekeeping: Housekeeping::Inspected,
                });
                floor_cursor = floor_cursor % floors + 1;
            }
        }
        engine
    }

    /// Register (or replace) a room type.
    pub fn register_room_type(&mut self, room_type: RoomType) {
        self.room_types.insert(room_type.code.clone(), room_type);
    }

    /// Add a physical room.
    pub fn add_room(&mut self, room: Room) {
        self.rooms.push(room);
    }

    /// All registered room types, ordered by code.
    #[must_use]
    pub fn room_types(&self) -> Vec<&RoomType> {
        self.room_types.values().collect()
    }

    /// Total physical room count.
    #[must_use]
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// All reservations recorded so far.
    #[must_use]
    pub fn reservations(&self) -> &[Reservation] {
        &self.reservations
    }

    /// Look up a reservation by id.
    #[must_use]
    pub fn reservation(&self, id: &str) -> Option<&Reservation> {
        self.reservations.iter().find(|r| r.id == id)
    }

    /// Look up a room by number.
    #[must_use]
    pub fn room(&self, number: &str) -> Option<&Room> {
        self.rooms.iter().find(|r| r.number == number)
    }

    /// Rooms of a given type that are free (not held by an overlapping
    /// reservation and currently sellable) for the whole `[arrival, departure)`
    /// window.
    #[must_use]
    pub fn available_rooms(
        &self,
        type_code: &str,
        arrival: NaiveDate,
        departure: NaiveDate,
    ) -> Vec<&Room> {
        self.rooms
            .iter()
            .filter(|room| room.type_code == type_code)
            .filter(|room| room.housekeeping.is_sellable())
            .filter(|room| !self.is_room_held(&room.number, arrival, departure))
            .collect()
    }

    fn is_room_held(&self, room_number: &str, arrival: NaiveDate, departure: NaiveDate) -> bool {
        self.reservations.iter().any(|res| {
            res.room_number == room_number
                && res.status.holds_inventory()
                && overlaps(res.arrival, res.departure, arrival, departure)
        })
    }

    /// Book a stay, assigning the first available room of the requested type.
    ///
    /// # Errors
    /// - [`ConciergeError::UnknownRoomType`] if `type_code` is not registered.
    /// - [`ConciergeError::InvalidStay`] if the date window is non-positive.
    /// - [`ConciergeError::NoAvailability`] if no room of the type is free.
    pub fn book(
        &mut self,
        guest: impl Into<String>,
        type_code: &str,
        arrival: NaiveDate,
        nights: u32,
    ) -> Result<Reservation, ConciergeError> {
        let room_type =
            self.room_types
                .get(type_code)
                .ok_or_else(|| ConciergeError::UnknownRoomType {
                    code: type_code.to_string(),
                })?;
        if nights == 0 {
            return Err(ConciergeError::InvalidStay {
                reason: "a stay must be at least one night".to_string(),
            });
        }
        let departure = arrival + Duration::days(i64::from(nights));
        let room_number = self
            .available_rooms(type_code, arrival, departure)
            .first()
            .map(|room| room.number.clone())
            .ok_or_else(|| ConciergeError::NoAvailability {
                code: type_code.to_string(),
            })?;

        self.next_id += 1;
        let reservation = Reservation {
            id: format!("RES-{:05}", self.next_id),
            guest: guest.into(),
            type_code: type_code.to_string(),
            room_number,
            arrival,
            departure,
            status: ReservationStatus::Booked,
            total_cents: room_type.base_rate_cents * nights,
        };
        self.reservations.push(reservation.clone());
        Ok(reservation)
    }

    /// Check a guest in, marking the room dirty-in-use.
    ///
    /// # Errors
    /// [`ConciergeError::UnknownReservation`] if `id` is unknown, or
    /// [`ConciergeError::InvalidTransition`] if the reservation is not `Booked`.
    pub fn check_in(&mut self, id: &str) -> Result<(), ConciergeError> {
        let room_number = {
            let reservation = self.require_reservation_mut(id)?;
            if reservation.status != ReservationStatus::Booked {
                return Err(ConciergeError::InvalidTransition {
                    id: id.to_string(),
                    from: format!("{:?}", reservation.status),
                    to: "checked-in".to_string(),
                });
            }
            reservation.status = ReservationStatus::CheckedIn;
            reservation.room_number.clone()
        };
        self.set_housekeeping(&room_number, Housekeeping::Dirty);
        Ok(())
    }

    /// Check a guest out; the room becomes dirty and awaits housekeeping.
    ///
    /// # Errors
    /// [`ConciergeError::UnknownReservation`] if `id` is unknown, or
    /// [`ConciergeError::InvalidTransition`] if not `CheckedIn`.
    pub fn check_out(&mut self, id: &str) -> Result<(), ConciergeError> {
        let room_number = {
            let reservation = self.require_reservation_mut(id)?;
            if reservation.status != ReservationStatus::CheckedIn {
                return Err(ConciergeError::InvalidTransition {
                    id: id.to_string(),
                    from: format!("{:?}", reservation.status),
                    to: "checked-out".to_string(),
                });
            }
            reservation.status = ReservationStatus::CheckedOut;
            reservation.room_number.clone()
        };
        self.set_housekeeping(&room_number, Housekeeping::Dirty);
        Ok(())
    }

    /// Cancel a reservation, releasing its held inventory.
    ///
    /// # Errors
    /// [`ConciergeError::UnknownReservation`] if `id` is unknown, or
    /// [`ConciergeError::InvalidTransition`] if already checked out.
    pub fn cancel(&mut self, id: &str) -> Result<(), ConciergeError> {
        let reservation = self.require_reservation_mut(id)?;
        if matches!(
            reservation.status,
            ReservationStatus::CheckedOut | ReservationStatus::Cancelled
        ) {
            return Err(ConciergeError::InvalidTransition {
                id: id.to_string(),
                from: format!("{:?}", reservation.status),
                to: "cancelled".to_string(),
            });
        }
        reservation.status = ReservationStatus::Cancelled;
        Ok(())
    }

    /// Run the housekeeping pass: clean and inspect every dirty room. Returns
    /// the room numbers that were serviced.
    pub fn run_housekeeping(&mut self) -> Vec<String> {
        let mut serviced = Vec::new();
        for room in &mut self.rooms {
            if room.housekeeping == Housekeeping::Dirty {
                room.housekeeping = Housekeeping::Inspected;
                serviced.push(room.number.clone());
            }
        }
        serviced.sort();
        serviced
    }

    /// Set a room's housekeeping status directly. Returns whether the room
    /// existed.
    pub fn set_housekeeping(&mut self, room_number: &str, status: Housekeeping) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.number == room_number) {
            room.housekeeping = status;
            true
        } else {
            false
        }
    }

    /// Availability for a single night across every channel. The channel
    /// manager publishes the same underlying inventory to each channel.
    #[must_use]
    pub fn channel_availability(&self, date: NaiveDate) -> Vec<ChannelAvailability> {
        let night_end = date + Duration::days(1);
        let mut by_type: BTreeMap<String, u32> = BTreeMap::new();
        for room_type in self.room_types.keys() {
            let count = self.available_rooms(room_type, date, night_end).len();
            by_type.insert(room_type.clone(), u32::try_from(count).unwrap_or(u32::MAX));
        }
        Channel::all()
            .into_iter()
            .map(|channel| ChannelAvailability {
                channel,
                date,
                by_type: by_type.clone(),
            })
            .collect()
    }

    fn require_reservation_mut(&mut self, id: &str) -> Result<&mut Reservation, ConciergeError> {
        self.reservations
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| ConciergeError::UnknownReservation { id: id.to_string() })
    }
}

/// Half-open interval overlap: `[a_start, a_end)` vs `[b_start, b_end)`.
fn overlaps(a_start: NaiveDate, a_end: NaiveDate, b_start: NaiveDate, b_end: NaiveDate) -> bool {
    a_start < b_end && b_start < a_end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concierge::design::{HotelBrief, Positioning, design_hotel};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn sample_engine() -> PmsEngine {
        let brief = HotelBrief::new("Test Inn", "Testville", Positioning::Midscale, 40, "t");
        let concept = design_hotel(&brief).unwrap();
        PmsEngine::from_concept(&concept)
    }

    #[test]
    fn from_concept_generates_all_rooms() {
        let engine = sample_engine();
        assert_eq!(engine.room_count(), 40);
        assert!(!engine.room_types().is_empty());
        // Every room references a registered type.
        for room in &engine.rooms {
            assert!(engine.room_types.contains_key(&room.type_code));
        }
    }

    #[test]
    fn booking_lifecycle_end_to_end() {
        let mut engine = sample_engine();
        let arrival = date(2026, 6, 1);
        let before = engine.available_rooms("STD", arrival, arrival + Duration::days(2));
        let before_count = before.len();
        assert!(before_count > 0);

        let res = engine.book("Ada Lovelace", "STD", arrival, 2).unwrap();
        assert_eq!(res.status, ReservationStatus::Booked);
        assert_eq!(res.nights(), 2);
        assert!(res.total_cents > 0);

        // Inventory dropped by one for the overlapping window.
        let after = engine.available_rooms("STD", arrival, arrival + Duration::days(2));
        assert_eq!(after.len(), before_count - 1);

        engine.check_in(&res.id).unwrap();
        assert_eq!(
            engine.room(&res.room_number).unwrap().housekeeping,
            Housekeeping::Dirty
        );

        engine.check_out(&res.id).unwrap();
        assert_eq!(
            engine.reservation(&res.id).unwrap().status,
            ReservationStatus::CheckedOut
        );

        // Immediately after check-out the room is dirty and not yet sellable.
        let dirty_window = engine.available_rooms("STD", arrival, arrival + Duration::days(2));
        assert_eq!(dirty_window.len(), before_count - 1);

        let serviced = engine.run_housekeeping();
        assert!(serviced.contains(&res.room_number));
        assert_eq!(
            engine.room(&res.room_number).unwrap().housekeeping,
            Housekeeping::Inspected
        );

        // Once housekeeping runs, inventory is released again.
        let released = engine.available_rooms("STD", arrival, arrival + Duration::days(2));
        assert_eq!(released.len(), before_count);
    }

    #[test]
    fn non_overlapping_stays_reuse_inventory() {
        let mut engine = sample_engine();
        // Book every STD room for one window, then a later window still sells.
        let arrival = date(2026, 7, 1);
        let std_total = engine
            .available_rooms("STD", arrival, arrival + Duration::days(1))
            .len();
        for i in 0..std_total {
            engine
                .book(format!("Guest {i}"), "STD", arrival, 1)
                .unwrap();
        }
        assert!(matches!(
            engine.book("Overflow", "STD", arrival, 1),
            Err(ConciergeError::NoAvailability { .. })
        ));
        // A non-overlapping later night is still fully available.
        let later = date(2026, 7, 2);
        assert_eq!(
            engine
                .available_rooms("STD", later, later + Duration::days(1))
                .len(),
            std_total
        );
    }

    #[test]
    fn cancel_releases_inventory() {
        let mut engine = sample_engine();
        let arrival = date(2026, 8, 1);
        let before = engine
            .available_rooms("STE", arrival, arrival + Duration::days(1))
            .len();
        let res = engine.book("G", "STE", arrival, 1).unwrap();
        engine.cancel(&res.id).unwrap();
        assert_eq!(
            engine
                .available_rooms("STE", arrival, arrival + Duration::days(1))
                .len(),
            before
        );
    }

    #[test]
    fn book_unknown_type_errors() {
        let mut engine = sample_engine();
        assert!(matches!(
            engine.book("G", "NOPE", date(2026, 1, 1), 1),
            Err(ConciergeError::UnknownRoomType { .. })
        ));
    }

    #[test]
    fn zero_night_stay_rejected() {
        let mut engine = sample_engine();
        assert!(matches!(
            engine.book("G", "STD", date(2026, 1, 1), 0),
            Err(ConciergeError::InvalidStay { .. })
        ));
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let mut engine = sample_engine();
        let res = engine.book("G", "STD", date(2026, 2, 1), 1).unwrap();
        // Cannot check out before checking in.
        assert!(matches!(
            engine.check_out(&res.id),
            Err(ConciergeError::InvalidTransition { .. })
        ));
        engine.check_in(&res.id).unwrap();
        engine.check_out(&res.id).unwrap();
        // Cannot cancel after check-out.
        assert!(matches!(
            engine.cancel(&res.id),
            Err(ConciergeError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn unknown_reservation_errors() {
        let mut engine = sample_engine();
        assert!(matches!(
            engine.check_in("RES-99999"),
            Err(ConciergeError::UnknownReservation { .. })
        ));
    }

    #[test]
    fn channel_availability_matches_inventory_and_all_channels() {
        let mut engine = sample_engine();
        let night = date(2026, 9, 10);
        let snapshot = engine.channel_availability(night);
        assert_eq!(snapshot.len(), Channel::all().len());
        let direct_total = snapshot
            .iter()
            .find(|c| c.channel == Channel::Direct)
            .unwrap()
            .total();
        assert_eq!(direct_total as usize, engine.room_count());

        // Booking one room reduces every channel's published availability.
        engine.book("G", "STD", night, 1).unwrap();
        let after = engine.channel_availability(night);
        for channel in after {
            assert_eq!(channel.total() as usize, engine.room_count() - 1);
        }
    }

    #[test]
    fn out_of_order_rooms_are_not_sellable() {
        let mut engine = sample_engine();
        let night = date(2026, 10, 1);
        let std_before = engine
            .available_rooms("STD", night, night + Duration::days(1))
            .len();
        let victim = engine
            .rooms
            .iter()
            .find(|r| r.type_code == "STD")
            .unwrap()
            .number
            .clone();
        assert!(engine.set_housekeeping(&victim, Housekeeping::OutOfOrder));
        assert_eq!(
            engine
                .available_rooms("STD", night, night + Duration::days(1))
                .len(),
            std_before - 1
        );
    }
}
