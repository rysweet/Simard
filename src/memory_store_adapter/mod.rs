//! Adapter that implements [`MemoryStore`] by delegating to [`CognitiveMemoryAdapter`].
//!
//! This adapters the gap between the simple key-value `MemoryStore` trait (used
//! by `RuntimePorts`) and the six-type cognitive memory system backed by LadybugDB.
//! Each `MemoryRecord` is stored as a semantic fact in the cognitive graph, with
//! the record key as concept and scope+session encoded in tags.
//!
//! When the cognitive adapter is unavailable (honest degradation), the adapter
//! falls back to a `FileBackedMemoryStore` so the runtime always functions.

mod convert;
mod store;

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_store;

const STORE_NAME: &str = "cognitive-adapter-memory";

/// Maximum retries for adapter read operations.
const ADAPTER_READ_MAX_RETRIES: usize = 1;

/// Backoff between adapter retries in milliseconds.
const ADAPTER_RETRY_BACKOFF_MS: u64 = 200;

// Re-export all public items so `crate::memory_store_adapter::X` still works.
pub use store::CognitiveMemoryStoreAdapter;
