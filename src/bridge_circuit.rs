use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::bridge::{BridgeRequest, BridgeResponse, BridgeTransport};
use crate::error::{SimardError, SimardResult};
use crate::metadata::BackendDescriptor;

/// Circuit breaker states following the standard pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CircuitState {
    /// Normal operation — calls pass through to the inner transport.
    Closed,
    /// Calls are rejected immediately after repeated failures.
    Open,
    /// One probe call is allowed to test if the transport has recovered.
    HalfOpen,
}

/// A bridge transport wrapper that implements the circuit breaker pattern.
///
/// When the inner transport fails repeatedly, the circuit opens and rejects
/// all calls immediately until a cooldown period passes. After cooldown,
/// one probe call is allowed through; if it succeeds the circuit closes,
/// otherwise it reopens.
pub struct CircuitBreakerTransport<T: BridgeTransport> {
    inner: T,
    config: CircuitBreakerConfig,
    state: Mutex<CircuitBreakerState>,
}

/// Configuration for the circuit breaker.
#[derive(Clone, Debug)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before the circuit opens.
    pub failure_threshold: u32,
    /// How long to wait before allowing a probe call.
    pub cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown: Duration::from_secs(30),
        }
    }
}

struct CircuitBreakerState {
    circuit: CircuitState,
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
}

impl<T: BridgeTransport> CircuitBreakerTransport<T> {
    pub fn new(inner: T, config: CircuitBreakerConfig) -> Self {
        Self {
            inner,
            config,
            state: Mutex::new(CircuitBreakerState {
                circuit: CircuitState::Closed,
                consecutive_failures: 0,
                last_failure_at: None,
            }),
        }
    }

    pub fn with_defaults(inner: T) -> Self {
        Self::new(inner, CircuitBreakerConfig::default())
    }

    /// Return the current circuit state for reflection and testing.
    pub fn circuit_state(&self) -> CircuitState {
        self.state
            .lock()
            .map(|s| s.circuit)
            .unwrap_or(CircuitState::Open)
    }

    fn record_success(state: &mut CircuitBreakerState) {
        state.consecutive_failures = 0;
        state.circuit = CircuitState::Closed;
    }

    fn record_failure(&self, state: &mut CircuitBreakerState) {
        state.consecutive_failures += 1;
        state.last_failure_at = Some(Instant::now());
        if state.consecutive_failures >= self.config.failure_threshold {
            state.circuit = CircuitState::Open;
        }
    }

    fn should_allow_call(&self, state: &mut CircuitBreakerState) -> bool {
        match state.circuit {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(last_failure) = state.last_failure_at
                    && last_failure.elapsed() >= self.config.cooldown
                {
                    state.circuit = CircuitState::HalfOpen;
                    return true;
                }
                false
            }
        }
    }
}

impl<T: BridgeTransport> BridgeTransport for CircuitBreakerTransport<T> {
    fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SimardError::StoragePoisoned {
                    store: "bridge-circuit".to_string(),
                })?;
            if !self.should_allow_call(&mut state) {
                return Err(SimardError::BridgeCircuitOpen {
                    bridge: self.inner.descriptor().identity.clone(),
                });
            }
        }

        match self.inner.call(request) {
            Ok(response) => {
                let is_transport_error = response
                    .error
                    .as_ref()
                    .is_some_and(|e| e.code == crate::bridge::BRIDGE_ERROR_TRANSPORT);
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| SimardError::StoragePoisoned {
                        store: "bridge-circuit".to_string(),
                    })?;
                if is_transport_error {
                    self.record_failure(&mut state);
                } else {
                    Self::record_success(&mut state);
                }
                Ok(response)
            }
            Err(error) => {
                if let Ok(mut state) = self.state.lock() {
                    self.record_failure(&mut state);
                }
                Err(error)
            }
        }
    }

    fn descriptor(&self) -> BackendDescriptor {
        let inner_desc = self.inner.descriptor();
        BackendDescriptor::for_runtime_type::<Self>(
            inner_desc.identity,
            format!("circuit-breaker<{}>", inner_desc.provenance.locator),
            inner_desc.freshness,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        BRIDGE_ERROR_INTERNAL, BRIDGE_ERROR_TRANSPORT, BridgeErrorPayload, BridgeHealth,
        new_request_id, unpack_bridge_response,
    };
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn health_request() -> BridgeRequest {
        BridgeRequest {
            id: new_request_id(),
            method: "bridge.health".to_string(),
            params: serde_json::json!({"server_name": "test", "healthy": true}),
        }
    }

    #[test]
    fn closed_circuit_passes_calls_through() {
        let inner = InMemoryBridgeTransport::echo("test");
        let cb = CircuitBreakerTransport::with_defaults(inner);
        let response = cb.call(health_request()).unwrap();
        let health: BridgeHealth =
            unpack_bridge_response("test", "bridge.health", response).unwrap();
        assert!(health.healthy);
        assert_eq!(cb.circuit_state(), CircuitState::Closed);
    }

    #[test]
    fn circuit_opens_after_threshold_failures() {
        let call_count = std::sync::Arc::new(AtomicU32::new(0));
        let counter = call_count.clone();
        let inner = InMemoryBridgeTransport::new("fail", move |_method, _params| {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(BridgeErrorPayload {
                code: BRIDGE_ERROR_TRANSPORT,
                message: "simulated transport failure".to_string(),
            })
        });
        let cb = CircuitBreakerTransport::new(
            inner,
            CircuitBreakerConfig {
                failure_threshold: 3,
                cooldown: Duration::from_secs(60),
            },
        );

        for _ in 0..3 {
            let _ = cb.call(health_request());
        }
        assert_eq!(cb.circuit_state(), CircuitState::Open);

        let result = cb.call(health_request());
        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn application_errors_do_not_trip_circuit() {
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
        assert_eq!(cb.circuit_state(), CircuitState::Closed);
    }

    #[test]
    fn circuit_transitions_to_half_open_after_cooldown() {
        let inner = InMemoryBridgeTransport::new("fail-then-ok", |_method, _params| {
            Err(BridgeErrorPayload {
                code: BRIDGE_ERROR_TRANSPORT,
                message: "down".to_string(),
            })
        });
        let cb = CircuitBreakerTransport::new(
            inner,
            CircuitBreakerConfig {
                failure_threshold: 1,
                cooldown: Duration::from_millis(1),
            },
        );

        let _ = cb.call(health_request());
        assert_eq!(cb.circuit_state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(10));

        let _ = cb.call(health_request());
        assert!(
            cb.circuit_state() == CircuitState::Open,
            "probe failed so circuit should reopen"
        );
    }

    #[test]
    fn successful_probe_closes_circuit() {
        let call_count = std::sync::Arc::new(AtomicU32::new(0));
        let counter = call_count.clone();
        let inner = InMemoryBridgeTransport::new("recover", move |_method, params| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(BridgeErrorPayload {
                    code: BRIDGE_ERROR_TRANSPORT,
                    message: "down".to_string(),
                })
            } else {
                Ok(params.clone())
            }
        });
        let cb = CircuitBreakerTransport::new(
            inner,
            CircuitBreakerConfig {
                failure_threshold: 2,
                cooldown: Duration::from_millis(1),
            },
        );

        let _ = cb.call(health_request());
        let _ = cb.call(health_request());
        assert_eq!(cb.circuit_state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(10));

        let response = cb.call(health_request()).unwrap();
        assert!(response.result.is_some());
        assert_eq!(cb.circuit_state(), CircuitState::Closed);
    }
}
