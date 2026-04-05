use std::path::PathBuf;

use super::*;

// --- Display: UnsupportedEngineerAction ---

#[test]
fn display_unsupported_engineer_action() {
    let err = SimardError::UnsupportedEngineerAction {
        reason: "no tooling".to_string(),
    };
    assert!(err.to_string().contains("no tooling"));
}

// --- Display: ActionExecutionFailed ---

#[test]
fn display_action_execution_failed() {
    let err = SimardError::ActionExecutionFailed {
        action: "cargo test".to_string(),
        reason: "exit code 1".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("cargo test"), "{msg}");
    assert!(msg.contains("exit code 1"), "{msg}");
}

// --- Display: CommandTimeout ---

#[test]
fn display_command_timeout() {
    let err = SimardError::CommandTimeout {
        action: "build".to_string(),
        timeout_secs: 120,
    };
    let msg = err.to_string();
    assert!(msg.contains("build"), "{msg}");
    assert!(msg.contains("120"), "{msg}");
}

// --- Display: VerificationFailed ---

#[test]
fn display_verification_failed() {
    let err = SimardError::VerificationFailed {
        reason: "tests did not pass".to_string(),
    };
    assert!(err.to_string().contains("tests did not pass"));
}

// --- Display: InvalidStateRoot ---

#[test]
fn display_invalid_state_root() {
    let err = SimardError::InvalidStateRoot {
        path: PathBuf::from("/bad/root"),
        reason: "not writable".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("/bad/root"), "{msg}");
    assert!(msg.contains("not writable"), "{msg}");
}

// --- Display: PersistentStoreIo ---

#[test]
fn display_persistent_store_io() {
    let err = SimardError::PersistentStoreIo {
        store: "memory".to_string(),
        action: "write".to_string(),
        path: PathBuf::from("/store/memory.json"),
        reason: "disk full".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("memory"), "{msg}");
    assert!(msg.contains("write"), "{msg}");
    assert!(msg.contains("disk full"), "{msg}");
}

// --- Display: BenchmarkScenarioNotFound ---

#[test]
fn display_benchmark_scenario_not_found() {
    let err = SimardError::BenchmarkScenarioNotFound {
        scenario_id: "missing-scenario".to_string(),
    };
    assert!(err.to_string().contains("missing-scenario"));
}

// --- Display: BenchmarkSuiteNotFound ---

#[test]
fn display_benchmark_suite_not_found() {
    let err = SimardError::BenchmarkSuiteNotFound {
        suite_id: "missing-suite".to_string(),
    };
    assert!(err.to_string().contains("missing-suite"));
}

// --- Display: BenchmarkComparisonUnavailable ---

#[test]
fn display_benchmark_comparison_unavailable() {
    let err = SimardError::BenchmarkComparisonUnavailable {
        scenario_id: "test-scenario".to_string(),
        reason: "not enough runs".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("test-scenario"), "{msg}");
    assert!(msg.contains("not enough runs"), "{msg}");
}

// --- Display: ArtifactIo ---

#[test]
fn display_artifact_io() {
    let err = SimardError::ArtifactIo {
        path: PathBuf::from("/artifacts/report.json"),
        reason: "corrupt json".to_string(),
    };
    assert!(err.to_string().contains("corrupt json"));
}

// --- Display: StoragePoisoned ---

#[test]
fn display_storage_poisoned() {
    let err = SimardError::StoragePoisoned {
        store: "evidence".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("evidence"), "{msg}");
    assert!(msg.contains("poisoned"), "{msg}");
}

// --- Display: ClockBeforeUnixEpoch ---

#[test]
fn display_clock_before_unix_epoch() {
    let err = SimardError::ClockBeforeUnixEpoch {
        reason: "system time error".to_string(),
    };
    assert!(err.to_string().contains("UNIX epoch"));
}

// --- Display: Bridge errors ---

#[test]
fn display_bridge_spawn_failed() {
    let err = SimardError::BridgeSpawnFailed {
        bridge: "memory".to_string(),
        reason: "python not found".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("memory"), "{msg}");
    assert!(msg.contains("python not found"), "{msg}");
}

