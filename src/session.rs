use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::sanitization::{normalize_objective_metadata, objective_metadata};

/// Opaque session identifier wrapping `session-<uuid>`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(format!("session-{uuid}"))
    }

    pub fn parse(value: impl AsRef<str>) -> SimardResult<Self> {
        let value = value.as_ref().trim();
        let uuid_value = value.strip_prefix("session-").unwrap_or(value);
        let uuid = Uuid::parse_str(uuid_value).map_err(|error| SimardError::InvalidSessionId {
            value: value.to_string(),
            reason: format!("expected a UUID or 'session-<uuid>' value: {error}"),
        })?;
        Ok(Self::from_uuid(uuid))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SessionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl TryFrom<&str> for SessionId {
    type Error = SimardError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// Generates unique [`SessionId`] values.
pub trait SessionIdGenerator: Send + Sync {
    fn next_id(&self) -> SessionId;
}

#[derive(Debug, Default)]
pub struct UuidSessionIdGenerator;

impl SessionIdGenerator for UuidSessionIdGenerator {
    fn next_id(&self) -> SessionId {
        SessionId::from_uuid(Uuid::now_v7())
    }
}

/// Lifecycle phase of an agent session (intake → complete or failed).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPhase {
    Intake,
    Preparation,
    Planning,
    Execution,
    Reflection,
    Persistence,
    Complete,
    Failed,
}

impl Display for SessionPhase {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Intake => "intake",
            Self::Preparation => "preparation",
            Self::Planning => "planning",
            Self::Execution => "execution",
            Self::Reflection => "reflection",
            Self::Persistence => "persistence",
            Self::Complete => "complete",
            Self::Failed => "failed",
        };
        f.write_str(label)
    }
}

impl SessionPhase {
    pub fn can_transition_to(self, next: SessionPhase) -> bool {
        matches!(
            (self, next),
            (Self::Intake, Self::Preparation)
                | (Self::Preparation, Self::Planning)
                | (Self::Planning, Self::Execution)
                | (Self::Execution, Self::Reflection)
                | (Self::Reflection, Self::Persistence)
                | (Self::Persistence, Self::Complete)
                | (_, Self::Failed)
        )
    }
}

/// Persisted record of a completed or in-progress session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub mode: OperatingMode,
    pub objective: String,
    pub phase: SessionPhase,
    pub selected_base_type: BaseTypeId,
    pub evidence_ids: Vec<String>,
    pub memory_keys: Vec<String>,
}

impl SessionRecord {
    pub fn new(
        mode: OperatingMode,
        objective: impl Into<String>,
        selected_base_type: BaseTypeId,
        session_ids: &dyn SessionIdGenerator,
    ) -> Self {
        Self {
            id: session_ids.next_id(),
            mode,
            objective: objective.into(),
            phase: SessionPhase::Intake,
            selected_base_type,
            evidence_ids: Vec::new(),
            memory_keys: Vec::new(),
        }
    }

    pub fn advance(&mut self, next: SessionPhase) -> SimardResult<()> {
        if !self.phase.can_transition_to(next) {
            return Err(SimardError::InvalidSessionTransition {
                from: self.phase,
                to: next,
            });
        }

        self.phase = next;
        Ok(())
    }

    pub fn redacted_for_handoff(&self) -> Self {
        let mut redacted = self.clone();
        redacted.objective = normalize_objective_metadata(&self.objective)
            .unwrap_or_else(|| objective_metadata(&self.objective));
        redacted
    }

    pub fn attach_evidence(&mut self, evidence_id: impl Into<String>) {
        self.evidence_ids.push(evidence_id.into());
    }

