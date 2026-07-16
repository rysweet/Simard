//! Simard Atelier — the industrial & furniture design pipeline.
//!
//! The Atelier identity turns a declarative product brief into a parametric
//! model plus fabrication-ready exports (STL mesh, orthographic render, cut
//! list, and bill of materials). The pipeline is deterministic and needs no
//! external dependency, while opportunistically using the `openscad` toolchain
//! for higher-fidelity STL and PNG renders when it is installed.
//!
//! This module backs the `simard_atelier_build` binary and the
//! `simard-atelier` identity's goal-session recipes.

pub mod brief;
pub mod error;
pub mod export;
pub mod model;
pub mod pipeline;

pub use brief::{Dimensions, ProductBrief, ProductType};
pub use error::{AtelierError, AtelierResult};
pub use model::{Model, SolidBox};
pub use pipeline::{
    Artifact, AtelierManifest, PipelineOptions, PipelineOutcome, run_pipeline,
    run_pipeline_from_file,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "atelier-mod-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        d
    }

    #[test]
    fn end_to_end_brief_to_exports() {
        let json = br#"{
            "name": "Reading Nook Shelf",
            "product_type": "shelf",
            "dimensions": {"length_mm": 900, "width_mm": 300, "height_mm": 1800, "thickness_mm": 18},
            "material": "18mm birch plywood",
            "quantity": 1,
            "shelf_count": 4
        }"#;
        let brief = ProductBrief::from_json_slice(json).unwrap();
        let dir = temp_dir();
        let outcome = run_pipeline(
            &brief,
            &dir,
            &PipelineOptions {
                use_openscad: false,
            },
        )
        .unwrap();
        // A shelf with 4 interior shelves: 2 sides + top + bottom + 4 = 8 parts.
        assert_eq!(outcome.manifest.part_count, 8);
        assert!(dir.join("model.stl").exists());
        assert!(dir.join("render.svg").exists());
        assert!(dir.join("cutlist.csv").exists());
        assert!(dir.join("bom.csv").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
