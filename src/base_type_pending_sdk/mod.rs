//! Shared adapter for agent SDK base types whose runtime bindings are not yet
//! available. Each pending SDK registers properly in the base type catalog and
//! returns an explicit error when a turn is attempted, so the system fails
//! closed rather than silently ignoring the delegation.

mod adapter;
pub(crate) mod session;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::base_type_pending_sdk::X` still works.
pub use adapter::PendingSdkAdapter;
