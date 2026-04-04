use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::base_types::{BaseTypeFactory, BaseTypeId};
use crate::identity::IdentityManifest;
use crate::reflection::ReflectionReport;
use crate::session::SessionRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeTopology {
    SingleProcess,
    MultiProcess,
    Distributed,
}

impl Display for RuntimeTopology {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::SingleProcess => "single-process",
            Self::MultiProcess => "multi-process",
            Self::Distributed => "distributed",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeState {
    Initializing,
    Ready,
    Active,
    Reflecting,
    Persisting,
    Failed,
    Stopping,
    Stopped,
}

impl Display for RuntimeState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Reflecting => "reflecting",
            Self::Persisting => "persisting",
            Self::Failed => "failed",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        };
        f.write_str(label)
    }
}

impl RuntimeState {
    pub fn can_transition_to(self, next: RuntimeState) -> bool {
        matches!(
            (self, next),
            (Self::Initializing, Self::Ready)
                | (Self::Initializing, Self::Stopping)
                | (Self::Ready, Self::Active)
                | (Self::Ready, Self::Stopping)
                | (Self::Active, Self::Reflecting)
                | (Self::Active, Self::Stopping)
                | (Self::Reflecting, Self::Persisting)
                | (Self::Reflecting, Self::Stopping)
                | (Self::Persisting, Self::Ready)
                | (Self::Persisting, Self::Stopping)
                | (Self::Failed, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
                | (Self::Initializing, Self::Failed)
                | (Self::Ready, Self::Failed)
                | (Self::Active, Self::Failed)
                | (Self::Reflecting, Self::Failed)
                | (Self::Persisting, Self::Failed)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct RuntimeNodeId(String);

impl RuntimeNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn local() -> Self {
        Self::new("node-local")
    }
}

impl Display for RuntimeNodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct RuntimeAddress(String);

impl RuntimeAddress {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn local(node: &RuntimeNodeId) -> Self {
        Self::new(format!("inmemory://{node}"))
    }
}

impl Display for RuntimeAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Default)]
pub struct BaseTypeRegistry {
    factories: BTreeMap<BaseTypeId, Arc<dyn BaseTypeFactory>>,
}

impl BaseTypeRegistry {
    pub fn register<F>(&mut self, factory: F)
    where
        F: BaseTypeFactory + 'static,
    {
        self.factories
            .insert(factory.descriptor().id.clone(), Arc::new(factory));
    }

    pub fn get(&self, id: &BaseTypeId) -> Option<Arc<dyn BaseTypeFactory>> {
        self.factories.get(id).map(Arc::clone)
    }

    pub fn registered_ids(&self) -> Vec<BaseTypeId> {
        self.factories.keys().cloned().collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRequest {
    pub manifest: IdentityManifest,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
}

impl RuntimeRequest {
    pub fn new(
        manifest: IdentityManifest,
        selected_base_type: BaseTypeId,
        topology: RuntimeTopology,
    ) -> Self {
        Self {
            manifest,
            selected_base_type,
            topology,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionOutcome {
    pub session: SessionRecord,
    pub plan: String,
    pub execution_summary: String,
    pub reflection: ReflectionReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- State transition tests ---

    #[test]
    fn valid_state_transitions_are_accepted() {
        assert!(RuntimeState::Initializing.can_transition_to(RuntimeState::Ready));
        assert!(RuntimeState::Ready.can_transition_to(RuntimeState::Active));
        assert!(RuntimeState::Active.can_transition_to(RuntimeState::Reflecting));
        assert!(RuntimeState::Reflecting.can_transition_to(RuntimeState::Persisting));
        assert!(RuntimeState::Persisting.can_transition_to(RuntimeState::Ready));
        assert!(RuntimeState::Stopping.can_transition_to(RuntimeState::Stopped));
    }

    #[test]
    fn invalid_state_transitions_are_rejected() {
        assert!(!RuntimeState::Ready.can_transition_to(RuntimeState::Initializing));
        assert!(!RuntimeState::Stopped.can_transition_to(RuntimeState::Ready));
        assert!(!RuntimeState::Active.can_transition_to(RuntimeState::Ready));
        assert!(!RuntimeState::Failed.can_transition_to(RuntimeState::Ready));
    }

    #[test]
    fn any_active_state_can_transition_to_failed() {
        assert!(RuntimeState::Initializing.can_transition_to(RuntimeState::Failed));
        assert!(RuntimeState::Ready.can_transition_to(RuntimeState::Failed));
        assert!(RuntimeState::Active.can_transition_to(RuntimeState::Failed));
        assert!(RuntimeState::Reflecting.can_transition_to(RuntimeState::Failed));
        assert!(RuntimeState::Persisting.can_transition_to(RuntimeState::Failed));
    }

    #[test]
    fn any_non_stopped_state_can_transition_to_stopping() {
        assert!(RuntimeState::Initializing.can_transition_to(RuntimeState::Stopping));
        assert!(RuntimeState::Ready.can_transition_to(RuntimeState::Stopping));
        assert!(RuntimeState::Active.can_transition_to(RuntimeState::Stopping));
        assert!(RuntimeState::Failed.can_transition_to(RuntimeState::Stopping));
    }

    #[test]
    fn registry_returns_none_for_missing_base_type() {
        let registry = BaseTypeRegistry::default();
        assert!(registry.get(&BaseTypeId::new("nonexistent")).is_none());
    }
}
