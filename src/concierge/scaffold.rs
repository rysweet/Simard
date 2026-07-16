//! Scaffolding: materialise a hotel concept plus a runnable reservations/PMS
//! prototype into an output directory, and load it back.
//!
//! `scaffold` writes three artifacts the Concierge produces for a property:
//!
//! - `concept.md` — the human-readable hotel concept.
//! - `prototype.json` — a machine-readable [`PrototypeSeed`]: a clean PMS engine
//!   built from the concept plus a seed list of booking requests to run.
//! - `README.md` — how to run the prototype (`simard concierge run <dir>`).
//!
//! `load` re-reads `prototype.json` so `simard concierge run` can execute the
//! prototype end-to-end. Everything stays in Rust/JSON — no non-Rust source is
//! generated — while remaining genuinely runnable via the engine in `pms.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::concept::HotelConcept;
use super::pms::PmsEngine;

/// A single booking request in the prototype seed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookingRequest {
    pub guest: String,
    pub category: String,
    pub nights: u32,
    pub channel: String,
}

/// Everything `simard concierge run` needs to execute the prototype: a clean
/// engine plus the bookings to drive through it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrototypeSeed {
    pub concept: HotelConcept,
    pub engine: PmsEngine,
    pub bookings: Vec<BookingRequest>,
}

impl PrototypeSeed {
    /// Build a seed from a concept, generating a deterministic set of demo
    /// bookings spread across the property's room categories and channels.
    pub fn from_concept(concept: HotelConcept) -> Self {
        let engine = PmsEngine::from_concept(&concept);
        let bookings = demo_bookings(&engine);
        Self {
            concept,
            engine,
            bookings,
        }
    }
}

/// Generate a deterministic booking set: for each category, book a handful of
/// stays alternating between direct and OTA channels, bounded by inventory.
fn demo_bookings(engine: &PmsEngine) -> Vec<BookingRequest> {
    use std::collections::BTreeMap;
    let mut per_category: BTreeMap<String, u32> = BTreeMap::new();
    for room in engine.rooms() {
        *per_category.entry(room.category.clone()).or_insert(0) += 1;
    }

    let channels = ["direct", "ota-expedia", "ota-booking"];
    let guests = [
        "Ada Lovelace",
        "Grace Hopper",
        "Alan Turing",
        "Katherine Johnson",
        "Edsger Dijkstra",
        "Barbara Liskov",
    ];

    let mut bookings = Vec::new();
    let mut guest_idx = 0usize;
    for (category, count) in per_category {
        // Book up to 3 stays per category, never exceeding inventory.
        let n = count.min(3);
        for i in 0..n {
            bookings.push(BookingRequest {
                guest: guests[guest_idx % guests.len()].to_string(),
                category: category.clone(),
                nights: (i % 3) + 1,
                channel: channels[(guest_idx) % channels.len()].to_string(),
            });
            guest_idx += 1;
        }
    }
    bookings
}

/// Absolute path of the prototype seed inside a scaffold directory.
pub fn seed_path(dir: &Path) -> PathBuf {
    dir.join("prototype.json")
}

/// Paths written by [`scaffold`], for reporting.
#[derive(Clone, Debug, PartialEq)]
pub struct ScaffoldOutput {
    pub concept_md: PathBuf,
    pub prototype_json: PathBuf,
    pub readme_md: PathBuf,
}

/// Materialise the concept and prototype seed into `dir`, creating it if needed.
pub fn scaffold(concept: &HotelConcept, dir: &Path) -> Result<ScaffoldOutput, String> {
    fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create scaffold dir {}: {e}", dir.display()))?;

    let concept_md = dir.join("concept.md");
    fs::write(&concept_md, concept.to_markdown())
        .map_err(|e| format!("failed to write {}: {e}", concept_md.display()))?;

    let seed = PrototypeSeed::from_concept(concept.clone());
    let prototype_json = seed_path(dir);
    let json = serde_json::to_string_pretty(&seed)
        .map_err(|e| format!("failed to serialise prototype seed: {e}"))?;
    fs::write(&prototype_json, json)
        .map_err(|e| format!("failed to write {}: {e}", prototype_json.display()))?;

    let readme_md = dir.join("README.md");
    fs::write(&readme_md, readme_contents(&seed))
        .map_err(|e| format!("failed to write {}: {e}", readme_md.display()))?;

    Ok(ScaffoldOutput {
        concept_md,
        prototype_json,
        readme_md,
    })
}

/// Load a prototype seed previously written by [`scaffold`].
pub fn load(dir: &Path) -> Result<PrototypeSeed, String> {
    let path = seed_path(dir);
    let json = fs::read_to_string(&path).map_err(|e| {
        format!(
            "failed to read prototype seed {} (run `simard concierge scaffold` first): {e}",
            path.display()
        )
    })?;
    serde_json::from_str(&json)
        .map_err(|e| format!("failed to parse prototype seed {}: {e}", path.display()))
}

fn readme_contents(seed: &PrototypeSeed) -> String {
    format!(
        "# {name} — Reservations/PMS Prototype\n\n\
         Scaffolded by the Simard **Concierge** identity.\n\n\
         ## What this is\n\n\
         A runnable in-memory property-management prototype covering reservations,\n\
         PMS front-desk (check-in/out), housekeeping, and channel management for\n\
         **{name}** ({rooms} rooms).\n\n\
         ## Files\n\n\
         - `concept.md` — the hotel concept (property layout, guest experience, brand).\n\
         - `prototype.json` — the clean PMS engine plus {bookings} seed bookings.\n\
         - `README.md` — this file.\n\n\
         ## Run it\n\n\
         ```sh\n\
         simard concierge run {dir}\n\
         ```\n\n\
         This books the seed reservations, checks guests in and out, runs a\n\
         housekeeping cycle, and pushes availability to the channels, printing an\n\
         operations trace and a final occupancy/housekeeping/channel report.\n",
        name = seed.concept.brief.name,
        rooms = seed.concept.brief.rooms,
        bookings = seed.bookings.len(),
        dir = "<this-directory>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concierge::concept::HotelBrief;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "simard-concierge-scaffold-{}",
            uuid::Uuid::now_v7()
        ))
    }

    #[test]
    fn scaffold_writes_all_artifacts_and_reloads() {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        let dir = temp_dir();
        let out = scaffold(&concept, &dir).unwrap();
        assert!(out.concept_md.is_file());
        assert!(out.prototype_json.is_file());
        assert!(out.readme_md.is_file());

        let seed = load(&dir).unwrap();
        assert_eq!(seed.concept, concept);
        assert!(!seed.bookings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn demo_bookings_are_within_inventory_and_valid_categories() {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        let seed = PrototypeSeed::from_concept(concept);
        let categories: std::collections::BTreeSet<_> =
            seed.engine.rooms().iter().map(|r| &r.category).collect();
        for b in &seed.bookings {
            assert!(
                categories.contains(&b.category),
                "category {} must exist",
                b.category
            );
            assert!(b.nights >= 1);
        }
    }

    #[test]
    fn load_missing_seed_is_an_error() {
        let dir = temp_dir();
        assert!(load(&dir).is_err());
    }

    #[test]
    fn seed_roundtrips_through_json() {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        let seed = PrototypeSeed::from_concept(concept);
        let json = serde_json::to_string(&seed).unwrap();
        let back: PrototypeSeed = serde_json::from_str(&json).unwrap();
        assert_eq!(seed, back);
    }
}
