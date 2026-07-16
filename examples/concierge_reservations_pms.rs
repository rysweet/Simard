//! Concierge reservations/PMS reference prototype (issue: simard-concierge
//! identity).
//!
//! A self-contained, dependency-free reservations / Property-Management-System
//! prototype. It is the runnable reference the `simard-concierge` identity
//! starts from when it scaffolds the software to run a hotel it has designed.
//! It lives in `examples/` so it builds and runs as an ordinary Rust binary —
//! the same pure-Rust discipline as the rest of the daemon (#3181): no Python.
//!
//! Run it (self-verifying end-to-end demo; exit 0 = PASS, non-zero = FAIL):
//!
//! ```text
//! cargo run --example concierge_reservations_pms
//! ```
//!
//! Test it (unit + invariant tests, wired via `test = true` in Cargo.toml):
//!
//! ```text
//! cargo test --example concierge_reservations_pms
//! ```
//!
//! What it models:
//!   * room types, rate plans, and physical rooms,
//!   * availability over a single shared inventory pool (channel-safe),
//!   * reservations with a folio,
//!   * check-in / check-out,
//!   * a housekeeping room-status lifecycle + board,
//!   * a channel-distribution snapshot.
//!
//! The concept produced by the Concierge design phases (property-layout room
//! types + brand rate plans) is loaded via [`seed_hotel`]; the same identifiers
//! (`type_code` / `plan_code`) appear in the concept document and here.

use std::collections::BTreeMap;
use std::fmt;

// --- Housekeeping room-status lifecycle -------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomStatus {
    Clean,
    Inspected,
    Dirty,
    OutOfOrder,
}

impl RoomStatus {
    /// A room is sellable only when it is turned over and in service.
    fn sellable(self) -> bool {
        matches!(self, RoomStatus::Clean | RoomStatus::Inspected)
    }
}

// --- Reservation lifecycle --------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservationStatus {
    Booked,
    InHouse,
    CheckedOut,
    Cancelled,
}

#[derive(Debug)]
pub struct PmsError(pub String);

impl fmt::Display for PmsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PmsError {}

type PmsResult<T> = Result<T, PmsError>;

fn err<T>(message: impl Into<String>) -> PmsResult<T> {
    Err(PmsError(message.into()))
}

// --- Domain types -----------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RoomType {
    pub code: String,
    pub name: String,
    pub count: u32,
    pub max_occupancy: u32,
    pub size_sqm: u32,
}

