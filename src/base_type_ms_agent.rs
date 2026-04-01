//! Microsoft Agent Framework base type — wraps the Microsoft Agent Framework
//! to delegate objectives through its agent orchestration pipeline.
//!
//! Status: Structural definition only. Uses [`PendingSdkAdapter`] until the
//! MS Agent Framework Rust integration is available.

use crate::base_type_pending_sdk::PendingSdkAdapter;
use crate::error::SimardResult;

/// A type alias for the MS Agent Framework adapter. When the real integration
/// becomes available, this will be replaced with a full implementation.
pub type MsAgentFrameworkAdapter = PendingSdkAdapter;

/// Construct a Microsoft Agent Framework adapter with the given ID.
pub fn ms_agent_framework_adapter(id: impl Into<String>) -> SimardResult<PendingSdkAdapter> {
    PendingSdkAdapter::registered(
        id,
        "ms-agent-framework::session-backend",
        "registered-base-type:ms-agent-framework",
        "Microsoft Agent Framework runtime is not yet implemented",
    )
}
