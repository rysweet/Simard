# Gym Benchmark Scenario Gap Analysis

Status: design draft (no scenarios implemented in this cycle)
Owner: Gym / eval suite
Related: `src/gym/scenarios.rs`, `src/gym/types.rs`

## 1. Survey of the existing suite

The gym suite currently lives in `src/gym/` and exposes its scenarios through a
single static array:

- `src/gym/scenarios.rs` — `const BENCHMARK_SCENARIOS: [BenchmarkScenario; 118]`
- `src/gym/types.rs` — `pub enum BenchmarkClass { ... }` (31 variants, lines 9–42)
- Supporting modules: `executor.rs`, `executor_metrics.rs`, `reporting.rs`,
  `mod.rs`, plus `tests_*.rs` companions.

There is **no `gym/` top-level directory** and no per-scenario YAML/markdown
files — every scenario is a literal `BenchmarkScenario { ... }` struct in
`scenarios.rs`. Scenarios are tagged with a single coverage dimension today
(`class: BenchmarkClass`); difficulty, domain, and language are implicit in the
prose `objective` field.

### 1.1 Scenario count by class (118 total)

| Class                          | Count |
| ------------------------------ | ----: |
| Refactoring                    |     9 |
| PerformanceAnalysis            |     7 |
| TestWriting                    |     5 |
| DataModeling                   |     5 |
| DataMigration                  |     5 |
| SecurityAudit                  |     4 |
| RepoExploration                |     4 |
| ErrorHandling                  |     4 |
| ConfigManagement               |     4 |
| CodeReview                     |     4 |
| BugFix                         |     4 |
| SessionQuality                 |     3 |
| SafeCodeChange                 |     3 |
| ReleaseManagement              |     3 |
| RateLimiting                   |     3 |
| ObservabilityInstrumentation   |     3 |
| MigrationPlanning              |     3 |
| InternationalizationReview     |     3 |
| IncidentResponse               |     3 |
| FeatureFlagging                |     3 |
| EventSourcing                  |     3 |
| Documentation                  |     3 |
| DependencyUpgrade              |     3 |
| DependencyAnalysis             |     3 |
| Debugging                      |     3 |
| DatabaseSchemaChange           |     3 |
| ConcurrencyAnalysis            |     3 |
| CicdPipeline                   |     3 |
| ChaosEngineering               |     3 |
| CachingStrategy                |     3 |
| AccessibilityReview            |     3 |
| ApiDesign                      |     3 |

### 1.2 Implicit coverage dimensions

Reading the `objective` strings, current scenarios cluster as follows:

- **Task type:** mostly *analysis* and *single-file-change* tasks. Few require
  coordinated multi-file edits, none span multiple repositories or workspaces.
- **Difficulty:** primarily small/medium. Most fit in a single LLM context
  window and do not require iterative tool use beyond one or two probes.
- **Domain:** the target repo is almost always Simard itself (Rust, single
  crate). Cargo, gh, and rg are the dominant tools.
- **Language:** Rust-only. There are no scenarios that exercise polyglot
  reasoning (e.g. Python tests + Rust core), no JS/TS/Go/Python targets, and
  no scenarios that require reading non-Rust dependency manifests.
- **Context length:** scenarios target named functions or short files. Nothing
  forces the agent to navigate a long-context corpus (>50k tokens of mixed
  source) and synthesise across distant call sites.
- **Failure surfaces:** plenty of "find a defect" prompts, but few "given this
  failing test output / stack trace, fix it and confirm the suite is green"
  closed-loop tasks.

## 2. Identified gaps

