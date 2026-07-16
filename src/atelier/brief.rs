//! Product-brief data model.
//!
//! A [`ProductBrief`] is the untrusted input to the Atelier pipeline: a
//! description of a furniture / industrial product to be designed. It is parsed
//! from JSON, validated for physical sanity, and then drives parametric
//! geometry generation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{AtelierError, AtelierResult};

/// Default sheet stock dimensions in millimetres (a standard 2440 × 1220 sheet).
pub const DEFAULT_SHEET_LENGTH_MM: f64 = 2440.0;
pub const DEFAULT_SHEET_WIDTH_MM: f64 = 1220.0;

/// A product to be designed, parsed from a brief JSON document.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProductBrief {
    /// Human-readable product name.
    pub name: String,
    /// Product family. Unknown values fall back to a generic panel carcass.
    pub kind: String,
    /// Overall bounding dimensions in millimetres.
    pub dimensions_mm: Dimensions,
    /// Primary sheet/board material.
    pub material: Material,
    /// Kind-specific parameters (shelves, legs, back panel, …).
    #[serde(default)]
    pub parameters: Parameters,
    /// Bill-of-materials hardware lines.
    #[serde(default)]
    pub hardware: Vec<HardwareItem>,
    /// Finish description (e.g. "clear matte lacquer").
    #[serde(default)]
    pub finish: Option<String>,
    /// Optional material budget in the brief's currency.
    #[serde(default)]
    pub budget: Option<f64>,
}

/// Overall bounding dimensions in millimetres.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Dimensions {
    pub width: f64,
    pub depth: f64,
    pub height: f64,
}

/// Sheet/board material description.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Material {
    pub name: String,
    pub thickness_mm: f64,
    #[serde(default)]
    pub sheet_mm: Option<Sheet>,
    #[serde(default)]
    pub cost_per_sheet: Option<f64>,
    /// Whether the material has a directional grain (affects the cut list).
    #[serde(default)]
    pub grain: bool,
}

impl Material {
    /// Sheet stock size, defaulting to a standard full sheet.
    pub fn sheet(&self) -> Sheet {
        self.sheet_mm.unwrap_or(Sheet {
            length: DEFAULT_SHEET_LENGTH_MM,
            width: DEFAULT_SHEET_WIDTH_MM,
        })
    }
}

/// Sheet stock dimensions in millimetres.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Sheet {
    pub length: f64,
    pub width: f64,
}

/// Kind-specific parameters. All optional with sensible per-generator defaults.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Parameters {
    #[serde(default)]
    pub shelves: Option<u32>,
    #[serde(default)]
    pub back_panel: Option<bool>,
    #[serde(default)]
    pub legs: Option<u32>,
    #[serde(default)]
    pub apron: Option<bool>,
    #[serde(default)]
    pub leg_size_mm: Option<f64>,
    #[serde(default)]
    pub open_front: Option<bool>,
    #[serde(default)]
    pub top_overhang_mm: Option<f64>,
    /// Any additional numeric knobs, preserved for documentation in the model.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A single hardware line for the bill of materials.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HardwareItem {
    pub name: String,
    /// Absolute quantity required for the whole product.
    pub qty: u32,
    #[serde(default)]
    pub unit_cost: Option<f64>,
}

/// Upper bound on structural part counts (shelves, legs) accepted from an
/// untrusted brief, guarding against resource exhaustion and integer overflow
/// in the geometry generators. A furniture piece with hundreds of a single
/// part is already far past anything physically sensible.
pub const MAX_PART_COUNT: u32 = 512;

/// Upper bound on a single hardware line quantity from an untrusted brief.
pub const MAX_HARDWARE_QTY: u32 = 100_000;

