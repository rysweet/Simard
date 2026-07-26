use std::path::PathBuf;

use super::*;

// --- Display: MissingRequiredConfig ---

#[test]
fn display_invalid_base_type_session_state() {
    let err = SimardError::InvalidBaseTypeSessionState {
        base_type: "terminal-shell".to_string(),
        action: "execute".to_string(),
        reason: "session closed".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("terminal-shell"), "{msg}");
    assert!(msg.contains("execute"), "{msg}");
}

// --- Display: MissingCapability ---

#[test]
fn display_missing_capability() {
    let err = SimardError::MissingCapability {
        base_type: "local-harness".to_string(),
        capability: BaseTypeCapability::TerminalSession,
    };
    let msg = err.to_string();
    assert!(msg.contains("local-harness"), "{msg}");
    assert!(msg.contains("terminal-session"), "{msg}");
}

// --- Display: UnsupportedTopology ---

#[test]
fn display_unsupported_topology() {
    let err = SimardError::UnsupportedTopology {
        base_type: "local-harness".to_string(),
        topology: RuntimeTopology::Distributed,
    };
    let msg = err.to_string();
    assert!(msg.contains("local-harness"), "{msg}");
    assert!(msg.contains("distributed"), "{msg}");
}

// --- Display: UnsupportedRuntimeTopology ---

#[test]
fn display_unsupported_runtime_topology() {
    let err = SimardError::UnsupportedRuntimeTopology {
        topology: RuntimeTopology::MultiProcess,
        driver: "basic-driver".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("basic-driver"), "{msg}");
    assert!(msg.contains("multi-process"), "{msg}");
}

// --- Display: InvalidRuntimeTransition ---

#[test]
fn display_invalid_runtime_transition() {
    let err = SimardError::InvalidRuntimeTransition {
        from: RuntimeState::Stopped,
        to: RuntimeState::Active,
    };
    let msg = err.to_string();
    assert!(msg.contains("stopped"), "{msg}");
    assert!(msg.contains("active"), "{msg}");
}

// --- Display: RuntimeStopped ---

#[test]
fn display_runtime_stopped() {
    let err = SimardError::RuntimeStopped {
        action: "execute".to_string(),
    };
    assert!(err.to_string().contains("execute"));
    assert!(err.to_string().contains("stopped"));
}

// --- Display: RuntimeFailed ---

#[test]
fn display_runtime_failed() {
    let err = SimardError::RuntimeFailed {
        action: "persist".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("persist"), "{msg}");
    assert!(msg.contains("failed"), "{msg}");
}

// --- Display: InvalidSessionTransition ---

#[test]
fn display_invalid_session_transition() {
    let err = SimardError::InvalidSessionTransition {
        from: SessionPhase::Complete,
        to: SessionPhase::Execution,
    };
    let msg = err.to_string();
    assert!(msg.contains("complete"), "{msg}");
    assert!(msg.contains("execution"), "{msg}");
}

// --- Display: InvalidHandoffSnapshot ---

#[test]
fn display_invalid_handoff_snapshot() {
    let err = SimardError::InvalidHandoffSnapshot {
        field: "session".to_string(),
        reason: "missing".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("session"), "{msg}");
    assert!(msg.contains("missing"), "{msg}");
}

// --- Display: NotARepo ---

#[test]
fn display_not_a_repo() {
    let err = SimardError::NotARepo {
        path: PathBuf::from("/home/user/project"),
        reason: "no .git directory".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("NOT_A_REPO"), "{msg}");
    assert!(msg.contains("/home/user/project"), "{msg}");
    assert!(msg.contains("no .git directory"), "{msg}");
}

// --- Display: MissingWorktree (issue #4744) ---

#[test]
fn display_missing_worktree() {
    let err = SimardError::MissingWorktree {
        claim_key: "engineer:goal-7f5afcca".to_string(),
        expected_path: PathBuf::from("/state/engineer-worktrees/eng-7f5afcca"),
    };
    let msg = err.to_string();
    assert!(msg.contains("MISSING_WORKTREE"), "{msg}");
    assert!(msg.contains("engineer:goal-7f5afcca"), "{msg}");
    assert!(
        msg.contains("/state/engineer-worktrees/eng-7f5afcca"),
        "{msg}"
    );
    // A MissingWorktree must never be renderable as a NotARepo false positive.
    assert!(!msg.contains("NOT_A_REPO"), "{msg}");
}

#[test]
fn missing_worktree_is_not_not_a_repo_variant() {
    let missing = SimardError::MissingWorktree {
        claim_key: "engineer:x".to_string(),
        expected_path: PathBuf::from("/state/engineer-worktrees/x"),
    };
    assert!(
        !matches!(missing, SimardError::NotARepo { .. }),
        "MissingWorktree must be a distinct variant from NotARepo (issue #4744)"
    );
}

