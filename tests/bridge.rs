//! Native bridge transport + circuit-breaker integration tests (issue #3181).
//!
//! Simard's bridges are pure Rust: the production transport is
//! [`NativeBridgeTransport`] and tests use [`InMemoryBridgeTransport`]. There is
//! no Python subprocess transport and no `.py` fixtures — this file replaces the
//! former subprocess-based `tests/bridge.rs`, preserving the circuit-breaker
//! coverage that guarded repeated transport failures.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use simard::bridge::{
    BRIDGE_ERROR_INTERNAL, BRIDGE_ERROR_METHOD_NOT_FOUND, BRIDGE_ERROR_TRANSPORT,
    BridgeErrorPayload, BridgeHealth, BridgeRequest, BridgeTransport, new_request_id,
    unpack_bridge_response,
};
use simard::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::error::SimardError;

fn health_request() -> BridgeRequest {
    BridgeRequest {
        id: new_request_id(),
        method: "bridge.health".to_string(),
        params: serde_json::json!({}),
    }
}

/// An in-memory transport that answers `bridge.health` with a well-formed
/// [`BridgeHealth`] and echoes any other method's params.
fn healthy_transport(name: &'static str) -> InMemoryBridgeTransport {
    InMemoryBridgeTransport::new(name, move |method, params| {
        if method == "bridge.health" {
            Ok(serde_json::json!({ "server_name": name, "healthy": true }))
        } else {
            Ok(params.clone())
        }
    })
}

// --- Transport: health check roundtrip (native, no subprocess) -------------

#[test]
fn native_bridge_health_check_roundtrips() {
    let transport = healthy_transport("echo");
    let health = transport.health().expect("health check should succeed");
    assert_eq!(health.server_name, "echo");
    assert!(health.healthy);
}

// --- Transport: typed unpack helper ----------------------------------------

#[test]
fn native_bridge_unpack_typed_health_response() {
    let transport = healthy_transport("echo");
    let response = transport
        .call(health_request())
        .expect("health call should succeed");
    let health: BridgeHealth =
        unpack_bridge_response("echo", "bridge.health", response).expect("unpack should succeed");
    assert_eq!(health.server_name, "echo");
}

// --- Transport: unknown method surfaces a method-not-found error ------------

#[test]
fn native_bridge_unknown_method_returns_error() {
    let transport = InMemoryBridgeTransport::new("strict", |method, _params| {
        Err(BridgeErrorPayload {
            code: BRIDGE_ERROR_METHOD_NOT_FOUND,
            message: format!("method '{method}' is not registered"),
        })
    });
    let request = BridgeRequest {
        id: new_request_id(),
        method: "nonexistent.method".to_string(),
        params: serde_json::json!({}),
    };
    let response = transport
        .call(request)
        .expect("call should return a response even for unknown methods");
    let error = response.error.expect("should have error payload");
    assert_eq!(error.code, BRIDGE_ERROR_METHOD_NOT_FOUND);
    assert!(error.message.contains("not registered"));
}

// --- Circuit breaker: pass-through on a healthy transport -------------------

#[test]
fn circuit_breaker_passes_through_on_healthy_bridge() {
    let inner = healthy_transport("echo");
    let cb = CircuitBreakerTransport::with_defaults(inner);
    let health = cb
        .health()
        .expect("health through circuit breaker should succeed");
    assert!(health.healthy);
    assert_eq!(cb.circuit_state(), CircuitState::Closed);
}

// --- Circuit breaker: opens after repeated transport-level failures ---------

#[test]
fn circuit_breaker_opens_on_repeated_transport_errors() {
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let inner = InMemoryBridgeTransport::new("failing", move |_method, _params| {
        counter.fetch_add(1, Ordering::SeqCst);
        Err(BridgeErrorPayload {
            code: BRIDGE_ERROR_TRANSPORT,
            message: "simulated transport failure".to_string(),
        })
    });
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_secs(60),
        },
    );

    // Two transport failures trip the breaker.
    let _ = cb.call(health_request());
    let _ = cb.call(health_request());
    assert_eq!(cb.circuit_state(), CircuitState::Open);

    // While open, calls are rejected immediately without reaching the inner
    // transport (call count stays at 2).
    let result = cb.call(health_request());
    match result {
        Err(SimardError::BridgeCircuitOpen { .. }) => {}
        other => panic!("expected BridgeCircuitOpen, got: {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "open circuit must not forward calls to the inner transport"
    );
}

// --- Circuit breaker: application errors do NOT trip the breaker ------------

#[test]
fn circuit_breaker_ignores_application_errors() {
    let inner = InMemoryBridgeTransport::new("app-err", |_method, _params| {
        Err(BridgeErrorPayload {
            code: BRIDGE_ERROR_INTERNAL,
            message: "application error".to_string(),
        })
    });
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_secs(1),
        },
    );

    for _ in 0..5 {
        let _ = cb.call(health_request());
    }
    assert_eq!(
        cb.circuit_state(),
        CircuitState::Closed,
        "application-level errors must not open the circuit"
    );
}

// --- Descriptors -----------------------------------------------------------

#[test]
fn native_bridge_descriptor_contains_bridge_name() {
    let transport = healthy_transport("echo-test");
    let desc = transport.descriptor();
    assert!(desc.identity.contains("echo-test"));
}

#[test]
fn circuit_breaker_descriptor_wraps_inner() {
    let inner = healthy_transport("echo");
    let cb = CircuitBreakerTransport::with_defaults(inner);
    let desc = cb.descriptor();
    assert!(desc.provenance.locator.contains("circuit-breaker"));
}
