//! Gym evaluation bridge connecting Simard to the amplihack-agent-eval suite.
//!
//! [`GymBridge`] wraps a [`BridgeTransport`] to communicate with a Python
//! bridge server that runs progressive test levels (L1-L12) and long-horizon
//! memory evaluations. All results are typed and serializable so they can
//! feed the scoring pipeline in [`crate::gym_scoring`].
//!
//! If the bridge is unavailable or a call fails, errors are surfaced with
//! full context (Pillar 11: honest degradation). Callers can inspect the
//! error to decide whether to continue with degraded results or abort.

use serde::{Deserialize, Serialize};

use crate::bridge::{BridgeRequest, BridgeTransport, new_request_id, unpack_bridge_response};
use crate::error::{SimardError, SimardResult};

const BRIDGE_NAME: &str = "simard-gym-eval";

/// Five scoring dimensions aligned with amplihack-agent-eval's
/// `long_horizon_memory.ALL_DIMENSIONS`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreDimensions {
    pub factual_accuracy: f64,
    pub specificity: f64,
    pub temporal_awareness: f64,
    pub source_attribution: f64,
    pub confidence_calibration: f64,
}

impl ScoreDimensions {
    /// Arithmetic mean of all five dimensions.
    pub fn mean(&self) -> f64 {
        (self.factual_accuracy
            + self.specificity
            + self.temporal_awareness
            + self.source_attribution
            + self.confidence_calibration)
            / 5.0
    }
}

/// Metadata describing a single evaluation scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GymScenario {
    pub id: String,
    pub name: String,
    pub description: String,
    pub level: String,
    pub question_count: usize,
    pub article_count: usize,
}

/// Result of running a single evaluation scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GymScenarioResult {
    pub scenario_id: String,
    pub success: bool,
    pub score: f64,
    pub dimensions: ScoreDimensions,
    pub question_count: usize,
    pub questions_answered: usize,
    /// Present when the scenario failed. Provides context per Pillar 11.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Sources that degraded during the run.
    #[serde(default)]
    pub degraded_sources: Vec<String>,
}

/// Result of running an entire evaluation suite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GymSuiteResult {
    pub suite_id: String,
    pub success: bool,
    pub overall_score: f64,
    pub dimensions: ScoreDimensions,
    pub scenario_results: Vec<GymScenarioResult>,
    pub scenarios_passed: usize,
    pub scenarios_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub degraded_sources: Vec<String>,
}

/// Bridge client for gym evaluations.
///
/// Wraps a [`BridgeTransport`] to call the Python gym bridge server.
/// Each method maps to a single bridge RPC call.
pub struct GymBridge {
    transport: Box<dyn BridgeTransport>,
}

impl GymBridge {
    pub fn new(transport: Box<dyn BridgeTransport>) -> Self {
        Self { transport }
    }

