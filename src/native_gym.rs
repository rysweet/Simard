//! Native Rust implementation of the gym evaluation bridge.
//!
//! Replaces `python/simard_gym_bridge.py` with in-process Rust logic.
//! Scenario listing works natively. Running scenarios and suites operates
//! in degraded mode, returning structured results indicating that the native
//! evaluator does not yet include the full progressive test suite.
//!
//! The `SIMARD_SKIP_GYM=1` workaround is preserved.

use std::sync::Arc;

use serde_json::Value;

use crate::bridge::BridgeErrorPayload;
use crate::bridge_subprocess::native::NativeBridgeTransport;

const ERROR_INTERNAL: i32 = -32603;

/// Built-in scenario definitions matching the Python ALL_LEVELS.
const BUILTIN_SCENARIOS: &[(&str, &str, &str, &str, usize, usize)] = &[
    (
        "L1",
        "Single source direct recall",
        "Baseline recall from a single article",
        "L1",
        5,
        1,
    ),
    (
        "L2",
        "Multi-source synthesis",
        "Combine information from multiple sources",
        "L2",
        5,
        3,
    ),
    (
        "L3",
        "Temporal reasoning",
        "Track information changes over time",
        "L3",
        5,
        2,
    ),
    (
        "L4",
        "Contradictory sources",
        "Handle conflicting information",
        "L4",
        5,
        2,
    ),
    (
        "L5",
        "Implicit knowledge",
        "Derive answers from implicit context",
        "L5",
        5,
        3,
    ),
    (
        "L6",
        "Multi-hop reasoning",
        "Chain multiple facts for complex queries",
        "L6",
        5,
        4,
    ),
    (
        "L7",
        "Source attribution",
        "Correctly attribute information to sources",
        "L7",
        5,
        3,
    ),
    (
        "L8",
        "Confidence calibration",
        "Appropriately express uncertainty",
        "L8",
        5,
        3,
    ),
    (
        "L9",
        "Knowledge boundaries",
        "Recognize limits of available knowledge",
        "L9",
        5,
        2,
    ),
    (
        "L10",
        "Context window stress",
        "Handle large volumes of context",
        "L10",
        10,
        5,
    ),
    (
        "L11",
        "Adversarial queries",
        "Handle misleading or trick questions",
        "L11",
        5,
        3,
    ),
    (
        "L12",
        "Full integration",
        "End-to-end evaluation across all dimensions",
        "L12",
        15,
        5,
    ),
];

fn zero_dims() -> Value {
    serde_json::json!({
        "factual_accuracy": 0.0,
        "specificity": 0.0,
        "temporal_awareness": 0.0,
        "source_attribution": 0.0,
        "confidence_calibration": 0.0,
    })
}

fn fail_result(scenario_id: &str, msg: &str, source: &str) -> Value {
    serde_json::json!({
        "scenario_id": scenario_id,
        "success": false,
        "score": 0.0,
        "dimensions": zero_dims(),
        "question_count": 0,
        "questions_answered": 0,
        "error_message": msg,
        "degraded_sources": if source.is_empty() { vec![] } else { vec![source.to_string()] },
    })
}

