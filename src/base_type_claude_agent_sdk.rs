//! Claude Agent SDK base type — wraps Anthropic's Claude Agent SDK to delegate
//! objectives through its agent orchestration pipeline.
//!
//! Status: Structural definition only. The Claude Agent SDK Rust bindings are
//! not yet published. Uses [`PendingSdkAdapter`] until the real SDK is available.

use crate::base_type_pending_sdk::PendingSdkAdapter;
use crate::error::SimardResult;

/// A type alias for the Claude Agent SDK adapter. When the real SDK Rust
/// bindings become available, this will be replaced with a full implementation.
pub type ClaudeAgentSdkAdapter = PendingSdkAdapter;

/// Construct a Claude Agent SDK adapter with the given ID.
pub fn claude_agent_sdk_adapter(id: impl Into<String>) -> SimardResult<PendingSdkAdapter> {
    PendingSdkAdapter::registered(
        id,
        "claude-agent-sdk::session-backend",
        "registered-base-type:claude-agent-sdk",
        "Claude Agent SDK runtime is not yet implemented",
    )
}
