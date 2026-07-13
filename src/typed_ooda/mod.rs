//! Parser-free OODA capability boundary.
//!
//! Semantic agents choose what to do. This module only authenticates and
//! authorizes typed requests, applies deterministic rails, persists durable
//! outcomes, and executes admitted effects.

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
