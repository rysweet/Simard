//! Simard **Atelier** — the industrial & furniture design identity's fabrication
//! engine.
//!
//! The Atelier identity takes a declarative [`ProductBrief`] and drives a
//! parametric CAD toolchain to produce shop-ready outputs: a parametric OpenSCAD
//! model, an STL and STEP export, a rendered preview, a cut list, and a bill of
//! materials. The engine is a pure function of the brief for its deterministic
//! artifacts, and gracefully degrades when the external CAD binaries
//! (OpenSCAD / FreeCAD) are not installed.
//!
//! ```no_run
//! use simard::atelier::{ProductBrief, fabricate};
//! use std::path::Path;
//!
//! let brief = ProductBrief::from_json(br#"{
//!     "name": "Oak Writing Desk",
//!     "kind": "table",
//!     "width_mm": 1200, "depth_mm": 600, "height_mm": 740,
//!     "panel_thickness_mm": 18, "material": "oak"
//! }"#).unwrap();
//! let out = fabricate(&brief, Path::new("target/atelier/desk")).unwrap();
//! println!("{}", out.summary);
//! ```

pub mod brief;
pub mod error;
pub mod fabrication;
pub mod model;
pub mod pipeline;

pub use brief::{ProductBrief, ProductKind};
pub use error::{AtelierError, AtelierResult};
pub use fabrication::{BillOfMaterials, BomLine, CutList, Part, bill_of_materials, cut_list};
pub use model::{generate_openscad, geometry_summary};
pub use pipeline::{
    Artifact, ArtifactStatus, FabricationOutput, SystemTools, ToolRunner, fabricate, run_pipeline,
};

/// A ready-to-run example brief for the `simard atelier demo` command and docs.
pub const DEMO_BRIEF_JSON: &str = r#"{
  "name": "Atelier Demo Bookcase",
  "kind": "shelf",
  "width_mm": 800,
  "depth_mm": 300,
  "height_mm": 1800,
  "panel_thickness_mm": 18,
  "material": "birch-plywood",
  "shelves": 4,
  "quantity": 1,
  "finish": "oil"
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_brief_is_valid() {
        let brief = ProductBrief::from_json(DEMO_BRIEF_JSON.as_bytes()).unwrap();
        assert_eq!(brief.kind, ProductKind::Shelf);
        assert_eq!(brief.shelves, 4);
    }

    #[test]
    fn public_api_is_reachable() {
        let brief = ProductBrief::from_json(DEMO_BRIEF_JSON.as_bytes()).unwrap();
        let _ = cut_list(&brief);
        let _ = bill_of_materials(&brief);
        let scad = generate_openscad(&brief);
        assert!(scad.contains("Simard Atelier"));
    }
}