// --- Display: StewardshipRoutingAmbiguous (issue #1167) ---

#[test]
fn display_stewardship_routing_ambiguous() {
    let err = SimardError::StewardshipRoutingAmbiguous {
        source: "totally_unknown_subsystem".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("stewardship"), "{msg}");
    assert!(msg.contains("totally_unknown_subsystem"), "{msg}");
}

// --- Display: StewardshipGhCommandFailed ---

#[test]
fn display_stewardship_gh_command_failed() {
    let err = SimardError::StewardshipGhCommandFailed {
        reason: "rate limit exceeded".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("stewardship"), "{msg}");
    assert!(msg.contains("gh"), "{msg}");
    assert!(msg.contains("rate limit exceeded"), "{msg}");
}

// --- Display: StewardshipInvalidRunSummary ---

#[test]
fn display_stewardship_invalid_run_summary() {
    let err = SimardError::StewardshipInvalidRunSummary { field: "run_id" };
    let msg = err.to_string();
    assert!(msg.contains("stewardship"), "{msg}");
    assert!(msg.contains("run_id"), "{msg}");
}

// --- Display: CiHealthGhCommandFailed ---

#[test]
fn display_ci_health_gh_command_failed() {
    let err = SimardError::CiHealthGhCommandFailed {
        reason: "gh run list exited 1".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("ci-health"), "{msg}");
    assert!(msg.contains("gh"), "{msg}");
    assert!(msg.contains("gh run list exited 1"), "{msg}");
}

// --- Display: MergeAuthorityGhCommandFailed ---

#[test]
fn display_merge_authority_gh_command_failed() {
    let err = SimardError::MergeAuthorityGhCommandFailed {
        reason: "exit 1: 'gh' not found".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("merge-authority"), "{msg}");
    assert!(msg.contains("gh command failed"), "{msg}");
    assert!(msg.contains("'gh' not found"), "{msg}");
}

// --- Display: MergeAuthorityEvaluationFailed ---

#[test]
fn display_merge_authority_evaluation_failed() {
    let err = SimardError::MergeAuthorityEvaluationFailed {
        reason: "could not parse statusCheckRollup JSON".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("merge-authority"), "{msg}");
    assert!(msg.contains("evaluation failed"), "{msg}");
    assert!(msg.contains("statusCheckRollup"), "{msg}");
}

// --- Display: DirtyWorktree ---

#[test]
fn display_dirty_worktree_single_file() {
    let err = SimardError::DirtyWorktree {
        changed_files: vec!["src/main.rs".to_string()],
    };
    let msg = err.to_string();
    assert!(msg.contains("pre-mutation guard"), "{msg}");
    assert!(msg.contains("1 uncommitted change"), "{msg}");
    assert!(msg.contains("clean repo"), "{msg}");
}

#[test]
fn display_dirty_worktree_multiple_files() {
    let err = SimardError::DirtyWorktree {
        changed_files: vec![
            "src/main.rs".to_string(),
            "Cargo.toml".to_string(),
            "README.md".to_string(),
        ],
    };
    let msg = err.to_string();
    assert!(msg.contains("3 uncommitted change"), "{msg}");
}

#[test]
fn dirty_worktree_equality() {
    let a = SimardError::DirtyWorktree {
        changed_files: vec!["a.rs".to_string()],
    };
    let b = SimardError::DirtyWorktree {
        changed_files: vec!["a.rs".to_string()],
    };
    assert_eq!(a, b);
}

// --- Display: supply-chain steward variants (#2741) ---

#[test]
fn display_supply_chain_audit_parse_failed() {
    let err = SimardError::SupplyChainAuditParseFailed {
        reason: "missing `vulnerabilities` key".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("cargo-audit"), "{msg}");
    assert!(msg.contains("missing `vulnerabilities` key"), "{msg}");
}

#[test]
fn display_supply_chain_remediation_failed() {
    let err = SimardError::SupplyChainRemediationFailed {
        reason: "cargo update -p crossbeam-epoch exited 101".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("remediation failed"), "{msg}");
    assert!(msg.contains("crossbeam-epoch"), "{msg}");
}

#[test]
fn display_supply_chain_suppression_without_tracker() {
    let err = SimardError::SupplyChainSuppressionWithoutTracker {
        advisory_id: "RUSTSEC-2026-0204".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("RUSTSEC-2026-0204"), "{msg}");
    assert!(msg.contains("hard rail"), "{msg}");
}

#[test]
fn supply_chain_suppression_without_tracker_equality() {
    let a = SimardError::SupplyChainSuppressionWithoutTracker {
        advisory_id: "RUSTSEC-2026-0204".to_string(),
    };
    let b = SimardError::SupplyChainSuppressionWithoutTracker {
        advisory_id: "RUSTSEC-2026-0204".to_string(),
    };
    assert_eq!(a, b);
}
