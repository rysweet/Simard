//! Deterministic hotel-concept designer.
//!
//! Turns a compact [`HotelBrief`] (name, location, room count, theme,
//! positioning) into a coherent [`HotelConcept`] covering the three design
//! surfaces the Concierge identity owns:
//!
//! 1. **Property layout** — floor plan, room mix, and public spaces sized to
//!    the requested room count.
//! 2. **Guest experience** — the arrival-to-departure journey and its signature
//!    touchpoints.
//! 3. **Brand design** — name rationale, palette, and voice.
//!
//! The builder is fully deterministic: the same brief always yields the same
//! concept, so it is testable without an LLM and gives the Concierge a
//! repeatable backbone the agentic recipes can refine on top of.

use serde::{Deserialize, Serialize};

/// Market positioning tier for the property. Drives room mix, amenity density,
/// and brand voice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Positioning {
    /// Value-focused, high-efficiency operations.
    Select,
    /// Full-service, balanced comfort and price.
    Upscale,
    /// Design-forward, high-touch service.
    Luxury,
}

impl Positioning {
    /// Parse a positioning label; defaults are handled by the caller.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "select" | "economy" | "value" => Ok(Self::Select),
            "upscale" | "midscale" | "full-service" => Ok(Self::Upscale),
            "luxury" | "premium" | "5-star" => Ok(Self::Luxury),
            other => Err(format!(
                "unknown positioning '{other}' (expected select|upscale|luxury)"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Select => "select-service",
            Self::Upscale => "upscale full-service",
            Self::Luxury => "luxury",
        }
    }

    /// Fraction of rooms allocated to suites (bounded, deterministic).
    fn suite_fraction(self) -> f64 {
        match self {
            Self::Select => 0.05,
            Self::Upscale => 0.12,
            Self::Luxury => 0.25,
        }
    }

    fn voice(self) -> &'static str {
        match self {
            Self::Select => "clear, efficient, and reassuring",
            Self::Upscale => "warm, confident, and attentive",
            Self::Luxury => "understated, precise, and quietly generous",
        }
    }
}

/// Compact input describing the hotel to design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HotelBrief {
    pub name: String,
    pub location: String,
    pub rooms: u32,
    /// Free-text design theme, e.g. "coastal modern" or "alpine lodge".
    pub theme: String,
    pub positioning: Positioning,
}

impl HotelBrief {
    /// The canonical demo brief — used by `simard concierge demo` and tests so
    /// the end-to-end path is exercised without operator input.
    pub fn demo() -> Self {
        Self {
            name: "The Cedar & Fern".to_string(),
            location: "Pacific Northwest coastline".to_string(),
            rooms: 120,
            theme: "coastal forest modern".to_string(),
            positioning: Positioning::Upscale,
        }
    }

    /// Validate the brief. Room count must be within a plausible design range so
    /// the deterministic layout math stays coherent.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("hotel name must not be empty".to_string());
        }
        if self.location.trim().is_empty() {
            return Err("hotel location must not be empty".to_string());
        }
        if self.theme.trim().is_empty() {
            return Err("hotel theme must not be empty".to_string());
        }
        if !(4..=2000).contains(&self.rooms) {
            return Err(format!(
                "room count {} out of supported design range (4..=2000)",
                self.rooms
            ));
        }
        Ok(())
    }
}

/// A single room category in the property's room mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoomCategory {
    pub name: String,
    pub count: u32,
    /// Nightly rate index relative to the base room (base = 100).
    pub rate_index: u32,
}

/// Physical layout of the property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyLayout {
    pub floors: u32,
    pub rooms_per_floor: u32,
    pub room_mix: Vec<RoomCategory>,
    pub public_spaces: Vec<String>,
}

/// A named moment in the guest journey.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JourneyTouchpoint {
    pub stage: String,
    pub description: String,
}

/// Guest-experience design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuestExperience {
    pub promise: String,
    pub journey: Vec<JourneyTouchpoint>,
    pub signature_moments: Vec<String>,
}

/// Brand design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrandDesign {
    pub name_rationale: String,
    pub palette: Vec<String>,
    pub voice: String,
    pub tagline: String,
}

/// The full hotel concept — the design deliverable of the Concierge identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HotelConcept {
    pub brief: HotelBrief,
    pub layout: PropertyLayout,
    pub experience: GuestExperience,
    pub brand: BrandDesign,
}

impl HotelConcept {
    /// Deterministically design a concept from a validated brief.
    pub fn design(brief: HotelBrief) -> Result<Self, String> {
        brief.validate()?;
        let layout = design_layout(&brief);
        let experience = design_experience(&brief);
        let brand = design_brand(&brief);
        Ok(Self {
            brief,
            layout,
            experience,
            brand,
        })
    }

