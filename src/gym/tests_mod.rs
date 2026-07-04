use super::*;
use crate::error::SimardError;
#[test]
fn default_output_root_returns_expected_path() {
    let path = default_output_root();
    assert_eq!(path, PathBuf::from("target/simard-gym"));
}

#[test]
fn default_output_root_is_relative() {
    let path = default_output_root();
    assert!(path.is_relative());
}

#[test]
fn starter_suite_id_constant() {
    assert_eq!(STARTER_SUITE_ID, "starter");
}

#[test]
fn run_benchmark_suite_rejects_unknown_suite_id() {
    let result = run_benchmark_suite("nonexistent-suite", default_output_root());
    assert!(result.is_err());
    match result.unwrap_err() {
        SimardError::BenchmarkSuiteNotFound { suite_id } => {
            assert_eq!(suite_id, "nonexistent-suite");
        }
        other => panic!("expected BenchmarkSuiteNotFound, got: {other:?}"),
    }
}

#[test]
fn run_benchmark_scenario_rejects_unknown_scenario_id() {
    let result = run_benchmark_scenario("nonexistent-scenario", default_output_root());
    assert!(result.is_err());
}

#[test]
fn runtime_ports_single_process() {
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let base_types = BaseTypeRegistry::default();
    let result = runtime_ports_for_topology(
        prompt_store,
        memory_store,
        evidence_store,
        base_types,
        RuntimeTopology::SingleProcess,
    );
    assert!(result.is_ok());
}

#[test]
fn runtime_ports_multi_process() {
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let base_types = BaseTypeRegistry::default();
    let result = runtime_ports_for_topology(
        prompt_store,
        memory_store,
        evidence_store,
        base_types,
        RuntimeTopology::MultiProcess,
    );
    assert!(result.is_ok());
}

#[test]
fn runtime_ports_distributed() {
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let base_types = BaseTypeRegistry::default();
    let result = runtime_ports_for_topology(
        prompt_store,
        memory_store,
        evidence_store,
        base_types,
        RuntimeTopology::Distributed,
    );
    assert!(result.is_ok());
}

// --- DEFAULT_OUTPUT_ROOT constant ---

#[test]
fn default_output_root_constant_matches() {
    assert_eq!(DEFAULT_OUTPUT_ROOT, "target/simard-gym");
}

// --- run_benchmark_suite edge cases ---

#[test]
fn run_benchmark_suite_empty_string_rejected() {
    let result = run_benchmark_suite("", default_output_root());
    assert!(result.is_err());
}

#[test]
fn run_benchmark_suite_whitespace_rejected() {
    let result = run_benchmark_suite("  ", default_output_root());
    assert!(result.is_err());
}

#[test]
fn run_benchmark_suite_wrong_case_rejected() {
    let result = run_benchmark_suite("Starter", default_output_root());
    assert!(result.is_err());
}

#[test]
fn run_benchmark_suite_error_has_suite_id() {
    let result = run_benchmark_suite("bogus-id-xyz", default_output_root());
    match result.unwrap_err() {
        SimardError::BenchmarkSuiteNotFound { suite_id } => {
            assert_eq!(suite_id, "bogus-id-xyz");
        }
        other => panic!("expected BenchmarkSuiteNotFound, got: {other:?}"),
    }
}

// --- run_benchmark_scenario edge cases ---

#[test]
fn run_benchmark_scenario_empty_string_rejected() {
    let result = run_benchmark_scenario("", default_output_root());
    assert!(result.is_err());
}

// --- compare_latest_benchmark_runs error cases ---

#[test]
fn compare_latest_rejects_unknown_scenario() {
    let result = compare_latest_benchmark_runs("nonexistent-xyz", default_output_root());
    assert!(result.is_err());
}

#[test]
fn compare_latest_rejects_empty_scenario_id() {
    let result = compare_latest_benchmark_runs("", default_output_root());
    assert!(result.is_err());
}

// --- runtime_ports_for_topology: verify all enum variants ---

