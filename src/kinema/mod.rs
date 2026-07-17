//! Kinema — 2D/3D animation & motion-graphics pipeline.
//!
//! Turns a shot brief into a rendered animated sequence end-to-end: a
//! storyboard, a rig, a portable Synfig vector source, and a rendered PNG frame
//! sequence, all described by a verified `manifest.json`.
//!
//! This module is the domain engine behind the `simard-kinema` identity and its
//! goal-session recipes (storyboarding, rigging, rendering). A pure-Rust
//! rasterizer is the guaranteed engine, so a brief always renders to a real
//! frame sequence; Blender (Grease Pencil), Synfig, and Natron are optional
//! enhancements that degrade gracefully when their tool is absent.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use simard::kinema::{build_package, BuildOptions};
//!
//! let manifest = build_package(
//!     Path::new("shot.json"),
//!     Path::new("out"),
//!     BuildOptions::default(),
//! )?;
//! assert!(manifest.verification.ok);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod brief;
pub mod drivers;
pub mod error;
pub mod manifest;
pub mod png;
pub mod raster;
pub mod render;
pub mod rig;
pub mod storyboard;
pub mod timeline;

pub use brief::{
    AnimatedObject, AnimationStyle, Color, Keyframe, ObjectKind, Resolution, ShotBrief,
};
pub use error::{KinemaError, KinemaResult};
pub use manifest::{
    BuildOptions, Manifest, Verification, build_package, build_package_from_brief, inspect,
};
pub use rig::{Armature, Bone, Rig};
pub use storyboard::{Panel, Storyboard};
