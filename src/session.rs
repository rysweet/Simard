use std::fmt::{self, Display, Formatter};

use uuid::Uuid;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

    pub fn attach_evidence(&mut self, evidence_id: impl Into<String>) {
        self.evidence_ids.push(evidence_id.into());
    }

    pub fn attach_memory(&mut self, memory_key: impl Into<String>) {
        self.memory_keys.push(memory_key.into());
    }
}