| Gap                                  | Why it matters                                                                  |
| ------------------------------------ | ------------------------------------------------------------------------------- |
| Multi-file refactors                 | Real refactors touch ≥3 files; current Refactoring scenarios stay single-file.  |
| Test-failure triage (closed-loop)    | Forces the agent to read failing output, locate the defect, and re-run tests.   |
| Dependency upgrades (compile + test) | Existing DependencyUpgrade entries are advisory; none execute the upgrade.      |
| Doc generation from code             | Documentation scenarios target one function — nothing builds a module overview. |
| Performance regression triage        | PerformanceAnalysis is static review only; no benchmark-vs-baseline scenarios.  |
| Security fix (not just audit)        | SecurityAudit produces reports; nothing requires landing a remediation patch.   |
| Cross-language tasks                 | Suite is Rust-only; agents are not exercised on Python/JS/Go interop.           |
| Long-context navigation              | No scenario forces synthesis across a large corpus (>50k tokens of source).     |
| Flaky-test stabilisation             | Distinct from triage: requires identifying nondeterminism, not just a bug.      |
| Public API deprecation rollout       | Touches code, docs, changelog, and call sites — exercises planning + execution. |

## 3. Proposed new scenarios (≥8)

Each proposal lists: **class** (existing variant or new variant suggestion),
**coverage dimensions** (task type / difficulty / domain / language /
context-length), and a draft **objective** suitable for `BenchmarkScenario`.

> Implementation note: proposals 4, 6, 7, and 9 likely require new
> `BenchmarkClass` variants (`PerfRegression`, `SecurityFix`,
> `CrossLanguage`, `LongContextNavigation`). These should be added to
> `src/gym/types.rs` and surfaced in reporting before scenarios land.

### 3.1 Multi-file refactor: extract a shared helper across modules

- **Class:** `Refactoring` (existing)
- **Dimensions:** multi-file edit / medium / Simard / Rust / medium context
- **Objective draft:** "Identify a logging helper duplicated in at least three
  modules under `src/`. Extract it into `src/util/logging.rs`, update every
  call site, ensure `cargo check` passes, and confirm no public API changed.
  Output the diff summary and the verification commands you ran."

### 3.2 Test-failure triage and fix (closed loop)