#[test]
fn runtime_ports_covers_all_topologies() {
    let topologies = [
        RuntimeTopology::SingleProcess,
        RuntimeTopology::MultiProcess,
        RuntimeTopology::Distributed,
    ];
    for topology in topologies {
        let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
        let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
        let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
        let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
        let base_types = BaseTypeRegistry::default();
        let result = runtime_ports_for_topology(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            topology,
        );
        assert!(result.is_ok(), "should succeed for {topology:?}");
    }
}

// --- benchmark_scenarios from scenarios module ---

#[test]
fn benchmark_scenarios_returns_nonempty() {
    assert!(!benchmark_scenarios().is_empty());
}

#[test]
fn benchmark_scenarios_all_have_positive_min_evidence() {
    for s in benchmark_scenarios() {
        assert!(
            s.expected_min_runtime_evidence > 0,
            "{} should require at least 1 evidence record",
            s.id
        );
    }
}

// --- default_output_root ---

#[test]
fn default_output_root_has_two_components() {
    let root = default_output_root();
    let components: Vec<_> = root.components().collect();
    assert_eq!(components.len(), 2);
}

// --- is_skippable_auth_error (issue #1743) ---

#[test]
fn skippable_auth_error_matches_adapter_invocation_with_authentication() {
    let err = SimardError::AdapterInvocationFailed {
        base_type: "rusty-clawd".to_string(),
        reason: "Copilot backend requires authentication. Use Client::new_copilot().".to_string(),
    };
    assert!(is_skippable_auth_error(&err, "rusty-clawd"));
}

#[test]
fn skippable_auth_error_does_not_match_local_harness() {
    let err = SimardError::AdapterInvocationFailed {
        base_type: "local-harness".to_string(),
        reason: "requires authentication".to_string(),
    };
    assert!(!is_skippable_auth_error(&err, "local-harness"));
}

#[test]
fn skippable_auth_error_does_not_match_unrelated_invocation_failure() {
    let err = SimardError::AdapterInvocationFailed {
        base_type: "rusty-clawd".to_string(),
        reason: "timeout after 30s".to_string(),
    };
    assert!(!is_skippable_auth_error(&err, "rusty-clawd"));
}

#[test]
fn skippable_auth_error_matches_adapter_not_registered() {
    let err = SimardError::AdapterNotRegistered {
        base_type: "rusty-clawd".to_string(),
    };
    assert!(is_skippable_auth_error(&err, "rusty-clawd"));
}

#[test]
fn skippable_auth_error_does_not_match_other_error_variants() {
    let err = SimardError::BenchmarkSuiteNotFound {
        suite_id: "nonexistent".to_string(),
    };
    assert!(!is_skippable_auth_error(&err, "rusty-clawd"));
}

// --- is_skippable_pty_unavailable / gate_prerequisite_skip (issue #2548, no-PTY hosts) ---

#[test]
fn pty_unavailable_skip_matches_terminal_shell_launch_failure() {
    // The exact launch-failure signature emitted by PtyTerminalSession::launch
    // when the `script` launcher cannot be spawned on a no-PTY host.
    let err = SimardError::AdapterInvocationFailed {
        base_type: "terminal-shell".to_string(),
        reason:
            "failed to launch local PTY shell via 'script': No such file or directory (os error 2)"
                .to_string(),
    };
    assert!(is_skippable_pty_unavailable(&err, "terminal-shell"));
    // ...and it is surfaced as a prerequisite skip (green gate, not false-RED).
    assert!(gate_prerequisite_skip(&err, "terminal-shell").is_some());
}

#[test]
fn pty_unavailable_skip_does_not_hide_a_real_terminal_shell_failure() {
    // A terminal-shell scenario that LAUNCHES but then produces the wrong
    // output / a non-zero exit is a genuine defect. Skipping it would
    // reintroduce the very false-green issue #2548 fixes, so it must NOT be
    // treated as an unavailable prerequisite.
    let err = SimardError::AdapterInvocationFailed {
        base_type: "terminal-shell".to_string(),
        reason: "terminal-shell session exited with status exit status: 1".to_string(),
    };
    assert!(!is_skippable_pty_unavailable(&err, "terminal-shell"));
    assert!(gate_prerequisite_skip(&err, "terminal-shell").is_none());
}

