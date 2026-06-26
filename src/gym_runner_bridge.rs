//! src/gym_runner_bridge.rs
//!
//! Thin Simard adapter that backs the `simard-gym-eval` bridge with the
//! upstream `amplihack-agent-eval` crate's native `GymRunner`.
//!
//! `register_gym_handlers` registers the three `gym.*` methods
//! (`gym.list_scenarios`, `gym.run_scenario`, `gym.run_suite`) on a
//! [`NativeBridgeTransport`], delegating the actual evaluation to
//! `amplihack_agent_eval::gym::GymRunner` and mapping the library's result
//! types onto Simard's existing, byte-stable wire JSON. The engine itself —
//! formerly the private `src/native_gym.rs` fork — now lives entirely in the
//! library; this module is *only* the bridge wiring.
//!
//! See `docs/architecture/gym-eval-library-adapter.md` for the full contract.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use amplihack_agent_eval::gym::{GymConfig, GymRunner, GymScenarioResult};

use crate::bridge::{BRIDGE_ERROR_INTERNAL, BridgeErrorPayload};
use crate::bridge_subprocess::NativeBridgeTransport;
use crate::gym_bridge::ScoreDimensions;

/// `degraded_sources` marker recorded by the `SIMARD_SKIP_GYM=1` fast path.
const SKIP_GYM_SOURCE: &str = "SIMARD_SKIP_GYM";

/// Build the [`GymConfig`] every evaluation runs with.
///
/// Self-grading is deterministic (`sdk = "mini"`): no LLM, network, or
/// subprocess. Artifacts land under the existing Simard gym output root.
fn gym_config() -> GymConfig {
    GymConfig {
        output_dir: PathBuf::from("target/simard-gym").join("eval"),
        agent_name: "simard-gym-eval".to_string(),
        sdk: "mini".to_string(),
        grader_votes: 3,
    }
}

/// True when the `SIMARD_SKIP_GYM` fast path is active.
fn skip_gym() -> bool {
    std::env::var("SIMARD_SKIP_GYM").as_deref() == Ok("1")
}

/// Collapse non-finite scores to `0.0`; otherwise pass through unchanged.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// Reject ids that could escape `output_dir` once the engine does
/// `output_dir.join(id)`. Mirrors the design-doc allowlist + dot-segment guard:
/// allow `^[A-Za-z0-9._-]{1,128}$`, then reject any `..` segment (the allowlist
/// permits `.`, so the dot-segment guard is required to block a bare `..`).
fn id_is_safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Map the library's optional, sparsely-populated per-scenario dimensions
/// (`HashMap<String, Option<f64>>`) onto the five non-nullable
/// [`ScoreDimensions`] fields the bridge wire contract requires.
///
/// Contract (see the design doc, "Dimension mapping"):
/// 1. All five `ALL_DIMENSIONS` keys are always present in the output.
/// 2. A missing key or an explicit `None` maps to `0.0`.
/// 3. A non-finite value (`NaN`, `+Inf`, `-Inf`) maps to `0.0`.
/// 4. The result is clamped to `[0.0, 1.0]`.
pub(crate) fn dimensions_from_optional(dims: &HashMap<String, Option<f64>>) -> ScoreDimensions {
    sanitized(|key| dims.get(key).copied().flatten())
}

/// Map the library's required suite-level dimensions
/// (`HashMap<String, f64>`) onto [`ScoreDimensions`] with the same
/// force-five-keys / sanitise / clamp rules as [`dimensions_from_optional`].
pub(crate) fn dimensions_from_required(dims: &HashMap<String, f64>) -> ScoreDimensions {
    sanitized(|key| dims.get(key).copied())
}

