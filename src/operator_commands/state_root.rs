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

/// Issue rysweet/Simard#1909: enforce explicit `<state-root>` for the
/// three read subcommands (`meeting read`, `improvement-curation read`,
/// and `review read`). Returns `SimardError::MissingRequiredConfig`
/// with the unified wording when `explicit` is `None`; otherwise
/// delegates to `bootstrap::validate_state_root`.
///
/// Contract:
///   * No implicit fallback to a synthesized probe default path.
///   * The `SIMARD_STATE_ROOT` environment variable is **not** honored
///     for these read paths (the error message says so explicitly).
///
/// See: docs/reference/operator-read-state-root-contract.md
pub(crate) fn require_explicit_state_root_for_read(
    explicit: Option<PathBuf>,
    subcommand: &str,
    base_type: &str,
) -> crate::SimardResult<PathBuf> {
    match explicit {
        Some(path) => crate::bootstrap::validate_state_root(path),
        None => Err(crate::SimardError::MissingRequiredConfig {
            key: "state-root".to_string(),
            help: format!(
                "state-root is required for `simard {subcommand} read {base_type}`: \
                 pass the positional <state-root> argument explicitly. The \
                 SIMARD_STATE_ROOT environment variable is not honored for this command."
            ),
        }),
    }
}

pub(crate) fn resolved_review_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
) -> crate::SimardResult<PathBuf> {
    require_explicit_state_root_for_read(explicit, "review", base_type)
}

pub(crate) fn resolved_improvement_curation_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    _topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root =
        require_explicit_state_root_for_read(explicit, "improvement-curation", base_type)?;
    validate_improvement_curation_read_state_root(&state_root)?;
    Ok(state_root)
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
    _topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = require_explicit_state_root_for_read(explicit, "meeting", base_type)?;
    validate_meeting_read_state_root(&state_root)?;
    Ok(state_root)
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

    // ---------------------------------------------------------------
    // Issue rysweet/Simard#1909 — resolver-layer hard-fail guard.
    //
    // These are TDD-failing tests: the function under test
    // (`require_explicit_state_root_for_read`) does not exist yet on
    // `main`. They pin the contract that:
    //   * `None` → `SimardError::MissingRequiredConfig` with the
    //     unified wording (subcommand + base-type named, env-var
    //     fallback explicitly disclaimed).
    //   * `Some(path)` → delegates to `bootstrap::validate_state_root`
    //     and yields the validated path back (or a non-#1909 error).
    // See: docs/reference/operator-read-state-root-contract.md
    // ---------------------------------------------------------------

    #[test]
    fn require_explicit_state_root_for_read_rejects_none_with_unified_wording() {
        let err = require_explicit_state_root_for_read(None, "meeting", "local-harness")
            .expect_err("missing explicit state-root must hard-fail");
        match err {
            crate::SimardError::MissingRequiredConfig { key, help } => {
                assert_eq!(key, "state-root", "error key must be `state-root`");
                assert!(
                    help.contains("state-root is required"),
                    "help should open with `state-root is required`, got: {help}"
                );
                assert!(
                    help.contains("simard meeting read local-harness"),
                    "help should name the failing subcommand+base-type, got: {help}"
                );
                assert!(
                    help.contains("pass the positional <state-root> argument explicitly"),
                    "help should describe the corrective action, got: {help}"
                );
                assert!(
                    help.contains("SIMARD_STATE_ROOT environment variable is not honored"),
                    "help must explicitly disclaim env-var fallback, got: {help}"
                );
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn require_explicit_state_root_for_read_rejects_none_for_each_subcommand_label() {
        for (subcommand, base) in [
            ("meeting", "local-harness"),
            ("improvement-curation", "local-harness"),
            ("review", "local-harness"),
        ] {
            let result = require_explicit_state_root_for_read(None, subcommand, base);
            assert!(
                matches!(
                    result,
                    Err(crate::SimardError::MissingRequiredConfig { ref key, .. })
                        if key == "state-root"
                ),
                "expected `MissingRequiredConfig {{ key: \"state-root\" }}` for \
                 {subcommand} read {base}, got {result:?}"
            );
        }
    }

    #[test]
    fn require_explicit_state_root_for_read_accepts_some_existing_dir() {
        let dir =
            std::env::temp_dir().join(format!("simard-issue-1909-explicit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir should be creatable");
        let result =
            require_explicit_state_root_for_read(Some(dir.clone()), "meeting", "local-harness");
        let _ = std::fs::remove_dir_all(&dir);
        let resolved = result.expect("explicit existing dir must pass the guard");
        // bootstrap::validate_state_root canonicalizes, so just compare
        // by file_name to keep the assertion platform-agnostic.
        assert_eq!(
            resolved.file_name(),
            dir.file_name(),
            "explicit path should round-trip through the guard"
        );
    }

    #[test]
    fn require_explicit_state_root_for_read_some_does_not_emit_missing_config() {
        let dir = std::env::temp_dir().join(format!(
            "simard-issue-1909-no-missing-config-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("tempdir should be creatable");
        let result = require_explicit_state_root_for_read(
            Some(dir.clone()),
            "improvement-curation",
            "local-harness",
        );
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(_) => {}
            Err(crate::SimardError::MissingRequiredConfig { .. }) => {
                panic!("explicit state-root must never produce MissingRequiredConfig");
            }
            Err(other) => panic!("unexpected non-#1909 error path: {other:?}"),
        }
    }
}
