---
title: Library-backed Gym Evaluation Engine (the sole engine)
description: How Simard's gym client is backed by the amplihack-agent-eval crate's native Rust GymRunner through the thin gym_runner_client adapter. As of the de-fork the private native_gym reimplementation has been deleted and the library is the only evaluation engine.
last_updated: 2026-09-03
owner: simard
doc_type: reference
related:
  - ./rpc-pattern.md
  - ../reference/rpc-wire-protocol.md
  - ./cognitive-memory-library-adapter.md
  - ../testing/ci-resilient-test-patterns.md
---

# Library-backed Gym Evaluation Engine (the sole engine)

Simard's progressive evaluation engine (L1–L12 plus the long-horizon memory
stress test) is implemented by the upstream
[`amplihack-agent-eval`](https://github.com/rysweet/amplihack-rs) crate. Its
native Rust `GymRunner` is reached through a single thin Simard adapter,
**`gym_runner_client`** (`src/gym_runner_client.rs`), which registers the three
`gym.*` client methods and maps the library's result types onto Simard's
existing wire protocol. This adapter is the **only** evaluation engine Simard
ships.

> **History.** Simard once maintained a *fork* of the gym/eval engine —
> `src/native_gym.rs`, a ~468-line native Rust module that "replaced the Python
> client". It hard-coded the twelve `L1`..`L12` scenario definitions and, for
> `run_scenario` / `run_suite`, returned **degraded** results whose
> `error_message` read *"native Rust evaluator: ... not yet implemented; full
> progressive test suite evaluation is a planned enhancement"*. Only
> `list_scenarios` worked. The de-fork (issue
> [#2323](https://github.com/rysweet/Simard/issues/2323)) retired that stub by
> adopting the real engine that already exists in `amplihack-agent-eval` —
> `amplihack_agent_eval::gym::{GymRunner, GymConfig, GymScenario,
> GymScenarioResult, GymSuiteResult}` — and **deleting** `native_gym.rs`.

This mirrors the earlier de-fork of cognitive memory; see
[Library-backed Cognitive Memory](cognitive-memory-library-adapter.md) for the
same pattern applied to the memory backend.

---

## What the de-fork changed

| Aspect | Before (`native_gym`) | Now (`gym_runner_client`) |
|---|---|---|
| Evaluation engine | private fork, hard-coded L1–L12 metadata | `amplihack_agent_eval::gym::GymRunner` (git dependency) |
| `run_scenario` / `run_suite` | always returned a degraded "not yet implemented" result | runs the scenario and returns a real, self-graded result |
| Scenario IDs | bare `"L1"`..`"L12"` | library IDs (`"L1-recall"`, `"L2-multi-source"`, …) + `"long-horizon-memory"` |
| Wiring module | `src/native_gym.rs` (engine **and** client wiring) | `src/gym_runner_client.rs` (client wiring **only**; engine is the library) |
| `lib.rs` module decl | `pub mod native_gym;` | `pub mod gym_runner_client;` |
| `rpc_subprocess_launcher::launch_gym_client_native` | `native_gym::register_gym_handlers` | `gym_runner_client::register_gym_handlers` |
| `SIMARD_SKIP_GYM=1` fast path | present in `native_gym` | preserved in `gym_runner_client` |
| Wire protocol (JSON on the client) | five-field `dimensions`, `scenario_id`, `success`, `score`, … | **byte-identical** — the adapter preserves the existing contract |

The **gym client and every downstream consumer are unchanged.**
`crate::gym_client::GymClient`, `src/gym_scoring/`, `src/gym_history/`,
`src/gym/`, and the OODA / self-improve call sites continue to depend only on
the typed `GymClient` API and the JSON wire shape. Only the *engine* behind the
three handlers moved into the library.

---

## The dependency

The engine is a pinned-revision git dependency in `Cargo.toml`, matching the
immutable-rev style already used for `amplihack-memory` and `rustyclawd-core`.
The authoritative pin is always the live line in the root `Cargo.toml`; the
snippet below records the rev current as of **2026-09-03** — the amplihack-rs
`v0.18.25` release source commit (the annotated tag `v0.18.25` dereferences to
it, and it was `main` HEAD at verification):

```toml
[dependencies]
amplihack-agent-eval = { git = "https://github.com/rysweet/amplihack-rs.git", rev = "9ee05a06eab98e9ab504a031bffaa4190700c2af" }
```

### Consumability

`amplihack-agent-eval` is a member of the `amplihack-rs` workspace, but it
consumes cleanly as a standalone git dependency:

- Its only transitive crates are light, crates.io-only: `serde`, `serde_json`,
  `thiserror`, `tracing`, `chrono`.
- The heavy workspace siblings (`lbug`, `cxx-build`) are **not** transitive
  dependencies of this crate.
- Both Simard and `amplihack-agent-eval` use Rust edition 2024, which the
  pinned toolchain (rustc ≥ 1.85; verified on 1.95.0) supports.

If a future revision of the crate pulls a heavy or unavailable transitive
dependency, treat that as a blocker: stop and document it rather than vendoring
the dependency tree.

---

## Architecture

The stable seam is the client wire protocol. Every gym call site depends only
on the typed `GymClient` client and the JSON contract, so swapping the engine
underneath required **no call-site changes**.

```text
callers (operator CLI `simard gym`, OODA, self-improve, dashboards)
        │
        ▼
   crate::gym_client::GymClient            (typed client — UNCHANGED)
        │   list_scenarios / run_scenario / run_suite
        ▼
   NativeRpcTransport "simard-gym-eval" (in-process JSON-RPC — UNCHANGED)
        │   gym.list_scenarios / gym.run_scenario / gym.run_suite
        ▼
   crate::gym_runner_client                (THIN ADAPTER — this page)
        │   • validates scenario_id / suite_id
        │   • honours SIMARD_SKIP_GYM
        │   • maps library results → ScoreDimensions wire JSON
        ▼
   amplihack_agent_eval::gym::GymRunner    (SOLE engine — the library)
        └─ ProgressiveSuite + SimpleGrader → self-graded results, JSON artifacts
```

`crate::gym_scoring/` and `crate::gym_history/` sit *beside* this path: they
consume the typed `GymScenarioResult` / `GymSuiteResult` returned by
`GymClient` and are unaffected by the engine swap.

---

## The adapter boundary (`gym_runner_client`)

`register_gym_handlers(transport: &mut NativeRpcTransport)` registers three
closures on the `simard-gym-eval` transport. Each builds (or shares) a
`GymRunner` and adapts its output to the existing wire JSON.

### Engine construction

The runner is built once with Simard defaults:

```text
GymConfig {
    output_dir:   target/simard-gym/eval         // under the existing gym output path
    agent_name:   "simard-gym-eval"
    sdk:          "mini"                           // deterministic, no LLM/network/subprocess
    grader_votes: 3
}
GymRunner::new(config)                             // loads the built-in L1–L12 + long-horizon scenarios
```

Self-grading is deterministic: it performs no network calls, spawns no
subprocess, and needs no API key. A gym run is therefore safe to execute in CI
and in tests.

### Dimension mapping

The library and the client model scoring dimensions differently, so the adapter
translates field-by-field — it never blindly re-serializes the library struct:

| Library type | Client type |
|---|---|
| `GymScenarioResult.dimensions: HashMap<String, Option<f64>>` | `ScoreDimensions` (five non-nullable `f64` fields) |
| `GymSuiteResult.dimensions: HashMap<String, f64>` | `ScoreDimensions` (five non-nullable `f64` fields) |

Mapping rules, applied to all five keys of
[`ALL_DIMENSIONS`](#scoring-dimensions):

1. Force all five keys: `factual_accuracy`, `specificity`,
   `temporal_awareness`, `source_attribution`, `confidence_calibration`.
2. A missing key or `None` → `0.0`.
3. A non-finite value (`NaN`, `+Inf`, `-Inf`) → `0.0`.
4. Clamp the result to `[0.0, 1.0]`.

This guarantees the wire JSON always carries exactly the five-field
`dimensions` object the `GymClient` client deserializes.

> **Engine fidelity (current).** For the L1–L12 levels the engine today derives
> per-dimension scores from a single self-grade: it sets `factual_accuracy` and
> `specificity` to the level's average score and leaves `temporal_awareness`,
> `source_attribution`, and `confidence_calibration` at `0.0`
> (`amplihack_agent_eval::gym::level_result_to_scenario`). The wire object still
> always carries all five keys, but only the first two carry independent signal
> at present, so `ScoreDimensions::mean()` in `gym_scoring` is correspondingly
> conservative. The long-horizon scenario instead populates whichever dimensions
> its category breakdown reports.

### Result normalization (ids and suite success)

The library's result types are *almost* the wire contract but carry two quirks
the adapter must normalize so the wire shape stays stable and self-consistent.

**1. `scenario_id` echo.** The library does **not** echo the caller's id back
faithfully. On a successful run, `level_result_to_scenario` rebuilds the id from
the numeric level (`LevelResult.level_id: u8`) as `format!("L{}", level_id)` —
so `run_scenario("L1-recall")` returns a result whose `scenario_id` is the
compact `"L1"`, while a *failing* run echoes the requested id verbatim. The
adapter removes this inconsistency: it **overrides the result `scenario_id` with
the caller's requested id** (e.g. always `"L1-recall"`). For `run_suite`, each
`scenario_results[i].scenario_id` comes back in the compact `"L{n}"` form; the
adapter maps it back to the advertised descriptive id whose prefix matches —
`"L1"` → `"L1-recall"`, `"L12"` → `"L12-far-transfer"` — using the runner's own
scenario list. The wire `scenario_id` is therefore always one of the ids
`gym.list_scenarios` advertises — never the bare `"L{n}"` form.

**2. Suite `success`.** The library's `run_suite` computes its top-level
`success` as
`!result.failed_levels.is_empty() || result.level_results.iter().all(|lr| lr.success)`
(`amplihack_agent_eval::gym::GymRunner::run_suite`). This is a **tautology: it
always evaluates to `true`.** `ProgressiveResult::add_result` records a level id
in `failed_levels` exactly when that level's result is *not* successful, so an
empty `failed_levels` implies every entry of `level_results` succeeded — meaning
whenever the first disjunct is `false`, the second is `true`. The flag is
therefore `true` on an all-pass suite, on a partially-failing suite, and on an
empty suite (`all()` over an empty iterator is `true`).

The consequence is stronger than "the flag is inverted": it carries **no
information at all**, so there is nothing to invert or correct. The adapter
ignores it outright and recomputes
`success = scenarios_passed == scenarios_total` from the per-scenario results it
already maps. (This upstream quirk should be filed as an issue against
`amplihack-rs`; until it is fixed, the adapter's recomputation is the contract
of record.) Per-scenario `success` values, which the engine computes correctly,
are passed through unchanged.

### Identity / path-traversal validation

`scenario_id` **and** `suite_id` are validated at the client boundary before
they reach the engine. Validation is **two** checks, not one:

```text
1. regex allowlist:   ^[A-Za-z0-9._-]{1,128}$   // blocks "/", "\", null bytes, absolute paths, empty
2. dot-segment guard: reject ids equal to "." or ".."
```

Both checks are required. Because `.` is an allowed character, the literal `..`
**matches** the regex — the regex alone does *not* reject it. The explicit
dot-segment guard is what actually rejects a bare `..`. This is not optional:
the library's `run_suite` performs `output_dir.join(suite_id)` with **no**
traversal check of its own (only `run_scenario` rejects `/`, `\`, and `..`
internally). The client therefore validates **both** ids itself rather than
delegating. Both `{"suite_id": "../../tmp/x"}` (blocked by the regex's `/`) and
`{"suite_id": ".."}` (blocked by the dot-segment guard) are rejected at the
boundary.

### Error mapping (honest degradation)

| Engine outcome | Client response |
|---|---|
| `Ok(result)` | RPC `result`: the normalized result. `run_scenario.success` is the engine's value; `run_suite.success` is **recomputed** as `scenarios_passed == scenarios_total` (see [Result normalization](#result-normalization-ids-and-suite-success)) |
| `Err(EvalError)` | RPC `result` with `success: false`, `error_message: Some(...)`, zeroed dimensions, and `degraded_sources` populated — the RPC envelope itself stays `Ok` |
| empty `scenario_id` | the **only** hard transport error: `error` envelope, code `-32603` |

Following Pillar 11 (honest degradation), an engine failure surfaces as a
structured failing result the caller can inspect, not a silent zero.

---

## Configuration

| Knob | Where | Effect |
|---|---|---|
| `SIMARD_SKIP_GYM=1` | environment variable | Fast path for dev/CI. The engine is **not** invoked; `run_scenario` / `run_suite` return a synthetic `success: true` result with `degraded_sources: ["SIMARD_SKIP_GYM"]`. |
| `GymConfig.output_dir` | adapter default | Root for the engine's per-scenario JSON artifacts; defaults under `target/simard-gym/eval`. |
| `GymConfig.grader_votes` | adapter default (`3`) | Number of deterministic grader votes per question. |
| `GymConfig.sdk` | adapter default (`"mini"`) | Deterministic self-grading backend; no LLM or network. |

`SIMARD_SKIP_GYM` is intended for environments that need the client to answer
quickly without doing real evaluation work. It **fails open**: it always records
`"SIMARD_SKIP_GYM"` in `degraded_sources` so the synthetic result is never
mistaken for a real one.

---

## Scenario IDs

`gym.list_scenarios` exposes the library's built-in scenarios. The IDs are the
library's descriptive identifiers (not the bare `"L1"`..`"L12"` the old fork
used):

| ID | Name |
|---|---|
| `L1-recall` | L1 Recall |
| `L2-multi-source` | L2 Multi-Source Synthesis |
| `L3-temporal` | L3 Temporal Reasoning |
| `L4-procedural` | L4 Procedural Learning |
| `L5-contradiction` | L5 Contradiction Handling |
| `L6-incremental` | L6 Incremental Learning |
| `L7-teacher-student` | L7 Teacher-Student |
| `L8-metacognition` | L8 Metacognition |
| `L9-causal` | L9 Causal Reasoning |
| `L10-counterfactual` | L10 Counterfactual Reasoning |
| `L11-novel-skill` | L11 Novel Skill Acquisition |
| `L12-far-transfer` | L12 Far Transfer |
| `long-horizon-memory` | Long-horizon memory stress test |

`run_suite` takes a `suite_id` that is a **label only** — it runs the twelve
progressive levels (`L1-recall`..`L12-far-transfer`) and tags the output
artifacts, so `scenarios_total` is `12`. The `long-horizon-memory` scenario is
**not** part of the suite (it lives outside `progressive_levels::all_levels()`);
run it on its own with `run_scenario("long-horizon-memory")`.
`run_suite("progressive")` is the canonical full-suite invocation.

No live consumer hard-codes scenario IDs against the client: `gym_history` and
`gym_client` tests use opaque keys or fixture transports. If a real consumer or
external script later breaks on the ID change, add a bare-`"L1"`..`"L12"` alias
map in the handler — but only when such a break actually surfaces.

### Scoring dimensions

The five dimensions are aligned 1:1 with the library's
`long_horizon::ALL_DIMENSIONS`:

```text
factual_accuracy, specificity, temporal_awareness, source_attribution, confidence_calibration
```

---

## Usage

### From the client (the supported path)

Callers use the unchanged typed `GymClient` client. The launcher wires the
adapter automatically:

```rust
use simard::rpc_subprocess_launcher::launch_gym_client_native;

let gym = launch_gym_client_native()?;           // registers gym_runner_client handlers

// List scenarios
for s in gym.list_scenarios()? {
    println!("{} — {} ({} questions)", s.id, s.name, s.question_count);
}

// Run one scenario
let result = gym.run_scenario("L1-recall")?;
println!("score={:.2} factual={:.2}", result.score, result.dimensions.factual_accuracy);

// Run the full suite
let suite = gym.run_suite("progressive")?;
println!("{}/{} passed, overall={:.2}",
    suite.scenarios_passed, suite.scenarios_total, suite.overall_score);
```

### The library API (the engine, for reference)

The adapter delegates to these methods on
`amplihack_agent_eval::gym::GymRunner`:

```rust
use amplihack_agent_eval::gym::{GymConfig, GymRunner};

let runner = GymRunner::new(GymConfig::default());

let scenarios = runner.list_scenarios();                 // Vec<GymScenario>
let result    = runner.run_scenario("L1-recall")?;       // Result<GymScenarioResult, EvalError>
let suite     = runner.run_suite("progressive")?;        // Result<GymSuiteResult, EvalError>
```

| Library method | Returns |
|---|---|
| `GymRunner::new(GymConfig)` | runner with built-in L1–L12 + long-horizon scenarios |
| `GymRunner::with_scenarios(GymConfig, Vec<LevelScenario>)` | runner with custom scenarios (testing / extension) |
| `list_scenarios()` | `Vec<GymScenario>` |
| `run_scenario(&str)` | `Result<GymScenarioResult, EvalError>` |
| `run_suite(&str)` | `Result<GymSuiteResult, EvalError>` |

### Skipping evaluation in CI

```bash
SIMARD_SKIP_GYM=1 cargo run --quiet -- gym run L1-recall
# → synthetic success, degraded_sources: ["SIMARD_SKIP_GYM"], engine not invoked
```

---

## Wire protocol

The three `gym.*` methods, their parameters, and their exact JSON result shapes
are specified in the
[RPC Wire Protocol Reference](../reference/rpc-wire-protocol.md#gym-rpc-methods).
The de-fork keeps that contract byte-stable.

---

## Testing

The adapter owns the tests that used to live in `native_gym`, plus new
adapter-specific coverage. They are deterministic and run in CI:

| Concern | Test |
|---|---|
| Dimension mapping (5 keys forced, `None`→`0.0`) | `dimensions_force_all_five_keys` |
| Non-finite sanitisation + clamp | `dimensions_nan_inf_become_zero`, `dimensions_clamped_to_unit_interval` |
| Result `scenario_id` echoes the requested id (not the engine's `L{n}`) | `run_scenario_echoes_requested_id` |
| Suite `success` recomputed (`scenarios_passed == scenarios_total`), not the engine flag | `run_suite_success_requires_all_passed` |
| `suite_id` traversal guard | `run_suite_rejects_path_traversal` (`"../../tmp/x"`) |
| `suite_id` dot-segment guard | `run_suite_rejects_dot_segment` (`".."` — the regex blind spot) |
| `scenario_id` traversal guard | `run_scenario_rejects_path_traversal` |
| Empty `scenario_id` → hard error | `run_scenario_empty_id_is_transport_error` |
| `SIMARD_SKIP_GYM` fast path | five `#[serial]` env-var tests (migrated from `native_gym`) |
| Engine error → failing result | `run_scenario_engine_error_maps_to_failure` |
| Round-trip wire stability | the unchanged `gym_client.rs` deserialization tests |

The five `SIMARD_SKIP_GYM` tests mutate a process-wide env var and are
annotated `#[serial]`; see
[CI-resilient test patterns → Pattern 3](../testing/ci-resilient-test-patterns.md#pattern-3-serial-for-env-var-mutating-tests).

```bash
# Adapter unit tests
cargo test -p simard --lib gym_runner_client::tests

# The full gym module set + the unchanged client contract tests
cargo test -p simard --lib gym_runner_client gym_client gym_scoring gym_history

# Release build gate
cargo build --release
```

---

## Security considerations

- **Path traversal (critical).** Both `scenario_id` and `suite_id` are
  allowlisted at the client before any `output_dir.join`. Do not rely on the
  library's internal checks — `run_suite` has none.
- **Result integrity.** Non-finite floats are sanitised and dimensions clamped
  to `[0,1]` before serialization, preserving the non-nullable five-field
  `ScoreDimensions` contract.
- **No secrets / PII.** Payloads and `error_message` strings carry no
  credentials; `output_dir` is workspace-scoped and `agent_name` is
  Simard-controlled.
- **Supply chain.** The engine is pinned to an immutable git revision (same
  posture as the other git dependencies), covered by `cargo audit`, and
  performs no build-time code generation.
- **Fail-open skip path.** `SIMARD_SKIP_GYM` is a dev/CI convenience only; it
  always tags `degraded_sources` so a synthetic result is never confused with a
  measured one.

---

## See also

- [RPC Transport Pattern](rpc-pattern.md) — the transport abstraction the gym client
  uses.
- [RPC Wire Protocol Reference](../reference/rpc-wire-protocol.md) — the
  exact `gym.*` method contracts.
- [Library-backed Cognitive Memory](cognitive-memory-library-adapter.md) — the
  earlier de-fork that established this adapter pattern.