/// Shared force-five-keys / sanitise / clamp core for both dimension mappers.
fn sanitized(lookup: impl Fn(&str) -> Option<f64>) -> ScoreDimensions {
    let get = |key: &str| match lookup(key) {
        Some(v) if v.is_finite() => v.clamp(0.0, 1.0),
        _ => 0.0,
    };
    ScoreDimensions {
        factual_accuracy: get("factual_accuracy"),
        specificity: get("specificity"),
        temporal_awareness: get("temporal_awareness"),
        source_attribution: get("source_attribution"),
        confidence_calibration: get("confidence_calibration"),
    }
}

/// Recompute the suite-level `success` flag from the per-scenario tallies.
///
/// The library's `GymRunner::run_suite` computes its top-level `success` with
/// inverted logic (it is `true` precisely when failures exist). The adapter
/// must NOT trust that flag — `success` is `true` only when every scenario in
/// the suite passed: `scenarios_passed == scenarios_total`.
pub(crate) fn suite_success(scenarios_passed: usize, scenarios_total: usize) -> bool {
    scenarios_passed == scenarios_total
}

/// Serialize a [`ScoreDimensions`] into its wire JSON object (all five keys).
///
/// `ScoreDimensions` is five already-sanitised, finite `f64` fields, so this
/// serialization cannot fail. An `Err` here would mean a broken invariant — not
/// a degraded result — so we surface it loudly via `expect` rather than emit a
/// misleading empty `{}` (Pillar 11: honest degradation, no silent zeros).
fn dims_value(dims: &ScoreDimensions) -> Value {
    serde_json::to_value(dims)
        .expect("ScoreDimensions (five finite f64 fields) is always JSON-serializable")
}

/// Wire JSON for the all-zero dimensions object used by the degraded
/// (`fail_*`) and synthetic (`skip_*`) results.
fn zero_dims() -> Value {
    dims_value(&ScoreDimensions::default())
}

/// Wire JSON for a failing scenario result (structured — never an RPC error).
fn fail_scenario(scenario_id: &str, message: &str) -> Value {
    json!({
        "scenario_id": scenario_id,
        "success": false,
        "score": 0.0,
        "dimensions": zero_dims(),
        "question_count": 0,
        "questions_answered": 0,
        "error_message": message,
        "degraded_sources": [],
    })
}

/// Wire JSON for a failing suite result.
fn fail_suite(suite_id: &str, message: &str) -> Value {
    json!({
        "suite_id": suite_id,
        "success": false,
        "overall_score": 0.0,
        "dimensions": zero_dims(),
        "scenario_results": [],
        "scenarios_passed": 0,
        "scenarios_total": 0,
        "error_message": message,
        "degraded_sources": [],
    })
}

/// Wire JSON for the `SIMARD_SKIP_GYM=1` synthetic scenario result: a
/// zero-score success that bypasses the engine and records the skip in
/// `degraded_sources`.
fn skip_scenario(scenario_id: &str) -> Value {
    json!({
        "scenario_id": scenario_id,
        "success": true,
        "score": 0.0,
        "dimensions": zero_dims(),
        "question_count": 0,
        "questions_answered": 0,
        "error_message": null,
        "degraded_sources": [SKIP_GYM_SOURCE],
    })
}

/// Wire JSON for the `SIMARD_SKIP_GYM=1` synthetic suite result: a
/// zero-scenario success that bypasses the engine and records the skip in
/// `degraded_sources`.
fn skip_suite(suite_id: &str) -> Value {
    json!({
        "suite_id": suite_id,
        "success": true,
        "overall_score": 0.0,
        "dimensions": zero_dims(),
        "scenario_results": [],
        "scenarios_passed": 0,
        "scenarios_total": 0,
        "error_message": null,
        "degraded_sources": [SKIP_GYM_SOURCE],
    })
}

/// Map a library [`GymScenarioResult`] onto the bridge wire JSON, overriding
/// the engine's compact `scenario_id` with `wire_id`.
fn scenario_value(wire_id: &str, r: &GymScenarioResult) -> Value {
    json!({
        "scenario_id": wire_id,
        "success": r.success,
        "score": finite_or_zero(r.score),
        "dimensions": dims_value(&dimensions_from_optional(&r.dimensions)),
        "question_count": r.question_count,
        "questions_answered": r.questions_answered,
        "error_message": r.error_message,
        "degraded_sources": r.degraded_sources,
    })
}

