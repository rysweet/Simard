use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeTopology;
use crate::session::SessionPhase;

/// Lightweight resource usage snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub memory_bytes: u64,
    pub active_sessions: usize,
}

/// Trait allowing agents to reflect on their own runtime configuration.
pub trait RuntimeReflection {
    fn topology(&self) -> RuntimeTopology;

    fn active_base_types(&self) -> Vec<String>;

    fn memory_backends(&self) -> Vec<String>;

    fn session_state(&self) -> SessionPhase;

    fn sibling_identities(&self) -> Vec<String>;

    fn uptime(&self) -> Duration;

    fn resource_usage(&self) -> ResourceSnapshot;
}

/// Serializable snapshot of all reflection data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub topology: RuntimeTopology,
    pub active_base_types: Vec<String>,
    pub memory_backends: Vec<String>,
    pub session_state: SessionPhase,
    pub sibling_identities: Vec<String>,
    pub uptime: Duration,
    pub resource_usage: ResourceSnapshot,
}

/// Captures a complete [`RuntimeSnapshot`] from any implementor.
pub fn snapshot(reflector: &dyn RuntimeReflection) -> RuntimeSnapshot {
    RuntimeSnapshot {
        topology: reflector.topology(),
        active_base_types: reflector.active_base_types(),
        memory_backends: reflector.memory_backends(),
        session_state: reflector.session_state(),
        sibling_identities: reflector.sibling_identities(),
        uptime: reflector.uptime(),
        resource_usage: reflector.resource_usage(),
    }
}

/// Local single-process implementation of [`RuntimeReflection`].
pub struct LocalReflector {
    topology: RuntimeTopology,
    start_time: Instant,
    base_types: Vec<String>,
    memory_backends: Vec<String>,
    identities: Vec<String>,
    session_phase: SessionPhase,
    active_sessions: usize,
}

impl LocalReflector {
    pub fn new(
        topology: RuntimeTopology,
        start_time: Instant,
        base_types: Vec<String>,
        memory_backends: Vec<String>,
        identities: Vec<String>,
    ) -> Self {
        Self {
            topology,
            start_time,
            base_types,
            memory_backends,
            identities,
            session_phase: SessionPhase::Intake,
            active_sessions: 0,
        }
    }

    pub fn set_session_phase(&mut self, phase: SessionPhase) {
        self.session_phase = phase;
    }

    pub fn set_active_sessions(&mut self, count: usize) {
        self.active_sessions = count;
    }
}

impl RuntimeReflection for LocalReflector {
    fn topology(&self) -> RuntimeTopology {
        self.topology
    }

    fn active_base_types(&self) -> Vec<String> {
        self.base_types.clone()
    }

    fn memory_backends(&self) -> Vec<String> {
        self.memory_backends.clone()
    }

    fn session_state(&self) -> SessionPhase {
        self.session_phase
    }

    fn sibling_identities(&self) -> Vec<String> {
        self.identities.clone()
    }

    fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    fn resource_usage(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            memory_bytes: 0,
            active_sessions: self.active_sessions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn test_reflector() -> LocalReflector {
        LocalReflector::new(
            RuntimeTopology::SingleProcess,
            Instant::now(),
            vec!["copilot".to_string(), "harness".to_string()],
            vec!["cognitive-bridge".to_string()],
            vec!["alpha".to_string(), "beta".to_string()],
        )
    }

    #[test]
    fn local_reflector_construction() {
        let r = test_reflector();
        assert_eq!(r.topology(), RuntimeTopology::SingleProcess);
        assert_eq!(r.session_state(), SessionPhase::Intake);
        assert_eq!(r.resource_usage().active_sessions, 0);
    }

    #[test]
    fn topology_returns_configured_value() {
        let r = LocalReflector::new(
            RuntimeTopology::Distributed,
            Instant::now(),
            vec![],
            vec![],
            vec![],
        );
        assert_eq!(r.topology(), RuntimeTopology::Distributed);
    }

    #[test]
    fn active_base_types_returns_adapters() {
        let r = test_reflector();
        assert_eq!(r.active_base_types(), vec!["copilot", "harness"]);
    }

    #[test]
    fn memory_backends_returns_stores() {
        let r = test_reflector();
        assert_eq!(r.memory_backends(), vec!["cognitive-bridge"]);
    }

    #[test]
    fn session_state_returns_phase() {
        let mut r = test_reflector();
        assert_eq!(r.session_state(), SessionPhase::Intake);
        r.set_session_phase(SessionPhase::Execution);
        assert_eq!(r.session_state(), SessionPhase::Execution);
    }

    #[test]
    fn sibling_identities_returns_names() {
        let r = test_reflector();
        assert_eq!(r.sibling_identities(), vec!["alpha", "beta"]);
    }

    #[test]
    fn uptime_advances() {
        let r = LocalReflector::new(
            RuntimeTopology::SingleProcess,
            Instant::now(),
            vec![],
            vec![],
            vec![],
        );
        thread::sleep(Duration::from_millis(10));
        assert!(r.uptime() >= Duration::from_millis(10));
    }

    #[test]
    fn resource_usage_tracks_sessions() {
        let mut r = test_reflector();
        r.set_active_sessions(3);
        let usage = r.resource_usage();
        assert_eq!(usage.active_sessions, 3);
        assert_eq!(usage.memory_bytes, 0);
    }

    #[test]
    fn resource_snapshot_default() {
        let d = ResourceSnapshot::default();
        assert_eq!(d.memory_bytes, 0);
        assert_eq!(d.active_sessions, 0);
    }

    #[test]
    fn snapshot_captures_all_fields() {
        let mut r = test_reflector();
        r.set_session_phase(SessionPhase::Planning);
        r.set_active_sessions(2);

        let snap = snapshot(&r);
        assert_eq!(snap.topology, RuntimeTopology::SingleProcess);
        assert_eq!(snap.active_base_types, vec!["copilot", "harness"]);
        assert_eq!(snap.memory_backends, vec!["cognitive-bridge"]);
        assert_eq!(snap.session_state, SessionPhase::Planning);
        assert_eq!(snap.sibling_identities, vec!["alpha", "beta"]);
        assert_eq!(snap.resource_usage.active_sessions, 2);
    }

    #[test]
    fn snapshot_serialization_round_trip() {
        let mut r = test_reflector();
        r.set_session_phase(SessionPhase::Reflection);
        r.set_active_sessions(1);

        let snap = snapshot(&r);
        let json = serde_json::to_string(&snap).expect("serialize");
        let restored: RuntimeSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(snap.topology, restored.topology);
        assert_eq!(snap.active_base_types, restored.active_base_types);
        assert_eq!(snap.memory_backends, restored.memory_backends);
        assert_eq!(snap.session_state, restored.session_state);
        assert_eq!(snap.sibling_identities, restored.sibling_identities);
        assert_eq!(snap.resource_usage, restored.resource_usage);
    }

    #[test]
    fn topology_enum_all_variants() {
        let variants = [
            RuntimeTopology::SingleProcess,
            RuntimeTopology::MultiProcess,
            RuntimeTopology::Distributed,
        ];
        for v in &variants {
            let r = LocalReflector::new(*v, Instant::now(), vec![], vec![], vec![]);
            assert_eq!(r.topology(), *v);
        }
    }

    #[test]
    fn snapshot_topology_serialization() {
        for topo in [
            RuntimeTopology::SingleProcess,
            RuntimeTopology::MultiProcess,
            RuntimeTopology::Distributed,
        ] {
            let r = LocalReflector::new(topo, Instant::now(), vec![], vec![], vec![]);
            let snap = snapshot(&r);
            let json = serde_json::to_string(&snap).expect("serialize");
            let restored: RuntimeSnapshot = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored.topology, topo);
        }
    }
}
