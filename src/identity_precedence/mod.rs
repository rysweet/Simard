//! Precedence-based identity resolution engine.
//!
//! When composing multiple [`IdentityManifest`] values, conflicts in prompt
//! assets, capabilities, and base types must be resolved by precedence order.
//! Index 0 in the input `Vec` is the highest-precedence manifest and wins
//! conflicts.

mod conflict;
mod resolver;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::identity_precedence::X` still works.
pub use conflict::{ConflictEntry, ConflictLog};
pub use resolver::{PrecedenceResolver, ResolvedIdentity};