/// Register all gym bridge method handlers on a NativeBridgeTransport.
pub fn register_gym_handlers(transport: &mut NativeBridgeTransport) {
    // gym.list_scenarios
    transport.register(
        "gym.list_scenarios",
        Arc::new(|_params: &Value| {
            let mut scenarios: Vec<Value> = BUILTIN_SCENARIOS
                .iter()
                .map(|(id, name, desc, level, qcount, acount)| {
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "description": desc,
                        "level": level,
                        "question_count": qcount,
                        "article_count": acount,
                    })
                })
                .collect();

            // Add long-horizon scenario
            scenarios.push(serde_json::json!({
                "id": "long-horizon-memory",
                "name": "Long-horizon memory stress test",
                "description": "1000-turn dialogue testing memory at scale",
                "level": "long-horizon",
                "question_count": 0,
                "article_count": 0,
            }));

            Ok(Value::Array(scenarios))
        }),
    );

    // gym.run_scenario
    transport.register(
        "gym.run_scenario",
        Arc::new(|params: &Value| {
            let sid = params
                .get("scenario_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if sid.is_empty() {
                return Err(BridgeErrorPayload {
                    code: ERROR_INTERNAL,
                    message: "scenario_id is required".to_string(),
                });
            }

            // Path traversal guard
            if sid.contains('/') || sid.contains('\\') || sid.contains("..") {
                return Ok(fail_result(
                    sid,
                    &format!("scenario_id contains illegal path characters: '{sid}'"),
                    "",
                ));
            }

            // Check SIMARD_SKIP_GYM
            if std::env::var("SIMARD_SKIP_GYM").as_deref() == Ok("1") {
                return Ok(serde_json::json!({
                    "scenario_id": sid,
                    "success": true,
                    "score": 0.0,
                    "dimensions": zero_dims(),
                    "question_count": 0,
                    "questions_answered": 0,
                    "error_message": null,
                    "degraded_sources": ["SIMARD_SKIP_GYM=1 workaround"],
                }));
            }

            // Verify scenario exists
            let known = BUILTIN_SCENARIOS.iter().any(|(id, ..)| *id == sid)
                || sid == "long-horizon-memory";

            if !known {
                return Ok(fail_result(sid, &format!("scenario '{sid}' not found"), ""));
            }

            // Return degraded result — full evaluation requires the Python
            // progressive test suite which is not yet ported to Rust.
            Ok(fail_result(
                sid,
                "native Rust evaluator: scenario execution not yet implemented; use Python fallback bridge for full evaluation",
                "native_gym_bridge",
            ))
        }),
    );

    // gym.run_suite
    transport.register(
        "gym.run_suite",
        Arc::new(|params: &Value| {
            let suite_id = params
                .get("suite_id")
                .and_then(|v| v.as_str())
                .unwrap_or("progressive");

            // Check SIMARD_SKIP_GYM
            if std::env::var("SIMARD_SKIP_GYM").as_deref() == Ok("1") {
                return Ok(serde_json::json!({
                    "suite_id": suite_id,
                    "success": true,
                    "overall_score": 0.0,
                    "dimensions": zero_dims(),
                    "scenario_results": [],
                    "scenarios_passed": 0,
                    "scenarios_total": 0,
                    "error_message": null,
                    "degraded_sources": ["SIMARD_SKIP_GYM=1 workaround"],
                }));
            }

            // Return degraded result for suite execution.
            Ok(serde_json::json!({
                "suite_id": suite_id,
                "success": false,
                "overall_score": 0.0,
                "dimensions": zero_dims(),
                "scenario_results": [],
                "scenarios_passed": 0,
                "scenarios_total": 0,
                "error_message": "native Rust evaluator: suite execution not yet implemented; use Python fallback bridge for full evaluation",
                "degraded_sources": ["native_gym_bridge"],
            }))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BridgeRequest, BridgeTransport, new_request_id};

    fn make_gym_transport() -> NativeBridgeTransport {
        let mut transport = NativeBridgeTransport::new("simard-gym-eval");
        register_gym_handlers(&mut transport);
        transport
    }

    fn call(transport: &NativeBridgeTransport, method: &str, params: Value) -> Value {
        let request = BridgeRequest {
            id: new_request_id(),
            method: method.to_string(),
            params,
        };
        let response = transport.call(request).unwrap();
        response.result.unwrap_or_else(|| {
            serde_json::json!({
                "error": response.error.map(|e| e.message).unwrap_or_default()
            })
        })
    }

    #[test]
    fn list_scenarios_returns_builtin_set() {
        let transport = make_gym_transport();
        let result = call(&transport, "gym.list_scenarios", serde_json::json!({}));
        let scenarios = result.as_array().unwrap();
        assert!(scenarios.len() >= 12);
        assert!(scenarios.iter().any(|s| s["id"] == "L1"));
        assert!(scenarios.iter().any(|s| s["id"] == "long-horizon-memory"));
    }

    #[test]
    fn run_scenario_returns_degraded_result() {
        let orig = std::env::var("SIMARD_SKIP_GYM").ok();
        unsafe {
            std::env::remove_var("SIMARD_SKIP_GYM");
        }
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_scenario",
            serde_json::json!({"scenario_id": "L1"}),
        );
        if let Some(v) = orig {
            unsafe {
                std::env::set_var("SIMARD_SKIP_GYM", v);
            }
        }
        assert_eq!(result["success"], false);
        assert!(
            result["error_message"]
                .as_str()
                .unwrap()
                .contains("not yet implemented")
        );
    }

    #[test]
    fn run_scenario_rejects_path_traversal() {
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_scenario",
            serde_json::json!({"scenario_id": "../etc/passwd"}),
        );
        assert_eq!(result["success"], false);
        assert!(
            result["error_message"]
                .as_str()
                .unwrap()
                .contains("illegal path characters")
        );
    }

    #[test]
    fn run_scenario_unknown_returns_not_found() {
        // Clear SIMARD_SKIP_GYM so the handler reaches the not-found check.
        let orig = std::env::var("SIMARD_SKIP_GYM").ok();
        unsafe {
            std::env::remove_var("SIMARD_SKIP_GYM");
        }
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_scenario",
            serde_json::json!({"scenario_id": "Z99"}),
        );
        if let Some(v) = orig {
            unsafe {
                std::env::set_var("SIMARD_SKIP_GYM", v);
            }
        }
        assert_eq!(result["success"], false);
        assert!(
            result["error_message"]
                .as_str()
                .unwrap()
                .contains("not found")
        );
    }

    #[test]
    fn run_suite_returns_degraded_result() {
        let orig = std::env::var("SIMARD_SKIP_GYM").ok();
        unsafe {
            std::env::remove_var("SIMARD_SKIP_GYM");
        }
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_suite",
            serde_json::json!({"suite_id": "progressive"}),
        );
        if let Some(v) = orig {
            unsafe {
                std::env::set_var("SIMARD_SKIP_GYM", v);
            }
        }
        assert_eq!(result["success"], false);
        assert!(
            result["error_message"]
                .as_str()
                .unwrap()
                .contains("not yet implemented")
        );
    }

    #[test]
    fn run_scenario_skip_gym_returns_synthetic_success() {
        // SAFETY: test-only
        unsafe {
            std::env::set_var("SIMARD_SKIP_GYM", "1");
        }
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_scenario",
            serde_json::json!({"scenario_id": "L1"}),
        );
        unsafe {
            std::env::remove_var("SIMARD_SKIP_GYM");
        }

        assert_eq!(result["success"], true);
        assert!(
            result["degraded_sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s.as_str().unwrap().contains("SIMARD_SKIP_GYM"))
        );
    }

    #[test]
    fn run_suite_skip_gym_returns_synthetic_success() {
        // SAFETY: test-only
        unsafe {
            std::env::set_var("SIMARD_SKIP_GYM", "1");
        }
        let transport = make_gym_transport();
        let result = call(
            &transport,
            "gym.run_suite",
            serde_json::json!({"suite_id": "progressive"}),
        );
        unsafe {
            std::env::remove_var("SIMARD_SKIP_GYM");
        }

        assert_eq!(result["success"], true);
    }

    #[test]
    fn health_check_works() {
        let transport = make_gym_transport();
        let health = transport.health().unwrap();
        assert!(health.healthy);
        assert_eq!(health.server_name, "simard-gym-eval");
    }

    #[test]
    fn default_score_dims_all_zero() {
        let d = crate::gym_bridge::ScoreDimensions::default();
        assert!((d.mean() - 0.0).abs() < f64::EPSILON);
    }
}
