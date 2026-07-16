//! The [`ProductBrief`] — the untrusted, human-authored description of the
//! physical product the Atelier identity must design and fabricate.
//!
//! A brief is deliberately small and declarative: a product *kind*, an outer
//! bounding box in millimetres, a sheet/stock thickness, and a handful of
//! finishing options. Everything downstream (parametric CAD source, cut list,
//! bill of materials) is *derived* from this single shape so the pipeline stays
//! a pure function of the brief.

use serde::{Deserialize, Serialize};

use super::error::{AtelierError, AtelierResult};

/// The family of physical product to fabricate. Each kind maps to a distinct
/// parametric model and cut-list decomposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProductKind {
    /// A four-legged table: top panel, four legs, four aprons/rails.
    Table,
    /// An open bookcase/shelf unit: two sides, top, bottom, back, N interior
    /// shelves.
    Shelf,
    /// A five-sided open box / carcass (cabinet body, crate): bottom, top, two
    /// sides, back.
    Box,
}

impl ProductKind {
    /// Parse a kebab/lowercase product-kind token.
    pub fn parse(token: &str) -> AtelierResult<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "shelf" | "bookcase" | "bookshelf" => Ok(Self::Shelf),
            "box" | "cabinet" | "carcass" | "crate" => Ok(Self::Box),
            other => Err(AtelierError::UnknownKind {
                requested: other.to_string(),
            }),
        }
    }

    /// Stable machine label for the kind.
    pub fn label(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Shelf => "shelf",
            Self::Box => "box",
        }
    }
}

/// A declarative product brief. Dimensions are the *outer* bounding box in
/// millimetres; `panel_thickness_mm` is the stock/sheet thickness used for the
/// carcass panels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductBrief {
    /// Human-facing product name (used to name output artifacts).
    pub name: String,
    /// Product family.
    pub kind: ProductKind,
    /// Outer width (X) in millimetres.
    pub width_mm: f64,
    /// Outer depth (Y) in millimetres.
    pub depth_mm: f64,
    /// Outer height (Z) in millimetres.
    pub height_mm: f64,
    /// Panel / sheet-stock thickness in millimetres.
    pub panel_thickness_mm: f64,
    /// Material label (e.g. `"birch-plywood"`, `"oak"`, `"mdf"`).
    #[serde(default = "default_material")]
    pub material: String,
    /// Number of interior shelves (only meaningful for [`ProductKind::Shelf`]).
    #[serde(default)]
    pub shelves: u32,
    /// Number of finished units to build. BOM quantities scale by this.
    #[serde(default = "default_quantity")]
    pub quantity: u32,
    /// Surface finish label (e.g. `"oil"`, `"lacquer"`, `"none"`).
    #[serde(default = "default_finish")]
    pub finish: String,
}

fn default_material() -> String {
    "birch-plywood".to_string()
}

fn default_finish() -> String {
    "oil".to_string()
}

fn default_quantity() -> u32 {
    1
}

/// Largest single outer dimension a brief may specify (100 m — generous for
/// furniture; bounds pathological geometry that could hang or OOM a render).
const MAX_DIMENSION_MM: f64 = 100_000.0;
/// Upper bound on interior shelves; keeps [`crate::atelier::CutList`] part
/// counts well within `u32` and the OpenSCAD shelf loop tractable.
const MAX_SHELVES: u32 = 1_000;
/// Upper bound on build quantity so bill-of-materials arithmetic can't overflow.
const MAX_QUANTITY: u32 = 100_000;

impl ProductBrief {
    /// Parse a brief from JSON bytes, then validate design invariants.
    pub fn from_json(bytes: &[u8]) -> AtelierResult<Self> {
        let brief: ProductBrief =
            serde_json::from_slice(bytes).map_err(|error| AtelierError::BriefParse {
                reason: error.to_string(),
            })?;
        brief.validate()?;
        Ok(brief)
    }

