//! Compile-time + behavioural compatibility guard for the pinned
//! `amplihack-agent-eval` crate surface Simard's gym adapter consumes.
//!
//! # Why this exists
//!
//! `Cargo.toml` pins `amplihack-agent-eval` to the amplihack-rs **v0.18.25**
//! release source commit `9ee05a06eab98e9ab504a031bffaa4190700c2af` (annotated
//! tag `v0.18.25` → `e947170a…`, which dereferences to that commit; it was also
//! `refs/heads/main` when verified on **2026-09-03**). The previous pin,
//! `14dc30b1…` (landed by issue #2767), was a 2026-07-07 UTC commit 200 commits
//! behind.
//!
//! `tests/issue_2626_amplihack_pin_bump.rs` guards *which* rev is pinned by
//! reading `Cargo.toml` / `Cargo.lock` as text. It deliberately never links the
//! crate, so it cannot tell whether the pinned code still *has* the API
//! `src/gym_runner_client.rs` calls. This file is the other half of that
//! contract: it **links the real pinned crate** and fails to compile if the
//! consumed surface drifts in any way that matters to Simard.
//!
//! # What "the consumed surface" is
//!
//! `src/gym_runner_client.rs` — the only Simard module that imports the crate —
//! uses exactly:
//!
//!   * `GymConfig { output_dir, agent_name, sdk, grader_votes }` (struct-literal
//!     construction, so every field must exist with the right type *and* no
//!     field may be added);
//!   * `GymRunner::new(GymConfig) -> GymRunner`;
//!   * `GymRunner::list_scenarios(&self) -> Vec<GymScenario>` and `GymScenario::id`;
//!   * `GymRunner::run_scenario(&self, &str) -> Result<GymScenarioResult, EvalError>`;
//!   * `GymRunner::run_suite(&self, &str) -> Result<GymSuiteResult, EvalError>`;
//!   * the `GymScenarioResult` / `GymSuiteResult` **types**, whose full field
//!     set Simard compiles against even though the adapter deliberately does
//!     not forward every field (see below).
//!
//! # Field tests are exhaustive, but do not imply every field is forwarded
//!
//! The destructuring tests assert **field-set compatibility**: the exact shape
//! Simard compiles against. They are written without `..` on purpose, so an
//! upstream field that is *added*, *removed*, *renamed*, or *retyped* is a
//! compile error. That is a stronger and different property than "the adapter
//! reads this field" — an upstream field Simard ignores today still changes the
//! type Simard links against, and a silently-absorbed new field is exactly the
//! drift this file exists to catch.
//!
//! Three destructured fields are, by design, **not** forwarded to the wire:
//!
//!   * `GymScenarioResult::scenario_id` — read only as a *lookup key*
//!     (`compact_id_map.get(&sr.scenario_id)`). The wire `scenario_id` is
//!     intentionally **replaced** by the adapter's `wire_id`, so the engine's
//!     bare `"L{n}"` form is normalized to the descriptive `"L{n}-{slug}"` id
//!     that `gym.list_scenarios` advertises.
//!   * `GymSuiteResult::suite_id` — **not consumed**; the handler echoes the
//!     caller's requested `suite_id` instead.
//!   * `GymSuiteResult::success` — **not consumed**; the adapter *recomputes*
//!     it as `scenarios_passed == scenarios_total`. The library's own flag is
//!     `!failed_levels.is_empty() || level_results.iter().all(|lr| lr.success)`,
//!     and because `ProgressiveResult::add_result` records a level in
//!     `failed_levels` exactly when it did *not* succeed, an empty
//!     `failed_levels` implies every level succeeded. One disjunct always
//!     holds, so the expression is a **tautology that is always `true`** — on
//!     all-pass, partially-failing, and empty suites alike. It carries no
//!     information whatsoever, so it is ignored rather than corrected.
//!
//! Every other destructured field is forwarded: `GymScenario` is serialized
//! whole by `gym.list_scenarios`, and the remaining `GymScenarioResult` /
//! `GymSuiteResult` fields are mapped onto the wire JSON.
//!
//! # How drift is detected
//!
//! * **Exhaustive destructuring** (`let Type { a, b, .. } = ..` is *not* used —
//!   patterns are written without `..`) makes an *added or removed* upstream
//!   field a compile error, not a silently ignored one.
//! * **Function-pointer coercion** to a written-out `fn(...) -> ...` type makes
//!   any signature/return-type change a compile error.
//! * Two **behavioural** tests call the real pinned engine: `list_scenarios`
//!   must still advertise the scenario ids `compact_id_map` splits on, and
//!   `run_scenario` must still reject an unknown id as `Err(EvalError)` (the
//!   adapter turns that `Err` into a structured failing result, so the error
//!   path is part of the consumed contract).
//!
//! # What this file deliberately does NOT cover
//!
//! It does **not** exercise the adapter's wire-JSON mapping. Those helpers
//! (`scenario_value`, `dims_value`, `suite_success`, `fail_*` / `skip_*`) are
//! `pub(crate)` / private and are covered by the in-crate unit tests in
//! `src/gym_runner_client.rs` (e.g. `run_suite_success_requires_all_passed`)
//! plus the `tests/gym_eval.rs` handler tests. This file's job is narrower and
//! complementary: prove the *pinned upstream crate* still offers the surface
//! those tests depend on.
//!
//! No network, no LLM, no subprocess, no filesystem writes: `list_scenarios` is
//! pure, `run_scenario` returns the unknown-id `Err` before it builds a grader
//! or touches `output_dir`, and every result value is constructed locally.

