//! Adapter that implements [`MemoryStore`] by delegating to [`CognitiveMemoryBridge`].
//!
//! This bridges the gap between the simple key-value `MemoryStore` trait (used
//! by `RuntimePorts`) and the six-type cognitive memory system backed by Kuzu.
//! Each `MemoryRecord` is stored as a semantic fact in the cognitive graph, with
//! the record key as concept and scope+session encoded in tags.
//!
//! When the cognitive bridge is unavailable (honest degradation), the adapter
//! falls back to a `FileBackedMemoryStore` so the runtime always functions.

mod convert;
mod store;

#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod bridge_tests;

const STORE_NAME: &str = "cognitive-bridge-memory";

/// Maximum retries for bridge read operations.
const BRIDGE_READ_MAX_RETRIES: usize = 1;

/// Backoff between bridge retries in milliseconds.
const BRIDGE_RETRY_BACKOFF_MS: u64 = 200;

// Re-export all public items so `crate::memory_bridge_adapter::X` still works.
pub use store::CognitiveBridgeMemoryStore;