impl ProductBrief {
    /// Read and validate a brief from a JSON file.
    pub fn from_path(path: &Path) -> AtelierResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| AtelierError::io(format!("reading brief {}", path.display()), e))?;
        let brief = Self::from_json_bytes(&bytes)?;
        Ok(brief)
    }

    /// Parse and validate a brief from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> AtelierResult<Self> {
        let brief: ProductBrief = serde_json::from_slice(bytes)
            .map_err(|e| AtelierError::parse("brief json", e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// Normalised product kind used to select a geometry generator.
    pub fn normalized_kind(&self) -> ProductKind {
        ProductKind::classify(&self.kind)
    }

    /// Reject physically impossible or malformed briefs.
    pub fn validate(&self) -> AtelierResult<()> {
        if self.name.trim().is_empty() {
            return Err(AtelierError::invalid_brief("name must not be empty"));
        }
        let Dimensions {
            width,
            depth,
            height,
        } = self.dimensions_mm;
        for (label, value) in [("width", width), ("depth", depth), ("height", height)] {
            if !value.is_finite() || value <= 0.0 {
                return Err(AtelierError::invalid_brief(format!(
                    "dimension '{label}' must be a positive, finite number (got {value})"
                )));
            }
        }
        let t = self.material.thickness_mm;
        if !t.is_finite() || t <= 0.0 {
            return Err(AtelierError::invalid_brief(format!(
                "material.thickness_mm must be positive and finite (got {t})"
            )));
        }
        // A carcass needs two thicknesses of material across its smallest span
        // plus a little clearance, or it cannot physically be assembled.
        let min_span = width.min(depth).min(height);
        if t * 2.0 >= min_span {
            return Err(AtelierError::invalid_brief(format!(
                "material thickness {t}mm is too large for smallest span {min_span}mm \
                 (need 2×thickness < smallest dimension)"
            )));
        }
        if self.material.name.trim().is_empty() {
            return Err(AtelierError::invalid_brief(
                "material.name must not be empty",
            ));
        }
        let sheet = self.material.sheet();
        if sheet.length <= 0.0 || sheet.width <= 0.0 {
            return Err(AtelierError::invalid_brief(
                "material.sheet_mm dimensions must be positive",
            ));
        }
        if let Some(cost) = self.material.cost_per_sheet
            && (!cost.is_finite() || cost < 0.0)
        {
            return Err(AtelierError::invalid_brief(
                "material.cost_per_sheet must be non-negative",
            ));
        }
        if let Some(budget) = self.budget
            && (!budget.is_finite() || budget < 0.0)
        {
            return Err(AtelierError::invalid_brief("budget must be non-negative"));
        }
        // Bound untrusted count knobs so a hostile brief cannot drive
        // unbounded allocation (resource exhaustion) or integer overflow in
        // the geometry generators.
        for (label, count) in [
            ("parameters.shelves", self.parameters.shelves),
            ("parameters.legs", self.parameters.legs),
        ] {
            if let Some(n) = count
                && n > MAX_PART_COUNT
            {
                return Err(AtelierError::invalid_brief(format!(
                    "{label} must not exceed {MAX_PART_COUNT} (got {n})"
                )));
            }
        }
        for hw in &self.hardware {
            if hw.qty > MAX_HARDWARE_QTY {
                return Err(AtelierError::invalid_brief(format!(
                    "hardware '{}' quantity must not exceed {MAX_HARDWARE_QTY} (got {})",
                    hw.name, hw.qty
                )));
            }
        }
        Ok(())
    }
}

/// Supported product families. Unknown kinds map to [`ProductKind::Carcass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductKind {
    /// Open shelving unit / bookcase with sides, top, bottom, and shelves.
    Bookcase,
    /// A table: top plus legs (and optional aprons).
    Table,
    /// A stool: seat plus legs (and optional stretchers).
    Stool,
    /// A closed box / cabinet carcass (also the generic fallback).
    Carcass,
}

