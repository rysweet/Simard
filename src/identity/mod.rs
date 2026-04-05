mod compose;
mod contract;
mod loader;
mod manifest;
mod types;

// Re-export all public items so `crate::identity::X` still works.
pub use contract::ManifestContract;
pub use loader::{BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader};
pub use manifest::{IdentityManifest, compose_with_precedence};
pub use types::{MemoryPolicy, OperatingMode};
