//! Gastronome — culinary, menu & event-design pipeline.
//!
//! Turns an event/menu brief into a costed, scheduled menu plan: scaled recipes,
//! a consolidated shopping list, per-guest & total nutrition, a back-timed prep
//! schedule, and a Markdown menu card — all described by a verified
//! `manifest.json`. An optional self-contained `prep_app.html` kitchen app can
//! be emitted to run the prep on the line.
//!
//! This module is the domain engine behind the `simard-gastronome` identity and
//! its goal-session recipes. It has no external tool dependency: every stage is
//! deterministic and pure-Rust, so the happy path always runs.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use simard::gastronome::{build_package, BuildOptions};
//!
//! let manifest = build_package(
//!     Path::new("brief.json"),
//!     Path::new("out"),
//!     BuildOptions { prep_app: true },
//! )?;
//! assert!(manifest.verification.ok);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod analysis;
pub mod app;
pub mod brief;
pub mod card;
pub mod error;
pub mod manifest;
pub mod menu;
pub mod schedule;

pub use brief::{Course, Dish, Ingredient, MenuBrief, Nutrition, PrepStep};
pub use error::{GastronomeError, GastronomeResult};
pub use manifest::{
    BuildOptions, Manifest, Verification, build_package, build_package_from_brief, inspect,
};
pub use menu::{Menu, ScaledDish, scale};
pub use schedule::PrepSchedule;