impl ProductKind {
    /// Map a free-form kind string to a generator family.
    pub fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bookcase" | "shelf" | "shelving" | "bookshelf" => Self::Bookcase,
            "table" | "desk" | "workbench" | "bench" => Self::Table,
            "stool" | "chair" | "seat" => Self::Stool,
            "box" | "cabinet" | "cupboard" | "crate" | "carcass" | "enclosure" => Self::Carcass,
            _ => Self::Carcass,
        }
    }

    /// Stable label for manifests and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bookcase => "bookcase",
            Self::Table => "table",
            Self::Stool => "stool",
            Self::Carcass => "carcass",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "name": "Two-shelf bookcase",
            "kind": "bookcase",
            "dimensions_mm": { "width": 800, "depth": 300, "height": 1000 },
            "material": {
                "name": "Birch plywood",
                "thickness_mm": 18,
                "cost_per_sheet": 55.0,
                "grain": true
            },
            "parameters": { "shelves": 2, "back_panel": true },
            "hardware": [ { "name": "Confirmat screw", "qty": 24, "unit_cost": 0.15 } ],
            "finish": "clear matte lacquer",
            "budget": 120.0
        }"#
    }

    #[test]
    fn parses_and_validates_sample() {
        let brief = ProductBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.name, "Two-shelf bookcase");
        assert_eq!(brief.normalized_kind(), ProductKind::Bookcase);
        assert_eq!(brief.parameters.shelves, Some(2));
        assert_eq!(brief.parameters.back_panel, Some(true));
        assert_eq!(brief.material.sheet().length, DEFAULT_SHEET_LENGTH_MM);
        assert_eq!(brief.hardware.len(), 1);
    }

    #[test]
    fn rejects_negative_dimension() {
        let json = r#"{"name":"x","kind":"box","dimensions_mm":{"width":-1,"depth":10,"height":10},
            "material":{"name":"ply","thickness_mm":3}}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_thickness_larger_than_span() {
        let json = r#"{"name":"x","kind":"box","dimensions_mm":{"width":100,"depth":100,"height":100},
            "material":{"name":"ply","thickness_mm":60}}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_empty_name() {
        let json = r#"{"name":"  ","kind":"box","dimensions_mm":{"width":100,"depth":100,"height":100},
            "material":{"name":"ply","thickness_mm":3}}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_shelf_count() {
        let json = r#"{"name":"x","kind":"bookcase","dimensions_mm":{"width":800,"depth":300,"height":1000},
            "material":{"name":"ply","thickness_mm":18},"parameters":{"shelves":4000000000}}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_leg_count() {
        let json = r#"{"name":"x","kind":"table","dimensions_mm":{"width":800,"depth":300,"height":1000},
            "material":{"name":"ply","thickness_mm":18},"parameters":{"legs":100000}}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_hardware_qty() {
        let json = r#"{"name":"x","kind":"box","dimensions_mm":{"width":100,"depth":100,"height":100},
            "material":{"name":"ply","thickness_mm":3},
            "hardware":[{"name":"screw","qty":4000000000}]}"#;
        let err = ProductBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
    }

    #[test]
    fn accepts_reasonable_counts_at_boundary() {
        let json = format!(
            r#"{{"name":"x","kind":"bookcase","dimensions_mm":{{"width":800,"depth":300,"height":1000}},
            "material":{{"name":"ply","thickness_mm":18}},"parameters":{{"shelves":{MAX_PART_COUNT}}}}}"#
        );
        assert!(ProductBrief::from_json_bytes(json.as_bytes()).is_ok());
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = ProductBrief::from_json_bytes(b"{not json").unwrap_err();
        assert!(matches!(err, AtelierError::Parse { .. }));
    }

    #[test]
    fn kind_mapping_is_stable() {
        assert_eq!(ProductKind::classify("Shelf"), ProductKind::Bookcase);
        assert_eq!(ProductKind::classify("DESK"), ProductKind::Table);
        assert_eq!(ProductKind::classify("chair"), ProductKind::Stool);
        assert_eq!(ProductKind::classify("cabinet"), ProductKind::Carcass);
        assert_eq!(ProductKind::classify("gizmo"), ProductKind::Carcass);
        assert_eq!(ProductKind::Table.label(), "table");
    }

    #[test]
    fn unknown_parameters_are_preserved() {
        let json = r#"{"name":"x","kind":"box","dimensions_mm":{"width":100,"depth":100,"height":100},
            "material":{"name":"ply","thickness_mm":3},"parameters":{"custom_knob":7}}"#;
        let brief = ProductBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert!(brief.parameters.extra.contains_key("custom_knob"));
    }
}
