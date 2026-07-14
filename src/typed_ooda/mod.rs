//! Parser-free OODA capability boundary.
//!
//! Semantic agents choose what to do. This module only authenticates and
//! authorizes typed requests, applies deterministic rails, persists durable
//! outcomes, and executes admitted effects.

use std::path::{Path, PathBuf};

mod actor;
mod executor;
mod ledger;
mod route;
mod types;

pub use actor::*;
pub use executor::*;
pub use ledger::*;
pub use route::*;
pub use types::*;

pub const LEDGER_RELATIVE_PATH: &str = "typed-ooda/outcomes.sqlite3";

pub fn ledger_path(state_root: &Path) -> PathBuf {
    state_root.join(LEDGER_RELATIVE_PATH)
}
