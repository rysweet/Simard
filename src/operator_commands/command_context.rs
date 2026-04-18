//! Unified context for operator command dispatch.
//!
//! Inspired by the `CommandContext` accessor pattern in steveyegge/beads
//! (which consolidates 20+ scattered globals into a single struct with
//! typed accessors), this module provides a `CommandContext` that captures
//! the common parameters threaded through every operator-command probe
//! function.
//!
//! Before this change each probe entry-point accepted 3-5 positional string
//! parameters (`topology`, `base_type`, `objective`, `state_root_override`,
//! …).  The new struct centralises those values so that:
//!
//! * Adding a cross-cutting parameter (e.g. verbose mode, dry-run flag)
//!   requires touching only one struct definition instead of every probe
//!   signature.
//! * The dispatch layer constructs a single `CommandContext` and passes it
//!   through, reducing parse-site boilerplate.
//! * Test helpers can build a context with `CommandContext::builder()` and
//!   override only the fields relevant to the test.

use std::path::{Path, PathBuf};

use crate::BootstrapInputs;
use crate::operator_commands::state_root::prompt_root;

/// Unified context passed to every operator command.
///
/// The struct is intentionally flat — it mirrors the positional parameters
/// that the probe functions already accept.  Fields that are only relevant
/// to a subset of commands are wrapped in `Option`.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// Network topology descriptor (e.g. `"single-process"`).
    pub topology: String,

    /// Explicit state-root directory.  When `None` the callee resolves a
    /// default from the identity / topology combination.
    pub state_root_override: Option<PathBuf>,

    /// Session base type (e.g. `"local-harness"`).  Required for meeting,
    /// goal-curation, improvement-curation, review and bootstrap probes.
    pub base_type: Option<String>,

    /// Operator-supplied objective text.  Required for "run" probes; absent
    /// for "read" probes.
    pub objective: Option<String>,

    /// Override for the agent identity string.  When `None` the callee
    /// picks its own default (e.g. `"simard-engineer"`).
    pub identity: Option<String>,

    /// Workspace root directory — only used by the engineer-loop probe.
    pub workspace_root: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Incremental builder for [`CommandContext`].
#[derive(Debug, Default)]
pub struct CommandContextBuilder {
    topology: Option<String>,
    state_root_override: Option<PathBuf>,
    base_type: Option<String>,
    objective: Option<String>,
    identity: Option<String>,
    workspace_root: Option<PathBuf>,
}

impl CommandContextBuilder {
    pub fn topology(mut self, value: impl Into<String>) -> Self {
        self.topology = Some(value.into());
        self
    }

    pub fn state_root_override(mut self, value: Option<PathBuf>) -> Self {
        self.state_root_override = value;
        self
    }

    pub fn base_type(mut self, value: impl Into<String>) -> Self {
        self.base_type = Some(value.into());
        self
    }

    pub fn objective(mut self, value: impl Into<String>) -> Self {
        self.objective = Some(value.into());
        self
    }

    pub fn identity(mut self, value: impl Into<String>) -> Self {
        self.identity = Some(value.into());
        self
    }

    pub fn workspace_root(mut self, value: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(value.into());
        self
    }

    /// Consume the builder and produce a [`CommandContext`].
    ///
    /// Returns `Err` when `topology` — the only universally-required field —
    /// has not been set.
    pub fn build(self) -> Result<CommandContext, &'static str> {
        Ok(CommandContext {
            topology: self.topology.ok_or("topology is required")?,
            state_root_override: self.state_root_override,
            base_type: self.base_type,
            objective: self.objective,
            identity: self.identity,
            workspace_root: self.workspace_root,
        })
    }
}

// ---------------------------------------------------------------------------
// Convenience helpers on CommandContext
// ---------------------------------------------------------------------------

impl CommandContext {
    /// Start building a new context.
    pub fn builder() -> CommandContextBuilder {
        CommandContextBuilder::default()
    }

    /// Whether a state-root was explicitly supplied by the operator.
    pub fn state_root_was_explicit(&self) -> bool {
        self.state_root_override.is_some()
    }