    /// List all scenarios available from the evaluation server.
    pub fn list_scenarios(&self) -> SimardResult<Vec<GymScenario>> {
        let request = BridgeRequest {
            id: new_request_id(),
            method: "gym.list_scenarios".to_string(),
            params: serde_json::json!({}),
        };
        let response = self
            .transport
            .call(request)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: BRIDGE_NAME.to_string(),
                method: "gym.list_scenarios".to_string(),
                reason: format!("transport error: {e}"),
            })?;
        unpack_bridge_response(BRIDGE_NAME, "gym.list_scenarios", response)
    }

    /// Run a single evaluation scenario by id.
    pub fn run_scenario(&self, scenario_id: &str) -> SimardResult<GymScenarioResult> {
        let request = BridgeRequest {
            id: new_request_id(),
            method: "gym.run_scenario".to_string(),
            params: serde_json::json!({"scenario_id": scenario_id}),
        };
        let response = self
            .transport
            .call(request)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: BRIDGE_NAME.to_string(),
                method: "gym.run_scenario".to_string(),
                reason: format!("transport error: {e}"),
            })?;
        unpack_bridge_response(BRIDGE_NAME, "gym.run_scenario", response)
    }

    /// Run all scenarios in a suite by suite id.
    pub fn run_suite(&self, suite_id: &str) -> SimardResult<GymSuiteResult> {
        let request = BridgeRequest {
            id: new_request_id(),
            method: "gym.run_suite".to_string(),
            params: serde_json::json!({"suite_id": suite_id}),
        };
        let response = self
            .transport
            .call(request)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: BRIDGE_NAME.to_string(),
                method: "gym.run_suite".to_string(),
                reason: format!("transport error: {e}"),
            })?;
        unpack_bridge_response(BRIDGE_NAME, "gym.run_suite", response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;

    fn fixed_result_transport(result: serde_json::Value) -> InMemoryBridgeTransport {
        InMemoryBridgeTransport::new("gym-test", move |_method, _params| Ok(result.clone()))
    }

    fn fixed_error_transport(code: i32, message: &str) -> InMemoryBridgeTransport {
        let msg = message.to_string();
        InMemoryBridgeTransport::new("gym-test", move |_method, _params| {
            Err(BridgeErrorPayload {
                code,
                message: msg.clone(),
            })
        })
    }

    #[test]
    fn score_dimensions_mean_is_average_of_five() {
        let dims = ScoreDimensions {
            factual_accuracy: 1.0,
            specificity: 0.8,
            temporal_awareness: 0.6,
            source_attribution: 0.4,
            confidence_calibration: 0.2,
        };
        let mean = dims.mean();
        assert!((mean - 0.6).abs() < 1e-9);
    }

    #[test]
    fn list_scenarios_deserializes_response() {
        let scenarios = serde_json::json!([
            {
                "id": "L1",
                "name": "Single source direct recall",
                "description": "Baseline recall test",
                "level": "L1",
                "question_count": 5,
                "article_count": 1
            }
        ]);
        let transport = fixed_result_transport(scenarios);
        let bridge = GymBridge::new(Box::new(transport));
        let result = bridge.list_scenarios().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "L1");
    }

    #[test]
    fn run_scenario_deserializes_success() {
        let result_json = serde_json::json!({
            "scenario_id": "L1",
            "success": true,
            "score": 0.85,
            "dimensions": {
                "factual_accuracy": 0.9,
                "specificity": 0.8,
                "temporal_awareness": 0.85,
                "source_attribution": 0.8,
                "confidence_calibration": 0.9
            },
            "question_count": 5,
            "questions_answered": 5,
            "degraded_sources": []
        });
        let transport = fixed_result_transport(result_json);
        let bridge = GymBridge::new(Box::new(transport));
        let result = bridge.run_scenario("L1").unwrap();
        assert!(result.success);
        assert_eq!(result.scenario_id, "L1");
        assert!((result.score - 0.85).abs() < 1e-9);
    }

    #[test]
    fn run_scenario_deserializes_failure_with_error() {
        let result_json = serde_json::json!({
            "scenario_id": "L12",
            "success": false,
            "score": 0.0,
            "dimensions": {
                "factual_accuracy": 0.0,
                "specificity": 0.0,
                "temporal_awareness": 0.0,
                "source_attribution": 0.0,
                "confidence_calibration": 0.0
            },
            "question_count": 10,
            "questions_answered": 0,
            "error_message": "Learning phase timed out after 600 seconds",
            "degraded_sources": ["progressive_test_suite"]
        });
        let transport = fixed_result_transport(result_json);
        let bridge = GymBridge::new(Box::new(transport));
        let result = bridge.run_scenario("L12").unwrap();
        assert!(!result.success);
        assert_eq!(
            result.error_message.as_deref(),
            Some("Learning phase timed out after 600 seconds")
        );
        assert_eq!(result.degraded_sources, vec!["progressive_test_suite"]);
    }

    #[test]
    fn run_suite_deserializes_result() {
        let suite_json = serde_json::json!({
            "suite_id": "progressive",
            "success": true,
            "overall_score": 0.72,
            "dimensions": {
                "factual_accuracy": 0.8,
                "specificity": 0.7,
                "temporal_awareness": 0.65,
                "source_attribution": 0.7,
                "confidence_calibration": 0.75
            },
            "scenario_results": [],
            "scenarios_passed": 6,
            "scenarios_total": 6,
            "degraded_sources": []
        });
        let transport = fixed_result_transport(suite_json);
        let bridge = GymBridge::new(Box::new(transport));
        let result = bridge.run_suite("progressive").unwrap();
        assert!(result.success);
        assert_eq!(result.scenarios_passed, 6);
    }

    #[test]
    fn bridge_error_surfaces_with_context() {
        let transport = fixed_error_transport(-32603, "eval server crashed");
        let bridge = GymBridge::new(Box::new(transport));
        let err = bridge.list_scenarios().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("simard-gym-eval"),
            "error should name the bridge: {msg}"
        );
        assert!(
            msg.contains("eval server crashed"),
            "error should contain reason: {msg}"
        );
    }
}