#[derive(Clone, Debug)]
pub struct RatePlan {
    pub code: String,
    pub name: String,
    pub base_rate: f64,
    pub cancellation: String,
    pub inclusions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Room {
    pub number: String,
    pub type_code: String,
    pub status: RoomStatus,
    pub reservation_id: Option<u64>,
    pub occupied: bool,
}

impl Room {
    fn sellable(&self) -> bool {
        self.reservation_id.is_none() && self.status.sellable()
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub code: String,
    pub name: String,
    /// Commission fraction, e.g. 0.15 for an OTA.
    pub commission: f64,
    /// Fraction applied to the base rate for this channel.
    pub rate_modifier: f64,
}

#[derive(Clone, Debug)]
pub struct FolioLine {
    pub description: String,
    pub amount: f64,
}

#[derive(Clone, Debug)]
pub struct Reservation {
    pub id: u64,
    pub guest_name: String,
    pub type_code: String,
    pub plan_code: String,
    pub nights: u32,
    pub channel: String,
    pub status: ReservationStatus,
    pub room_number: Option<String>,
    pub folio: Vec<FolioLine>,
}

impl Reservation {
    pub fn folio_total(&self) -> f64 {
        round2(self.folio.iter().map(|line| line.amount).sum())
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// --- The PMS core -----------------------------------------------------------

pub struct Hotel {
    pub name: String,
    pub default_plan_code: String,
    room_types: BTreeMap<String, RoomType>,
    rate_plans: BTreeMap<String, RatePlan>,
    channels: BTreeMap<String, Channel>,
    rooms: Vec<Room>,
    reservations: BTreeMap<u64, Reservation>,
    next_id: u64,
}

impl Hotel {
    pub fn new(name: impl Into<String>, default_plan_code: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default_plan_code: default_plan_code.into(),
            room_types: BTreeMap::new(),
            rate_plans: BTreeMap::new(),
            channels: BTreeMap::new(),
            rooms: Vec::new(),
            reservations: BTreeMap::new(),
            next_id: 1,
        }
    }

    // -- configuration -------------------------------------------------------

    pub fn add_room_type(&mut self, room_type: RoomType) {
        let floor_base = self.room_types.len() + 1;
        for i in 0..room_type.count {
            self.rooms.push(Room {
                number: format!("{floor_base}{:02}", i + 1),
                type_code: room_type.code.clone(),
                status: RoomStatus::Inspected,
                reservation_id: None,
                occupied: false,
            });
        }
        self.room_types.insert(room_type.code.clone(), room_type);
    }

    pub fn add_rate_plan(&mut self, plan: RatePlan) {
        self.rate_plans.insert(plan.code.clone(), plan);
    }

    pub fn add_channel(&mut self, channel: Channel) {
        self.channels.insert(channel.code.clone(), channel);
    }

    pub fn total_rooms(&self) -> usize {
        self.rooms.len()
    }

    pub fn room_type_codes(&self) -> Vec<String> {
        self.room_types.keys().cloned().collect()
    }

    // -- availability --------------------------------------------------------

    pub fn availability(&self, type_code: &str) -> PmsResult<u32> {
        if !self.room_types.contains_key(type_code) {
            return err(format!("unknown room type: {type_code}"));
        }
        Ok(self
            .rooms
            .iter()
            .filter(|r| r.type_code == type_code && r.sellable())
            .count() as u32)
    }

    // -- reservations --------------------------------------------------------

    pub fn reserve(
        &mut self,
        guest_name: &str,
        type_code: &str,
        plan_code: Option<&str>,
        nights: u32,
        channel: &str,
    ) -> PmsResult<u64> {
        if !self.room_types.contains_key(type_code) {
            return err(format!("unknown room type: {type_code}"));
        }
        let plan_code = plan_code.unwrap_or(&self.default_plan_code).to_string();
        let plan = match self.rate_plans.get(&plan_code) {
            Some(plan) => plan.clone(),
            None => return err(format!("unknown rate plan: {plan_code}")),
        };
        if nights < 1 {
            return err("nights must be >= 1");
        }

        let room_index = self
            .rooms
            .iter()
            .position(|r| r.type_code == type_code && r.sellable());
        let room_index = match room_index {
            Some(index) => index,
            None => return err(format!("no availability for room type {type_code}")),
        };

        let id = self.next_id;
        self.next_id += 1;

        let room_number = self.rooms[room_index].number.clone();
        // Hold the room out of the shared pool immediately (channel-safe).
        self.rooms[room_index].reservation_id = Some(id);

        let charge = round2(plan.base_rate * nights as f64);
        let reservation = Reservation {
            id,
            guest_name: guest_name.to_string(),
            type_code: type_code.to_string(),
            plan_code: plan_code.clone(),
            nights,
            channel: channel.to_string(),
            status: ReservationStatus::Booked,
            room_number: Some(room_number),
            folio: vec![FolioLine {
                description: format!("Room x{nights} ({plan_code})"),
                amount: charge,
            }],
        };
        self.reservations.insert(id, reservation);
        Ok(id)
    }

    pub fn reservation(&self, id: u64) -> PmsResult<&Reservation> {
        self.reservations
            .get(&id)
            .ok_or_else(|| PmsError(format!("unknown reservation: {id}")))
    }

    fn room_index_for(&self, res_id: u64) -> PmsResult<usize> {
        let number = self
            .reservations
            .get(&res_id)
            .and_then(|r| r.room_number.clone())
            .ok_or_else(|| PmsError(format!("reservation {res_id} has no room assigned")))?;
        self.rooms
            .iter()
            .position(|r| r.number == number)
            .ok_or_else(|| PmsError(format!("room {number} not found")))
    }

    pub fn check_in(&mut self, res_id: u64) -> PmsResult<String> {
        let status = self.reservation(res_id)?.status;
        if status != ReservationStatus::Booked {
            return err(format!("reservation {res_id} is not checkable-in"));
        }
        let index = self.room_index_for(res_id)?;
        self.rooms[index].occupied = true;
        let number = self.rooms[index].number.clone();
        self.reservations.get_mut(&res_id).unwrap().status = ReservationStatus::InHouse;
        Ok(number)
    }

    pub fn add_charge(&mut self, res_id: u64, description: &str, amount: f64) -> PmsResult<()> {
        let res = self
            .reservations
            .get_mut(&res_id)
            .ok_or_else(|| PmsError(format!("unknown reservation: {res_id}")))?;
        if !matches!(
            res.status,
            ReservationStatus::Booked | ReservationStatus::InHouse
        ) {
            return err("cannot post charges to a closed reservation");
        }
        res.folio.push(FolioLine {
            description: description.to_string(),
            amount: round2(amount),
        });
        Ok(())
    }

    pub fn check_out(&mut self, res_id: u64) -> PmsResult<f64> {
        let status = self.reservation(res_id)?.status;
        if status != ReservationStatus::InHouse {
            return err(format!("reservation {res_id} is not in-house"));
        }
        let index = self.room_index_for(res_id)?;
        let total = self.reservation(res_id)?.folio_total();
        // Departures free the room and send it to housekeeping.
        self.rooms[index].occupied = false;
        self.rooms[index].reservation_id = None;
        self.rooms[index].status = RoomStatus::Dirty;
        self.reservations.get_mut(&res_id).unwrap().status = ReservationStatus::CheckedOut;
        Ok(total)
    }

    // -- housekeeping --------------------------------------------------------

    fn room_index(&self, number: &str) -> PmsResult<usize> {
        self.rooms
            .iter()
            .position(|r| r.number == number)
            .ok_or_else(|| PmsError(format!("unknown room: {number}")))
    }

    pub fn service_room(&mut self, number: &str, inspect: bool) -> PmsResult<()> {
        let index = self.room_index(number)?;
        if self.rooms[index].status == RoomStatus::OutOfOrder {
            return err("out-of-order rooms must be restored before servicing");
        }
        self.rooms[index].status = if inspect {
            RoomStatus::Inspected
        } else {
            RoomStatus::Clean
        };
        Ok(())
    }

    pub fn set_out_of_order(&mut self, number: &str) -> PmsResult<()> {
        let index = self.room_index(number)?;
        if self.rooms[index].reservation_id.is_some() {
            return err("cannot take an occupied/held room out of order");
        }
        self.rooms[index].status = RoomStatus::OutOfOrder;
        Ok(())
    }

    pub fn restore_room(&mut self, number: &str) -> PmsResult<()> {
        let index = self.room_index(number)?;
        if self.rooms[index].status != RoomStatus::OutOfOrder {
            return err("room is not out of order");
        }
        self.rooms[index].status = RoomStatus::Dirty;
        Ok(())
    }

    pub fn housekeeping_board(&self) -> Vec<HousekeepingEntry> {
        let mut board: Vec<HousekeepingEntry> = self
            .rooms
            .iter()
            .map(|room| {
                let is_departure =
                    room.status == RoomStatus::Dirty && room.reservation_id.is_none();
                let priority = if is_departure {
                    1
                } else if room.occupied {
                    2
                } else {
                    3
                };
                HousekeepingEntry {
                    room: room.number.clone(),
                    type_code: room.type_code.clone(),
                    status: room.status,
                    occupied: room.occupied,
                    is_departure,
                    priority,
                }
            })
            .collect();
        board.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.room.cmp(&b.room)));
        board
    }