    /// Render the concept as a human-readable Markdown document.
    pub fn to_markdown(&self) -> String {
        let b = &self.brief;
        let mut out = String::new();
        out.push_str(&format!("# {} — Hotel Concept\n\n", b.name));
        out.push_str(&format!(
            "- **Location:** {}\n- **Positioning:** {}\n- **Theme:** {}\n- **Rooms:** {}\n\n",
            b.location,
            b.positioning.label(),
            b.theme,
            b.rooms
        ));

        out.push_str("## 1. Property Layout\n\n");
        out.push_str(&format!(
            "- **Floors:** {} (≈{} rooms per guest floor)\n",
            self.layout.floors, self.layout.rooms_per_floor
        ));
        out.push_str("- **Room mix:**\n");
        for cat in &self.layout.room_mix {
            out.push_str(&format!(
                "  - {} × {} (rate index {})\n",
                cat.count, cat.name, cat.rate_index
            ));
        }
        out.push_str("- **Public spaces:**\n");
        for space in &self.layout.public_spaces {
            out.push_str(&format!("  - {space}\n"));
        }
        out.push('\n');

        out.push_str("## 2. Guest Experience\n\n");
        out.push_str(&format!("> {}\n\n", self.experience.promise));
        out.push_str("**Journey:**\n\n");
        for tp in &self.experience.journey {
            out.push_str(&format!("- **{}** — {}\n", tp.stage, tp.description));
        }
        out.push_str("\n**Signature moments:**\n\n");
        for moment in &self.experience.signature_moments {
            out.push_str(&format!("- {moment}\n"));
        }
        out.push('\n');

        out.push_str("## 3. Brand Design\n\n");
        out.push_str(&format!("- **Tagline:** {}\n", self.brand.tagline));
        out.push_str(&format!(
            "- **Name rationale:** {}\n",
            self.brand.name_rationale
        ));
        out.push_str(&format!("- **Voice:** {}\n", self.brand.voice));
        out.push_str(&format!(
            "- **Palette:** {}\n",
            self.brand.palette.join(", ")
        ));
        out.push('\n');

        out
    }
}

fn design_layout(brief: &HotelBrief) -> PropertyLayout {
    let rooms = brief.rooms;

    // Target ~18 rooms per guest floor; keep at least one floor. Deterministic
    // ceil division so the floor count always accommodates every room.
    let rooms_per_floor = 18u32.min(rooms.max(1));
    let floors = rooms.div_ceil(rooms_per_floor).max(1);

    let suites = ((rooms as f64) * brief.positioning.suite_fraction()).round() as u32;
    let suites = suites.min(rooms.saturating_sub(1)); // never all suites
    let accessible = (rooms / 20).max(1); // ~5% accessible, at least one
    let accessible = accessible.min(rooms.saturating_sub(suites).saturating_sub(1).max(1));
    let standard = rooms.saturating_sub(suites).saturating_sub(accessible);

    let mut room_mix = vec![RoomCategory {
        name: "Standard King/Queen".to_string(),
        count: standard,
        rate_index: 100,
    }];
    if accessible > 0 {
        room_mix.push(RoomCategory {
            name: "Accessible Standard".to_string(),
            count: accessible,
            rate_index: 100,
        });
    }
    if suites > 0 {
        room_mix.push(RoomCategory {
            name: "Suite".to_string(),
            count: suites,
            rate_index: match brief.positioning {
                Positioning::Select => 150,
                Positioning::Upscale => 185,
                Positioning::Luxury => 240,
            },
        });
    }

    let mut public_spaces = vec![
        "Lobby & front desk".to_string(),
        format!("Signature restaurant ({} theme)", brief.theme),
        "Fitness studio".to_string(),
    ];
    match brief.positioning {
        Positioning::Select => {
            public_spaces.push("Grab-and-go market".to_string());
        }
        Positioning::Upscale => {
            public_spaces.push("Lobby bar & lounge".to_string());
            public_spaces.push("Flexible meeting rooms".to_string());
        }
        Positioning::Luxury => {
            public_spaces.push("Destination bar & lounge".to_string());
            public_spaces.push("Full-service spa".to_string());
            public_spaces.push("Ballroom & event lawn".to_string());
        }
    }

    PropertyLayout {
        floors,
        rooms_per_floor,
        room_mix,
        public_spaces,
    }
}