use std::collections::HashMap;
use std::path::PathBuf;

use amplihack_agent_eval::error::EvalError;
use amplihack_agent_eval::gym::{
    GymConfig, GymRunner, GymScenario, GymScenarioResult, GymSuiteResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// Compile-time surface guards
// ─────────────────────────────────────────────────────────────────────────────

/// `GymRunner`'s three methods must keep the exact signatures the adapter calls.
///
/// Coercing each method to a written-out `fn` type is a compile error the moment
/// a parameter, receiver, or return type changes (e.g. `run_scenario` becoming
/// `async`, taking `&mut self`, or returning a different error type).
#[test]
fn gym_runner_method_signatures_are_unchanged() {
    let _new: fn(GymConfig) -> GymRunner = GymRunner::new;
    let _list: fn(&GymRunner) -> Vec<GymScenario> = GymRunner::list_scenarios;
    let _run_scenario: fn(&GymRunner, &str) -> Result<GymScenarioResult, EvalError> =
        GymRunner::run_scenario;
    let _run_suite: fn(&GymRunner, &str) -> Result<GymSuiteResult, EvalError> =
        GymRunner::run_suite;
}

/// `GymConfig` must keep exactly the four public fields the adapter sets.
///
/// The struct literal fails to compile if a field is removed or retyped; the
/// field-list-exhaustive destructuring (no `..`) fails if upstream *adds* one,
/// which would silently change the config the adapter builds.
#[test]
fn gym_config_has_exactly_the_fields_the_adapter_sets() {
    let config = GymConfig {
        output_dir: PathBuf::from("target/simard-gym").join("eval"),
        agent_name: "simard-gym-eval".to_string(),
        sdk: "mini".to_string(),
        grader_votes: 3,
    };

    let GymConfig {
        output_dir,
        agent_name,
        sdk,
        grader_votes,
    } = config;

    let _: PathBuf = output_dir;
    let _: String = agent_name;
    let _: String = sdk;
    let _: u8 = grader_votes;
}

/// `GymScenarioResult` must keep exactly this field set.
///
/// Exhaustive by design — this asserts the *shape* Simard compiles against, not
/// that the adapter forwards each field. `scenario_id` in particular is read
/// only as a `compact_id_map` lookup key; the wire `scenario_id` is
/// intentionally replaced by the adapter's `wire_id`.
///
/// `dimensions` must stay `HashMap<String, Option<f64>>` — the adapter's
/// `dimensions_from_optional` depends on the `Option` inner type to distinguish
/// "not measured" from "measured zero" (honest degradation).
#[test]
fn gym_scenario_result_field_set_is_unchanged() {
    let result = GymScenarioResult {
        scenario_id: "L1".to_string(),
        success: true,
        score: 0.5,
        dimensions: HashMap::from([("recall".to_string(), Some(0.5))]),
        question_count: 4,
        questions_answered: 2,
        error_message: None,
        degraded_sources: vec!["src".to_string()],
    };

    let GymScenarioResult {
        scenario_id,
        success,
        score,
        dimensions,
        question_count,
        questions_answered,
        error_message,
        degraded_sources,
    } = result;

    let _: String = scenario_id;
    let _: bool = success;
    let _: f64 = score;
    let _: HashMap<String, Option<f64>> = dimensions;
    let _: usize = question_count;
    let _: usize = questions_answered;
    let _: Option<String> = error_message;
    let _: Vec<String> = degraded_sources;
}

/// `GymSuiteResult` must keep exactly this field set.
///
/// Exhaustive by design, and two of these fields are deliberately **not**
/// consumed by the `gym.run_suite` handler: `suite_id` (the handler echoes the
/// caller's requested id) and `success` (recomputed as
/// `scenarios_passed == scenarios_total`, because the library's own flag is a
/// tautology that always evaluates to `true` — see `suite_success` in
/// `src/gym_runner_client.rs` for the derivation — and so carries no
/// information). They are still destructured so an upstream change to either is
/// a compile error rather than a silent shape drift.
///
/// Note `dimensions` here is `HashMap<String, f64>` (NOT `Option`) — the
/// adapter uses a different converter (`dimensions_from_required`) for the
/// suite than for the scenario, so the two inner types must not converge.
#[test]
fn gym_suite_result_field_set_is_unchanged() {
    let result = GymSuiteResult {
        suite_id: "progressive".to_string(),
        success: false,
        overall_score: 0.25,
        dimensions: HashMap::from([("recall".to_string(), 0.25)]),
        scenario_results: Vec::new(),
        scenarios_passed: 1,
        scenarios_total: 2,
        error_message: Some("boom".to_string()),
        degraded_sources: Vec::new(),
    };

    let GymSuiteResult {
        suite_id,
        success,
        overall_score,
        dimensions,
        scenario_results,
        scenarios_passed,
        scenarios_total,
        error_message,
        degraded_sources,
    } = result;

    let _: String = suite_id;
    let _: bool = success;
    let _: f64 = overall_score;
    let _: HashMap<String, f64> = dimensions;
    let _: Vec<GymScenarioResult> = scenario_results;
    let _: usize = scenarios_passed;
    let _: usize = scenarios_total;
    let _: Option<String> = error_message;
    let _: Vec<String> = degraded_sources;
}

/// `GymScenario` must keep exactly this field set.
///
/// Here the whole struct *is* consumed: `gym.list_scenarios` serializes
/// `Vec<GymScenario>` verbatim with serde, so every field below reaches the
/// wire, and `compact_id_map` additionally splits `id`.
#[test]
fn gym_scenario_field_set_is_unchanged() {
    let scenario = GymScenario {
        id: "L1-recall".to_string(),
        name: "Recall".to_string(),
        description: "desc".to_string(),
        level: "L1".to_string(),
        question_count: 4,
        article_count: 2,
    };

    let GymScenario {
        id,
        name,
        description,
        level,
        question_count,
        article_count,
    } = scenario;

    let _: String = id;
    let _: String = name;
    let _: String = description;
    let _: String = level;
    let _: usize = question_count;
    let _: usize = article_count;
}

// ─────────────────────────────────────────────────────────────────────────────
// Behavioural guards — the pinned engine still answers the adapter's calls
// ─────────────────────────────────────────────────────────────────────────────

/// The adapter's exact `GymConfig` (see `gym_runner_client::gym_config`) must
/// still build a runner whose advertised scenario ids match what
/// `compact_id_map` assumes. `list_scenarios` is pure — no LLM, no network, no
/// subprocess, and it does not touch `output_dir`.
///
/// The contract `compact_id_map` depends on: the progressive scenarios are
/// advertised as `"L{n}-{slug}"`, while `run_suite` reports their per-level
/// results under the bare `"L{n}"` form. The adapter rebuilds the descriptive
/// id by splitting the advertised id on the first `-`, so those `L{n}` prefixes
/// must be **unique** — a collision would silently drop a mapping and emit the
/// wrong descriptive id on the wire. The runner also always appends the
/// `"long-horizon-memory"` scenario, which `run_scenario` special-cases.
#[test]
fn pinned_runner_advertises_the_scenario_ids_the_adapter_maps() {
    let runner = GymRunner::new(GymConfig {
        output_dir: PathBuf::from("target/simard-gym").join("eval"),
        agent_name: "simard-gym-eval".to_string(),
        sdk: "mini".to_string(),
        grader_votes: 3,
    });

    let scenarios = runner.list_scenarios();
    assert!(
        !scenarios.is_empty(),
        "pinned amplihack-agent-eval must still advertise at least one gym \
         scenario; `gym.list_scenarios` would otherwise return an empty list."
    );

    let ids: Vec<&str> = scenarios.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"long-horizon-memory"),
        "the runner must still advertise the `long-horizon-memory` scenario \
         that `run_scenario` special-cases. Advertised ids: {ids:?}"
    );

    // Every advertised id must produce a non-empty compact key.
    for id in &ids {
        let compact = id
            .split('-')
            .next()
            .expect("split always yields at least one element");
        assert!(
            !compact.is_empty(),
            "scenario id `{id}` has an empty compact prefix; `compact_id_map` \
             would map suite results onto an empty key."
        );
    }

    // The progressive `L{n}-…` ids must exist and have unique `L{n}` prefixes.
    let level_prefixes: Vec<&str> = ids
        .iter()
        .filter_map(|id| id.split('-').next())
        .filter(|c| {
            c.starts_with('L') && c.len() > 1 && c[1..].chars().all(|ch| ch.is_ascii_digit())
        })
        .collect();
    assert!(
        !level_prefixes.is_empty(),
        "the runner must still advertise progressive `L{{n}}-…` scenarios; \
         `compact_id_map` has nothing to map otherwise. Advertised ids: {ids:?}"
    );
    let unique: std::collections::BTreeSet<&str> = level_prefixes.iter().copied().collect();
    assert_eq!(
        unique.len(),
        level_prefixes.len(),
        "advertised `L{{n}}` compact prefixes must be unique so `compact_id_map` \
         cannot silently drop a mapping. Got {level_prefixes:?}"
    );
}

/// `GymRunner::run_scenario` must still report an unknown scenario id as an
/// `Err(EvalError)` rather than panicking or inventing a success. The adapter
/// converts that `Err` into a *structured failing result* (`fail_scenario`),
/// never an RPC error, so the error path is part of the consumed contract.
#[test]
fn pinned_runner_rejects_an_unknown_scenario_id_as_an_error() {
    let runner = GymRunner::new(GymConfig {
        output_dir: PathBuf::from("target/simard-gym").join("eval"),
        agent_name: "simard-gym-eval".to_string(),
        sdk: "mini".to_string(),
        grader_votes: 3,
    });

    let err = runner
        .run_scenario("definitely-not-a-scenario")
        .expect_err("an unknown scenario id must be an Err, not a success");

    // The adapter puts the rendered error into the wire `error_message`, so it
    // must render as non-empty text.
    assert!(
        !err.to_string().trim().is_empty(),
        "EvalError must render a non-empty message; the adapter surfaces it as \
         the wire `error_message` for a failing gym.run_scenario."
    );
}