    // -- channel management --------------------------------------------------

    pub fn channel_snapshot(&self) -> Vec<ChannelRow> {
        let plan = &self.rate_plans[&self.default_plan_code];
        let mut rows = Vec::new();
        for channel in self.channels.values() {
            for type_code in self.room_types.keys() {
                rows.push(ChannelRow {
                    channel: channel.code.clone(),
                    type_code: type_code.clone(),
                    sellable: self.availability(type_code).unwrap_or(0),
                    rate: round2(plan.base_rate * (1.0 + channel.rate_modifier)),
                    plan: plan.code.clone(),
                    commission: channel.commission,
                });
            }
        }
        rows
    }
}

#[derive(Clone, Debug)]
pub struct HousekeepingEntry {
    pub room: String,
    pub type_code: String,
    pub status: RoomStatus,
    pub occupied: bool,
    pub is_departure: bool,
    pub priority: u8,
}

#[derive(Clone, Debug)]
pub struct ChannelRow {
    pub channel: String,
    pub type_code: String,
    pub sellable: u32,
    pub rate: f64,
    pub plan: String,
    pub commission: f64,
}

// --- Concept seed -----------------------------------------------------------

/// Build a [`Hotel`] from a hotel concept. This encodes a small, coherent
/// example concept — the "Simard Aerie" boutique urban hotel — so the reference
/// prototype is runnable out of the box. When the Concierge scaffolds a real
/// engagement it replaces these room types / rate plans with the ones from THIS
/// concept, keeping the same `type_code` / `plan_code` identifiers used in the
/// concept document (property-layout room-type table + brand rate-plan table).
pub fn seed_hotel() -> Hotel {
    let mut hotel = Hotel::new("Simard Aerie", "BAR");

    // Room-type mix (property layout phase). Counts sum to 40 keys.
    hotel.add_room_type(RoomType {
        code: "STD".into(),
        name: "Standard King".into(),
        count: 24,
        max_occupancy: 2,
        size_sqm: 24,
    });
    hotel.add_room_type(RoomType {
        code: "DLX".into(),
        name: "Deluxe Terrace".into(),
        count: 12,
        max_occupancy: 3,
        size_sqm: 32,
    });
    hotel.add_room_type(RoomType {
        code: "STE".into(),
        name: "Aerie Suite".into(),
        count: 4,
        max_occupancy: 4,
        size_sqm: 55,
    });

    // Rate plans (brand design phase).
    hotel.add_rate_plan(RatePlan {
        code: "BAR".into(),
        name: "Best Available Rate".into(),
        base_rate: 189.0,
        cancellation: "flexible".into(),
        inclusions: vec!["wifi".into()],
    });
    hotel.add_rate_plan(RatePlan {
        code: "ADV".into(),
        name: "Advance Saver".into(),
        base_rate: 159.0,
        cancellation: "non-refundable".into(),
        inclusions: vec!["wifi".into()],
    });
    hotel.add_rate_plan(RatePlan {
        code: "PKG".into(),
        name: "Bed & Breakfast".into(),
        base_rate: 219.0,
        cancellation: "flexible".into(),
        inclusions: vec!["wifi".into(), "breakfast".into()],
    });

    // Distribution channels (channel-management phase).
    hotel.add_channel(Channel {
        code: "direct".into(),
        name: "Brand.com".into(),
        commission: 0.0,
        rate_modifier: 0.0,
    });
    hotel.add_channel(Channel {
        code: "ota".into(),
        name: "Online Travel Agency".into(),
        commission: 0.15,
        rate_modifier: 0.0,
    });
    hotel.add_channel(Channel {
        code: "gds".into(),
        name: "Global Distribution System".into(),
        commission: 0.10,
        rate_modifier: 0.02,
    });

    hotel
}

// --- Self-verifying runnable demo -------------------------------------------

/// Runs the end-to-end operational flow and asserts the invariants. Returns the
/// number of checks that passed. Panics (fails the demo) on any violation.
pub fn run_end_to_end_demo() -> u32 {
    let mut checks = 0u32;
    let mut ok = |condition: bool, label: &str| {
        assert!(condition, "FAILED: {label}");
        checks += 1;
        println!("  ok - {label}");
    };

    let mut hotel = seed_hotel();
    ok(hotel.total_rooms() == 40, "seed builds 40 rooms");
    ok(
        hotel.availability("STD").unwrap() == 24,
        "24 standard rooms sellable at open",
    );

    // Shared inventory: a booking on one channel is seen by all channels.
    let ste_before: BTreeMap<String, u32> = hotel
        .channel_snapshot()
        .into_iter()
        .filter(|row| row.type_code == "STE")
        .map(|row| (row.channel, row.sellable))
        .collect();
    let _ = hotel
        .reserve("Grace Hopper", "STE", None, 1, "direct")
        .unwrap();
    let ste_after: BTreeMap<String, u32> = hotel
        .channel_snapshot()
        .into_iter()
        .filter(|row| row.type_code == "STE")
        .map(|row| (row.channel, row.sellable))
        .collect();
    for channel in ["direct", "ota", "gds"] {
        ok(
            ste_after[channel] == ste_before[channel] - 1,
            "direct booking reduces every channel's sellable (shared inventory)",
        );
    }

    // End-to-end stay: book -> check-in -> incidental -> check-out.
    let res = hotel
        .reserve("Ada Lovelace", "DLX", Some("PKG"), 3, "ota")
        .unwrap();
    ok(
        (hotel.reservation(res).unwrap().folio_total() - 657.0).abs() < 1e-6,
        "folio = 3 nights x 219 (PKG)",
    );
    ok(
        hotel.availability("DLX").unwrap() == 11,
        "booking removes a room from shared inventory",
    );
    let room = hotel.check_in(res).unwrap();
    ok(
        hotel.reservation(res).unwrap().status == ReservationStatus::InHouse,
        "check-in puts the reservation in-house",
    );
    hotel.add_charge(res, "Minibar", 12.5).unwrap();
    let total = hotel.check_out(res).unwrap();
    ok(
        (total - 669.5).abs() < 1e-6,
        "check-out settles folio incl. incidentals",
    );
    ok(
        hotel.availability("DLX").unwrap() == 11,
        "departed room is dirty and not sellable until serviced",
    );
    hotel.service_room(&room, false).unwrap();
    ok(
        hotel.availability("DLX").unwrap() == 12,
        "housekeeping returns the room to shared inventory",
    );

    // Housekeeping board sorts departures first.
    let res2 = hotel
        .reserve("Alan Turing", "STD", None, 1, "direct")
        .unwrap();
    hotel.check_in(res2).unwrap();
    hotel.check_out(res2).unwrap();
    let board = hotel.housekeeping_board();
    ok(
        board[0].priority == 1 && board[0].is_departure,
        "departures sort to the top of the housekeeping board",
    );

    // Channel parity + commission.
    let std_rows: BTreeMap<String, ChannelRow> = hotel
        .channel_snapshot()
        .into_iter()
        .filter(|row| row.type_code == "STD")
        .map(|row| (row.channel.clone(), row))
        .collect();
    ok(
        (std_rows["direct"].rate - 189.0).abs() < 1e-6,
        "direct sells BAR at base rate",
    );
    ok(
        std_rows["ota"].commission == 0.15,
        "OTA carries a commission",
    );

    checks
}

fn main() {
    println!("Simard Concierge — reservations/PMS reference prototype");
    let hotel = seed_hotel();
    println!(
        "Hotel: {} | {} rooms | types {:?} | plans loaded",
        hotel.name,
        hotel.total_rooms(),
        hotel.room_type_codes()
    );
    println!("Running end-to-end flow:");
    let checks = run_end_to_end_demo();
    println!("\nresult: ok. {checks} checks passed; 0 failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_builds_shared_inventory() {
        let hotel = seed_hotel();
        assert_eq!(hotel.total_rooms(), 40);
        assert_eq!(hotel.availability("STD").unwrap(), 24);
        assert_eq!(hotel.availability("STE").unwrap(), 4);
        assert!(hotel.availability("NOPE").is_err());
    }

    #[test]
    fn end_to_end_stay_settles_folio_and_dirties_room() {
        let mut hotel = seed_hotel();
        let res = hotel
            .reserve("Ada Lovelace", "DLX", Some("PKG"), 3, "ota")
            .unwrap();
        assert_eq!(hotel.availability("DLX").unwrap(), 11);
        let room = hotel.check_in(res).unwrap();
        hotel.add_charge(res, "Minibar", 12.5).unwrap();
        let total = hotel.check_out(res).unwrap();
        assert!((total - 669.5).abs() < 1e-6);
        // Dirty until serviced.
        assert_eq!(hotel.availability("DLX").unwrap(), 11);
        hotel.service_room(&room, true).unwrap();
        assert_eq!(hotel.availability("DLX").unwrap(), 12);
    }

    #[test]
    fn booking_is_shared_across_channels() {
        let mut hotel = seed_hotel();
        let before: Vec<u32> = hotel
            .channel_snapshot()
            .into_iter()
            .filter(|r| r.type_code == "STE")
            .map(|r| r.sellable)
            .collect();
        hotel
            .reserve("Grace Hopper", "STE", None, 1, "direct")
            .unwrap();
        let after: Vec<u32> = hotel
            .channel_snapshot()
            .into_iter()
            .filter(|r| r.type_code == "STE")
            .map(|r| r.sellable)
            .collect();
        for (b, a) in before.iter().zip(after.iter()) {
            assert_eq!(*a, *b - 1, "every channel sees the shared decrement");
        }
    }

    #[test]
    fn out_of_order_excludes_from_availability() {
        let mut hotel = seed_hotel();
        // Find a sellable STD room number deterministically.
        let board = hotel.housekeeping_board();
        let std_room = board
            .iter()
            .find(|e| e.type_code == "STD")
            .unwrap()
            .room
            .clone();
        let before = hotel.availability("STD").unwrap();
        hotel.set_out_of_order(&std_room).unwrap();
        assert_eq!(hotel.availability("STD").unwrap(), before - 1);
        hotel.restore_room(&std_room).unwrap();
        // Restored room needs housekeeping before it is sellable again.
        assert_eq!(hotel.availability("STD").unwrap(), before - 1);
        hotel.service_room(&std_room, false).unwrap();
        assert_eq!(hotel.availability("STD").unwrap(), before);
    }

    #[test]
    fn housekeeping_board_prioritises_departures() {
        let mut hotel = seed_hotel();
        let res = hotel
            .reserve("Alan Turing", "STD", None, 1, "direct")
            .unwrap();
        hotel.check_in(res).unwrap();
        hotel.check_out(res).unwrap();
        let board = hotel.housekeeping_board();
        assert_eq!(board[0].priority, 1);
        assert!(board[0].is_departure);
    }

    #[test]
    fn cannot_oversell_a_room_type() {
        let mut hotel = seed_hotel();
        for _ in 0..4 {
            hotel.reserve("guest", "STE", None, 1, "direct").unwrap();
        }
        assert_eq!(hotel.availability("STE").unwrap(), 0);
        assert!(hotel.reserve("overflow", "STE", None, 1, "direct").is_err());
    }

    #[test]
    fn demo_runs_all_checks() {
        assert!(run_end_to_end_demo() >= 10);
    }
}
