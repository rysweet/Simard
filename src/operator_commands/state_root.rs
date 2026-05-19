use std::path::PathBuf;

use crate::base_types::BaseTypeId;
use crate::{
    BuiltinIdentityLoader, Freshness, IdentityLoadRequest, IdentityLoader, ManifestContract,
    Provenance, RuntimeTopology, builtin_base_type_registry_for_manifest,
};

use super::validation::{
    validate_engineer_read_state_root, validate_improvement_curation_read_state_root,
    validate_meeting_read_state_root, validate_terminal_read_state_root,
};

pub(crate) fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

pub(crate) struct EngineerReadArtifacts {
    pub(crate) handoff_path: PathBuf,
    pub(crate) handoff_file_name: String,
    pub(crate) memory_path: PathBuf,
    pub(crate) evidence_path: PathBuf,
}

pub(crate) struct ValidatedRuntimeSegments {
    pub(crate) base_type: BaseTypeId,
    pub(crate) topology: RuntimeTopology,
}

pub(crate) fn validated_runtime_segments(
    identity: &str,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ValidatedRuntimeSegments> {
    let topology = parse_runtime_topology(topology)?;
    let contract = ManifestContract::new(
        concat!(module_path!(), "::validated_runtime_segments"),
        "operator-cli -> identity-loader -> base-type-registry",
        vec![
            format!("identity:{identity}"),
            format!("base-type:{base_type}"),
            format!("topology:{topology}"),
        ],
        Provenance::runtime(format!("operator-cli/default-state-root/{identity}")),
        Freshness::now()?,
    )?;
    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        identity,
        env!("CARGO_PKG_VERSION"),
        contract,
    ))?;
    let base_types = builtin_base_type_registry_for_manifest(&manifest)?;
    let requested_base_type = BaseTypeId::new(base_type);
    let factory = base_types.get(&requested_base_type).ok_or_else(|| {
        crate::SimardError::AdapterNotRegistered {
            base_type: base_type.to_string(),
        }
    })?;
    if !factory.descriptor().supports_topology(topology) {
        return Err(crate::SimardError::UnsupportedTopology {
            base_type: base_type.to_string(),
            topology,
        });
    }

    Ok(ValidatedRuntimeSegments {
        base_type: factory.descriptor().id.clone(),
        topology,
    })
}

fn state_root(
    identity: &str,
    base_type: &BaseTypeId,
    topology: RuntimeTopology,
    probe: &str,
) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/operator-probe-state")
        .join(probe)
        .join(identity)
        .join(base_type.as_str())
        .join(topology.to_string())
}

pub(crate) fn resolved_state_root(
    explicit: Option<PathBuf>,
    identity: &str,
    base_type: &str,
    topology: &str,
    probe: &str,
) -> crate::SimardResult<PathBuf> {
    match explicit {
        Some(path) => crate::bootstrap::validate_state_root(path),
        None => {
            let segments = validated_runtime_segments(identity, base_type, topology)?;
            crate::bootstrap::validate_state_root(state_root(
                identity,
                &segments.base_type,
                segments.topology,
                probe,
            ))
        }
    }
}

pub(crate) fn resolved_goal_curation_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    resolved_state_root(
        explicit,
        "simard-goal-curator",
        base_type,
        topology,
        "goal-curation-run",
    )
}

/// Result of resolving a read-companion's state root with the
/// canonical-daemon-store fallback (issue #1909).
///
/// `used_override` records whether the path came from the operator's
/// explicit `[state-root]` argument (strict-mode contract) or from the
/// daemon fallback (lenient-mode contract). Callers use this flag to
/// decide whether missing artifacts must hard-fail (override case) or
/// should render as `<none>` / `0` (fallback case).
pub(crate) struct ResolvedReadStateRoot {
    pub(crate) path: PathBuf,
    pub(crate) used_override: bool,
}

/// Shared resolver for `<mode> read` subcommands that need to fall back
/// to the canonical daemon store (`memory_ipc::default_state_root()`,
/// i.e. `$SIMARD_STATE_ROOT` or `$HOME/.simard/state`) when the operator
/// does not pass an explicit `[state-root]` argument.
///
/// Resolution order:
///   1. `explicit` — when the operator passes a path, honor it after
///      `validate_state_root` checks. Strict contract: subsequent
///      validations (mode-specific artifact requirements) still apply.
///   2. `memory_ipc::default_state_root()` — the canonical daemon store
///      that the OODA daemon, the meeting greeting banner, and
///      `goal-curation read` all share (issue #1742, #1909). Lenient
///      contract: callers should render `<none>` / `0` instead of
///      erroring on missing artifacts.
///
/// `identity`, `base_type`, and `topology` are validated even on the
/// fallback path so bogus values still fail fast with a clear error —
/// preserving the prior probe-resolution contract for the operator's
/// mental model.
pub(crate) fn resolve_read_state_root_with_daemon_fallback(
    explicit: Option<PathBuf>,
    identity: &str,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ResolvedReadStateRoot> {
    match explicit {
        Some(path) => Ok(ResolvedReadStateRoot {
            path: crate::bootstrap::validate_state_root(path)?,
            used_override: true,
        }),
        None => {
            // Validate base_type / topology so bogus values still fail
            // fast with a clear error (preserves the prior probe
            // resolution contract — see goal-curation #1742).
            let _ = validated_runtime_segments(identity, base_type, topology)?;
            let path =
                crate::bootstrap::validate_state_root(crate::memory_ipc::default_state_root())?;
            Ok(ResolvedReadStateRoot {
                path,
                used_override: false,
            })
        }
    }
}

pub(crate) fn resolved_review_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    resolved_state_root(
        explicit,
        "simard-engineer",
        base_type,
        topology,
        "review-run",
    )
}

