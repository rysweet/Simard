//! Cartographer — data storytelling & interactive dashboards.
//!
//! Turns a dataset + an analytical question into a **served interactive
//! dashboard** with a **written narrative**, end to end: profile the data,
//! surface quantitative findings, design charts, write the story, and render a
//! self-contained `dashboard.html` (Plotly + D3) plus optional Streamlit /
//! Observable delivery sources — all described by a verified `manifest.json`.
//!
//! This module is the domain engine behind the `simard-cartographer` identity
//! and its goal-session recipes. The HTML dashboard is generated purely in Rust
//! and served by a built-in static server, so the core path has no external
//! dependencies; Streamlit and Observable are optional targets whose runtime
//! availability is recorded but never required.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use simard::cartographer::{build_package, BuildOptions};
//!
//! let manifest = build_package(
//!     Path::new("study.json"),
//!     Path::new("out"),
//!     BuildOptions::default(),
//! )?;
//! assert!(manifest.verification.ok);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod analysis;
pub mod brief;
pub mod dashboard;
pub mod dataset;
pub mod drivers;
pub mod error;
pub mod manifest;
pub mod narrative;
pub mod serve;
pub mod viz;

pub use analysis::{Finding, FindingKind, Findings};
pub use brief::{AppTarget, DatasetSource, Hints, StudyBrief};
pub use dataset::{Column, ColumnKind, Dataset};
pub use error::{CartographerError, CartographerResult};
pub use manifest::{
    BuildOptions, Manifest, Verification, build_package, build_package_from_brief, inspect,
};
pub use serve::{ServeReport, serve};
pub use viz::{ChartSpec, ChartType};