    pub fn attach_memory(&mut self, memory_key: impl Into<String>) {
        self.memory_keys.push(memory_key.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SessionId ---

    #[test]
    fn session_id_from_uuid_produces_prefixed_string() {
        let uuid = Uuid::nil();
        let id = SessionId::from_uuid(uuid);
        assert_eq!(id.as_str(), "session-00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn session_id_parse_accepts_prefixed_uuid() {
        let id = SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(id.as_str(), "session-00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn session_id_parse_accepts_bare_uuid() {
        let id = SessionId::parse("00000000-0000-0000-0000-000000000002").unwrap();
        assert_eq!(id.as_str(), "session-00000000-0000-0000-0000-000000000002");
    }

    #[test]
    fn session_id_parse_trims_whitespace() {
        let id = SessionId::parse("  00000000-0000-0000-0000-000000000003  ").unwrap();
        assert_eq!(id.as_str(), "session-00000000-0000-0000-0000-000000000003");
    }

    #[test]
    fn session_id_parse_rejects_invalid_input() {
        let err = SessionId::parse("not-a-uuid").unwrap_err();
        assert!(matches!(err, SimardError::InvalidSessionId { .. }));
    }

    #[test]
    fn session_id_parse_rejects_empty_string() {
        let err = SessionId::parse("").unwrap_err();
        assert!(matches!(err, SimardError::InvalidSessionId { .. }));
    }

    #[test]
    fn session_id_display_matches_as_str() {
        let id = SessionId::from_uuid(Uuid::nil());
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn session_id_try_from_str_delegates_to_parse() {
        let id: SessionId = "00000000-0000-0000-0000-000000000004".try_into().unwrap();
        assert_eq!(id.as_str(), "session-00000000-0000-0000-0000-000000000004");
    }

    #[test]
    fn session_id_from_uuid_trait() {
        let uuid = Uuid::nil();
        let id: SessionId = uuid.into();
        assert_eq!(id, SessionId::from_uuid(Uuid::nil()));
    }

    // --- UuidSessionIdGenerator ---

    #[test]
    fn uuid_generator_produces_unique_ids() {
        let generator = UuidSessionIdGenerator;
        let a = generator.next_id();
        let b = generator.next_id();
        assert_ne!(a, b);
    }

    // --- SessionPhase ---

    #[test]
    fn session_phase_happy_path_transitions() {
        assert!(SessionPhase::Intake.can_transition_to(SessionPhase::Preparation));
        assert!(SessionPhase::Preparation.can_transition_to(SessionPhase::Planning));
        assert!(SessionPhase::Planning.can_transition_to(SessionPhase::Execution));
        assert!(SessionPhase::Execution.can_transition_to(SessionPhase::Reflection));
        assert!(SessionPhase::Reflection.can_transition_to(SessionPhase::Persistence));
        assert!(SessionPhase::Persistence.can_transition_to(SessionPhase::Complete));
    }

    #[test]
    fn any_phase_can_transition_to_failed() {
        let phases = [
            SessionPhase::Intake,
            SessionPhase::Preparation,
            SessionPhase::Planning,
            SessionPhase::Execution,
            SessionPhase::Reflection,
            SessionPhase::Persistence,
            SessionPhase::Complete,
        ];
        for phase in phases {
            assert!(
                phase.can_transition_to(SessionPhase::Failed),
                "{phase} should be able to transition to Failed"
            );
        }
    }

    #[test]
    fn backward_transitions_are_rejected() {
        assert!(!SessionPhase::Preparation.can_transition_to(SessionPhase::Intake));
        assert!(!SessionPhase::Complete.can_transition_to(SessionPhase::Execution));
        assert!(!SessionPhase::Planning.can_transition_to(SessionPhase::Preparation));
    }

    #[test]
    fn session_phase_display_renders_lowercase() {
        assert_eq!(SessionPhase::Intake.to_string(), "intake");
        assert_eq!(SessionPhase::Complete.to_string(), "complete");
        assert_eq!(SessionPhase::Failed.to_string(), "failed");
    }

    // --- SessionRecord ---

    struct FixedSessionIdGenerator(SessionId);

    impl SessionIdGenerator for FixedSessionIdGenerator {
        fn next_id(&self) -> SessionId {
            self.0.clone()
        }
    }

    fn test_session_record() -> SessionRecord {
        let id_gen = FixedSessionIdGenerator(SessionId::from_uuid(Uuid::nil()));
        SessionRecord::new(
            OperatingMode::Engineer,
            "Test objective",
            BaseTypeId::new("test-adapter"),
            &id_gen,
        )
    }

    #[test]
    fn session_record_new_starts_at_intake() {
        let rec = test_session_record();
        assert_eq!(rec.phase, SessionPhase::Intake);
        assert_eq!(rec.objective, "Test objective");
        assert!(rec.evidence_ids.is_empty());
        assert!(rec.memory_keys.is_empty());
    }

    #[test]
    fn session_record_advance_follows_happy_path() {
        let mut rec = test_session_record();
        rec.advance(SessionPhase::Preparation).unwrap();
        assert_eq!(rec.phase, SessionPhase::Preparation);
        rec.advance(SessionPhase::Planning).unwrap();
        assert_eq!(rec.phase, SessionPhase::Planning);
    }

    #[test]
    fn session_record_advance_rejects_invalid_transition() {
        let mut rec = test_session_record();
        let err = rec.advance(SessionPhase::Complete).unwrap_err();
        assert!(matches!(err, SimardError::InvalidSessionTransition { .. }));
        assert_eq!(rec.phase, SessionPhase::Intake);
    }

    #[test]
    fn session_record_attach_evidence_accumulates() {
        let mut rec = test_session_record();
        rec.attach_evidence("ev-1");
        rec.attach_evidence("ev-2");
        assert_eq!(rec.evidence_ids, vec!["ev-1", "ev-2"]);
    }

    #[test]
    fn session_record_attach_memory_accumulates() {
        let mut rec = test_session_record();
        rec.attach_memory("mem-a");
        rec.attach_memory("mem-b");
        assert_eq!(rec.memory_keys, vec!["mem-a", "mem-b"]);
    }

    #[test]
    fn session_record_redacted_replaces_objective_with_metadata() {
        let rec = test_session_record();
        let redacted = rec.redacted_for_handoff();
        assert_ne!(redacted.objective, rec.objective);
        assert!(redacted.objective.contains("objective-metadata("));
        assert_eq!(redacted.id, rec.id);
        assert_eq!(redacted.phase, rec.phase);
    }

    #[test]
    fn session_record_serde_roundtrip() {
        let rec = test_session_record();
        let json = serde_json::to_string(&rec).unwrap();
        let deserialized: SessionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, rec);
    }
}
