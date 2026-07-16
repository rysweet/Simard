//! Product brief parsing and validation.
//!
//! A product brief is the untrusted, human-authored request that the Atelier
//! identity turns into a parametric model. It is intentionally small and
//! declarative so the same brief drives both the deterministic Rust geometry
//! generator and the richer OpenSCAD/FreeCAD/Blender toolchains.

use serde::{Deserialize, Serialize};

use super::error::{AtelierError, AtelierResult};

/// The family of furniture/product the brief describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProductType {
    /// A flat panel (single sheet-good part) — the simplest primitive.
    Panel,
    /// An open-top box / tray / drawer carcass (bottom + four walls).
    Box,
    /// A four-legged table with a rectangular top.
    Table,
    /// A bookshelf: two uprights, a top, a bottom, and interior shelves.
    Shelf,
}

impl ProductType {
    /// Human label used in cut lists, BOMs, and rendered titles.
    pub fn label(self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Box => "box",
            Self::Table => "table",
            Self::Shelf => "shelf",
        }
    }
}

/// Outer dimensions of the product, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dimensions {
    /// Overall length (X axis).
    pub length_mm: f64,
    /// Overall width / depth (Y axis).
    pub width_mm: f64,
    /// Overall height (Z axis).
    pub height_mm: f64,
    /// Material / sheet-good thickness.
    pub thickness_mm: f64,
}

/// A fully-parsed, validated product brief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductBrief {
    /// Short human name, e.g. "Standing Desk".
    pub name: String,
    /// Which primitive family to generate.
    pub product_type: ProductType,
    /// Outer dimensions in millimetres.
    pub dimensions: Dimensions,
    /// Material description, e.g. "18mm birch plywood".
    #[serde(default = "default_material")]
    pub material: String,
    /// How many units to fabricate (drives the BOM totals).
    #[serde(default = "default_quantity")]
    pub quantity: u32,
    /// Interior shelves for `Shelf` products (ignored otherwise).
    #[serde(default = "default_shelf_count")]
    pub shelf_count: u32,
    /// Square leg cross-section (mm) for `Table` products.
    #[serde(default = "default_leg_section")]
    pub leg_section_mm: f64,
}

fn default_material() -> String {
    "18mm plywood".to_string()
}
fn default_quantity() -> u32 {
    1
}
fn default_shelf_count() -> u32 {
    3
}
fn default_leg_section_mm() -> f64 {
    50.0
}
fn default_leg_section() -> f64 {
    default_leg_section_mm()
}

impl ProductBrief {
    /// Parse a brief from JSON bytes and validate it.
    pub fn from_json_slice(bytes: &[u8]) -> AtelierResult<Self> {
        let brief: ProductBrief =
            serde_json::from_slice(bytes).map_err(|e| AtelierError::BriefParse {
                reason: e.to_string(),
            })?;
        brief.validate()?;
        Ok(brief)
    }

