---
title: De-flaking the known flaky tests (prompt-delivery env race + goal-board state-root race + parallel `cargo test --lib` reaper/timing aborts)
description: >
  How the known-flaky tests were made deterministic under parallel
  `cargo test`: the prompt_delivery env-var race (issue #2412) is closed by
  sharing the prompt_delivery_env serial key on the two Auto-mode size tests,
  the goal-board state-root race (issues #2408 / #2384) is closed by
  threading an explicit state root through the goal-CRUD handlers (the `_at`
  cores) so the dashboard's read/write path never resolves the root ambiently
  from the process environment during a test, the stewardship `gh` spawn
  `ETXTBSY` race (#4523) is closed by a bounded exec-retry, and the self-deploy
  `unit-test` gate aborts (#4619) — a family of process-wide-reaper `ECHILD`
  races and CPU-oversubscription timing windows — are closed by errno-gated
  retries, `simard_process_reaper` serial grouping, and widened timing windows.
last_updated: 2026-07-25
review_schedule: when a new env-reading handler is added to the dashboard, when serial_test is upgraded, when the stewardship `gh` spawn path changes, or when a new subprocess-spawning unit test is added under the process-wide `reap_zombies()` reaper
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./install-serial-isolation.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
  - ../reference/goal-board-api.md
  - ../prompt-delivery.md
---

# De-flaking the known flaky tests

This page documents the finished state of the work that made the known-flaky
tests deterministic under parallel `cargo test`. It is the test-author and
reviewer contract for the four de-flake mechanisms that closed the flakes — two
env-isolation fixes, one `gh` spawn `ETXTBSY` retry, and one `unit-test`-gate
reaper/timing de-flake — and the verification gate that keeps them closed.

The flakes and their fixes:

| Issue | Flaky test | Root cause | Fix |
| ----- | ---------- | ---------- | --- |
| [#2412](https://github.com/rysweet/Simard/issues/2412) | `prompt_delivery::applied_mode_reports_inline_for_small_prompt`, `…_tempfile_for_large_prompt` | `Auto` mode reads `AMPLIHACK_PROMPT_DELIVERY` (`ENV_OVERRIDE`) and could observe a leaked `inline`/`tempfile` value set by a *concurrent* env test | Share the `prompt_delivery_env` serial key on the two Auto-mode size tests |
| [#2408](https://github.com/rysweet/Simard/issues/2408) · [#2384](https://github.com/rysweet/Simard/issues/2384) | `operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud` (and siblings) | The goal-CRUD handlers resolved `SIMARD_STATE_ROOT` ambiently via `resolve_state_root()`, so a concurrent `setenv`/`remove_var` in an unrelated lib-binary test could tear the read and route a handler at the wrong state root | Thread an explicit state root through the goal-CRUD path (the `_at` handler cores) so the exercised path never reads the env |
| [#4523](https://github.com/rysweet/Simard/pull/4523) | `stewardship::gh_client::tests::create_issue_reports_nonzero_exit_and_stderr_without_body_content` (and its sibling `fake_gh` test) | Under parallel `cargo test`, one thread writes its per-test `fake_gh` script and immediately `Command::spawn()`s it while another thread's concurrent `fork()` has inherited a still-open write descriptor to that file, so the kernel refuses the `exec` with `ETXTBSY` (`Text file busy`, `os error 26`) — a transient exec-vs-write race, not a logic bug | Wrap the `execute_create_issue` spawn in a bounded, `ETXTBSY`-exclusive retry (`retry_on_etxtbsy`) so the transient kernel condition is retried and the test path is deterministic |
| [#4619](https://github.com/rysweet/Simard/pull/4627) | `base_type_copilot` fake-meeting turns, the five `base_type_rustyclawd::tool_executor` `Bash` tests, `terminal_session::execution` timeout test, `ooda_actions::tests_dispatch_concurrency` overlap test, `overseer::claim_reaper` deadline test | The self-deploy `unit-test` gate aborted intermittently (exit status 101) running the full ~9295-test suite under the gate's high `CARGO_BUILD_JOBS`. A family of parallelism-only flakes: the dominant class is an **`ECHILD` race** — `agent_supervisor::reap_zombies()` does a process-wide `waitpid(-1)` and, run concurrently with a subprocess-spawning test, steals that test's child so the test's own `wait()`/`output()` returns "No child processes" (issue #1779); the rest are scheduler-starvation timing windows too tight under CPU oversubscription. (The truncated gate tail `⠋ Drop t...` was a red herring — a raw spinner frame the `spinner_drop_cleans_up` test writes to fd 2, bypassing libtest capture.) | Per flake class, all additive: errno-gated `ECHILD` retry for the copilot fake-meeting turns **and** the PRODUCTION daemon self-update self-test; `#[serial(simard_process_reaper)]` grouping for the non-idempotent `tool_executor` Bash tests; widened timing windows for the three load-sensitive timing tests. No test deleted or `#[ignore]`d; every assertion keeps its original strength |

All four fixes preserve full test parallelism. None blanket-serializes the
suite, and the de-flake mechanisms leave production behaviour unchanged with a
single deliberate exception: the HTTP handlers still resolve the state root from
the environment exactly as before, `select_mode` still reads the same env
override, and `gh_client` still spawns the same `gh` argv over the same
`--body-file -` stdin channel; the suite is simply prevented from racing on those
reads, from tripping over the transient exec-vs-write `ETXTBSY` window, and (Fix
#4619) from being reaped out from under its own `waitpid` or starved past a tight
timing window. The one intended production change is Fix #4619's errno-gated
`ECHILD` retry on the `cmd_self_update` daemon self-test — a fix, not a
behavioural drift (see the Fix #4619 section).

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
> - **#4523:** wrap the `execute_create_issue` `.spawn()` in
>   `src/stewardship/gh_client.rs` with `retry_on_etxtbsy` — a bounded (8 × 5 ms),
>   `ETXTBSY`-exclusive retry that tolerates the transient exec-vs-write race on
>   the `fake_gh` helper without deleting or ignoring any test. Leaves
>   `tool_executor.rs` (the PR's ECHILD/SIGCHLD fix) untouched.
> - **#4619:** de-flake the parallel `cargo test --lib` aborts that were blocking
>   the self-deploy `unit-test` gate. Six files, fixed by flake class: errno-gated
>   `ECHILD` retry for the `base_type_copilot` fake-meeting turns **and** the
>   PRODUCTION `cmd_self_update` daemon self-test; `#[serial(simard_process_reaper)]`
>   on the five non-idempotent `tool_executor` `Bash` tests; widened timing windows
>   in `terminal_session::execution`, `ooda_actions::tests_dispatch_concurrency`,
>   and `overseer::claim_reaper`. The deploy gate itself is untouched. See the
>   Fix #4619 section.
> - **Gate:** ≥20 consecutive parallel runs of the affected tests with zero
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

## Fix #4523 — the `gh` spawn `ETXTBSY` race

### The race this eliminates

`stewardship::gh_client` shells out to the real `gh` binary in production, but its
unit tests exercise `execute_create_issue` against a small **`fake_gh`** helper —
a `#!/bin/sh` script that each test writes into its own `tempfile::tempdir()`,
`chmod 0o700`s, and then spawns. Under parallel `cargo test`, two things happen
on different threads at nearly the same time:

1. One test thread writes its per-test `fake_gh` script and, in the window before
   that write handle is fully closed, is about to `exec` it.
2. Another test thread concurrently calls `Command::spawn()` (a `fork`), whose
   child inherits the still-open *writable* file descriptor to that script.

If the `execve(2)` lands while the forked child still holds that write handle,
Linux refuses the exec with **`ETXTBSY` — `Text file busy`
(`os error 26`)**. The failure surfaced as an intermittent red `coverage` check
on PR [#4523](https://github.com/rysweet/Simard/pull/4523):

```
stewardship::gh_client::tests::create_issue_reports_nonzero_exit_and_stderr_without_body_content
  panicked: failed to spawn `gh issue create`: Text file busy (os error 26)
```

This is a classic **exec-vs-write TOCTOU** at the OS layer. It is not a defect in
the ECHILD/SIGCHLD reaping fix that PR #4523 actually delivers
(`src/base_type_rustyclawd/tool_executor.rs`) — that file is untouched by this
de-flake. The red check simply co-located with an unrelated, pre-existing flaky
test, and the fix belongs where CI proves the failure lives:
`src/stewardship/gh_client.rs`.

> **TL;DR**
>
> - The `.spawn()` inside `execute_create_issue` is wrapped in
>   `retry_on_etxtbsy`, a bounded retry that retries **only** `ETXTBSY`.
> - Retry budget: **8 attempts, 5 ms constant sleep** between attempts
>   (≤ ~35 ms added latency, and only on the rare retry path).
> - Every other error (`ENOENT`, `EACCES`, `EPERM`, …) and every `Ok` returns
>   **immediately** — the fix never masks a real failure.
> - Classification is **numeric** (`err.raw_os_error() == Some(libc::ETXTBSY)`),
>   never a locale-dependent match on the error string.
> - Production behaviour is unchanged: the same argv, the same `--body-file -`
>   stdin channel, the same no-shell `Command::args` exec. The only difference is
>   that a transient kernel `ETXTBSY` is retried instead of surfaced.

### The mechanism

Two small, pure helpers were added to `src/stewardship/gh_client.rs`, and the
existing spawn call site was wrapped in the retry.

**`is_etxtbsy` — numeric classification.**

```rust
/// True iff `err` is the transient `ETXTBSY` (`Text file busy`, os error 26)
/// exec-vs-write race. Classification is numeric only — no string matching.
fn is_etxtbsy(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::ETXTBSY) // 26
}
```

`libc` is already a direct dependency of the crate (`libc = "=0.2.185"` in
`Cargo.toml`), so the named constant `libc::ETXTBSY` is used directly — no new
dependency is added and no magic number is hard-coded. `libc::ETXTBSY` is `26`
on Linux; the value is compared numerically, never against the error string.

**`retry_on_etxtbsy` — bounded, `ETXTBSY`-exclusive retry.**

```rust
/// Run `op`, retrying **only** on `ETXTBSY`. All other `Err` and any `Ok`
/// return immediately. Bounded to `MAX_ATTEMPTS` with a constant backoff so a
/// persistent condition can never hang the caller.
fn retry_on_etxtbsy<T, F: FnMut() -> io::Result<T>>(mut op: F) -> io::Result<T> {
    const MAX_ATTEMPTS: usize = 8;
    const BACKOFF: Duration = Duration::from_millis(5);

    for attempt in 1..=MAX_ATTEMPTS {
        match op() {
            Err(err) if is_etxtbsy(&err) && attempt < MAX_ATTEMPTS => {
                // Retry path only: log the attempt index and error code —
                // never the body, title, args, repo, or token.
                tracing::debug!(
                    attempt,
                    os_error = err.raw_os_error(),
                    "gh spawn hit ETXTBSY; retrying"
                );
                std::thread::sleep(BACKOFF);
            }
            other => return other,
        }
    }
    unreachable!("loop returns on the final attempt")
}
```

**The wrapped spawn** in `execute_create_issue`:

```rust
let mut child = retry_on_etxtbsy(|| {
    Command::new(executable)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
})
.map_err(CreateIssueExecutionError::Spawn)?;
```

Everything downstream — the piped-stdin `--body-file -` write, `wait_with_output`,
and the `CreateIssueExecutionError` mapping — is byte-for-byte identical.

### Why not just serialize the tests?

Adding `#[serial]` to the two `fake_gh` tests narrows the window but does **not**
close it: the `fork`/`exec` race is against *any* concurrent `Command::spawn()`
anywhere in the test binary (each fork can inherit the just-opened write
descriptor), not only against the sibling `fake_gh` test, and serialization would
needlessly drop test parallelism. The retry closes the race at its source — the
transient kernel condition — and keeps the suite fully parallel. Serial grouping
may be layered on as defence-in-depth, but it is not the fix.

### What this fix deliberately does *not* do

- It does **not** delete or `#[ignore]` any flaky test. Both `fake_gh` tests and
  the no-secret-leak tests stay live and green.
- It does **not** retry anything other than `ETXTBSY`. `ENOENT` (binary missing),
  `EACCES`/`EPERM` (permissions), and `ENOMEM` fail loud on the first attempt.
- It does **not** touch `src/base_type_rustyclawd/tool_executor.rs`. PR #4523's
  ECHILD-tolerance fix and its PRD are preserved intact.
- It does **not** log any issue content. The retry-path `tracing::debug!` emits
  only the attempt index and the numeric OS error — no title, body, argv, repo,
  or token — so the existing no-leak tests remain green.

### Security properties

| # | Property | Guarantee |
| - | -------- | --------- |
| S1 | No secret leakage | Retry-path `tracing::debug!` logs only `attempt` + `raw_os_error()`; never body/title/args/repo/token. No-leak tests stay green. |
| S2 | Numeric classification | `ETXTBSY` matched via `raw_os_error() == Some(libc::ETXTBSY)` (26), never by string, so locale/format changes cannot mis-route. |
| S3 | Fail-loud on real errors | Only `ETXTBSY` is retried; all other errors surface immediately and unmodified. |
| S4 | Bounded, no busy-spin | Hard cap of 8 attempts × 5 ms synchronous sleep — no unbounded loop, no local DoS/hang. |
| S5 | Unchanged exec boundary | Same `Command::args` no-shell exec and `--body-file -` stdin channel; user input is not re-parsed on retry. |
| S6 | No new TOCTOU window | Only the transient kernel condition is retried; no runtime `chmod`, fd reopen, or temp-file creation is introduced. |

### The tests

Deterministic, subprocess-free unit tests were added alongside the two existing
`fake_gh` tests in the inline `#[cfg(test)] mod tests` of `gh_client.rs`:

- **`is_etxtbsy` classifies correctly** — returns `true` for a synthetic
  `io::Error::from_raw_os_error(26)` and `false` for `ENOENT` (2) and `EACCES`
  (13). No subprocess, no sleep.
- **`retry_on_etxtbsy` retries only `ETXTBSY`** — a closure that returns
  `ETXTBSY` on the first N calls and then `Ok` is retried and ultimately
  succeeds; a closure returning `ENOENT` returns on the **first** call (asserts
  the counter is `1`); a closure returning `Ok` is called exactly once.
- **`retry_on_etxtbsy` respects the cap** — a closure that always returns
  `ETXTBSY` returns `Err(ETXTBSY)` after exactly `MAX_ATTEMPTS` calls, proving the
  bound and the fail-loud surrender.

Because the retry tests drive `retry_on_etxtbsy` with in-memory closures (not a
real spawn), they add **zero** real backoff to the suite and cannot themselves
flake.

### Verification

```bash
# Targeted: the stewardship gh_client tests, including the two fake_gh tests.
cargo test -p simard stewardship::gh_client

# Parallel stress: the historically flaky binary, many times, high thread count.
for i in $(seq 1 30); do
  cargo test -p simard stewardship::gh_client -- --test-threads=16 \
    || { echo "FLAKE on run $i"; break; }
done
```

The gate for this fix: **≥30 consecutive parallel runs with zero `ETXTBSY`
failures**, a green `cargo test --all-features`, and — the authoritative signal —
`gh pr checks 4523` reporting **0 FAILURE** (all required checks green,
including the `coverage` check that surfaced the flake). Verify against real CI,
not local assumption:

```bash
gh pr checks 4523
```

> **Do not merge.** This work leaves PR #4523 green and merge-ready; landing it is
> a human/merge-queue action, out of scope here.

## Fix #4619 — the parallel `cargo test --lib` reaper/timing aborts

### The failure this eliminates

The self-deploy **`unit-test` gate** aborted intermittently with **exit status
101** on `Running unittests src/lib.rs`, blocking 10+ consecutive Overseer ticks
(2026-07-24 18:32Z–23:44Z) and freezing the running binary at `7d0964ff`
(12 commits behind main). The gate runs the full ~9295-test lib-test binary
under a high `CARGO_BUILD_JOBS`, so the whole suite executes inside **one
process** at heavy CPU oversubscription — the exact conditions that surface a
family of parallelism-only flakes that never occur in production or in a
single-threaded run.

> **The `⠋ Drop t...` tail was a red herring.** The gate captures only the last
> ~200 bytes of stderr. `meeting_repl::spinner`'s `spinner_drop_cleans_up` test
> writes a raw spinner frame (`\r  ⠋ Drop test`, no newline) directly to fd 2,
> bypassing libtest's per-test capture, so that fragment is simply whatever
> happened to be last on the shared stderr — it is **not** the failing test.
> The real aborts are the reaper/timing flakes below.

### The flake classes and their fixes

Six files change, grouped by root cause. Every fix is additive and
non-breaking: no test is deleted or `#[ignore]`d, the deploy gate is untouched,
every assertion keeps its original strength, and only structured `tracing` is
added (no stray `println!`).

#### Class 1 — the `ECHILD` process-wide-reaper race (issue #1779)

The dominant class. `agent_supervisor::reap_zombies()` reaps process-wide with
`waitpid(-1)`. Run concurrently with a test that spawned its own child, the
reaper **steals that child's `SIGCHLD`**, so the test's own `wait()` / `output()`
returns `ECHILD` ("No child processes", errno 10) even though the child ran to
completion. Three sites, three remedies chosen by idempotency:

- **`src/base_type_copilot/tests.rs` — retry.** The existing `run_fake_meeting_turn`
  retry loop only recognised `ETXTBSY`. It now classifies transient spawn/wait
  races via a new `is_transient_meeting_spawn_race(reason)` predicate that matches
  **both** `"Text file busy"` (exec race) and `"No child processes"` (wait race),
  and retries the whole fake turn (8 attempts, 20 ms × attempt backoff). Re-running
  a fake meeting turn is idempotent, so retry is safe. Any *other* error still
  `panic!`s immediately, and exhausting the retries still `panic!`s loudly — real
  regressions are never masked.

- **`src/cmd_self_update/update.rs` — PRODUCTION retry.** This is the one
  production-behaviour change in the PR. The daemon self-update path runs inside a
  Tokio runtime whose signal driver reaps children process-wide, so a
  `<binary> self-test` child can be reaped before `Command::output`'s own `waitpid`,
  making a *valid* update wrongly look like a failed self-test. A new
  `run_self_test_output()` retries **only** `ECHILD` (matched by
  `raw_os_error() == Some(libc::ECHILD)`, locale-independent) up to
  `SELF_TEST_MAX_ATTEMPTS = 8` with a `SELF_TEST_RETRY_BACKOFF = 20 ms` constant
  sleep, logging each retry at `tracing::debug!`. A self-test is a pure health
  probe (idempotent), so re-running is safe; `ENOENT`, `EACCES`, and every other
  error surface immediately and unchanged.

- **`src/base_type_rustyclawd/tool_executor.rs` — serial grouping.** The five
  `Bash` tests each spawn a real `sh -c …` child. Because **re-running an
  arbitrary bash command is not idempotent**, retry is the wrong tool; instead all
  five join the `#[serial_test::serial(simard_process_reaper)]` group — the
  documented #1779 remedy — so they never run alongside the reaper. This leaves
  `tool_executor.rs`'s production ECHILD/SIGCHLD tolerance (from PR #4523's scope)
  intact and only adds the serial attribute to the tests.

#### Class 2 — CPU-oversubscription timing windows

Three tests encode a timing window that is comfortable on an idle machine but too
tight when the gate starves threads under oversubscription. Each window is
widened; **no assertion is weakened**:

- **`src/terminal_session/execution.rs`** — the timeout test's
  `wait-timeout-seconds` goes **1 → 10**. The `TimedOut` branch is still the path
  under test; the wider window only lets the PTY shell fork/exec and emit its
  `command not found` diagnostic before the timeout fires, so the test has real
  output to assert on instead of a bare "did not emit expected output".

- **`src/ooda_actions/tests_dispatch_concurrency.rs`** — the per-`run_turn` "live"
  overlap window goes **200 ms → 1000 ms**. Under saturation, worker-thread start
  can stagger by >200 ms, serialising the calls so `peak` never reaches 2. The
  wider window swamps that scheduling jitter; both the `peak >= 2` (real overlap)
  and `cap == 1` assertions are unchanged.

- **`src/overseer/claim_reaper.rs`** — the deadline-bounding test gets headroom:
  child `sleep` **10 → 30 s** and the elapsed bound **3 → 10 s**. Its 20 ms poll
  loop can be starved for seconds under oversubscription, so a sub-second bound was
  unrealistic. The test still proves boundedness against the child's full 30 s
  runtime (a removed/broken deadline would take ~30 s), not a tight wake-up.

### Why not just serialize the whole suite?

Blanket-serializing would hide the races at the cost of a much slower gate and
would mask any *genuine* concurrency regression. Each fix instead targets the
specific mechanism: retry where re-execution is idempotent (copilot turns, the
self-test probe), a **scoped** serial group only for the non-idempotent bash
tests, and honest timing headroom for load-sensitive windows. Suite parallelism
is otherwise preserved.

### What this fix deliberately does *not* do

- It does **not** delete, `#[ignore]`, or weaken any test. Every assertion keeps
  its original strength; the widened windows change only *how long the test is
  willing to wait*, never *what it proves*.
- It does **not** touch the deploy gate, the spinner, or `reap_zombies()` itself —
  the process-wide reaper is correct; the tests are made robust to it.
- It does **not** retry anything but the classified transient races. In
  `cmd_self_update` only `ECHILD` is retried; `ENOENT`/`EACCES`/etc. fail loud on
  the first attempt.
- It does **not** log sensitive content. The new `tracing::debug!` lines emit only
  the attempt index and numeric errno.
- It leaves the residual production spawn-starvation (the underlying EAGAIN/ETXTBSY
  load sensitivity) as an optional follow-up (bounded spawn-retry with backoff);
  it is out of scope for restoring a green gate.

### Security properties

| # | Property | Guarantee |
| - | -------- | --------- |
| S1 | Fail-loud on real errors | `cmd_self_update` retries only `ECHILD`; the copilot loop retries only the two classified reasons. All other spawn/wait errors surface immediately and unchanged, so a genuinely un-spawnable or malicious binary is never silently accepted. |
| S2 | Numeric/exact classification | `ECHILD` matched via `raw_os_error() == Some(libc::ECHILD)`; the copilot predicate matches fixed reason substrings — locale/format drift cannot mis-route. |
| S3 | Bounded, no busy-spin | Hard caps (`SELF_TEST_MAX_ATTEMPTS = 8`, 8 copilot attempts) with short synchronous backoff — no unbounded loop, no local DoS. |
| S4 | Idempotent retry only | Retry is used only where re-execution is a pure probe (self-test) or a rebuildable fake turn; the non-idempotent real-bash tests are serial-grouped, never retried. |
| S5 | No new attack surface | Test-only changes plus one errno-gated retry on an existing self-test spawn — no new I/O, parsing, privilege, network, secret, or dependency change. |

### Verification

```bash
# Build the lib-test binary once.
cargo test --lib --no-run

# Parallel stress: rebuild-free reruns of the whole lib-test binary at high
# thread count. The reaper/timing races only surface with the FULL suite in one
# process under oversubscription, so never name-filter this loop.
for i in $(seq 1 15); do
  cargo test --lib || { echo "FLAKE on run $i"; break; }
done

# Control: single-threaded run (the races cannot occur serially).
cargo test --lib -- --test-threads=1
```

The gate for this fix: a **15-iteration full-suite stress loop of the rebuilt
lib-test binary passing 15/15** (9295 passed, 0 failed every run), versus **5/10
failing on the pre-fix binary**, plus a green `cargo test --all-features`. The
authoritative signal is real CI on PR #4627:

```bash
gh pr checks 4627
```

> **Do not merge.** This work leaves PR #4627 green and merge-ready; landing it is
> a human/merge-queue action, out of scope here.

## Verification gate

A change to any of the four fixes is merge-ready only when all of the following
pass. Fix #4619 additionally carries its own full-suite stress proof in the
Verification subsection of the Fix #4619 section (15× `cargo test --lib`,
15/15 green) — run that too when touching the reaper/timing de-flakes.

### Targeted stress (the de-flake proof)

Run the formerly-flaky tests ≥**20 consecutive** times under parallel
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
| `CARGO_BUILD_JOBS` | Parallelism of the self-deploy `unit-test` gate | Not a Simard variable; its high gate value is what oversubscribes the CPU and surfaces the Fix #4619 reaper/timing races. Fix #4619 makes the affected tests robust to it rather than lowering it |

### Local stress-run memory budget

The saved workstation preference for memory-heavy local runs is
`NODE_OPTIONS=--max-old-space-size=32768` (used by the tooling that drives the
stress loops, not by `cargo` itself). To change it, edit
`~/.amplihack/config`. It is not required for CI and does not affect the Rust
tests.

---

## Scope

**In scope:** the four de-flake mechanisms above (the two env-isolation fixes,
the Fix #4523 `gh` spawn `ETXTBSY` retry, and the Fix #4619 `unit-test`-gate
reaper/timing de-flake across six files), their tests, the one coupled removal
bug fix (`remove_goal` now persists removals via
`save_goal_board_with_removals`, surfaced while wiring `remove_goal_at`), the one
Fix #4619 production change (the errno-gated `ECHILD` retry on the
`cmd_self_update` daemon self-test), and this page. Fix #4523 touches only
`src/stewardship/gh_client.rs` (spawn retry + deterministic unit tests). Fix
#4619 adds `#[serial(simard_process_reaper)]` to the `tool_executor` **Bash
tests** but leaves that module's production ECHILD/SIGCHLD tolerance unchanged.

**Out of scope (confirmed):** snapshot/redeploy docs; refactoring `select_mode`
or the goal-board logic beyond the isolation needs and that coupled removal fix;
broad `rustyclawd` refactors or any change to the ECHILD-tolerance production
fix; lowering `CARGO_BUILD_JOBS` or otherwise changing the deploy gate; the
residual production spawn-starvation (bounded spawn-retry with backoff is a noted
optional follow-up, not part of restoring a green gate); merging any PR
(including #4523 and #4619 — left green for the merge queue); unrelated flaky
tests (e.g. the `ooda_brain::recipe_brain` recipe-resolution tests, which only
fail on a developer box that has a live `~/.simard` install / `copilot` on
`PATH` and pass/skip in clean CI); any change to CI workflow definitions.

## Related pages

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how a
  single test allocates an isolated state root with `HermeticState`.
- [serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
  — the whole-binary watched-env contract and the `serial_guard` meta-test.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — companion
  patterns (lazy config resolution, serial env-var tests).
- [Subprocess prompt delivery](../prompt-delivery.md) — the `prompt_delivery`
  module, `select_mode`, and `ENV_OVERRIDE`.
- [Goal-board API](../reference/goal-board-api.md) — the dashboard goal-CRUD
  endpoints these handlers back.
- [`stewardship::gh_client`](../prompt-delivery.md) — the GitHub issue-creation
  path (`src/stewardship/gh_client.rs`) whose `gh` spawn is guarded by the
  `ETXTBSY` retry (Fix #4523).
- [PID-reuse-safe subordinate reaper](../concepts/pid-reuse-safe-subordinate-reaper.md)
  — the process-wide `reap_zombies()` / `waitpid(-1)` behaviour behind the Fix
  #4619 `ECHILD` race (issue #1779) and the `simard_process_reaper` serial group.