fn design_experience(brief: &HotelBrief) -> GuestExperience {
    let promise = format!(
        "Every guest at {} should feel the {} theme from the first message to the last goodbye.",
        brief.name, brief.theme
    );

    let journey = vec![
        JourneyTouchpoint {
            stage: "Pre-arrival".to_string(),
            description:
                "Confirmation with a personalised note and a one-tap room-preference form."
                    .to_string(),
        },
        JourneyTouchpoint {
            stage: "Arrival".to_string(),
            description: format!(
                "Warm {} welcome; keyless check-in offered alongside a staffed desk.",
                brief.positioning.label()
            ),
        },
        JourneyTouchpoint {
            stage: "In-room".to_string(),
            description: format!(
                "Room dressed to the {} theme with a local welcome amenity.",
                brief.theme
            ),
        },
        JourneyTouchpoint {
            stage: "Stay".to_string(),
            description: "Proactive housekeeping and a single messaging thread for any request."
                .to_string(),
        },
        JourneyTouchpoint {
            stage: "Departure".to_string(),
            description: "Express checkout with an emailed folio and a return-stay offer."
                .to_string(),
        },
    ];

    let signature_moments = match brief.positioning {
        Positioning::Select => vec![
            "Fast, friendly keyless check-in".to_string(),
            "Complimentary morning coffee ritual".to_string(),
        ],
        Positioning::Upscale => vec![
            "Evening lobby tasting inspired by the theme".to_string(),
            "Turndown note referencing the guest's stay".to_string(),
        ],
        Positioning::Luxury => vec![
            "Personal host assigned at booking".to_string(),
            "Curated in-room welcome tailored to the guest profile".to_string(),
            "Signature spa ritual on arrival".to_string(),
        ],
    };

    GuestExperience {
        promise,
        journey,
        signature_moments,
    }
}

fn design_brand(brief: &HotelBrief) -> BrandDesign {
    let name_rationale = format!(
        "\"{}\" anchors the property to its {} setting and its {} theme, giving staff and guests a single, memorable story.",
        brief.name, brief.location, brief.theme
    );

    let palette = match brief.positioning {
        Positioning::Select => vec![
            "Slate #2F3E46".to_string(),
            "Fog #CAD2C5".to_string(),
            "Amber #E9C46A".to_string(),
        ],
        Positioning::Upscale => vec![
            "Deep teal #1F4E5F".to_string(),
            "Warm sand #E4D5B7".to_string(),
            "Copper #B07D62".to_string(),
        ],
        Positioning::Luxury => vec![
            "Ink #14213D".to_string(),
            "Champagne #E5DCC3".to_string(),
            "Brass #C9A227".to_string(),
        ],
    };

    let tagline = format!("{} — {}.", brief.name, brief.theme);

    BrandDesign {
        name_rationale,
        palette,
        voice: brief.positioning.voice().to_string(),
        tagline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioning_parse_accepts_synonyms() {
        assert_eq!(Positioning::parse("value").unwrap(), Positioning::Select);
        assert_eq!(
            Positioning::parse("Full-Service").unwrap(),
            Positioning::Upscale
        );
        assert_eq!(Positioning::parse("5-star").unwrap(), Positioning::Luxury);
        assert!(Positioning::parse("mystery").is_err());
    }

    #[test]
    fn brief_validation_rejects_bad_input() {
        let mut brief = HotelBrief::demo();
        brief.rooms = 0;
        assert!(brief.validate().is_err());
        brief.rooms = 100;
        brief.name = "  ".to_string();
        assert!(brief.validate().is_err());
    }

    #[test]
    fn design_is_deterministic() {
        let a = HotelConcept::design(HotelBrief::demo()).unwrap();
        let b = HotelConcept::design(HotelBrief::demo()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn room_mix_sums_to_room_count() {
        for rooms in [4u32, 25, 120, 480, 2000] {
            for pos in [
                Positioning::Select,
                Positioning::Upscale,
                Positioning::Luxury,
            ] {
                let brief = HotelBrief {
                    name: "Test".to_string(),
                    location: "Anywhere".to_string(),
                    rooms,
                    theme: "test theme".to_string(),
                    positioning: pos,
                };
                let concept = HotelConcept::design(brief).unwrap();
                let total: u32 = concept.layout.room_mix.iter().map(|c| c.count).sum();
                assert_eq!(total, rooms, "room mix must sum to {rooms} for {pos:?}");
                assert!(concept.layout.floors >= 1);
            }
        }
    }

    #[test]
    fn luxury_has_more_public_spaces_than_select() {
        let mut brief = HotelBrief::demo();
        brief.positioning = Positioning::Select;
        let select = HotelConcept::design(brief.clone()).unwrap();
        brief.positioning = Positioning::Luxury;
        let luxury = HotelConcept::design(brief).unwrap();
        assert!(luxury.layout.public_spaces.len() > select.layout.public_spaces.len());
    }

    #[test]
    fn markdown_contains_all_three_sections() {
        let md = HotelConcept::design(HotelBrief::demo())
            .unwrap()
            .to_markdown();
        assert!(md.contains("## 1. Property Layout"));
        assert!(md.contains("## 2. Guest Experience"));
        assert!(md.contains("## 3. Brand Design"));
        assert!(md.contains("The Cedar & Fern"));
    }

    #[test]
    fn concept_roundtrips_through_json() {
        let concept = HotelConcept::design(HotelBrief::demo()).unwrap();
        let json = serde_json::to_string(&concept).unwrap();
        let back: HotelConcept = serde_json::from_str(&json).unwrap();
        assert_eq!(concept, back);
    }
}
