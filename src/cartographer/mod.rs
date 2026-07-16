//! Cartographer — data storytelling & interactive dashboards.
//!
//! Turns a dataset + a question into a narrated, interactive dashboard: it
//! profiles the data (exploratory analysis), designs charts (visualization
//! design), renders an interactive Plotly dashboard plus a written narrative
//! (app delivery), and describes everything in a verified `manifest.json`.
//!
//! This module is the domain engine behind the `simard-cartographer` identity
//! and its goal-session recipes. The primary dashboard is dependency-free
//! (client-side Plotly.js); Streamlit / Observable / Node are optional
//! alternate deliveries that degrade gracefully when absent.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use simard::cartographer::{build_package, BuildOptions};
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
pub mod dashboard;
pub mod dataset;
pub mod drivers;
pub mod error;
pub mod manifest;
pub mod narrative;
pub mod serve;
pub mod viz;

pub use brief::{DashboardBrief, DatasetFormat};
pub use dataset::{Column, ColumnType, Dataset, DatasetProfile};
pub use error::{CartographerError, CartographerResult};
pub use manifest::{
    AnalysisDocument, BuildOptions, Manifest, Verification, build_package, build_package_ad_hoc,
    build_package_from_brief, inspect,
};
pub use serve::DashboardServer;
pub use viz::{ChartData, ChartSpec, design_charts};