pub(crate) fn resolved_improvement_curation_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ResolvedReadStateRoot> {
    let resolved = resolve_read_state_root_with_daemon_fallback(
        explicit,
        "simard-engineer",
        base_type,
        topology,
    )?;
    if resolved.used_override {
        validate_improvement_curation_read_state_root(&resolved.path)?;
    }
    Ok(resolved)
}

pub(crate) fn resolved_engineer_read_state_root(
    explicit: Option<PathBuf>,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_state_root(
        explicit,
        "simard-engineer",
        "terminal-shell",
        topology,
        "engineer-loop-run",
    )?;
    validate_engineer_read_state_root(&state_root)?;
    Ok(state_root)
}

pub(crate) fn resolved_terminal_read_state_root(
    explicit: Option<PathBuf>,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_state_root(
        explicit,
        "simard-engineer",
        "terminal-shell",
        topology,
        "terminal-run",
    )?;
    validate_terminal_read_state_root(&state_root)?;
    Ok(state_root)
}

pub(crate) fn resolved_meeting_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ResolvedReadStateRoot> {
    let resolved = resolve_read_state_root_with_daemon_fallback(
        explicit,
        "simard-meeting",
        base_type,
        topology,
    )?;
    if resolved.used_override {
        validate_meeting_read_state_root(&resolved.path)?;
    }
    Ok(resolved)
}

/// Resolve the state root for `review read` with the daemon-store
/// fallback (issue #1909). Distinct from `resolved_review_state_root`,
/// which is reused by both `review run` and `improvement-curation run`
/// to point at the probe-isolated sandbox.
///
/// `review read` itself does not require a mode-specific structural
/// validation beyond the path checks `validate_state_root` already
/// performs; the downstream code asserts (or — on the fallback path —
/// gracefully reports) the presence of the review artifact directory.
pub(crate) fn resolved_review_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ResolvedReadStateRoot> {
    resolve_read_state_root_with_daemon_fallback(explicit, "simard-engineer", base_type, topology)
}

pub(crate) fn parse_runtime_topology(value: &str) -> crate::SimardResult<RuntimeTopology> {
    match value {
        "single-process" => Ok(RuntimeTopology::SingleProcess),
        "multi-process" => Ok(RuntimeTopology::MultiProcess),
        "distributed" => Ok(RuntimeTopology::Distributed),
        other => Err(crate::SimardError::InvalidConfigValue {
            key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
            value: other.to_string(),
            help: "expected 'single-process', 'multi-process', or 'distributed'".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_root_ends_with_prompt_assets() {
        let root = prompt_root();
        assert!(
            root.ends_with("prompt_assets"),
            "prompt_root should end with 'prompt_assets', got: {}",
            root.display()
        );
    }

    #[test]
    fn state_root_builds_expected_path() {
        let base_type = BaseTypeId::new("my-type");
        let path = state_root(
            "my-id",
            &base_type,
            RuntimeTopology::SingleProcess,
            "probe-x",
        );
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("target/operator-probe-state"));
        assert!(path_str.contains("probe-x"));
        assert!(path_str.contains("my-id"));
        assert!(path_str.contains("my-type"));
        assert!(path_str.contains("single-process"));
    }

    #[test]
    fn state_root_multi_process_topology() {
        let base_type = BaseTypeId::new("local-harness");
        let path = state_root(
            "simard-engineer",
            &base_type,
            RuntimeTopology::MultiProcess,
            "handoff-roundtrip",
        );
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("multi-process"));
        assert!(path_str.contains("handoff-roundtrip"));
    }

    #[test]
    fn state_root_distributed_topology() {
        let base_type = BaseTypeId::new("rusty-clawd");
        let path = state_root(
            "simard-meeting",
            &base_type,
            RuntimeTopology::Distributed,
            "meeting-run",
        );
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("distributed"));
        assert!(path_str.contains("meeting-run"));
        assert!(path_str.contains("rusty-clawd"));
    }

    #[test]
    fn parse_runtime_topology_single_process() {
        assert_eq!(
            parse_runtime_topology("single-process").unwrap(),
            RuntimeTopology::SingleProcess
        );
    }

    #[test]
    fn parse_runtime_topology_multi_process() {
        assert_eq!(
            parse_runtime_topology("multi-process").unwrap(),
            RuntimeTopology::MultiProcess
        );
    }

    #[test]
    fn parse_runtime_topology_distributed() {
        assert_eq!(
            parse_runtime_topology("distributed").unwrap(),
            RuntimeTopology::Distributed
        );
    }

    #[test]
    fn parse_runtime_topology_invalid_value() {
        let err = parse_runtime_topology("bogus").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("SIMARD_RUNTIME_TOPOLOGY"),
            "error should reference the config key"
        );
        assert!(
            msg.contains("single-process"),
            "error should list valid values"
        );
    }

    #[test]
    fn parse_runtime_topology_empty_string() {
        assert!(parse_runtime_topology("").is_err());
    }

    #[test]
    fn parse_runtime_topology_case_sensitive() {
        assert!(parse_runtime_topology("Single-Process").is_err());
        assert!(parse_runtime_topology("DISTRIBUTED").is_err());
    }
}
