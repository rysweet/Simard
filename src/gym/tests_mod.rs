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