    /// Validate semantic invariants that JSON typing alone cannot enforce.
    pub fn validate(&self) -> AtelierResult<()> {
        if self.name.trim().is_empty() {
            return Err(AtelierError::invalid("name", "must not be empty"));
        }
        let d = &self.dimensions;
        for (field, value) in [
            ("dimensions.length_mm", d.length_mm),
            ("dimensions.width_mm", d.width_mm),
            ("dimensions.height_mm", d.height_mm),
            ("dimensions.thickness_mm", d.thickness_mm),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(AtelierError::invalid(field, "must be a positive number"));
            }
        }
        if self.quantity == 0 {
            return Err(AtelierError::invalid("quantity", "must be at least 1"));
        }
        // For carcass products (box, shelf) two opposing walls plus their
        // thickness must fit inside the outer envelope, otherwise the walls
        // would overlap and the geometry is meaningless. Flat panels and table
        // tops legitimately have thickness == height, so they only require the
        // thickness to fit within the outer dimensions.
        let min_outer = d.length_mm.min(d.width_mm).min(d.height_mm);
        match self.product_type {
            ProductType::Box | ProductType::Shelf => {
                if self.dimensions.thickness_mm * 2.0 >= min_outer {
                    return Err(AtelierError::invalid(
                        "dimensions.thickness_mm",
                        "too large for the outer dimensions (walls would overlap)",
                    ));
                }
            }
            ProductType::Panel | ProductType::Table => {
                if self.dimensions.thickness_mm > min_outer {
                    return Err(AtelierError::invalid(
                        "dimensions.thickness_mm",
                        "must not exceed the smallest outer dimension",
                    ));
                }
            }
        }
        if matches!(self.product_type, ProductType::Table) && self.leg_section_mm <= 0.0 {
            return Err(AtelierError::invalid(
                "leg_section_mm",
                "must be a positive number for tables",
            ));
        }
        Ok(())
    }

    /// A filesystem-safe slug derived from the brief name.
    pub fn slug(&self) -> String {
        let mut slug = String::new();
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
            "product".to_string()
        } else {
            trimmed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_brief() -> ProductBrief {
        ProductBrief {
            name: "Test Table".into(),
            product_type: ProductType::Table,
            dimensions: Dimensions {
                length_mm: 1200.0,
                width_mm: 600.0,
                height_mm: 720.0,
                thickness_mm: 18.0,
            },
            material: "18mm birch plywood".into(),
            quantity: 1,
            shelf_count: 3,
            leg_section_mm: 50.0,
        }
    }

    #[test]
    fn valid_brief_passes() {
        base_brief().validate().unwrap();
    }

    #[test]
    fn rejects_empty_name() {
        let mut b = base_brief();
        b.name = "   ".into();
        assert!(b.validate().is_err());
    }

    #[test]
    fn rejects_nonpositive_dimension() {
        let mut b = base_brief();
        b.dimensions.height_mm = 0.0;
        assert!(b.validate().is_err());
        b = base_brief();
        b.dimensions.width_mm = -1.0;
        assert!(b.validate().is_err());
    }

    #[test]
    fn rejects_zero_quantity() {
        let mut b = base_brief();
        b.quantity = 0;
        assert!(b.validate().is_err());
    }

    #[test]
    fn rejects_overlapping_thickness() {
        // Carcass walls would overlap when 2*thickness >= smallest outer dim.
        let mut b = base_brief();
        b.product_type = ProductType::Box;
        b.dimensions.thickness_mm = 400.0; // width 600 -> 2*400 >= 600
        assert!(b.validate().is_err());
    }

    #[test]
    fn rejects_thickness_exceeding_outer_for_panel() {
        let mut b = base_brief();
        b.product_type = ProductType::Panel;
        b.dimensions = Dimensions {
            length_mm: 800.0,
            width_mm: 400.0,
            height_mm: 18.0,
            thickness_mm: 25.0, // > min outer (18)
        };
        assert!(b.validate().is_err());
    }

    #[test]
    fn parses_json_with_defaults() {
        let json = br#"{
            "name": "Simple Panel",
            "product_type": "panel",
            "dimensions": {"length_mm": 800, "width_mm": 400, "height_mm": 18, "thickness_mm": 18}
        }"#;
        let brief = ProductBrief::from_json_slice(json).unwrap();
        assert_eq!(brief.material, "18mm plywood");
        assert_eq!(brief.quantity, 1);
        assert_eq!(brief.shelf_count, 3);
        assert_eq!(brief.product_type, ProductType::Panel);
    }

    #[test]
    fn parse_error_surfaces_reason() {
        let err = ProductBrief::from_json_slice(b"{ not json").unwrap_err();
        assert!(matches!(err, AtelierError::BriefParse { .. }));
    }

    #[test]
    fn slug_is_filesystem_safe() {
        let mut b = base_brief();
        b.name = "  Standing Desk / v2!! ".into();
        assert_eq!(b.slug(), "standing-desk-v2");
    }

    #[test]
    fn slug_falls_back_when_empty() {
        let mut b = base_brief();
        b.name = "@@@".into();
        assert_eq!(b.slug(), "product");
    }

    #[test]
    fn product_type_labels() {
        assert_eq!(ProductType::Panel.label(), "panel");
        assert_eq!(ProductType::Box.label(), "box");
        assert_eq!(ProductType::Table.label(), "table");
        assert_eq!(ProductType::Shelf.label(), "shelf");
    }

    #[test]
    fn product_type_roundtrips_kebab_case() {
        for (pt, s) in [
            (ProductType::Panel, "\"panel\""),
            (ProductType::Box, "\"box\""),
            (ProductType::Table, "\"table\""),
            (ProductType::Shelf, "\"shelf\""),
        ] {
            assert_eq!(serde_json::to_string(&pt).unwrap(), s);
            let back: ProductType = serde_json::from_str(s).unwrap();
            assert_eq!(back, pt);
        }
    }
}