    /// Resolve the `base_type` or return an error describing the missing
    /// value.
    pub fn require_base_type(&self) -> Result<&str, Box<dyn std::error::Error>> {
        self.base_type
            .as_deref()
            .ok_or_else(|| "base_type is required for this command".into())
    }

    /// Resolve the `objective` or return an error.
    pub fn require_objective(&self) -> Result<&str, Box<dyn std::error::Error>> {
        self.objective
            .as_deref()
            .ok_or_else(|| "objective is required for this command".into())
    }

    /// Resolve the `workspace_root` or return an error.
    pub fn require_workspace_root(&self) -> Result<&Path, Box<dyn std::error::Error>> {
        self.workspace_root
            .as_deref()
            .ok_or_else(|| "workspace_root is required for this command".into())
    }

    /// Build a [`BootstrapInputs`] value from this context.
    ///
    /// The caller supplies the `identity` default and a pre-resolved
    /// `state_root` (since state-root resolution is command-specific).
    pub fn to_bootstrap_inputs(
        &self,
        resolved_identity: &str,
        resolved_state_root: PathBuf,
    ) -> BootstrapInputs {
        BootstrapInputs {
            prompt_root: Some(prompt_root()),
            objective: self.objective.clone(),
            state_root: Some(resolved_state_root),
            identity: Some(
                self.identity
                    .clone()
                    .unwrap_or_else(|| resolved_identity.to_string()),
            ),
            base_type: self.base_type.clone(),
            topology: Some(self.topology.clone()),
            ..BootstrapInputs::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_requires_topology() {
        let result = CommandContext::builder().build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_produces_context_with_topology_only() {
        let ctx = CommandContext::builder()
            .topology("single-process")
            .build()
            .unwrap();
        assert_eq!(ctx.topology, "single-process");
        assert!(ctx.base_type.is_none());
        assert!(ctx.objective.is_none());
        assert!(ctx.state_root_override.is_none());
        assert!(!ctx.state_root_was_explicit());
    }

    #[test]
    fn builder_captures_all_fields() {
        let ctx = CommandContext::builder()
            .topology("multi-node")
            .base_type("local-harness")
            .objective("run tests")
            .identity("simard-meeting")
            .state_root_override(Some(PathBuf::from("/state")))
            .workspace_root("/workspace")
            .build()
            .unwrap();

        assert_eq!(ctx.topology, "multi-node");
        assert_eq!(ctx.require_base_type().unwrap(), "local-harness");
        assert_eq!(ctx.require_objective().unwrap(), "run tests");
        assert_eq!(ctx.identity.as_deref(), Some("simard-meeting"));
        assert!(ctx.state_root_was_explicit());
        assert_eq!(
            ctx.require_workspace_root().unwrap(),
            Path::new("/workspace")
        );
    }

    #[test]
    fn require_base_type_errors_when_absent() {
        let ctx = CommandContext::builder().topology("t").build().unwrap();
        assert!(ctx.require_base_type().is_err());
    }

    #[test]
    fn require_objective_errors_when_absent() {
        let ctx = CommandContext::builder().topology("t").build().unwrap();
        assert!(ctx.require_objective().is_err());
    }

    #[test]
    fn require_workspace_root_errors_when_absent() {
        let ctx = CommandContext::builder().topology("t").build().unwrap();
        assert!(ctx.require_workspace_root().is_err());
    }

    #[test]
    fn to_bootstrap_inputs_uses_identity_override() {
        let ctx = CommandContext::builder()
            .topology("t")
            .identity("custom-id")
            .objective("do stuff")
            .build()
            .unwrap();
        let inputs = ctx.to_bootstrap_inputs("default-id", PathBuf::from("/sr"));
        assert_eq!(inputs.identity.as_deref(), Some("custom-id"));
    }

    #[test]
    fn to_bootstrap_inputs_uses_default_identity() {
        let ctx = CommandContext::builder()
            .topology("t")
            .objective("do stuff")
            .build()
            .unwrap();
        let inputs = ctx.to_bootstrap_inputs("default-id", PathBuf::from("/sr"));
        assert_eq!(inputs.identity.as_deref(), Some("default-id"));
    }
}
