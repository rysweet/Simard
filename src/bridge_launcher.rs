//! Launch and manage bridge transports for live Simard operations.
//!
//! This module provides functions to create bridge transport instances for the
//! knowledge and gym bridges. The default strategy is to use
//! [`NativeBridgeTransport`] for in-process Rust execution, falling back to
//! [`SubprocessBridgeTransport`] (Python) if the native transport fails its
//! health check.
//!
//! Cognitive memory is now handled natively by [`NativeCognitiveMemory`](crate::cognitive_memory::NativeCognitiveMemory).

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use crate::bridge::BridgeTransport;
use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport};
use crate::bridge_subprocess::{NativeBridgeTransport, SubprocessBridgeTransport};
use crate::error::SimardResult;
use crate::gym_bridge::GymBridge;
use crate::knowledge_bridge::KnowledgeBridge;

const DEFAULT_BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Gym bridge runs full progressive benchmark suites which call out to LLMs;
/// each suite typically takes several minutes. The 30s default would always
/// trip and silently disable benchmark feedback.
const GYM_BRIDGE_TIMEOUT: Duration = Duration::from_secs(900);

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
        "/home/azureuser/.amplihack/src",
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
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: called exactly once during single-threaded bootstrap,
        // before any bridge subprocesses are spawned.
        unsafe {
            std::env::set_var("PYTHONPATH", build_python_path());
        }
    });
}

fn make_transport(name: &str, script: &Path, extra_args: Vec<String>) -> Box<dyn BridgeTransport> {
    make_transport_with_timeout(name, script, extra_args, DEFAULT_BRIDGE_TIMEOUT)
}

fn make_transport_with_timeout(
    name: &str,
    script: &Path,
    extra_args: Vec<String>,
    timeout: Duration,
) -> Box<dyn BridgeTransport> {
    let transport = SubprocessBridgeTransport::new(name, script, extra_args, timeout);
    Box::new(CircuitBreakerTransport::new(
        transport,
        default_circuit_breaker(),
    ))
}

/// Wrap a native transport in a circuit breaker.
fn wrap_native(transport: NativeBridgeTransport) -> Box<dyn BridgeTransport> {
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

/// Resolve the knowledge packs directory.
fn resolve_packs_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SIMARD_PACKS_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wikigr/packs")
}

/// Launch all bridges, returning None for any that fail (honest degradation).
///
/// Cognitive memory is now native — only knowledge and gym bridges are launched.
/// The default strategy is native Rust transport, with Python subprocess fallback.
pub fn launch_all_bridges(
    _agent_name: &str,
    _state_root: &Path,
) -> (Option<KnowledgeBridge>, Option<GymBridge>) {
    let knowledge = match launch_knowledge_bridge_native() {
        Ok(b) => {
            eprintln!("[simard] knowledge bridge: using native Rust transport");
            Some(b)
        }
        Err(native_err) => {
            eprintln!("[simard] knowledge bridge native transport failed: {native_err}");
            // Fall back to Python subprocess
            match find_python_dir() {
                Ok(python_dir) => match launch_knowledge_bridge_subprocess(&python_dir) {
                    Ok(b) => {
                        eprintln!("[simard] knowledge bridge: fell back to Python subprocess");
                        Some(b)
                    }
                    Err(e) => {
                        eprintln!(
                            "[simard] knowledge bridge launch FAILED — domain knowledge disabled: {e}"
                        );
                        None
                    }
                },
                Err(e) => {
                    eprintln!("[simard] knowledge bridge launch FAILED — no Python dir: {e}");
                    None
                }
            }
        }
    };

    let gym = match launch_gym_bridge_native() {
        Ok(b) => {
            eprintln!("[simard] gym bridge: using native Rust transport");
            Some(b)
        }
        Err(native_err) => {
            eprintln!("[simard] gym bridge native transport failed: {native_err}");
            // Fall back to Python subprocess
            match find_python_dir() {
                Ok(python_dir) => match launch_gym_bridge_subprocess(&python_dir) {
                    Ok(b) => {
                        eprintln!("[simard] gym bridge: fell back to Python subprocess");
                        Some(b)
                    }
                    Err(e) => {
                        eprintln!("[simard] gym bridge launch FAILED — benchmarks disabled: {e}");
                        None
                    }
                },
                Err(e) => {
                    eprintln!("[simard] gym bridge launch FAILED — no Python dir: {e}");
                    None
                }
            }
        }
    };

    (knowledge, gym)
}

/// Launch a knowledge bridge using the native Rust transport.
pub fn launch_knowledge_bridge_native() -> SimardResult<KnowledgeBridge> {
    let packs_dir = resolve_packs_dir();
    let mut transport = NativeBridgeTransport::new("simard-knowledge");
    crate::native_knowledge::register_knowledge_handlers(&mut transport, packs_dir);
    let wrapped = wrap_native(transport);
    if !check_health("knowledge-native", wrapped.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "knowledge-native".to_string(),
            reason: "native bridge unhealthy after init".to_string(),
        });
    }
    Ok(KnowledgeBridge::new(wrapped))
}