- **Class:** `BugFix` (existing) — flag with new `closed_loop: true` metadata
- **Dimensions:** debugging / medium / Simard / Rust / short context
- **Objective draft:** "Given the failing output of `cargo test -p simard
  -- gym::tests_scenarios::scenarios_have_unique_ids`, locate the cause in
  `src/gym/scenarios.rs`, propose a minimal fix, apply it, and rerun the test
  to confirm it passes. Report the original failure, the fix, and the
  post-fix test summary."

### 3.3 Dependency upgrade: bump a transitive crate and resolve breakage

- **Class:** `DependencyUpgrade` (existing)
- **Dimensions:** dependency work / medium / Simard / Rust + Cargo.lock /
  medium context
- **Objective draft:** "Bump `serde` to the latest minor release in
  `Cargo.toml`, run `cargo update -p serde`, then run `cargo check --all
  --tests`. If the upgrade breaks compilation, identify each call site that
  needs adjustment and propose the minimal patch. Otherwise, summarise the
  changed lockfile entries and the new MSRV implications."

### 3.4 Documentation generation: synthesise a module README

- **Class:** `Documentation` (existing)
- **Dimensions:** synthesis / medium / Simard / Rust / long context
- **Objective draft:** "Read every file in `src/gym/` (≈3,800 lines). Produce
  a `src/gym/README.md` that documents: (1) the public surface, (2) how
  scenarios are added, (3) how the executor records metrics, (4) the
  reporting pipeline. The README must reference at least 6 distinct symbols
  by name and not exceed 300 lines."

### 3.5 Performance regression triage

- **Class:** *new* `PerfRegression` (proposed) — falls back to
  `PerformanceAnalysis` if not added
- **Dimensions:** measurement + analysis / hard / Simard / Rust / short context
- **Objective draft:** "Given two captured benchmark JSON outputs (baseline
  and candidate) for `cargo bench --bench gym_executor`, identify which
  benchmark regressed by >10%, propose a hypothesis for the cause based on
  the relevant source under `src/gym/executor.rs`, and recommend a
  diagnostic next step (e.g., a flamegraph capture)."

### 3.6 Security fix (apply a remediation patch)

- **Class:** *new* `SecurityFix` (proposed) — distinct from `SecurityAudit`
- **Dimensions:** code change / medium / Simard / Rust / short context
- **Objective draft:** "An audit found that `src/runtime/session.rs` writes
  the raw `provider_api_key` field to a tracing log at debug level. Apply
  the minimal patch to redact the value (preserve the log line for
  observability), add a regression test that asserts the log output does
  not contain the secret, and confirm `cargo test -p simard
  runtime::session::redaction` passes."

### 3.7 Cross-language task: keep Rust enum and Python bindings in sync

- **Class:** *new* `CrossLanguage` (proposed)
- **Dimensions:** polyglot edit / medium / Simard + python/ / Rust + Python /
  medium context
- **Objective draft:** "A new variant `BenchmarkClass::PerfRegression` was
  added in `src/gym/types.rs`. Update the corresponding Python enum in
  `python/simard_gym/classes.py` (or the closest equivalent), regenerate
  the stub file, and run `python -m pytest python/tests/test_classes.py` to
  confirm parity. Report both diffs and the test summary."

### 3.8 Long-context navigation: trace an end-to-end request

- **Class:** *new* `LongContextNavigation` (proposed)
- **Dimensions:** synthesis / hard / Simard / Rust / very long context
- **Objective draft:** "Starting from `bin.js`, trace the full call chain that
  executes when a user runs `simard recipe run smart-orchestrator -c
  task_description=...`. Enumerate every file touched (expect ≥15), the
  function entered in each, and the side effects (filesystem writes,
  subprocess launches, network calls). Produce a numbered call-graph and
  cite file:line for each step."

### 3.9 Flaky-test stabilisation

- **Class:** `BugFix` (existing) — tagged with new `flake: true` metadata
- **Dimensions:** debugging / hard / Simard / Rust / short context
- **Objective draft:** "Given a test that passes 8/10 runs of `cargo test
  -p simard runtime::session::tests::session_replay_round_trip --
  --test-threads=1 --nocapture` but fails intermittently with a timing
  assertion, identify the source of nondeterminism (clock, ordering,
  filesystem race), propose a fix that does not weaken the assertion, and
  describe how you would prove the fix removed the flake."

### 3.10 Public API deprecation rollout

- **Class:** `ApiDesign` (existing) — multi-file
- **Dimensions:** coordinated edit / hard / Simard / Rust + docs / medium
  context
- **Objective draft:** "Mark `pub fn run_benchmark_suite` in `src/gym/mod.rs`
  as `#[deprecated(since = \"x.y.z\", note = \"use run_benchmark_suite_v2\")]`.
  Add a thin wrapper `run_benchmark_suite_v2` that takes the new options
  struct. Update every internal caller, add a CHANGELOG.md entry under
  `## Unreleased / Deprecated`, and ensure `cargo check --all -- -D
  warnings` still passes (i.e., no internal call sites trigger the
  deprecation lint)."

## 4. Recommended sequencing

1. Land the new `BenchmarkClass` variants (`PerfRegression`, `SecurityFix`,
   `CrossLanguage`, `LongContextNavigation`) plus reporting buckets.
2. Add scenarios 3.1, 3.2, 3.4, 3.10 — they reuse existing classes and can
   ship without scoring changes.
3. Add scenarios 3.5, 3.6, 3.7, 3.8 — these depend on step 1.
4. Add 3.3 and 3.9 last — both are closed-loop and may require executor
   work to capture and replay test output.

Each scenario above should become its own GitHub issue under the label
`gym-scenario`, with this document linked as the design rationale.

## 5. Out of scope for this cycle

- Implementation of any scenario.
- Changes to `src/gym/scenarios.rs` or `src/gym/types.rs`.
- Scoring rubric changes.
- Filing the per-scenario tracking issues (follow-up cycle).
