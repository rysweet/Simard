mod assembly;
mod config;
mod types;
mod validation;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_config;

// Shared constants accessible to child modules.
const DEFAULT_IDENTITY: &str = "simard-engineer";
const DEFAULT_OBJECTIVE: &str = "bootstrap the Simard engineer loop";
const DEFAULT_STATE_ROOT: &str = "target/simard-state";
const LOCAL_BASE_TYPE: &str = "local-harness";

// Re-export all public items so `crate::bootstrap::X` still works.
pub use assembly::{
    LocalSessionExecution, assemble_local_runtime, assemble_local_runtime_from_handoff,
    builtin_base_type_registry_for_manifest, latest_local_handoff, run_local_session,
};
pub use config::BootstrapConfig;
pub use types::{BootstrapInputs, BootstrapMode, ConfigValue, ConfigValueSource};
pub(crate) use validation::validate_state_root;

pub fn bootstrap_entrypoint() -> &'static str {
    concat!(module_path!(), "::assemble_local_runtime")
}

#[cfg(test)]
mod tests {
    #[test]
    fn bootstrap_entrypoint_contains_module_path() {
        let entry = super::bootstrap_entrypoint();
        assert!(entry.contains("bootstrap"));
        assert!(entry.contains("assemble_local_runtime"));
    }
}
