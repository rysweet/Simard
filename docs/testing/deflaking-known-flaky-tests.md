---
title: De-flaking the known flaky tests (prompt-delivery env race + goal-board state-root race)
description: >
  How the three known-flaky tests were made deterministic under parallel
  `cargo test`: the prompt_delivery env-var race (issue #2412) is closed by
  sharing the prompt_delivery_env serial key on the two Auto-mode size tests,
  and the goal-board state-root race (issues #2408 / #2384) is closed by
  threading an explicit state root through the goal-CRUD handlers (the `_at`
  cores) so the dashboard's read/write path never resolves the root ambiently
  from the process environment during a test.
last_updated: 2026-06-28
review_schedule: when a new env-reading handler is added to the dashboard, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-ooda-config-default.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
  - ../reference/goal-board-api.md
  - ../prompt-delivery.md
---

# De-flaking the known flaky tests

This page documents the finished state of the work that made three known-flaky
tests deterministic under parallel `cargo test`. It is the test-author and
reviewer contract for the two isolation mechanisms that closed the flakes, and
the verification gate that keeps them closed.

The flakes and their fixes:

| Issue | Flaky test | Root cause | Fix |
| ----- | ---------- | ---------- | --- |
| [#2412](https://github.com/rysweet/Simard/issues/2412) | `prompt_delivery::applied_mode_reports_inline_for_small_prompt`, `…_tempfile_for_large_prompt` | `Auto` mode reads `AMPLIHACK_PROMPT_DELIVERY` (`ENV_OVERRIDE`) and could observe a leaked `inline`/`tempfile` value set by a *concurrent* env test | Share the `prompt_delivery_env` serial key on the two Auto-mode size tests |
| [#2408](https://github.com/rysweet/Simard/issues/2408) · [#2384](https://github.com/rysweet/Simard/issues/2384) | `operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud` (and siblings) | The goal-CRUD handlers resolved `SIMARD_STATE_ROOT` ambiently via `resolve_state_root()`, so a concurrent `setenv`/`remove_var` in an unrelated lib-binary test could tear the read and route a handler at the wrong state root | Thread an explicit state root through the goal-CRUD path (the `_at` handler cores) so the exercised path never reads the env |

Both fixes preserve full test parallelism. Neither blanket-serializes the suite,
and the de-flake mechanisms themselves do not change production behaviour: the
HTTP handlers still resolve the state root from the environment exactly as
before, and `select_mode` still reads the same env override. The suite is simply
prevented from racing on those reads.

> **One coupled bug fix.** Wiring `remove_goal` through the explicit-root core
> surfaced a pre-existing dashboard bug: it persisted removals through the plain
> *merge-on-write* save, which resurrected the just-removed goal. `remove_goal`
> now uses `dashboard_save_goal_board_with_removals`, so dashboard removals
> actually persist — matching the CLI `simard goal remove` contract. This is the
> only production-behaviour change, and it is a fix. See the goal-board section.

> **TL;DR**
>
> - **#2412:** add `#[serial(prompt_delivery_env)]` to the two Auto-mode size
>   tests in `tests/prompt_delivery.rs`. They consult `ENV_OVERRIDE` through
>   `select_mode`, so they must never run concurrently with the four tests that
>   mutate `ENV_OVERRIDE`.
> - **#2408 / #2384:** each goal-CRUD handler is split into a thin ambient
>   **wrapper** (resolves the root once) and an env-free **`_at(state_root: &Path)`
>   core**. Tests call the `_at` cores with `HermeticState::state_root()`, so the
>   goal-board read/write path is driven by an explicit, test-owned path rather
>   than the process-global environment.
> - **Gate:** ≥20 consecutive parallel runs of the three tests with zero
>   failures, plus a green `cargo test --all-features`.

---

## Fix #2412 — the prompt-delivery env race

### The race this eliminates

`tests/prompt_delivery.rs` runs many `#[test]` functions concurrently in one
binary. Four of them mutate the process-global override variable
`AMPLIHACK_PROMPT_DELIVERY` (exported as
`simard::prompt_delivery::ENV_OVERRIDE`) via `set_var`/`remove_var`:

- `env_override_forces_inline_for_short_prompt` — sets `inline`
- `env_override_forces_tempfile_for_short_prompt` — sets `tempfile`
- `invalid_env_value_falls_back_to_auto_without_panic` — sets `totally-bogus`
- `caller_override_ignores_env_var` — sets `tempfile`, then asserts an
  *explicit* mode beats the override

These four already carry `#[serial(prompt_delivery_env)]`, so they never run
concurrently *with each other*. (Note the last one passes an explicit mode yet
is still keyed — it is keyed because it **writes** the env, not because it reads
it.) The gap was the two **size-based** Auto tests:

- `applied_mode_reports_inline_for_small_prompt`
- `applied_mode_reports_tempfile_for_large_prompt`

These assert that `Auto` picks `Inline` for a tiny prompt and `TempFile` for a
large one **based on size alone**. But `Auto` consults the env override *first*
(via `select_mode` → `std::env::var(ENV_OVERRIDE)`). When one of the four
env-mutating tests had `AMPLIHACK_PROMPT_DELIVERY=inline` set at the instant a
size test ran on another thread, the size test observed the leaked override and
its mode assertion failed. That is the ~intermittent failure reported in
[#2412](https://github.com/rysweet/Simard/issues/2412).

### The rule

> **Every `#[test]` in `tests/prompt_delivery.rs` that reaches `select_mode`
> (directly, or via `apply_std`/`apply_tokio` with `PromptDelivery::Auto`)
> shares the `prompt_delivery_env` serial key** — because that path reads
> `ENV_OVERRIDE`, and an env read must never be concurrent with an env write.

Tests that pass an **explicit** `PromptDelivery::Stdin` / `PromptDelivery::TempFile`
/ `PromptDelivery::Inline` never consult the override and stay parallel
(unkeyed). Only `Auto`-mode tests and the override-mutating tests need the key.

### What the finished test looks like

```rust
use serial_test::serial;
use simard::prompt_delivery::{PromptDelivery, apply_std, STDIN_PREFERRED_MAX_BYTES};

#[test]
#[serial(prompt_delivery_env)] // Auto reads ENV_OVERRIDE — must not race env writers
fn applied_mode_reports_inline_for_small_prompt() {
    let mut cmd = std::process::Command::new("/bin/cat");
    let applied = apply_std(&mut cmd, b"tiny", PromptDelivery::Auto).unwrap();
    assert_eq!(applied.mode(), PromptDelivery::Inline);
    assert!(applied.temp_path().is_none());
}

#[test]
#[serial(prompt_delivery_env)] // Auto reads ENV_OVERRIDE — must not race env writers
fn applied_mode_reports_tempfile_for_large_prompt() {
    let mut cmd = std::process::Command::new("/bin/cat");
    let big = vec![b'x'; STDIN_PREFERRED_MAX_BYTES + 16];
    let applied = apply_std(&mut cmd, &big, PromptDelivery::Auto).unwrap();
    assert_eq!(applied.mode(), PromptDelivery::TempFile);
    assert!(applied.temp_path().is_some());
}
```

The `prompt_delivery_env` key is the same one already used by the inline unit
tests in `src/prompt_delivery/mod.rs`, so the lock is shared across the unit and
integration test surfaces.

### Adding a new prompt-delivery test

Ask one question: **does this test cause `Auto` to be selected, or does it set
`AMPLIHACK_PROMPT_DELIVERY`?**

- **Yes →** annotate it `#[serial(prompt_delivery_env)]`.
- **No (explicit mode, no env mutation) →** leave it unkeyed so it runs in
  parallel.

---

## Fix #2408 / #2384 — the goal-board state-root race

[#2408](https://github.com/rysweet/Simard/issues/2408) and
[#2384](https://github.com/rysweet/Simard/issues/2384) are the same defect:
`full_goal_lifecycle_crud` (and its `tests_goals_crud` siblings) drives the
dashboard goal-CRUD handlers in `src/operator_commands_dashboard/goals.rs`. Each
handler resolved its state root **ambiently** through
`routes::resolve_state_root()`, which reads `SIMARD_STATE_ROOT` from the
process-global environment:

```rust
// routes.rs — ambient resolution (production default, unchanged)
pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}
```

`HermeticState` pins `SIMARD_STATE_ROOT` to a per-test `TempDir` for its
lifetime, but the read inside the handler is still a *process-global* `getenv`.
A concurrent `setenv`/`remove_var` in an unrelated lib-binary test (glibc
`setenv`/`getenv` are not thread-safe) could tear that read, so a handler
occasionally resolved to the wrong state root mid-lifecycle, the board snapshot
came back unexpectedly empty, and the test's final assertion (`board.active.len() == 3`)
failed.

The [`cognitive_memory` serial isolation contract](./cognitive-memory-serial-isolation.md)
serializes all env-*mutating* lib-binary tests under one key and is the suite's
primary defence against this class. This fix adds **true isolation in depth** so
the goal-CRUD path is correct regardless of any concurrent env tear: the
exercised path no longer reads the environment at all.

### The mechanism: ambient wrapper + env-free `_at` core

Each goal-CRUD handler is split into two functions:

1. A thin **wrapper** keeping the original Axum signature. It resolves the state
   root **exactly once** via `resolve_state_root()` and delegates to the core.
   This is the only HTTP entry point; routing is unchanged.
2. An env-free **`_at` core** that takes the resolved root as an explicit
   `state_root: &Path` parameter and does all the work — load board, mutate,
   save — using *only* that path.

The shared `load_board_or_empty()` helper is **replaced** by an env-free
`load_board_or_empty_at(state_root)`, so the single ambient resolution is never
duplicated inside a core. Every caller (`add_goal_at`, `remove_goal_at`, …) now
threads its explicit root in, leaving the old ambient wrapper with no callers —
so it is dropped (an unused wrapper would trip `cargo clippy -- -D warnings`):

```rust
// goals.rs

/// Env-free helper. The board is read from `state_root` ONLY — never from the
/// process environment. `state_root` MUST come from `resolve_state_root()`
/// (production) or `HermeticState::state_root()` (tests), never from request
/// data.
fn load_board_or_empty_at(state_root: &std::path::Path) -> GoalBoard {
    dashboard_goal_board_snapshot(state_root).unwrap_or_default()
}
```

> **The single-resolution invariant.** A handler resolves the state root **once**
> and threads that one `&Path` into every load and save it performs. Before this
> fix, each mutating handler resolved the root *twice* — once directly (e.g.
> `add_goal`'s `let state_root = resolve_state_root();`) and once again inside the
> `load_board_or_empty()` it called — so the load and the save could observe two
> different roots if an env tear landed between them. That double read is exactly
> what this fix removes: the `_at` core takes the path as a parameter, so load and
> save are guaranteed to use the same root.

### API: the goal-CRUD handler cores

All seven handlers gain a `pub(crate)` **async** `_at` core. The wrappers keep
their existing names and Axum extractor signatures; the cores append `_at`, take
`state_root: &Path` as the **first** parameter, and keep the wrapper's remaining
extractor parameters (`Json<Value>`, `Path<String>`) verbatim. Each core is thus
an exact **async twin** of its wrapper — the only difference is *where the state
root comes from* — and a wrapper delegates with `.await`. Keeping the extractor
parameter types lets tests call a core directly with constructed extractors
(`Json(json!(…))`, `Path("…".into())`), exercising the same code the HTTP path
runs.

| Wrapper (HTTP entry, ambient) | Core (env-free, explicit root) |
| ----------------------------- | ------------------------------ |
| `goals() -> Json<Value>` | `async goals_at(state_root: &Path) -> Json<Value>` |
| `seed_goals() -> Json<Value>` | `async seed_goals_at(state_root: &Path) -> Json<Value>` |
| `add_goal(Json(body): Json<Value>) -> Json<Value>` | `async add_goal_at(state_root: &Path, Json(body): Json<Value>) -> Json<Value>` |
| `remove_goal(Path(id): Path<String>) -> Json<Value>` | `async remove_goal_at(state_root: &Path, Path(id): Path<String>) -> Json<Value>` |
| `update_goal_status(Path(id), Json(body)) -> Json<Value>` | `async update_goal_status_at(state_root: &Path, Path(id): Path<String>, Json(body): Json<Value>) -> Json<Value>` |
| `promote_backlog_item(Path(id): Path<String>) -> Json<Value>` | `async promote_backlog_item_at(state_root: &Path, Path(id): Path<String>) -> Json<Value>` |
| `demote_goal(Path(id): Path<String>) -> Json<Value>` | `async demote_goal_at(state_root: &Path, Path(id): Path<String>) -> Json<Value>` |

Wrapper contract (illustrative — `add_goal`):

```rust
/// HTTP entry point. Resolves the state root once from the environment and
/// delegates to the env-free core. Bound by the dashboard router.
pub(crate) async fn add_goal(Json(body): Json<Value>) -> Json<Value> {
    add_goal_at(&resolve_state_root(), Json(body)).await
}

/// Env-free core. Reads and writes the goal board through `state_root` only.
///
/// # State-root source invariant
/// `state_root` is trusted-internal: it MUST originate from
/// `resolve_state_root()` (production) or `HermeticState::state_root()`
/// (tests). It MUST NEVER be derived from request data — wiring a
/// request-controlled path here would expose path traversal.
pub(crate) async fn add_goal_at(state_root: &Path, Json(body): Json<Value>) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);
    // … validate body, mutate board (unchanged logic) …
    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => Json(json!({ "status": "ok", /* … */ })),
        Err(e) => Json(json!({ "status": "error", "error": format!("save failed: {e}") })),
    }
}
```

The cores are `async` to mirror the wrapper signatures Axum expects; the
goal-board read/write path itself is synchronous, so the cores simply `.await`
nothing of their own. All existing extractor validation and status-code
behaviour is preserved verbatim — the only change is *where the state root comes
from*.

> **Removal is the one exception to the plain save.** `remove_goal_at` persists
> through `dashboard_save_goal_board_with_removals(state_root, &board, &[id])`
> instead of `dashboard_save_goal_board`. The plain save is *merge-on-write*: it
> unions the in-flight board with the persisted snapshot so a concurrent writer's
> goals are never lost (#1915). That same merge would **resurrect** a goal the
> operator just removed (its id is absent from the in-flight board, so
> `merge_boards` keeps the persisted copy). Force-removing the id defeats the
> resurrection, matching the CLI `simard goal remove` path (#1923 / #1925 /
> #1926). The other five mutating cores are moves or additions that merge-on-write
> resolves correctly, so they keep the plain save.

The `# State-root source invariant` doc-comment shown above is **required on all
seven cores**, not just the illustrative `add_goal_at`: it is the only thing that
stops a future caller from wiring a request-controlled path into a core and
opening a path-traversal hole. Treat a missing invariant comment on any `_at`
core as a review blocker.

### What the finished tests look like

Two complementary tests pin the contract:

1. The original `full_goal_lifecycle_crud` is **left unchanged** — it still drives
   the **wrappers** (`seed_goals()`, `add_goal()`, …), so it keeps coverage of the
   exact ambient-resolution path production runs. Its determinism is guaranteed by
   the `cognitive_memory` serial group: while it holds that lock no other
   lib-binary test can mutate `SIMARD_STATE_ROOT` (the `serial_guard` meta-test
   proves every env mutator is in the group).
2. A new sibling, `full_goal_lifecycle_crud_via_at_is_env_independent`, drives the
   whole lifecycle through the **`_at` cores** with the hermetic root while a
   second `HermeticState` (`decoy`) deliberately points the ambient
   `SIMARD_STATE_ROOT` at a *different* directory. A correct env-free core touches
   only the explicit `target` root; an ambient one would touch the `decoy`, so the
   final decoy-empty assertion would fail. This is the isolation-in-depth proof:

```rust
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn full_goal_lifecycle_crud_via_at_is_env_independent() {
    let target = HermeticState::new();
    let _mem = init_empty_board(&target);
    let decoy = HermeticState::new(); // ambient SIMARD_STATE_ROOT now != target
    let root = target.state_root();

    // Cores are async and take the wrapper's extractors verbatim.
    assert_eq!(seed_goals_at(root).await.0["status"], "ok");
    let r = add_goal_at(root, Json(json!({"description": "New backlog idea", "type": "backlog"}))).await;
    assert_eq!(r.0["status"], "ok");
    let backlog_id = r.0["id"].as_str().unwrap().to_string();

    // promote → in-progress → completed → demote → remove, all on `root`.
    assert_eq!(promote_backlog_item_at(root, Path(backlog_id.clone())).await.0["status"], "ok");
    assert_eq!(update_goal_status_at(root, Path(backlog_id.clone()), Json(json!({"status": "in-progress"}))).await.0["status"], "ok");
    assert_eq!(update_goal_status_at(root, Path(backlog_id.clone()), Json(json!({"status": "completed"}))).await.0["status"], "ok");
    assert_eq!(demote_goal_at(root, Path(backlog_id.clone())).await.0["status"], "ok");
    assert_eq!(remove_goal_at(root, Path(backlog_id)).await.0["status"], "ok");

    // Only the 3 seeded goals remain — in the explicit root.
    let board = dashboard_goal_board_snapshot(root).unwrap();
    assert_eq!(board.active.len(), 3, "only seeded goals should remain active");

    // The ambient/decoy root must never have been written.
    let decoy_board = dashboard_goal_board_snapshot(decoy.state_root()).unwrap_or_default();
    assert!(decoy_board.active.is_empty() && decoy_board.backlog.is_empty());
}
```

`HermeticState` and `#[serial_test::serial(cognitive_memory)]` are **retained** on
both tests, not removed: they keep the shared in-process cognitive-memory writer
registered and keep the tests inside the watched serial group enforced by the
[`serial_guard` meta-test](./cognitive-memory-serial-isolation.md). Removing
either would regress that meta-test. The `_at` threading is isolation *in
addition to* serialization, not a replacement for it. Per-core sibling tests
(`seed_goals_at_writes_to_explicit_root_not_ambient_env`, `remove_goal_at_uses_explicit_root`, …)
pin each core against the same `target`-vs-`decoy` contract.

### Writing a new goal-CRUD handler test

- Allocate a `HermeticState` and bind it for the whole test.
- Bind the in-process writer (`let _mem = init_empty_board(&state);`).
- Keep `#[serial_test::serial(cognitive_memory)]`.
- To prove env-independence, call the **`_at` cores** with `state.state_root()`
  (optionally with a `decoy` `HermeticState`); to cover the production HTTP path,
  call the **wrappers** instead. Both are valid — choose per what the test asserts.

### Production callers are unaffected

The dashboard router still binds the **wrappers** (`goals`, `add_goal`, …). Each
wrapper resolves the state root from the environment once, exactly as before, so
the OODA daemon, bootstrap assembly, and standalone `dashboard serve` behave
identically. Only the *test* path opts into explicit-root threading.

---

## Verification gate

A change to either fix is merge-ready only when all of the following pass.

### Targeted stress (the de-flake proof)

Run the three formerly-flaky tests ≥**20 consecutive** times under parallel
execution with **zero** failures. The repo's pre-push
`cargo-test-race-subset` approach (issue
[#1631](https://github.com/rysweet/Simard/issues/1631)) is the reference:

```bash
# 20 parallel runs of the WHOLE prompt-delivery binary (PR A).
# Do NOT filter to `applied_mode_reports_`: the race only exists when the
# env-mutating writers (`env_override_forces_*`, `invalid_env_value_*`) run
# concurrently with the two size readers. A name-filtered run drops the writers,
# so it would pass even without the `#[serial]` fix and prove nothing.
for i in $(seq 1 20); do
  cargo test --test prompt_delivery -- --test-threads=8 \
    || { echo "FLAKE on run $i"; break; }
done

# 20 parallel runs of the WHOLE lib-test binary (PR B). The goal-board race only
# surfaces when an unrelated env-mutating lib test runs concurrently, so the
# subset must run inside the full binary, never name-filtered.
#
# On a developer box that has a real `~/.simard/prompt_assets` install, two
# UNRELATED test groups fail environment-specifically (they read the live install
# instead of a temp dir and do not isolate $HOME) — they pass in clean CI. Skip
# them so the loop reports only the de-flake signal:
for i in $(seq 1 20); do
  ./target/debug/deps/simard-<hash> \
    --skip ooda_brain::recipe_brain::tests::resolve_recipe_path \
    --skip ooda_brain::recipe_brain::tests::new_returns_none_when \
    --skip base_type_copilot::tests \
    || { echo "FLAKE on run $i"; break; }
done
```

**Run the whole binary, never a single-test filter.** Both races only surface
when an unrelated env-*mutating* test runs concurrently with the test under
proof, so a filtered run that excludes those writers can pass by luck while the
real binary still flakes. That is why the PR A loop runs the entire
`prompt_delivery` integration binary (writers + size readers together) and the
PR B loop runs the entire lib-test binary (the goal-board subset alongside every
other env-mutating lib test), not either subset in isolation.

### Full suite

```bash
cargo test --all-features
```

Must pass with no regressions, including the
[`serial_guard` meta-test](./cognitive-memory-serial-isolation.md) that enforces
the `cognitive_memory` watched-env contract.

### CI

The existing `.github/workflows/verify.yml` → pre-commit pipeline must go green.
**No CI-workflow edits** are part of this work unless strictly required.

---

## Configuration & environment

These fixes add no new runtime configuration. The relevant variables are:

| Variable | Role | Notes |
| -------- | ---- | ----- |
| `AMPLIHACK_PROMPT_DELIVERY` (`ENV_OVERRIDE`) | Forces `Auto` mode selection in `prompt_delivery` | Read by `select_mode`; the `prompt_delivery_env` serial key prevents test reads from racing test writes |
| `SIMARD_STATE_ROOT` (`STATE_ROOT_ENV`) | Ambient state-root source for `resolve_state_root()` | Still read by the handler **wrappers** in production; the `_at` cores take it as an explicit `&Path` and never read the env |
| `SIMARD_MEMORY_SOCKET` (`MEMORY_SOCKET_ENV`) | Cognitive-memory socket path | Unset by `HermeticState` so the socket follows the state root |

### Local stress-run memory budget

The saved workstation preference for memory-heavy local runs is
`NODE_OPTIONS=--max-old-space-size=32768` (used by the tooling that drives the
stress loops, not by `cargo` itself). To change it, edit
`~/.amplihack/config`. It is not required for CI and does not affect the Rust
tests.

---

## Scope

**In scope:** the two isolation mechanisms above, their tests, the one coupled
removal bug fix (`remove_goal` now persists removals via
`save_goal_board_with_removals`, surfaced while wiring `remove_goal_at`), and this
page.

**Out of scope (confirmed):** snapshot/redeploy docs; refactoring `select_mode`
or the goal-board logic beyond the isolation needs and that coupled removal fix;
unrelated flaky tests (e.g. the `ooda_brain::recipe_brain` recipe-resolution and
`base_type_copilot` copilot-spawning tests, which only fail on a developer box
that has a live `~/.simard` install / `copilot` on `PATH` and pass/skip in clean
CI); any change to CI workflow definitions.

## Related pages

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how a
  single test allocates an isolated state root with `HermeticState`.
- [serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
  — the whole-binary watched-env contract and the `serial_guard` meta-test.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — companion
  patterns (lazy config resolution, serial env-var tests).
- [Subprocess prompt delivery](../prompt-delivery.md) — the `prompt_delivery`
  module, `select_mode`, and `ENV_OVERRIDE`.
- [De-flaking `ooda_config_default_values`](./deflaking-ooda-config-default.md)
  — the sibling `OodaConfig` env-leak flake closed with the shared
  `cognitive_memory` serial key.
- [Goal-board API](../reference/goal-board-api.md) — the dashboard goal-CRUD
  endpoints these handlers back.
