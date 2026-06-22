//! Launch and manage bridge transports for live Simard operations.
//!
//! This module provides functions to create bridge transport instances for the
//! knowledge and gym bridges using [`NativeBridgeTransport`] for in-process
//! Rust execution.
//!
//! Cognitive memory is handled by the library-backed
//! [`LibraryCognitiveMemory`](crate::cognitive_memory::LibraryCognitiveMemory).

use std::path::PathBuf;
use std::time::Duration;

use crate::bridge::BridgeTransport;
use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport};
use crate::bridge_subprocess::NativeBridgeTransport;
use crate::error::SimardResult;
use crate::gym_bridge::GymBridge;
use crate::knowledge_bridge::KnowledgeBridge;

fn default_circuit_breaker() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown: Duration::from_secs(30),
    }
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
/// Both use the native Rust transport exclusively.
pub fn launch_all_bridges(
    _agent_name: &str,
    _state_root: &std::path::Path,
) -> (Option<KnowledgeBridge>, Option<GymBridge>) {
    let knowledge = match launch_knowledge_bridge_native() {
        Ok(b) => {
            eprintln!("[simard] knowledge bridge: using native Rust transport");
            Some(b)
        }
        Err(e) => {
            eprintln!("[simard] knowledge bridge launch FAILED — domain knowledge disabled: {e}");
            None
        }
    };

    let gym = match launch_gym_bridge_native() {
        Ok(b) => {
            eprintln!("[simard] gym bridge: using native Rust transport");
            Some(b)
        }
        Err(e) => {
            eprintln!("[simard] gym bridge launch FAILED — benchmarks disabled: {e}");
            None
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
    crate::gym_runner_bridge::register_gym_handlers(&mut transport);
    let wrapped = wrap_native(transport);
    if !check_health("gym-native", wrapped.as_ref()) {
        return Err(crate::error::SimardError::BridgeSpawnFailed {
            bridge: "gym-native".to_string(),
            reason: "native bridge unhealthy after init".to_string(),
        });
    }
    Ok(GymBridge::new(wrapped))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── Native transport tests ──

    #[test]
    fn launch_knowledge_bridge_native_succeeds() {
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
