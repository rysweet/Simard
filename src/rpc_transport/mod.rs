mod in_memory;
pub mod native;

// Re-export all public items so `crate::rpc_transport::X` still works.
pub use in_memory::InMemoryRpcTransport;
pub use native::NativeRpcTransport;