#[test]
fn pty_unavailable_skip_only_applies_to_terminal_shell() {
    // The same launch-failure text on a different base type is not a PTY skip.
    let err = SimardError::AdapterInvocationFailed {
        base_type: "local-harness".to_string(),
        reason: "failed to launch local PTY shell via 'script': boom".to_string(),
    };
    assert!(!is_skippable_pty_unavailable(&err, "local-harness"));
    assert!(gate_prerequisite_skip(&err, "local-harness").is_none());
}

#[test]
fn gate_prerequisite_skip_covers_auth_and_pty_but_not_genuine_failures() {
    let auth = SimardError::AdapterInvocationFailed {
        base_type: "rusty-clawd".to_string(),
        reason: "Copilot backend requires authentication. Use Client::new_copilot().".to_string(),
    };
    assert!(gate_prerequisite_skip(&auth, "rusty-clawd").is_some());

    let pty = SimardError::AdapterInvocationFailed {
        base_type: "terminal-shell".to_string(),
        reason: "failed to launch local PTY shell via 'script': not found".to_string(),
    };
    assert!(gate_prerequisite_skip(&pty, "terminal-shell").is_some());

    // A deterministic local-harness content failure is a real gate failure.
    let real = SimardError::AdapterInvocationFailed {
        base_type: "local-harness".to_string(),
        reason: "requires authentication".to_string(),
    };
    assert!(gate_prerequisite_skip(&real, "local-harness").is_none());
}

// --- starter suite gate membership (issue #2548) ---

/// The IDs of the deterministic scenarios the health gate is expected to run.
const EXPECTED_GATE_IDS: [&str; 3] = [
    "composite-session-review",
    "interactive-terminal-driving",
    "session-quality-memory-export",
];

#[test]
fn gate_base_types_are_credential_free() {
    // The gate must never depend on external auth, so its base types are the
    // deterministic local ones.
    assert_eq!(GATE_BASE_TYPES, ["local-harness", "terminal-shell"]);
}

#[test]
fn starter_gate_selects_exactly_the_deterministic_session_quality_scenarios() {
    let gate: Vec<&str> = benchmark_scenarios()
        .iter()
        .filter(|s| is_starter_gate_scenario(s))
        .map(|s| s.id)
        .collect();
    for expected in EXPECTED_GATE_IDS {
        assert!(
            gate.contains(&expected),
            "gate should include deterministic scenario '{expected}', got {gate:?}"
        );
    }
    assert_eq!(
        gate.len(),
        EXPECTED_GATE_IDS.len(),
        "gate should contain exactly the deterministic scenarios, got {gate:?}"
    );
}

#[test]
fn starter_gate_excludes_llm_content_check_scenarios() {
    // These local-harness scenarios are graded by LLM-content checks the
    // deterministic harness cannot satisfy; they were the false-green trigger.
    let excluded = [
        "repo-exploration-local",
        "repo-exploration-deep-scan",
        "doc-generation-public-fn",
        "safe-code-change-add-derive",
        "safe-change-add-enum-variant",
    ];
    for id in excluded {
        let scenario = benchmark_scenarios()
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("scenario '{id}' should still exist in the catalogue"));
        assert!(
            !is_starter_gate_scenario(scenario),
            "content-check scenario '{id}' must not gate the binary's health"
        );
    }
}

#[test]
fn every_gate_scenario_is_session_quality_on_a_deterministic_base_type() {
    for scenario in benchmark_scenarios()
        .iter()
        .filter(|s| is_starter_gate_scenario(s))
    {
        assert!(
            matches!(scenario.class, BenchmarkClass::SessionQuality),
            "gate scenario '{}' must be SessionQuality",
            scenario.id
        );
        assert!(
            GATE_BASE_TYPES.contains(&scenario.base_type),
            "gate scenario '{}' must use a credential-free base type, has '{}'",
            scenario.id,
            scenario.base_type
        );
    }
}

#[test]
fn starter_gate_is_nonempty() {
    let count = benchmark_scenarios()
        .iter()
        .filter(|s| is_starter_gate_scenario(s))
        .count();
    assert!(count > 0, "health gate must run at least one scenario");
}