    /// Enforce the physical invariants a brief must satisfy before it can be
    /// turned into a model or cut list.
    pub fn validate(&self) -> AtelierResult<()> {
        if self.name.trim().is_empty() {
            return Err(AtelierError::InvalidBrief {
                field: "name".into(),
                reason: "must not be empty".into(),
            });
        }
        for (field, value) in [("name", &self.name), ("material", &self.material)] {
            if value.chars().any(|c| c.is_control()) {
                return Err(AtelierError::InvalidBrief {
                    field: field.into(),
                    reason: "must not contain control characters or newlines".into(),
                });
            }
        }
        for (field, value) in [
            ("width_mm", self.width_mm),
            ("depth_mm", self.depth_mm),
            ("height_mm", self.height_mm),
            ("panel_thickness_mm", self.panel_thickness_mm),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(AtelierError::InvalidBrief {
                    field: field.into(),
                    reason: "must be a finite value greater than zero".into(),
                });
            }
            if value > MAX_DIMENSION_MM {
                return Err(AtelierError::InvalidBrief {
                    field: field.into(),
                    reason: format!("must not exceed {MAX_DIMENSION_MM:.0}mm"),
                });
            }
        }
        // A panel cannot be thicker than the smallest outer dimension, or the
        // carcass would have no interior. Guard against degenerate geometry.
        let min_outer = self.width_mm.min(self.depth_mm).min(self.height_mm);
        if self.panel_thickness_mm * 2.0 >= min_outer {
            return Err(AtelierError::InvalidBrief {
                field: "panel_thickness_mm".into(),
                reason: format!(
                    "two panels ({:.1}mm) must fit within the smallest outer dimension ({:.1}mm)",
                    self.panel_thickness_mm * 2.0,
                    min_outer
                ),
            });
        }
        if self.shelves > MAX_SHELVES {
            return Err(AtelierError::InvalidBrief {
                field: "shelves".into(),
                reason: format!("must not exceed {MAX_SHELVES}"),
            });
        }
        if self.quantity == 0 {
            return Err(AtelierError::InvalidBrief {
                field: "quantity".into(),
                reason: "must be at least 1".into(),
            });
        }
        if self.quantity > MAX_QUANTITY {
            return Err(AtelierError::InvalidBrief {
                field: "quantity".into(),
                reason: format!("must not exceed {MAX_QUANTITY}"),
            });
        }
        Ok(())
    }

    /// A filesystem-safe slug derived from the product name, used to name
    /// artifacts (`table-oak-desk` → `table-oak-desk`).
    pub fn slug(&self) -> String {
        let mut slug = String::with_capacity(self.name.len());
        let mut prev_dash = false;
        for ch in self.name.trim().chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                prev_dash = false;
            } else if !prev_dash {
                slug.push('-');
                prev_dash = true;
            }
        }
        let trimmed = slug.trim_matches('-').to_string();
        if trimmed.is_empty() {
            self.kind.label().to_string()
        } else {
            trimmed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_brief() -> ProductBrief {
        ProductBrief {
            name: "Oak Writing Desk".into(),
            kind: ProductKind::Table,
            width_mm: 1200.0,
            depth_mm: 600.0,
            height_mm: 740.0,
            panel_thickness_mm: 18.0,
            material: "oak".into(),
            shelves: 0,
            quantity: 1,
            finish: "oil".into(),
        }
    }

    #[test]
    fn kind_parse_accepts_aliases() {
        assert_eq!(ProductKind::parse("table").unwrap(), ProductKind::Table);
        assert_eq!(ProductKind::parse("BOOKCASE").unwrap(), ProductKind::Shelf);
        assert_eq!(ProductKind::parse("cabinet").unwrap(), ProductKind::Box);
        assert_eq!(ProductKind::parse(" crate ").unwrap(), ProductKind::Box);
    }

    #[test]
    fn kind_parse_rejects_unknown() {
        let err = ProductKind::parse("spaceship").unwrap_err();
        assert!(matches!(err, AtelierError::UnknownKind { .. }));
    }

    #[test]
    fn from_json_roundtrips_and_applies_defaults() {
        let json = br#"{
            "name": "Simple Shelf",
            "kind": "shelf",
            "width_mm": 800,
            "depth_mm": 300,
            "height_mm": 1800,
            "panel_thickness_mm": 18,
            "shelves": 4
        }"#;
        let brief = ProductBrief::from_json(json).unwrap();
        assert_eq!(brief.kind, ProductKind::Shelf);
        assert_eq!(brief.shelves, 4);
        // defaults
        assert_eq!(brief.quantity, 1);
        assert_eq!(brief.material, "birch-plywood");
        assert_eq!(brief.finish, "oil");
    }

    #[test]
    fn from_json_rejects_malformed() {
        let err = ProductBrief::from_json(b"{ not json").unwrap_err();
        assert!(matches!(err, AtelierError::BriefParse { .. }));
    }

    #[test]
    fn validate_rejects_non_positive_dimension() {
        let mut brief = valid_brief();
        brief.width_mm = 0.0;
        assert!(matches!(
            brief.validate().unwrap_err(),
            AtelierError::InvalidBrief { field, .. } if field == "width_mm"
        ));
    }

    #[test]
    fn validate_rejects_nan_dimension() {
        let mut brief = valid_brief();
        brief.height_mm = f64::NAN;
        assert!(brief.validate().is_err());
    }

    #[test]
    fn validate_rejects_panel_too_thick() {
        let mut brief = valid_brief();
        brief.depth_mm = 30.0;
        brief.panel_thickness_mm = 18.0; // 36mm of panel > 30mm depth
        assert!(matches!(
            brief.validate().unwrap_err(),
            AtelierError::InvalidBrief { field, .. } if field == "panel_thickness_mm"
        ));
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut brief = valid_brief();
        brief.name = "   ".into();
        assert!(matches!(
            brief.validate().unwrap_err(),
            AtelierError::InvalidBrief { field, .. } if field == "name"
        ));
    }

    #[test]
    fn validate_rejects_zero_quantity() {
        let mut brief = valid_brief();
        brief.quantity = 0;
        assert!(brief.validate().is_err());
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(valid_brief().slug(), "oak-writing-desk");
        let mut brief = valid_brief();
        brief.name = "!!!".into();
        assert_eq!(brief.slug(), "table");
    }

    #[test]
    fn validate_rejects_control_chars_in_name_and_material() {
        let mut brief = valid_brief();
        brief.name = "Desk\ncube(9);".into();
        assert!(brief.validate().is_err());

        let mut brief = valid_brief();
        brief.material = "oak\t// x".into();
        assert!(brief.validate().is_err());
    }

    #[test]
    fn validate_rejects_oversized_dimension() {
        let mut brief = valid_brief();
        brief.width_mm = MAX_DIMENSION_MM + 1.0;
        assert!(brief.validate().is_err());
    }

    #[test]
    fn validate_rejects_excessive_shelves() {
        let mut brief = valid_brief();
        brief.kind = ProductKind::Shelf;
        brief.shelves = MAX_SHELVES + 1;
        assert!(brief.validate().is_err());
    }

    #[test]
    fn validate_rejects_excessive_quantity() {
        let mut brief = valid_brief();
        brief.quantity = MAX_QUANTITY + 1;
        assert!(brief.validate().is_err());
    }

    #[test]
    fn validate_accepts_bounds_at_limit() {
        let mut brief = valid_brief();
        brief.kind = ProductKind::Shelf;
        brief.shelves = MAX_SHELVES;
        brief.quantity = MAX_QUANTITY;
        assert!(brief.validate().is_ok());
    }
}
