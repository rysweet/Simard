use std::path::PathBuf;
use std::time::Duration;

use simard::bridge::{
    BridgeHealth, BridgeRequest, BridgeTransport, new_request_id, unpack_bridge_response,
};
use simard::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
use simard::bridge_subprocess::SubprocessBridgeTransport;
use simard::error::SimardError;

fn echo_bridge_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python/bridge_server.py")
}

fn echo_transport() -> SubprocessBridgeTransport {
    SubprocessBridgeTransport::new(
        "echo-test",
        echo_bridge_script(),
        vec![],
        Duration::from_secs(5),
    )
}

fn health_request() -> BridgeRequest {
    BridgeRequest {
        id: new_request_id(),
        method: "bridge.health".to_string(),
        params: serde_json::json!({}),
    }
}

// --- Outside-in: health check roundtrip ---

#[test]
fn subprocess_bridge_health_check_roundtrips() {
    let transport = echo_transport();
    let health = transport.health().expect("health check should succeed");
    assert_eq!(health.server_name, "echo");
    assert!(health.healthy);
}

// --- Outside-in: echo method roundtrip ---

#[test]
fn subprocess_bridge_echo_roundtrips_params() {
    let transport = echo_transport();
    let request = BridgeRequest {
        id: new_request_id(),
        method: "echo".to_string(),
        params: serde_json::json!({"key": "value", "number": 42}),
    };
    let response = transport.call(request).expect("echo call should succeed");
    let result = response.result.expect("echo should return a result");
    assert_eq!(result["key"], "value");
    assert_eq!(result["number"], 42);
}

// --- Outside-in: unknown method returns method-not-found ---

#[test]
fn subprocess_bridge_unknown_method_returns_error() {
    let transport = echo_transport();
    let request = BridgeRequest {
        id: new_request_id(),
        method: "nonexistent.method".to_string(),
        params: serde_json::json!({}),
    };
    let response = transport
        .call(request)
        .expect("call should return a response even for unknown methods");
    let error = response.error.expect("should have error payload");
    assert_eq!(error.code, -32601);
    assert!(error.message.contains("not registered"));
}

// --- Outside-in: multiple sequential calls reuse the same subprocess ---

#[test]
fn subprocess_bridge_reuses_child_across_calls() {
    let transport = echo_transport();
    for i in 0..5 {
        let request = BridgeRequest {
            id: new_request_id(),
            method: "echo".to_string(),
            params: serde_json::json!({"call": i}),
        };
        let response = transport
            .call(request)
            .expect("sequential call should succeed");
        let result = response.result.expect("should have result");
        assert_eq!(result["call"], i);
    }
}

// --- Outside-in: typed unpack helper ---

#[test]
fn subprocess_bridge_unpack_typed_health_response() {
    let transport = echo_transport();
    let request = BridgeRequest {
        id: new_request_id(),
        method: "bridge.health".to_string(),
        params: serde_json::json!({}),
    };
    let response = transport.call(request).expect("health call should succeed");
    let health: BridgeHealth =
        unpack_bridge_response("echo", "bridge.health", response).expect("unpack should succeed");
    assert_eq!(health.server_name, "echo");
}

// --- Feral: bridge script does not exist ---

#[test]
fn subprocess_bridge_missing_script_fails_with_bridge_error() {
    let transport = SubprocessBridgeTransport::new(
        "missing",
        "/nonexistent/path/bridge.py",
        vec![],
        Duration::from_secs(1),
    );
    let result = transport.call(health_request());
    assert!(result.is_err());
    let error = result.unwrap_err();
    // python3 spawns but fails to open the script, so we get either
    // BridgeSpawnFailed (if python3 itself isn't found) or
    // BridgeTransportError (python3 exits after failing to open the file).
    match error {
        SimardError::BridgeSpawnFailed { bridge, .. } => {
            assert_eq!(bridge, "missing");
        }
        SimardError::BridgeTransportError { bridge, reason } => {
            assert_eq!(bridge, "missing");
            assert!(
                reason.contains("closed stdout") || reason.contains("process exited"),
                "reason: {reason}"
            );
        }
        other => panic!("expected BridgeSpawnFailed or BridgeTransportError, got: {other}"),
    }
}

// --- Feral: bridge script that exits immediately ---

#[test]
fn subprocess_bridge_immediate_exit_returns_transport_error() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exit_immediately.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "import sys; sys.exit(0)\n").unwrap();

    let transport =
        SubprocessBridgeTransport::new("exit-test", &script, vec![], Duration::from_secs(2));
    let result = transport.call(health_request());
    assert!(result.is_err());
    match result.unwrap_err() {
        SimardError::BridgeTransportError { reason, .. } => {
            assert!(
                reason.contains("closed stdout") || reason.contains("process exited"),
                "reason: {reason}"
            );
        }
        other => panic!("expected BridgeTransportError, got: {other}"),
    }

    let _ = std::fs::remove_file(&script);
}

// --- Feral: bridge script that returns malformed JSON ---

#[test]
fn subprocess_bridge_malformed_json_returns_protocol_error() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed_json.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
        &script,
        r#"import sys
for line in sys.stdin:
    sys.stdout.write("not json at all\n")
    sys.stdout.flush()
"#,
    )
    .unwrap();

    let transport =
        SubprocessBridgeTransport::new("malformed", &script, vec![], Duration::from_secs(2));
    let result = transport.call(health_request());
    assert!(result.is_err());
    match result.unwrap_err() {
        SimardError::BridgeProtocolError { reason, .. } => {
            assert!(reason.contains("malformed"), "reason: {reason}");
        }
        other => panic!("expected BridgeProtocolError, got: {other}"),
    }

    let _ = std::fs::remove_file(&script);
}

// --- Circuit breaker: wrapping subprocess transport ---

#[test]
fn circuit_breaker_passes_through_on_healthy_bridge() {
    let inner = echo_transport();
    let cb = CircuitBreakerTransport::with_defaults(inner);
    let health = cb
        .health()
        .expect("health through circuit breaker should succeed");
    assert!(health.healthy);
    assert_eq!(cb.circuit_state(), CircuitState::Closed);
}

// --- Circuit breaker: opens on repeated subprocess failures ---

#[test]
fn circuit_breaker_opens_on_repeated_transport_errors() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/always_exit.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "import sys; sys.exit(1)\n").unwrap();

    let inner = SubprocessBridgeTransport::new("failing", &script, vec![], Duration::from_secs(1));
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_secs(60),
        },
    );

    let _ = cb.call(health_request());
    let _ = cb.call(health_request());
    assert_eq!(cb.circuit_state(), CircuitState::Open);

    let result = cb.call(health_request());
    assert!(result.is_err());
    match result.unwrap_err() {
        SimardError::BridgeCircuitOpen { .. } => {}
        other => panic!("expected BridgeCircuitOpen, got: {other}"),
    }

    let _ = std::fs::remove_file(&script);
}

// --- Descriptor includes bridge name ---

#[test]
fn subprocess_bridge_descriptor_contains_bridge_name() {
    let transport = echo_transport();
    let desc = transport.descriptor();
    assert!(desc.identity.contains("echo-test"));
}

#[test]
fn circuit_breaker_descriptor_wraps_inner() {
    let inner = echo_transport();
    let cb = CircuitBreakerTransport::with_defaults(inner);
    let desc = cb.descriptor();
    assert!(desc.provenance.locator.contains("circuit-breaker"));
}
