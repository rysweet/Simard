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
