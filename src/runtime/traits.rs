use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};

use super::types::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};

pub trait RuntimeTopologyDriver: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn supports_topology(&self, topology: RuntimeTopology) -> bool;

    fn local_node(&self) -> SimardResult<RuntimeNodeId>;
}

pub trait RuntimeMailboxTransport: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress>;
}

pub trait RuntimeSupervisor: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;
}

#[derive(Debug)]
pub struct InProcessTopologyDriver {
    descriptor: BackendDescriptor,
}

impl InProcessTopologyDriver {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "topology::in-process",
                "runtime-port:topology-driver",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeTopologyDriver for InProcessTopologyDriver {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        matches!(topology, RuntimeTopology::SingleProcess)
    }

    fn local_node(&self) -> SimardResult<RuntimeNodeId> {
        Ok(RuntimeNodeId::local())
    }
}

#[derive(Debug)]
pub struct InMemoryMailboxTransport {
    descriptor: BackendDescriptor,
}

impl InMemoryMailboxTransport {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "transport::in-memory-mailbox",
                "runtime-port:mailbox-transport",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeMailboxTransport for InMemoryMailboxTransport {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress> {
        Ok(RuntimeAddress::local(node))
    }
}

#[derive(Debug)]
pub struct LoopbackMeshTopologyDriver {
    descriptor: BackendDescriptor,
}

impl LoopbackMeshTopologyDriver {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "topology::loopback-mesh",
                "runtime-port:topology-driver",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeTopologyDriver for LoopbackMeshTopologyDriver {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        matches!(
            topology,
            RuntimeTopology::MultiProcess | RuntimeTopology::Distributed
        )
    }

    fn local_node(&self) -> SimardResult<RuntimeNodeId> {
        Ok(RuntimeNodeId::new("node-loopback-mesh"))
    }
}

#[derive(Debug)]
pub struct LoopbackMailboxTransport {
    descriptor: BackendDescriptor,
}

impl LoopbackMailboxTransport {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "transport::loopback-mailbox",
                "runtime-port:mailbox-transport",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeMailboxTransport for LoopbackMailboxTransport {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress> {
        Ok(RuntimeAddress::new(format!("loopback://{node}")))
    }
}

#[derive(Debug)]
pub struct InProcessSupervisor {
    descriptor: BackendDescriptor,
}

impl InProcessSupervisor {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "supervisor::in-process",
                "runtime-port:supervisor",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeSupervisor for InProcessSupervisor {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }
}

#[derive(Debug)]
pub struct CoordinatedSupervisor {
    descriptor: BackendDescriptor,
}

impl CoordinatedSupervisor {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "supervisor::coordinated",
                "runtime-port:supervisor",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeSupervisor for CoordinatedSupervisor {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_process_topology_supports_single_process_only() {
        let driver = InProcessTopologyDriver::try_default().unwrap();
        assert!(driver.supports_topology(RuntimeTopology::SingleProcess));
        assert!(!driver.supports_topology(RuntimeTopology::MultiProcess));
        assert!(!driver.supports_topology(RuntimeTopology::Distributed));
    }

    #[test]
    fn loopback_mesh_supports_multi_and_distributed() {
        let driver = LoopbackMeshTopologyDriver::try_default().unwrap();
        assert!(!driver.supports_topology(RuntimeTopology::SingleProcess));
        assert!(driver.supports_topology(RuntimeTopology::MultiProcess));
        assert!(driver.supports_topology(RuntimeTopology::Distributed));
    }
}