#[test]
fn display_bridge_transport_error() {
    let err = SimardError::BridgeTransportError {
        bridge: "knowledge".to_string(),
        reason: "connection refused".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("knowledge"), "{msg}");
    assert!(msg.contains("transport error"), "{msg}");
}

#[test]
fn display_bridge_protocol_error() {
    let err = SimardError::BridgeProtocolError {
        bridge: "gym".to_string(),
        reason: "invalid json".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("gym"), "{msg}");
    assert!(msg.contains("protocol error"), "{msg}");
}

#[test]
fn display_bridge_call_failed() {
    let err = SimardError::BridgeCallFailed {
        bridge: "memory".to_string(),
        method: "store_episode".to_string(),
        reason: "timeout".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("memory"), "{msg}");
    assert!(msg.contains("store_episode"), "{msg}");
    assert!(msg.contains("timeout"), "{msg}");
}

#[test]
fn display_bridge_circuit_open() {
    let err = SimardError::BridgeCircuitOpen {
        bridge: "memory".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("memory"), "{msg}");
    assert!(msg.contains("circuit"), "{msg}");
}

// --- Display: Planning / Review ---

#[test]
fn display_planning_unavailable() {
    let err = SimardError::PlanningUnavailable {
        reason: "no API key".to_string(),
    };
    assert!(err.to_string().contains("no API key"));
}

#[test]
fn display_review_unavailable() {
    let err = SimardError::ReviewUnavailable {
        reason: "session closed".to_string(),
    };
    assert!(err.to_string().contains("session closed"));
}

#[test]
fn display_review_blocked() {
    let err = SimardError::ReviewBlocked {
        summary: "security issue found".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("review blocked"), "{msg}");
    assert!(msg.contains("security issue found"), "{msg}");
}

// --- Display: GitCommandFailed ---

#[test]
fn display_git_command_failed() {
    let err = SimardError::GitCommandFailed {
        command: "git push".to_string(),
        reason: "rejected".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("git push"), "{msg}");
    assert!(msg.contains("rejected"), "{msg}");
}

// --- Error trait ---

#[test]
fn simard_error_implements_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(SimardError::RuntimeStopped {
        action: "test".to_string(),
    });
    assert!(err.source().is_none());
}

// --- Clone / PartialEq ---

#[test]
fn simard_error_clone_eq() {
    let err = SimardError::UnknownIdentity {
        requested: "test".to_string(),
    };
    let cloned = err.clone();
    assert_eq!(err, cloned);
}

#[test]
fn simard_error_debug_format() {
    let err = SimardError::StoragePoisoned {
        store: "memory".to_string(),
    };
    let debug = format!("{err:?}");
    assert!(debug.contains("StoragePoisoned"), "{debug}");
}

// --- fmt_field_reason branch coverage ---

#[test]
fn fmt_field_reason_identity_composition() {
    let err = SimardError::InvalidIdentityComposition {
        identity: "test-id".to_string(),
        reason: "bad".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("identity 'test-id' has invalid composition: bad"),
        "{msg}"
    );
}

#[test]
fn fmt_field_reason_manifest_contract() {
    let err = SimardError::InvalidManifestContract {
        field: "name".to_string(),
        reason: "empty".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid manifest contract field 'name': empty"),
        "{msg}"
    );
}

#[test]
fn fmt_field_reason_goal_record() {
    let err = SimardError::InvalidGoalRecord {
        field: "priority".to_string(),
        reason: "out of range".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid goal record field 'priority': out of range"),
        "{msg}"
    );
}

#[test]
fn fmt_field_reason_research_record() {
    let err = SimardError::InvalidResearchRecord {
        field: "source".to_string(),
        reason: "blank".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid research record field 'source': blank"),
        "{msg}"
    );
}

#[test]
fn fmt_field_reason_meeting_record() {
    let err = SimardError::InvalidMeetingRecord {
        field: "decisions".to_string(),
        reason: "malformed".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid meeting record field 'decisions': malformed"),
        "{msg}"
    );
}

#[test]
fn fmt_field_reason_improvement_record() {
    let err = SimardError::InvalidImprovementRecord {
        field: "plan".to_string(),
        reason: "incomplete".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid improvement record field 'plan': incomplete"),
        "{msg}"
    );
}
