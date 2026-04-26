use crate::base_types::BaseTypeId;
use crate::identity::OperatingMode;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

#[test]
fn session_record_starts_at_intake() {
    let session = SessionRecord::new(
        OperatingMode::Engineer,
        "test-objective",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );
    assert_eq!(session.phase, SessionPhase::Intake);
    assert!(session.evidence_ids.is_empty());
    assert!(session.memory_keys.is_empty());
}

#[test]
fn session_advance_through_full_lifecycle() {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        "test",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );
    let phases = [
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
        SessionPhase::Reflection,
        SessionPhase::Persistence,
        SessionPhase::Complete,
    ];
    for phase in phases {
        session.advance(phase).unwrap();
        assert_eq!(session.phase, phase);
    }
}

#[test]
fn session_advance_invalid_transition_errors() {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        "test",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );
    // Intake -> Complete should fail (must go through intermediate phases)
    let result = session.advance(SessionPhase::Complete);
    assert!(result.is_err());
}

#[test]
fn session_advance_to_failed_from_any_phase() {
    for start_phase in [
        SessionPhase::Intake,
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
    ] {
        let mut session = SessionRecord::new(
            OperatingMode::Engineer,
            "test",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        // Advance to the starting phase first
        let intermediates: Vec<SessionPhase> = match start_phase {
            SessionPhase::Intake => vec![],
            SessionPhase::Preparation => vec![SessionPhase::Preparation],
            SessionPhase::Planning => {
                vec![SessionPhase::Preparation, SessionPhase::Planning]
            }
            SessionPhase::Execution => vec![
                SessionPhase::Preparation,
                SessionPhase::Planning,
                SessionPhase::Execution,
            ],
            _ => vec![],
        };
        for phase in intermediates {
            session.advance(phase).unwrap();
        }
        // Failed should always be reachable
        session.advance(SessionPhase::Failed).unwrap();
        assert_eq!(session.phase, SessionPhase::Failed);
    }
}

#[test]
fn session_attach_evidence_and_memory() {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        "test",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );
    session.attach_evidence("ev-1");
    session.attach_evidence("ev-2");
    session.attach_memory("mem-1");
    assert_eq!(session.evidence_ids.len(), 2);
    assert_eq!(session.memory_keys.len(), 1);
}

#[test]
fn session_redacted_for_handoff_changes_objective() {
    let session = SessionRecord::new(
        OperatingMode::Engineer,
        "secret objective text",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );
    let redacted = session.redacted_for_handoff();
    assert_ne!(redacted.objective, "secret objective text");
    assert!(
        redacted.objective.starts_with("objective-metadata("),
        "redacted objective should start with 'objective-metadata(', got: {}",
        redacted.objective
    );
}

#[test]
fn session_phase_display() {
    assert_eq!(SessionPhase::Intake.to_string(), "intake");
    assert_eq!(SessionPhase::Complete.to_string(), "complete");
    assert_eq!(SessionPhase::Failed.to_string(), "failed");
}
