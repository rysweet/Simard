mod in_memory;
pub mod native;
mod subprocess;

// Re-export all public items so `crate::bridge_subprocess::X` still works.
pub use in_memory::InMemoryBridgeTransport;
pub use native::NativeBridgeTransport;
pub use subprocess::SubprocessBridgeTransport;
