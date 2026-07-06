//! Tab content renderers.
//!
//! One module per tab in the #2627 consolidated seven-tab taxonomy. `overview`
//! stacks the Summary/Health/Stats panels (absorbing the former Status and
//! Stats tabs); `workers` is the former Engineers process view; `chat` is the
//! former Meeting REPL.

pub mod activity;
pub mod chat;
pub mod goals;
pub mod journal;
pub mod overseer;
pub mod overview;
pub mod workers;
