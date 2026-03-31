//! Launch and manage Python bridge subprocesses for live Simard operations.
//!
//! This module provides functions to create [`SubprocessBridgeTransport`]
//! instances for the memory, knowledge, and gym bridges, wrapped in
//! [`CircuitBreakerTransport`] for fault tolerance.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bridge::BridgeTransport;
use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport};
use crate::bridge_subprocess::SubprocessBridgeTransport;
use crate::error::SimardResult;
use crate::gym_bridge::GymBridge;
use crate::knowledge_bridge::KnowledgeBridge;
use crate::memory_bridge::CognitiveMemoryBridge;

const DEFAULT_BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

fn default_circuit_breaker() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown: Duration::from_secs(30),
    }
}

/// Locate the `python/` directory by walking up from the working directory.
pub fn find_python_dir() -> SimardResult<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join("python").join("bridge_server.py");
        if candidate.exists() {
            return Ok(ancestor.join("python"));
        }
    }
    Err(crate::error::SimardError::BridgeSpawnFailed {
        bridge: "launcher".to_string(),
        reason: "could not find python/ directory with bridge_server.py".to_string(),
    })
}

/// Build PYTHONPATH with ecosystem dependencies.
fn build_python_path() -> String {
    let candidates = [
        "/home/azureuser/src/amplirusty/amplihack-memory-lib/src",
        "/home/azureuser/src/agent-kgpacks",
        "/home/azureuser/src/amplihack/src",
    ];
    let mut paths: Vec<String> = candidates
        .iter()
        .filter(|p| Path::new(p).exists())
        .map(|p| p.to_string())
        .collect();
    if let Ok(existing) = std::env::var("PYTHONPATH") {
        paths.push(existing);
    }
    paths.join(":")
}

fn set_python_path() {
    // SAFETY: We only call this during single-threaded bootstrap, before
    // any bridge subprocesses are spawned.
    unsafe {
        std::env::set_var("PYTHONPATH", build_python_path());
    }
}

fn make_transport(name: &str, script: &Path, extra_args: Vec<String>) -> Box<dyn BridgeTransport> {
    let transport =
        SubprocessBridgeTransport::new(name, script, extra_args, DEFAULT_BRIDGE_TIMEOUT);
    Box::new(CircuitBreakerTransport::new(
        transport,
        default_circuit_breaker(),
    ))
}

/// Check bridge health and return it if healthy, or None with a log message.
fn check_health(name: &str, transport: &dyn BridgeTransport) -> bool {
    match transport.health() {
        Ok(h) if h.healthy => true,
        Ok(_) => {
            eprintln!("[simard] {name} bridge reports unhealthy");
            false
        }
        Err(e) => {
            eprintln!("[simard] {name} bridge health check failed: {e}");
            false
        }
    }
}

/// Launch a cognitive memory bridge backed by a Python subprocess.
pub fn launch_memory_bridge(
    agent_name: &str,
    db_path: &Path,
    python_dir: &Path,
) -> SimardResult<CognitiveMemoryBridge> {
    set_python_path();
    let script = python_dir.join("simard_memory_bridge.py");
    let transport = make_transport(
        "cognitive-memory",
        &script,
        vec![
            "--agent-name".to_string(),
            agent_name.to_string(),
            "--db-path".to_string(),
            db_path.to_string_lossy().to_string(),
        ],
    );
    if !check_health("memory", transport.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "cognitive-memory".to_string(),
            reason: "bridge unhealthy after launch".to_string(),
        });
    }
    Ok(CognitiveMemoryBridge::new(transport))
}

/// Launch a knowledge graph pack bridge.
pub fn launch_knowledge_bridge(python_dir: &Path) -> SimardResult<KnowledgeBridge> {
    set_python_path();
    let script = python_dir.join("simard_knowledge_bridge.py");
    let transport = make_transport("knowledge", &script, vec![]);
    if !check_health("knowledge", transport.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "knowledge".to_string(),
            reason: "bridge unhealthy after launch".to_string(),
        });
    }
    Ok(KnowledgeBridge::new(transport))
}

/// Launch a gym/eval bridge.
pub fn launch_gym_bridge(python_dir: &Path) -> SimardResult<GymBridge> {
    set_python_path();
    let script = python_dir.join("simard_gym_bridge.py");
    let transport = make_transport("gym-eval", &script, vec![]);
    if !check_health("gym", transport.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "gym-eval".to_string(),
            reason: "bridge unhealthy after launch".to_string(),
        });
    }
    Ok(GymBridge::new(transport))
}

/// Launch all bridges, returning None for any that fail (honest degradation).
pub fn launch_all_bridges(
    agent_name: &str,
    state_root: &Path,
) -> (
    Option<CognitiveMemoryBridge>,
    Option<KnowledgeBridge>,
    Option<GymBridge>,
) {
    let python_dir = match find_python_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[simard] bridge launcher: {e}");
            return (None, None, None);
        }
    };

    let db_path = state_root.join("cognitive_memory");
    let memory = launch_memory_bridge(agent_name, &db_path, &python_dir).ok();
    let knowledge = launch_knowledge_bridge(&python_dir).ok();
    let gym = launch_gym_bridge(&python_dir).ok();

    if memory.is_none() {
        eprintln!("[simard] memory bridge unavailable — memories will not persist to Kuzu");
    }
    if knowledge.is_none() {
        eprintln!("[simard] knowledge bridge unavailable — domain knowledge disabled");
    }
    if gym.is_none() {
        eprintln!("[simard] gym bridge unavailable — benchmarks disabled");
    }

    (memory, knowledge, gym)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_python_dir_from_repo_root() {
        let result = find_python_dir();
        assert!(result.is_ok(), "should find python/ from repo root");
        assert!(result.unwrap().join("bridge_server.py").exists());
    }

    #[test]
    fn build_python_path_is_nonempty() {
        let path = build_python_path();
        assert!(!path.is_empty());
    }
}