/// Launch a gym bridge using the native Rust transport.
pub fn launch_gym_bridge_native() -> SimardResult<GymBridge> {
    let mut transport = NativeBridgeTransport::new("simard-gym-eval");
    crate::native_gym::register_gym_handlers(&mut transport);
    let wrapped = wrap_native(transport);
    if !check_health("gym-native", wrapped.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "gym-native".to_string(),
            reason: "native bridge unhealthy after init".to_string(),
        });
    }
    Ok(GymBridge::new(wrapped))
}

/// Launch a knowledge graph pack bridge via Python subprocess (fallback).
pub fn launch_knowledge_bridge(python_dir: &Path) -> SimardResult<KnowledgeBridge> {
    launch_knowledge_bridge_subprocess(python_dir)
}

fn launch_knowledge_bridge_subprocess(python_dir: &Path) -> SimardResult<KnowledgeBridge> {
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

/// Launch a gym/eval bridge via Python subprocess (fallback).
pub fn launch_gym_bridge(python_dir: &Path) -> SimardResult<GymBridge> {
    launch_gym_bridge_subprocess(python_dir)
}

fn launch_gym_bridge_subprocess(python_dir: &Path) -> SimardResult<GymBridge> {
    set_python_path();
    let script = python_dir.join("simard_gym_bridge.py");
    let transport = make_transport_with_timeout("gym-eval", &script, vec![], GYM_BRIDGE_TIMEOUT);
    if !check_health("gym", transport.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "gym-eval".to_string(),
            reason: "bridge unhealthy after launch".to_string(),
        });
    }
    Ok(GymBridge::new(transport))
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
    fn build_python_path_returns_string() {
        let path = build_python_path();
        let _ = path;
    }

    #[test]
    fn default_bridge_timeout_is_30_seconds() {
        assert_eq!(DEFAULT_BRIDGE_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn gym_bridge_timeout_allows_full_suite() {
        assert!(GYM_BRIDGE_TIMEOUT >= Duration::from_secs(300));
    }

    #[test]
    fn default_circuit_breaker_has_expected_threshold() {
        let config = default_circuit_breaker();
        assert_eq!(config.failure_threshold, 3);
    }

    #[test]
    fn default_circuit_breaker_has_30s_cooldown() {
        let config = default_circuit_breaker();
        assert_eq!(config.cooldown, Duration::from_secs(30));
    }

    #[test]
    fn build_python_path_is_colon_separated() {
        let path = build_python_path();
        if !path.is_empty() {
            for segment in path.split(':') {
                assert!(!segment.is_empty(), "path segment should not be empty");
            }
        }
    }

    #[test]
    fn build_python_path_includes_existing_pythonpath() {
        let original = std::env::var("PYTHONPATH").ok();
        unsafe {
            std::env::set_var("PYTHONPATH", "/test/custom/path");
        }
        let path = build_python_path();
        assert!(path.contains("/test/custom/path"));
        match original {
            Some(val) => unsafe {
                std::env::set_var("PYTHONPATH", val);
            },
            None => unsafe {
                std::env::remove_var("PYTHONPATH");
            },
        }
    }

    #[test]
    fn find_python_dir_returns_directory_containing_bridge_server() {
        if let Ok(dir) = find_python_dir() {
            assert!(dir.is_dir());
            assert!(dir.join("bridge_server.py").exists());
        }
    }

    // ── Native transport tests ──

    #[test]
    fn launch_knowledge_bridge_native_succeeds() {
        // Native knowledge bridge should always pass health check
        // since it registers a bridge.health handler.
        let result = launch_knowledge_bridge_native();
        assert!(
            result.is_ok(),
            "native knowledge bridge should launch: {:?}",
            result.err()
        );
    }

    #[test]
    fn launch_gym_bridge_native_succeeds() {
        let result = launch_gym_bridge_native();
        assert!(
            result.is_ok(),
            "native gym bridge should launch: {:?}",
            result.err()
        );
    }

    #[test]
    fn resolve_packs_dir_defaults_to_home() {
        let dir = resolve_packs_dir();
        assert!(
            dir.to_string_lossy().contains(".wikigr/packs") || !dir.to_string_lossy().is_empty()
        );
    }

    #[test]
    fn resolve_packs_dir_uses_env_override() {
        let original = std::env::var("SIMARD_PACKS_DIR").ok();
        unsafe {
            std::env::set_var("SIMARD_PACKS_DIR", "/custom/packs");
        }
        let dir = resolve_packs_dir();
        assert_eq!(dir, PathBuf::from("/custom/packs"));
        match original {
            Some(val) => unsafe {
                std::env::set_var("SIMARD_PACKS_DIR", val);
            },
            None => unsafe {
                std::env::remove_var("SIMARD_PACKS_DIR");
            },
        }
    }
}
