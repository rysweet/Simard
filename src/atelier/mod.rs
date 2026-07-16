//! Atelier — industrial & furniture design pipeline.
//!
//! Turns a product brief into a fabrication-ready package: a parametric OpenSCAD
//! model, an STL mesh, a PNG render, a cut list, a bill of materials, and
//! (optionally) a STEP solid — all described by a verified `manifest.json`.
//!
//! This module is the domain engine behind the `simard-atelier` identity and
//! its goal-session recipes. OpenSCAD is the required engine; FreeCAD (STEP) and
//! Blender (photoreal render) are optional and degrade gracefully when absent.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use simard::atelier::{build_package, BuildOptions};
//!
//! let manifest = build_package(
//!     Path::new("brief.json"),
//!     Path::new("out"),
//!     BuildOptions::default(),
//! )?;
//! assert!(manifest.verification.ok);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod brief;
pub mod drivers;
pub mod error;
pub mod fabrication;
pub mod geometry;
pub mod manifest;
pub mod scad;

pub use brief::{Dimensions, Material, ProductBrief, ProductKind};
pub use error::{AtelierError, AtelierResult};
pub use geometry::{Assembly, Grain, Panel};
pub use manifest::{
    BuildOptions, Manifest, Verification, build_package, build_package_from_brief, inspect,
};