/// Build a `compact -> descriptive` id map (`"L1" -> "L1-recall"`) from the
/// runner's advertised scenarios, so suite per-scenario ids are normalized to
/// the ids `gym.list_scenarios` advertises rather than the engine's bare
/// `"L{n}"` form.
fn compact_id_map(runner: &GymRunner) -> HashMap<String, String> {
    runner
        .list_scenarios()
        .into_iter()
        .filter_map(|s| {
            // `s` is owned, so move `s.id` into the value instead of cloning;
            // only the small compact prefix needs a fresh allocation.
            let compact = s.id.split('-').next()?.to_string();
            Some((compact, s.id))
        })
        .collect()
}

/// Register the three `gym.*` method handlers on a native transport.
///
/// This is the sole entry point `bridge_launcher::launch_gym_bridge_native`
/// calls (replacing the deleted `native_gym::register_gym_handlers`). The
/// evaluation engine itself lives entirely in `amplihack_agent_eval::gym`.
pub fn register_gym_handlers(transport: &mut NativeBridgeTransport) {
    transport.register(
        "gym.list_scenarios",
        Arc::new(|_params: &Value| -> Result<Value, BridgeErrorPayload> {
            let runner = GymRunner::new(gym_config());
            // `GymScenario` carries only String/usize fields, so this
            // serialization cannot fail. An `Err` would be a broken invariant —
            // not a degraded result — so surface it loudly rather than emit a
            // misleading empty `[]` (Pillar 11: honest degradation).
            Ok(serde_json::to_value(runner.list_scenarios())
                .expect("Vec<GymScenario> (String/usize fields only) is always JSON-serializable"))
        }),
    );

    transport.register(
        "gym.run_scenario",
        Arc::new(|params: &Value| -> Result<Value, BridgeErrorPayload> {
            let scenario_id = params
                .get("scenario_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // An empty / absent id is the only hard transport error.
            if scenario_id.is_empty() {
                return Err(BridgeErrorPayload {
                    code: BRIDGE_ERROR_INTERNAL,
                    message: "scenario_id is required".to_string(),
                });
            }

            // SIMARD_SKIP_GYM bypasses the engine entirely for any id.
            if skip_gym() {
                return Ok(skip_scenario(scenario_id));
            }

            if !id_is_safe(scenario_id) {
                return Ok(fail_scenario(
                    scenario_id,
                    &format!("scenario_id contains illegal path characters: '{scenario_id}'"),
                ));
            }

            let runner = GymRunner::new(gym_config());
            match runner.run_scenario(scenario_id) {
                Ok(result) => Ok(scenario_value(scenario_id, &result)),
                Err(e) => Ok(fail_scenario(scenario_id, &e.to_string())),
            }
        }),
    );

    transport.register(
        "gym.run_suite",
        Arc::new(|params: &Value| -> Result<Value, BridgeErrorPayload> {
            let suite_id = params
                .get("suite_id")
                .and_then(|v| v.as_str())
                .unwrap_or("progressive");

            if skip_gym() {
                return Ok(skip_suite(suite_id));
            }

            // The library's run_suite joins suite_id onto output_dir with no
            // traversal check of its own, so the bridge must validate it here.
            if !id_is_safe(suite_id) {
                return Ok(fail_suite(
                    suite_id,
                    &format!("suite_id contains illegal path characters: '{suite_id}'"),
                ));
            }

            let runner = GymRunner::new(gym_config());
            match runner.run_suite(suite_id) {
                Ok(result) => {
                    let id_map = compact_id_map(&runner);
                    let scenario_results: Vec<Value> = result
                        .scenario_results
                        .iter()
                        .map(|sr| {
                            // Borrow the wire id straight from the map (or fall
                            // back to the engine's own id) — no per-scenario
                            // String allocation; `scenario_value` takes `&str`.
                            let wire_id = id_map
                                .get(&sr.scenario_id)
                                .map(String::as_str)
                                .unwrap_or(sr.scenario_id.as_str());
                            scenario_value(wire_id, sr)
                        })
                        .collect();
                    Ok(json!({
                        "suite_id": suite_id,
                        "success": suite_success(result.scenarios_passed, result.scenarios_total),
                        "overall_score": finite_or_zero(result.overall_score),
                        "dimensions": dims_value(&dimensions_from_required(&result.dimensions)),
                        "scenario_results": scenario_results,
                        "scenarios_passed": result.scenarios_passed,
                        "scenarios_total": result.scenarios_total,
                        "error_message": result.error_message,
                        "degraded_sources": result.degraded_sources,
                    }))
                }
                Err(e) => Ok(fail_suite(suite_id, &e.to_string())),
            }
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        BRIDGE_ERROR_INTERNAL, BridgeRequest, BridgeResponse, BridgeTransport, new_request_id,
    };
    use serde_json::json;
    use serial_test::serial;

    // ── Test plumbing ────────────────────────────────────────────────────

    fn make_transport() -> NativeBridgeTransport {
        let mut transport = NativeBridgeTransport::new("simard-gym-eval");
        register_gym_handlers(&mut transport);
        transport
    }

    /// Issue a bridge call and return the raw response (so error envelopes are
    /// inspectable).
    fn raw_call(transport: &NativeBridgeTransport, method: &str, params: Value) -> BridgeResponse {
        let request = BridgeRequest {
            id: new_request_id(),
            method: method.to_string(),
            params,
        };
        transport
            .call(request)
            .expect("native transport.call must not surface a SimardError")
    }

    /// Issue a bridge call and return the `result` payload, asserting success.
    fn result_of(transport: &NativeBridgeTransport, method: &str, params: Value) -> Value {
        raw_call(transport, method, params)
            .result
            .expect("expected a result payload")
    }

    fn dims_opt(pairs: &[(&str, Option<f64>)]) -> HashMap<String, Option<f64>> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    const ALL_FIVE: [&str; 5] = [
        "factual_accuracy",
        "specificity",
        "temporal_awareness",
        "source_attribution",
        "confidence_calibration",
    ];

    /// RAII guard that sets/clears `SIMARD_SKIP_GYM` and restores the previous
    /// value on drop (including during unwind from a failed assertion), so the
    /// `#[serial]` env-var tests never leak state into one another.
    struct SkipGuard(Option<String>);

    impl SkipGuard {
        fn set(value: &str) -> Self {
            let prev = std::env::var("SIMARD_SKIP_GYM").ok();
            // SAFETY: test-only; serialised via #[serial].
            unsafe { std::env::set_var("SIMARD_SKIP_GYM", value) };
            SkipGuard(prev)
        }

        fn clear() -> Self {
            let prev = std::env::var("SIMARD_SKIP_GYM").ok();
            // SAFETY: test-only; serialised via #[serial].
            unsafe { std::env::remove_var("SIMARD_SKIP_GYM") };
            SkipGuard(prev)
        }
    }

    impl Drop for SkipGuard {
        fn drop(&mut self) {
            match &self.0 {
                // SAFETY: test-only; serialised via #[serial].
                Some(value) => unsafe { std::env::set_var("SIMARD_SKIP_GYM", value) },
                None => unsafe { std::env::remove_var("SIMARD_SKIP_GYM") },
            }
        }
    }

    // ── Dimension mapping (INV-1 / INV-2 / SEC-4) — pure, no engine ───────

    #[test]
    fn dimensions_force_all_five_keys() {
        let input = dims_opt(&[("factual_accuracy", Some(0.9)), ("specificity", Some(0.8))]);
        let d = dimensions_from_optional(&input);
        assert!((d.factual_accuracy - 0.9).abs() < 1e-9);
        assert!((d.specificity - 0.8).abs() < 1e-9);
        // The three keys absent from the input still appear, defaulted to 0.0.
        assert_eq!(d.temporal_awareness, 0.0);
        assert_eq!(d.source_attribution, 0.0);
        assert_eq!(d.confidence_calibration, 0.0);
    }

    #[test]
    fn dimensions_missing_and_none_become_zero() {
        let input = dims_opt(&[("factual_accuracy", None), ("specificity", Some(0.5))]);
        let d = dimensions_from_optional(&input);
        assert_eq!(d.factual_accuracy, 0.0, "explicit None must map to 0.0");
        assert!((d.specificity - 0.5).abs() < 1e-9);
        assert_eq!(d.temporal_awareness, 0.0, "missing key must map to 0.0");
    }

    #[test]
    fn dimensions_nan_inf_become_zero() {
        let input = dims_opt(&[
            ("factual_accuracy", Some(f64::NAN)),
            ("specificity", Some(f64::INFINITY)),
            ("temporal_awareness", Some(f64::NEG_INFINITY)),
            ("source_attribution", Some(0.42)),
        ]);
        let d = dimensions_from_optional(&input);
        assert_eq!(d.factual_accuracy, 0.0, "NaN must be sanitised to 0.0");
        assert_eq!(d.specificity, 0.0, "+Inf must be sanitised to 0.0");
        assert_eq!(d.temporal_awareness, 0.0, "-Inf must be sanitised to 0.0");
        assert!((d.source_attribution - 0.42).abs() < 1e-9);
    }

    #[test]
    fn dimensions_clamped_to_unit_interval() {
        let input = dims_opt(&[
            ("factual_accuracy", Some(1.5)),
            ("specificity", Some(-0.3)),
            ("temporal_awareness", Some(1.0)),
            ("source_attribution", Some(0.0)),
        ]);
        let d = dimensions_from_optional(&input);
        assert_eq!(d.factual_accuracy, 1.0, "value > 1 clamps to 1.0");
        assert_eq!(d.specificity, 0.0, "value < 0 clamps to 0.0");
        assert_eq!(d.temporal_awareness, 1.0);
        assert_eq!(d.source_attribution, 0.0);
    }

    #[test]
    fn dimensions_from_required_forces_all_five_keys() {
        let mut input: HashMap<String, f64> = HashMap::new();
        input.insert("factual_accuracy".to_string(), 0.7);
        let d = dimensions_from_required(&input);
        assert!((d.factual_accuracy - 0.7).abs() < 1e-9);
        assert_eq!(d.specificity, 0.0);
        assert_eq!(d.temporal_awareness, 0.0);
        assert_eq!(d.source_attribution, 0.0);
        assert_eq!(d.confidence_calibration, 0.0);
    }

    #[test]
    fn dimensions_from_required_sanitizes_and_clamps() {
        let mut input: HashMap<String, f64> = HashMap::new();
        input.insert("factual_accuracy".to_string(), f64::NAN);
        input.insert("specificity".to_string(), 2.0);
        input.insert("temporal_awareness".to_string(), -1.0);
        let d = dimensions_from_required(&input);
        assert_eq!(d.factual_accuracy, 0.0);
        assert_eq!(d.specificity, 1.0);
        assert_eq!(d.temporal_awareness, 0.0);
    }

    // ── Suite success recompute (the upstream inverted-flag fix) ──────────

    #[test]
    fn run_suite_success_requires_all_passed() {
        assert!(
            suite_success(12, 12),
            "every scenario passing => success: true"
        );
        assert!(
            !suite_success(11, 12),
            "a single failure => success: false (must not trust the engine's inverted flag)"
        );
        assert!(
            !suite_success(0, 12),
            "all scenarios failing => success: false"
        );
    }

    // ── Identity / path-traversal validation (SEC-2 / SEC-3) ──────────────

    #[test]
    #[serial]
    fn run_scenario_rejects_path_traversal() {
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "../etc/passwd"}),
        );
        assert_eq!(r["success"], false);
        let msg = r["error_message"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            msg.contains("illegal") || msg.contains("path") || msg.contains("invalid"),
            "rejection must explain the bad id, got: {msg}"
        );
    }

    #[test]
    #[serial]
    fn run_suite_rejects_path_traversal() {
        // The library's run_suite does `output_dir.join(suite_id)` with NO
        // traversal check; the bridge must reject it itself.
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_suite",
            json!({"suite_id": "../../tmp/x"}),
        );
        assert_eq!(r["success"], false);
        assert!(
            r["error_message"].is_string(),
            "rejected suite must carry an error_message"
        );
    }

    #[test]
    #[serial]
    fn run_suite_rejects_dot_segment() {
        // A bare ".." matches the `[A-Za-z0-9._-]` allowlist (because `.` is
        // allowed); the explicit dot-segment guard is what rejects it.
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let r = result_of(&transport, "gym.run_suite", json!({"suite_id": ".."}));
        assert_eq!(r["success"], false);
        assert!(r["error_message"].is_string());
    }

    #[test]
    #[serial]
    fn run_scenario_empty_id_is_transport_error() {
        // Empty scenario_id is the ONLY hard transport error (error envelope).
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let resp = raw_call(&transport, "gym.run_scenario", json!({"scenario_id": ""}));
        assert!(
            resp.result.is_none(),
            "empty scenario_id must not produce a result payload"
        );
        let err = resp
            .error
            .expect("empty scenario_id must surface a hard error envelope");
        assert_eq!(err.code, BRIDGE_ERROR_INTERNAL);
    }

    #[test]
    #[serial]
    fn run_scenario_missing_id_is_transport_error() {
        // A wholly absent scenario_id is treated like an empty one.
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let resp = raw_call(&transport, "gym.run_scenario", json!({}));
        assert!(resp.result.is_none());
        let err = resp
            .error
            .expect("missing scenario_id must surface an error");
        assert_eq!(err.code, BRIDGE_ERROR_INTERNAL);
    }

    // ── SIMARD_SKIP_GYM fast path (five #[serial] env-var tests) ──────────

    #[test]
    #[serial]
    fn run_scenario_skip_gym_returns_synthetic_success() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "L1-recall"}),
        );
        assert_eq!(r["success"], true);
        let sources = r["degraded_sources"]
            .as_array()
            .expect("degraded_sources must be an array");
        assert!(
            sources
                .iter()
                .any(|s| s.as_str().unwrap_or_default().contains("SIMARD_SKIP_GYM")),
            "skip result must record SIMARD_SKIP_GYM in degraded_sources: {sources:?}"
        );
    }

    #[test]
    #[serial]
    fn run_suite_skip_gym_returns_synthetic_success() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_suite",
            json!({"suite_id": "progressive"}),
        );
        assert_eq!(r["success"], true);
        let sources = r["degraded_sources"]
            .as_array()
            .expect("degraded_sources must be an array");
        assert!(
            sources
                .iter()
                .any(|s| s.as_str().unwrap_or_default().contains("SIMARD_SKIP_GYM"))
        );
    }

    #[test]
    #[serial]
    fn run_scenario_skip_gym_dimensions_present_and_zero() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "L1-recall"}),
        );
        let dims = &r["dimensions"];
        for key in ALL_FIVE {
            assert_eq!(dims[key], 0.0, "skip dimensions must be zero for {key}");
        }
    }

    #[test]
    #[serial]
    fn run_scenario_skip_gym_bypasses_engine_for_any_valid_id() {
        // Under SIMARD_SKIP_GYM the engine is never consulted, so even an id
        // the engine does not know still returns synthetic success.
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "definitely-not-a-real-scenario"}),
        );
        assert_eq!(r["success"], true);
    }

    #[test]
    #[serial]
    fn run_suite_skip_gym_reports_zero_scenarios() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_suite",
            json!({"suite_id": "progressive"}),
        );
        assert_eq!(r["scenarios_total"], 0);
        assert_eq!(r["scenarios_passed"], 0);
        assert!(
            r["scenario_results"].as_array().unwrap().is_empty(),
            "skip suite must report no per-scenario results"
        );
    }

    // ── Engine-backed behavioral contract (deterministic, CI-safe) ────────

    #[test]
    #[serial]
    fn list_scenarios_returns_library_ids() {
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let r = result_of(&transport, "gym.list_scenarios", json!({}));
        let arr = r.as_array().expect("list_scenarios returns an array");
        assert!(
            arr.len() >= 13,
            "expected >= 13 scenarios (12 levels + long-horizon), got {}",
            arr.len()
        );
        assert!(
            arr.iter().any(|s| s["id"] == "L1-recall"),
            "the library descriptive id 'L1-recall' must be advertised"
        );
        assert!(
            arr.iter().any(|s| s["id"] == "long-horizon-memory"),
            "the long-horizon scenario must be advertised"
        );
        assert!(
            !arr.iter().any(|s| s["id"] == "L1"),
            "the old fork's bare 'L1' id must NOT be advertised after the de-fork"
        );
    }

    #[test]
    #[serial]
    fn run_scenario_echoes_requested_id() {
        // The engine reformats a successful result's id to the compact "L1";
        // the adapter must override it with the caller's requested id.
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "L1-recall"}),
        );
        assert_eq!(
            r["scenario_id"], "L1-recall",
            "wire scenario_id must echo the requested id, not the engine's compact L{{n}}"
        );
        let _ = std::fs::remove_dir_all("target/simard-gym/eval");
    }

    #[test]
    #[serial]
    fn run_scenario_engine_error_maps_to_failure() {
        // A well-formed but unknown id reaches the engine, which returns an
        // EvalError. Per Pillar 11 the adapter maps that to a structured
        // failing RESULT, not a hard RPC error.
        let _g = SkipGuard::clear();
        let transport = make_transport();
        let resp = raw_call(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "L99-nonexistent"}),
        );
        assert!(
            resp.error.is_none(),
            "an engine EvalError must surface as a result, not an RPC error envelope"
        );
        let r = resp.result.expect("expected a degraded result payload");
        assert_eq!(r["success"], false);
        assert!(
            r["error_message"]
                .as_str()
                .map(|m| !m.is_empty())
                .unwrap_or(false),
            "a failing result must carry a non-empty error_message"
        );
    }

    // ── Wire stability (INV-6): handler JSON ↔ unchanged client types ─────

    #[test]
    #[serial]
    fn skip_result_deserializes_as_gym_scenario_result() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_scenario",
            json!({"scenario_id": "L1-recall"}),
        );
        let typed: crate::gym_bridge::GymScenarioResult = serde_json::from_value(r)
            .expect("handler JSON must deserialize into the unchanged GymScenarioResult");
        assert!(typed.success);
        assert!(
            typed
                .degraded_sources
                .iter()
                .any(|s| s.contains("SIMARD_SKIP_GYM"))
        );
    }

    #[test]
    #[serial]
    fn skip_suite_result_deserializes_as_gym_suite_result() {
        let _g = SkipGuard::set("1");
        let transport = make_transport();
        let r = result_of(
            &transport,
            "gym.run_suite",
            json!({"suite_id": "progressive"}),
        );
        let typed: crate::gym_bridge::GymSuiteResult = serde_json::from_value(r)
            .expect("handler JSON must deserialize into the unchanged GymSuiteResult");
        assert!(typed.success);
        assert_eq!(typed.scenarios_total, 0);
    }

    // ── Bridge health (unchanged transport behaviour) ─────────────────────

    #[test]
    fn gym_transport_is_healthy() {
        let transport = make_transport();
        let health = transport.health().unwrap();
        assert!(health.healthy);
        assert_eq!(health.server_name, "simard-gym-eval");
    }
}
