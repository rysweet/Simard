---
title: serial(cognitive_memory) test isolation — the watched env surface
description: >
  The whole-binary contract that prevents the process-global environment race
  behind the tests_goals_crud flake (issue #2360): every lib-binary test that
  mutates or reads the guard's watched env surface (the cognitive-memory
  state-root, LLM-provider, and meetings-resolver variables) shares the
  cognitive_memory serial key, a regression-guard meta-test enforces it, and the
  rule is documented at the HermeticState source. Extending enforcement to every
  process-global variable is tracked follow-up (issue #2375). The same
  one-resource/one-group-name invariant is applied to the meetings-dir surface to
  unfreeze Simard self-deploy (issue #4520).
last_updated: 2026-07-23
review_schedule: when a new process-global env var is read by a production handler, or when serial_test is upgraded
owner: cognitive-memory
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ./deflaking-known-flaky-tests.md
  - ./COVERAGE_BASELINE.md
  - ../reference/goal-board-api.md
  - ../reference/cognitive-memory-client-helpers.md
---

# serial(cognitive_memory) test isolation — the watched env surface

This page is the whole-binary companion to
[Writing hermetic tests against cognitive memory](./hermetic-tests.md). The
hermetic-tests page tells an individual test *how* to allocate an isolated
state root. This page tells the test suite *as a whole* how those tests are
kept from racing each other's process-global environment.

It is the test-author and reviewer contract for the `cognitive_memory` serial
group, the regression-guard meta-test that enforces it, and the single
documented invariant the whole scheme rests on. It was introduced to close
issue [#2360](https://github.com/rysweet/Simard/issues/2360) — the ~15% flake
of
`operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud`
(and its `tests_goals_crud` / `tests_goal_records_migration` siblings) on
pre-commit `cargo test`.

## TL;DR

- The `cargo test --lib` test binary runs many tests **concurrently in one
  process**. The OS environment (`environ`) is **process-global** and glibc
  `setenv`/`getenv` are **not thread-safe**.
- Any test that mutates a process env var can corrupt a *concurrent* env read
  in an unrelated test — including the dashboard handlers' read of
  `SIMARD_STATE_ROOT`. This is var-agnostic: the *writer* and the *reader* do
  not have to touch the same variable.
- The fix is a single rule: **every lib-binary test that mutates *any*
  process-global env var — or reads the cognitive-memory state-root surface —
  shares one serial key, `cognitive_memory`**, so no env mutation is ever
  concurrent with an env read. Since
  [issue #2375](https://github.com/rysweet/Simard/issues/2375) the guard's
  *mutation* watch is `EnvWatch::AnyVar` (`src/test_support/serial_guard.rs`): it
  flags a `set_var`/`remove_var` of *any* variable that lacks the key. The
  narrower *read* watch (`READ_WATCHED_VARS`) still targets the state-root /
  provider / meetings surface (`SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`,
  `SIMARD_LLM_PROVIDER`, `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`,
  `SIMARD_HANDOFF_DIR` — `HOME` excluded, because a torn `HOME` *read* is not the
  race; only a `HOME` *write*, which can tear a `SIMARD_STATE_ROOT` read, is).
- A **regression-guard meta-test** parses the source tree (with `syn`) and
  fails the build if a hand-written test touches that watched surface without
  the key, so an ordinary PR cannot silently reintroduce the race.
- No production code changes. The handlers still resolve the state root from
  the environment exactly as before; the suite is simply prevented from
  racing on it.

---

## The race this eliminates

### Mechanism

`HermeticState` (`src/test_support/hermetic.rs`)
pins the process environment for its lifetime:

```text
HermeticState::new()
  → set_var(SIMARD_STATE_ROOT, <tempdir>)   // process-global
  → remove_var(SIMARD_MEMORY_SOCKET)        // process-global
  …drop…
  → restores both to their previous values  // process-global
```

The dashboard goal/board handlers resolve the state root **at call time** from
that same global environment:

```rust
// src/operator_commands_dashboard/routes.rs
pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}
```

`seed_goals()`, `add_goal`, `dashboard_goal_board_snapshot`,
`dashboard_save_goal_board`, and the rest of the goal CRUD surface all route
their reads and writes through `resolve_state_root()`.

`#[serial_test::serial(cognitive_memory)]` only provides **mutual exclusion
between tests that share the key**. Before this fix, a number of tests mutated
process env under a *different* key, a *bare* `#[serial]` (the empty key `""`),
or *no* serial annotation at all. Those tests ran **concurrently** with the
`cognitive_memory` group under cargo's multi-threaded runner.

glibc's `setenv`/`unsetenv` may `realloc` (and free) the `environ` array; a
concurrent `getenv` can then read freed memory. The practical symptom is that
`std::env::var("SIMARD_STATE_ROOT")` returns `Err` (or garbage) **even though
`HermeticState` had it set** — so `resolve_state_root()` falls back to
`HOME/.simard`. (`std::env::set_var` / `remove_var` are themselves `unsafe`
under concurrency in the Rust 2024 edition for exactly this reason.)

### Why the failure was a flake, and why it pointed at the assertion

`full_goal_lifecycle_crud` is **read/write asymmetric**:

| Step | Operation | State root it uses |
|------|-----------|--------------------|
| 1–7  | seed / add / promote / save via the handlers | racy `resolve_state_root()` |
| 8    | final assertion reads the board | explicit hermetic `state.state_root()` |

Under a torn read, steps 1–7 write into `HOME/.simard` while step 8 reads the
hermetic tempdir. The board the assertion loads is empty, so
`board.active.len() != 3` and the test panics around
`src/operator_commands_dashboard/tests_goals_crud.rs:605`. The assertion was
correct; it was the *only* place the discrepancy could surface, which is why
the panic always landed there. The fix does **not** weaken or move that
assertion.

---

## The contract — the `cognitive_memory` serial group

> **Invariant (enforced surface).** In the `cargo test --lib` binary, **no test
> that mutates the guard's watched env surface — the cognitive-memory
> state-root / LLM-provider / meetings-resolver variables (`SIMARD_STATE_ROOT`,
> `SIMARD_MEMORY_SOCKET`, `HOME`, `SIMARD_LLM_PROVIDER`, `SIMARD_MEETINGS_DIR`,
> `SIMARD_MEETINGS_ROOT`, `SIMARD_HANDOFF_DIR`) — may run concurrently with any test that reads that
> surface.** This is enforced by routing *every* such test through the single
> serial key `cognitive_memory`.

`cognitive_memory` is the **canonical key** for this property. It already
guarded the `HermeticState` writers and the cognitive-memory readers; #2360
extends it to cover every lib-binary test that touches the watched surface (the
read set `READ_WATCHED_VARS` in `src/test_support/serial_guard.rs`), making the
guarantee **total for that surface**. The race is fundamentally
variable-agnostic, so every env var a test mutates must be keyed; since
[#2375](https://github.com/rysweet/Simard/issues/2375) the guard's *mutation*
watch is `EnvWatch::AnyVar` and auto-enforces this process-wide (see
*Process-wide enforcement* below).

### Annotation Decision Rule

A test in the **`src/` lib binary** MUST carry the `cognitive_memory` serial
key — either alone or as one member of a multi-key annotation — **if and only
if**, at run time, it does any of:

- **(A)** Constructs a `HermeticState` (`::new`, `::new_in`, `::default`,
  `new_with_temp`).
- **(B)** Calls `set_var` / `remove_var` on **any** process-global variable —
  directly, or through a **recognized env-helper** (`EnvGuard::set`/`unset`,
  `SkipGuard::set`/`clear`). The race is var-agnostic, so `HOME`,
  `CARGO_TARGET_DIR`, `SIMARD_SKIP_GYM`, `TZ`, etc. all count — not only
  `SIMARD_STATE_ROOT` and `SIMARD_MEMORY_SOCKET`. *Enforcement note:* since
  [#2375](https://github.com/rysweet/Simard/issues/2375) the `serial_guard`
  meta-test auto-detects (B) for **every** variable (`EnvWatch::AnyVar`), so this
  is no longer only an author obligation — an unkeyed `set_var`/`remove_var` (or
  recognized-helper mutation) of any name fails the build. The rule-(C) *read*
  check remains scoped to `READ_WATCHED_VARS` (the state-root surface minus
  `HOME`). See *Process-wide enforcement* below.
- **(C)** Reads `SIMARD_STATE_ROOT` / `SIMARD_MEMORY_SOCKET` from the global
  env, directly or via `resolve_state_root()` / `memory_ipc::socket_path_for`
  default resolution.
- **(D)** Opens cognitive memory at the **env-derived default path**
  (`LibraryCognitiveMemory::open`, `CognitiveMemoryClient`, `open_native`,
  `launch_writer_client` against a path obtained from the global env) **or**
  invokes one of the **async dashboard route handlers** that resolve the state
  root internally via `resolve_state_root()` — `seed_goals`, `add_goal`,
  `remove_goal`, `update_goal_status`, `promote_backlog_item`, `demote_goal`,
  and the goal/board snapshot route (all `async fn … -> Json<Value>` in
  `operator_commands_dashboard/goals.rs`).

  > **Historical note (#2307):** pre-de-fork native-fork tests that used
  > `NativeCognitiveMemory::open` belong to the same env-derived-path class, but
  > that symbol is deleted and is no longer a current API.

  > **Not rule (D):** `dashboard_goal_board_snapshot(state_root)`,
  > `dashboard_save_goal_board(state_root, board)`, `save_goal_board(board, client)`,
  > `save_goal_board_with_removals(…)`, and `load_goal_board(client)` take an
  > **explicit** `state_root: &Path` or `client: &dyn CognitiveMemoryOps` and
  > **never read the environment**. Calling them is isolated by construction
  > (exclusion #2); they require the key only if the test *also* derives that
  > path or client from the global env (via `resolve_state_root()` or
  > `HermeticState`).

**Exclusions — do NOT annotate solely because of these:**

- The token (`SIMARD_STATE_ROOT`, `HermeticState`, …) appears only in a string
  literal or assertion message, with no real env or cognitive-memory
  interaction. Example: an assertion that help text *contains* the string
  `"SIMARD_STATE_ROOT"`.
- The test opens cognitive memory / calls handlers **exclusively** against an
  explicit tempdir `state_root`, mutates and reads **no** process-global env,
  and the open API derives its socket purely from the passed path. Such a test
  is isolated by construction. (Threading the explicit path is the preferred
  defense-in-depth — see [Explicit-path threading](#4-explicit-path-threading).)

### Scope: which test binary

The rule applies to the **single in-process lib test binary** (`cargo test
--lib`). Each `src/bin/<tool>` target and each `tests/*.rs` integration file
compiles to a **separate process** with its **own** `environ`; a mutation in
one process cannot tear a read in another. Those binaries are therefore **out
of scope** for this contract. In particular,
`src/bin/simard_tui/goals.rs` reads the default path but lives in its own
binary with no in-process env mutators, so it is correctly left unannotated.

---

## Process-wide enforcement (`EnvWatch::AnyVar`) — closed by #2375

The invariant is now enforced for **every** process-global env mutation, not
just the guard's original state-root watched surface. The underlying hazard is
variable-agnostic: glibc `setenv` may `realloc` (and free) the whole `environ`
array when a variable is first added, so a concurrent `getenv` anywhere in the
process can read freed memory **even when the writer and reader touch different
variable names**.

[Issue #2375](https://github.com/rysweet/Simard/issues/2375) closed the residual
class the #2360 work left open by:

1. Switching the production audit default from `EnvWatch::StateRootSurface` to
   **`EnvWatch::AnyVar`** (`AuditOptions::default().watched`), so the guard now
   flags *any* `set_var`/`remove_var` in a lib-binary test that does not carry
   the `cognitive_memory` key.
2. Multi-keying every remaining env mutator into `cognitive_memory` (adding —
   never removing — the key alongside any existing semantic group). This folded
   in the previously-uncaught variables, including `SIMARD_SKIP_GYM`
   (`gym_runner_client`), `NO_COLOR` (`meeting_repl`), `ENV_OVERRIDE`
   (`prompt_delivery_env`), `SIMARD_NO_UPDATE_CHECK` (`update_check_env`),
   `ENV_LLM_PROVIDER` (`runtime_config`), `ANTHROPIC_API_KEY` (`review_pipeline`,
   `self_improve_executor`), `SIMARD_DASHBOARD_PORT`, `SIMARD_OPERATOR_NAME`,
   `SIMARD_SCALING`, `SIMARD_ENGINEER_AGENT`, `SIMARD_SAFE_UPDATE_SKIP_HANDOVER`,
   `SIMARD_DISK_PRESSURE_MIN_FREE_GB`, `SIMARD_AMPLIHACK_BIN`, and
   `CARGO_TARGET_DIR` (the `cmd_cleanup` canary tests) — 84 tests across 24
   modules, plus the bare-`#[serial]` / module-local `ENV_LOCK` neighbours so the
   migration introduced no new cross-group concurrency.

The residual was **much rarer** than the #2360 symptom: the watched-surface
tears caused a frequent, deterministic mis-resolution (a write to the wrong root
left an empty board and tripped `board.active.len() == 3`, or routed an autosave
into the wrong meetings dir), whereas this residual was an infrequent
`environ`-realloc read of a value the assertion does not depend on. It is now
closed **by construction**: with `AnyVar`, the `serial-guard` meta-test fails the
build the moment any new env-mutating test omits the key. The
`anyvar_default_isolates_every_env_writer_across_sessions` regression test pins
the `AnyVar` default and proves an unrelated-variable writer is isolated from the
cognitive-memory readers only when it shares the `cognitive_memory` key.

---

## Multi-key annotations — preserving semantic-group parallelism

Some env-mutating tests already belong to a *semantic* serial group that exists
for a different reason (e.g. `simard_meetings_dir_env`,
`prompt_delivery_env`, `simard_disk_pressure_env`). You do **not** collapse
those into `cognitive_memory`; you **add** `cognitive_memory` as a second key:

```rust
#[test]
#[serial_test::serial(simard_meetings_dir_env, cognitive_memory)]
fn meetings_dir_env_overrides_default() { … }
```

`serial_test` acquires **all** named keys before the test runs, so a multi-key
annotation is mutually exclusive with both the `simard_meetings_dir_env` group
**and** the `cognitive_memory` group, while two `simard_meetings_dir_env`
tests that do **not** also name `cognitive_memory` can still run in parallel
with the cognitive-memory readers only if they touch no env — which, by the
rule, they may not. The net effect for the watched surface: every reader/writer
of it is serialized against every other, while unrelated semantic groups keep
whatever extra parallelism they had. (Since #2375 the guard watches
`EnvWatch::AnyVar`, so mutators of *other* env vars in unrelated groups are now
folded in as well — see *Process-wide enforcement* above.)

> Rule of thumb: **never remove** an existing key when adding `cognitive_memory`.
> Append it. Removing a key silently widens concurrency for the original group.

### Bare `#[serial]` → keyed: the safety check

Converting a bare `#[serial]` (key `""`) test to
`#[serial(cognitive_memory)]` **drops** its mutual exclusion with the other
bare-serial tests. That is only safe if the test shares **no other global
singleton** (current working directory, a process-wide registry, a fixed TCP
port, …) with those bare tests — i.e. it was bare *only* to serialize env
access. The `cmd_cleanup` cleanup tests qualify: each operates inside its own
per-test temporary `HOME`, sharing nothing but the environment. A bare test
that is **not** an env mutator (for example a pure formatter/display test) is
left bare.

---

## API: the regression-guard meta-test

The invariant is self-enforcing for the hand-written test surface. A meta-test
in the lib binary parses the source tree and fails if any hand-written test
violates the Annotation Decision Rule, so a future PR cannot silently
reintroduce the race through an ordinary test. (The macro- and helper-generated
blind spots in [Known limitations](#known-limitations-false-negatives) are the
only gaps, each with a mitigation.)

### Location and entry points

```text
src/test_support/serial_guard.rs
```

| Item | Kind | Purpose |
|------|------|---------|
| `audit_env_mutating_tests(opts: &AuditOptions) -> Vec<Offender>` | `pub(crate)` fn | Pure, side-effect-free `syn`-based parse of the source tree. Returns one `Offender` per test that mutates/reads guarded env but lacks the `cognitive_memory` key. Usable from ad-hoc tooling. |
| `every_env_mutating_test_is_serialized()` | `#[test]` | The enforcement test. Calls `audit_env_mutating_tests` with the shipped `AuditOptions::default()` and asserts the offender list is empty, printing a remediation report when it is not. |
| `AuditOptions` | `pub(crate) struct` | Configuration (watched variables, scanned roots, exclusions, allowlist). |
| `Offender` | `pub(crate) struct` | `{ file: PathBuf, line: usize, test_name: String, reason: Reason }`. |
| `Reason` | `pub(crate) enum` | `MutatesEnv { var }`, `ReadsStateRootDefault`, `ConstructsHermeticState`, `CallsEnvReadingHandler { handler }`, `EmptyAllowlistJustification`. |

### Parsing strategy

The scanner is **AST-based, not text-based**. It parses each `.rs` file with
[`syn`](https://docs.rs/syn) (`syn::parse_file`) and walks the result with a
`syn::visit::Visit` visitor. This is what makes the guard trustworthy: it is
robust to multi-line attributes, attribute ordering, raw/byte-string literals,
doc comments, and `#[cfg(...)]`-gated code that a regex would mishandle. For
each `#[test]` / `#[tokio::test]` function the visitor records the function's
serial keys (the arguments of every `serial_test::serial(...)` attribute) and
inspects the body for the rule (A)–(D) trigger calls; a function that fires a
trigger but whose key set does **not** contain `cognitive_memory` becomes an
`Offender`.

The scan is a **per-file two pass** so it also catches mutation reached through
a *same-file* helper (the common test pattern `with_state_root(&root, …)`,
`with_temp_home(|| …)`). Pass 1 records every function's direct trigger reason
and the set of same-file functions it calls; pass 2 propagates the
**mutation** reasons (`MutatesEnv`, `ConstructsHermeticState`) along the call
graph to a fixpoint, so a test that writes env through a thin helper is flagged
like a direct writer. To preserve the no-false-positive guarantee, only
mutation reasons propagate — a **read** is flagged only when it appears
*directly* in the test body, never propagated through a branchy production
dispatcher that reads the state root in an untaken code path. Same-name
associated functions on unrelated types (e.g. `BuildLock::default_state_root`,
which resolves a build-lock path from `HOME`, not the cognitive-memory root) are
excluded by qualifier so the resolver match does not collide.

`syn` 2.x and `proc-macro2` are **already in the dependency graph
transitively**, so the guard only promotes them to explicit `[dev-dependencies]`
(`syn = { features = ["full", "visit"] }`, `proc-macro2 = { features =
["span-locations"] }` for accurate file:line offenders) — it introduces no new
third-party crates. The scan is pure and side-effect-free, adds no overhead to
the tests it guards, and completes in well under a second.

### Known limitations (false negatives)

Source scanning is sound for hand-written test functions in the scanned roots,
which is the surface that actually flaked in #2360. It cannot, however, see
through code generation. The guard never emits a false *positive* (an
`Offender` is reported only when a concrete trigger is observed without the
key), but it has deliberate false-*negative* blind spots, each with a
mitigation:

| Blind spot | Why the scanner misses it | Mitigation |
|------------|---------------------------|------------|
| Tests synthesized by a declarative or proc macro (no literal `#[test]` fn in source) | The AST holds the macro *invocation*, not the generated `#[test]` items | Emit such tests from a generator that already attaches the `cognitive_memory` key, or add an `allowlist` entry; call out the macro in review. |
| Env mutation reached only through an unrecognized project helper (a custom wrapper that calls `set_var` internally) | The visitor matches a fixed trigger set, not arbitrary transitive call graphs | Keep env mutation behind the recognized helpers (`HermeticState`, `EnvGuard::set`/`unset`, `SkipGuard::set`/`clear`, direct `set_var`/`remove_var`); register new helper names in the trigger set when introduced. |
| `#[cfg(...)]`-gated tests | The cfg predicate is not evaluated | None needed — the function is still parsed structurally and audited. |
| `src/bin/**` and `tests/**` | Separate processes, out of scope by design (see [Scope](#scope-which-test-binary)) | Excluded via `AuditOptions::excluded_prefixes`. |

The practical guarantee is therefore precise: **any hand-written lib-binary
test that directly mutates or reads guarded env without the `cognitive_memory`
key fails the build immediately.** The macro and helper blind spots are the
only ways an offender can slip past, and both are closed by review plus the
allowlist mechanism below.

### `AuditOptions` configuration

```rust
pub(crate) struct AuditOptions {
    /// Source roots to scan, relative to CARGO_MANIFEST_DIR. Default: ["src"].
    pub roots: Vec<PathBuf>,

    /// Path prefixes (relative, `/`-separated) NOT part of the lib test
    /// binary. Default: ["src/bin"] — each bin is a separate process.
    pub excluded_prefixes: Vec<String>,

    /// Which env-var mutations trip the rule. Default (since #2375):
    /// `EnvWatch::AnyVar` — any `set_var`/`remove_var` in a lib-binary test
    /// must carry the `cognitive_memory` key. The narrower
    /// `EnvWatch::StateRootSurface` (the cognitive-memory state-root, provider,
    /// and meeting artifact-dir surface: `SIMARD_STATE_ROOT`,
    /// `SIMARD_MEMORY_SOCKET`, `HOME`, `SIMARD_LLM_PROVIDER`,
    /// `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`, `SIMARD_HANDOFF_DIR`) and
    /// `EnvWatch::Vars(set)` remain available for scoped audits. The rule-(C)
    /// read check `READ_WATCHED_VARS` is the state-root surface minus `HOME`.
    pub watched: EnvWatch,

    /// Tests that are exempt with a written, machine-checked reason. Each
    /// entry is `(test_name, justification)`. The justification is required;
    /// an allowlist entry without one fails the audit.
    pub allowlist: Vec<(String, String)>,
}
```

`AuditOptions::default()` ships the production configuration: scan `src`,
exclude `src/bin`, watch **every process-global env mutation**
(`EnvWatch::AnyVar`, since #2375), empty allowlist. The rule-(C) *read* check
(`READ_WATCHED_VARS`) is narrower — the state-root / provider / meetings surface
**minus `HOME`** (`SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`,
`SIMARD_LLM_PROVIDER`, `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`,
`SIMARD_HANDOFF_DIR`) — a torn `HOME` *read* is not the cognitive-memory race;
only a `HOME` *write*, which can tear a `SIMARD_STATE_ROOT` read, is.

### Reading a failure

When the meta-test fails it prints one line per offender and a fix hint:

```text
serial-guard: 2 test(s) mutate or read process-global env without the
`cognitive_memory` serial key. Every such test in the lib binary must share
that key so env mutation is never concurrent with env reads. See
docs/testing/cognitive-memory-serial-isolation.md.

  src/foo/tests.rs:412  fn writes_home_env
      reason: mutates HOME via std::env::set_var
      fix:    add #[serial_test::serial(cognitive_memory)]
              (or append `cognitive_memory` to its existing serial keys)

  src/bar/tests.rs:88   fn reads_state_root_default
      reason: reads SIMARD_STATE_ROOT default path (resolve_state_root)
      fix:    add #[serial_test::serial(cognitive_memory)], or thread an
              explicit state_root so the test is isolated by construction
```

A tripped guard **always** means a real change is needed — there is no
scenario in which retrying or ignoring it is correct. The two valid responses
are (a) add the key, or (b) make the test isolated-by-construction (explicit
`state_root`, no env mutation) so it legitimately falls under the exclusions.

### Granting a deliberate exemption

If a test must mutate env but provably cannot race (rare; normally only a test
that *forks a subprocess* and mutates env only in the child), add an allowlist
entry with a justification:

```rust
AuditOptions {
    allowlist: vec![(
        "install_harness::tests::live_state_opt_out".into(),
        "sets SIMARD_TEST_ALLOW_LIVE_STATE in a spawned child only; \
         parent process never mutates env — see hermetic-tests.md".into(),
    )],
    ..AuditOptions::default()
}
```

An allowlist entry with an empty justification is itself an audit failure, so
exemptions cannot be added silently.

---

## Configuration reference

### Environment variables

| Variable | Read by | Test-time owner | Notes |
|----------|---------|-----------------|-------|
| `SIMARD_STATE_ROOT` | `resolve_state_root()`, `memory_ipc::default_state_root()` | `HermeticState` sets it to a `TempDir` | The variable at the center of the race. Mutating **or** reading it requires the `cognitive_memory` key. |
| `SIMARD_MEMORY_SOCKET` | `memory_ipc::socket_path_for` | `HermeticState` **unsets** it | When unset, the socket path follows the state root. |
| `HOME` | `resolve_state_root()` fallback, `cmd_cleanup`, `ooda_brain` | Per-test `TempDir` in the affected tests | A `HOME` mutation can tear a concurrent `SIMARD_STATE_ROOT` read, so `HOME` mutators are in scope. |
| `SIMARD_TEST_ALLOW_LIVE_STATE` | hermetic guard (see [hermetic-tests.md](./hermetic-tests.md)) | install/fake harnesses only | Opt-out of the hermetic state-root guard; unrelated to the serial key, but a test that sets it still mutates env and so needs the key (or an allowlist entry). |

### Serial keys

| Key | Meaning |
|-----|---------|
| `cognitive_memory` | The canonical key for "this test touches process-global env and/or cognitive memory." Required by the Annotation Decision Rule. |
| `simard_meetings_dir_env`, `prompt_delivery_env`, `simard_disk_pressure_env`, … | Pre-existing **semantic** groups. Keep them; **append** `cognitive_memory` rather than replacing. |
| `""` (bare `#[serial]`) | Legacy catch-all. For env mutators, migrate to `cognitive_memory` after the [bare→keyed safety check](#bare-serial-keyed-the-safety-check). |

---

## The documented invariant at the source

`src/test_support/hermetic.rs` carries an inline note, adjacent to the
`EnvBinding::set` `SAFETY` comment, that states the rule for anyone reading the
helper:

```rust
// INVARIANT (issue #2360): EVERY test in the lib binary that touches cognitive
// memory OR mutates/reads process-global env (SIMARD_STATE_ROOT set,
// SIMARD_MEMORY_SOCKET unset here; HOME and any other var elsewhere) MUST be
// keyed into the `serial(cognitive_memory)` group. HermeticState mutates
// process-global env, and glibc setenv/getenv are not thread-safe, so a
// concurrent env mutation in any other test can tear a handler's
// `std::env::var` read and send writes to HOME/.simard. The `serial_guard`
// meta-test auto-enforces this for its watched surface (SIMARD_STATE_ROOT /
// SIMARD_MEMORY_SOCKET / HOME / SIMARD_LLM_PROVIDER / SIMARD_MEETINGS_DIR /
// SIMARD_MEETINGS_ROOT / SIMARD_HANDOFF_DIR); keying any OTHER var is an author obligation the guard
// does not yet check (EnvWatch::AnyVar tracked as #2375).
// See docs/testing/cognitive-memory-serial-isolation.md.
```

The `SAFETY` comment that justifies the `unsafe { std::env::set_var(..) }` block
depends on this invariant: the call is sound **only because** the serial group
excludes all concurrent env access.

The fix also corrects a **stale doc-comment** on the private `EnvBinding`
helper. The comment that ships today claims tests can "drop their per-file
`EnvGuard` copies and import this one instead" — but `EnvBinding` is
module-private and stays that way, because the migration is **annotation-only**
(see [Example 2](#2-annotate-a-test-that-only-mutates-home)). The corrected comment describes
`EnvBinding` as the helper's *internal* env save/restore mechanism and no
longer implies an importable shared guard, so the source matches reality: of
the env helpers, `HermeticState` — not `EnvBinding` — is the one
`test_support` re-exports. (Promoting
`EnvBinding` to `pub(crate)` + re-export was considered and **rejected** — it
would invite body rewrites of migrated tests, contradicting the "no production
code changes / annotation-only" scope.)

---

## Examples and tutorials

### 1. Write a new cognitive-memory test (the common case)

```rust
use simard::test_support::HermeticState;

#[test]
#[serial_test::serial(cognitive_memory)]   // required by the Annotation Decision Rule (A)
fn promotes_backlog_item_into_active() {
    let state = HermeticState::new();
    let client = launch_writer_client(state.state_root()).expect("client");

    save_goal_board(&seed_board(), client.ops()).expect("seed");
    promote_backlog_item(&id, client.ops()).expect("promote");

    let board = load_goal_board(client.ops()).expect("load");
    assert_eq!(board.active.len(), 3);
}
```

### 2. Annotate a test that only mutates `HOME`

A test that never touches cognitive memory but sets `HOME` is **still** in
scope — its `set_var` can tear another test's `SIMARD_STATE_ROOT` read. The
migration is **annotation-only**: add the attribute, leave the body untouched.

```rust
#[test]
#[serial_test::serial(cognitive_memory)]   // rule (B): mutates a process-global var
fn cap_home_cargo_targets_under_cap_is_noop() {
    // body unchanged — it points HOME at a per-test tempdir (saving and
    // restoring the previous value) and asserts on cleanup behavior.
}
```

> The fix adds only the attribute line. It does **not** rewrite the env
> plumbing or swap in a shared guard type — consistent with the
> "no production code changes" scope. (The private `EnvBinding` helper inside
> `hermetic.rs` is **not** re-exported, so tests cannot import it; among the env
> helpers, `HermeticState` is what `test_support` exposes.)

### 3. Add `cognitive_memory` to a test already in a semantic group

```rust
// before
#[serial_test::serial(simard_meetings_dir_env)]
// after — append, never replace
#[serial_test::serial(simard_meetings_dir_env, cognitive_memory)]
```

### 4. Explicit-path threading

The preferred defense-in-depth: prefer the **explicit-`state_root`** entry
points over the env-reading async route handlers, so the test no longer depends
on the global env for that operation. The test still needs the key if it does
**anything else** in scope (constructs `HermeticState`, mutates env), but the
explicit path removes one racy read and makes intent obvious:

```rust
// env-default path — the async route handler reads resolve_state_root() internally
let resp = seed_goals().await;

// explicit path — synchronous, isolated by construction for this call
let board = dashboard_goal_board_snapshot(state.state_root())?;
```

`dashboard_goal_board_snapshot(state_root)` / `dashboard_save_goal_board(state_root, board)`
are **already** the explicit-`state_root` form (they call
`open_reader_client`/`launch_writer_client` on the passed path); there is no
zero-arg env-default form and no separate `_at` overload to add.
`full_goal_lifecycle_crud` already reads its final assertion via the explicit,
synchronous `dashboard_goal_board_snapshot(state.state_root())`
(`tests_goals_crud.rs:604`), while its write steps 1–7 go through the
env-reading async route handlers (`seed_goals`, `add_goal`, …). The fix keeps
that assertion and additionally serializes those write-side handlers so the two
halves can never observe different roots.

### 5. Fix a `serial-guard` failure

1. Read the offender line — it names the file, line, test, and reason.
2. If the test really touches env: add `#[serial_test::serial(cognitive_memory)]`
   (or append the key to an existing annotation), running the
   [bare→keyed safety check](#bare-serial-keyed-the-safety-check) if it was a
   bare `#[serial]`.
3. If the test can be made isolated-by-construction instead: thread an explicit
   `state_root` and remove the env mutation, so it falls under the exclusions.
4. Re-run `cargo test --lib serial_guard` until green.

---

## Migrated test inventory (finished state)

The `cognitive_memory` key now covers the initial state-root / provider race
surface — every unkeyed **env writer** the guard found. The 19 tests migrated by
the #2360 fix
(all of which write `HOME`, directly or through a same-file helper):

| File | Test(s) | Previous annotation | Now |
|------|---------|---------------------|-----|
| `cmd_cleanup/tests.rs` | `cap_home_cargo_targets_{missing_root_is_noop, records_error_on_unremovable_target, rotates_lru_over_cap, under_cap_is_noop}`, `corrupt_db_removed_when_older_than_threshold`, `rotate_keeps_newest_n_backups`, `rotate_noop_when_under_threshold`, `trim_snapshots_keeps_newest_n` (write `HOME`) | bare `#[serial_test::serial]` | `#[serial_test::serial(cognitive_memory)]` |
| `self_metrics/tests.rs` | `record_and_query_metric`, `query_metrics_with_since_filter`, `query_metrics_empty_file`, `daily_report_empty`, `daily_report_with_data`, `recent_metrics_limit`, `collect_and_record_all_records_four_metrics`, `malformed_lines_skipped` (write `HOME` via the `with_temp_home` helper) | bare `#[serial]` | `#[serial(cognitive_memory)]` |
| `ooda_brain/prompt_store_tests.rs` | `env_var_takes_precedence_over_home`, `home_used_when_env_var_unset` (write `HOME`) | none | `#[serial_test::serial(cognitive_memory)]` |
| `operator_commands_dashboard/tests_routes_b.rs` | `host_enumeration_reads_load_hosts` (writes `HOME`) | none | `#[serial_test::serial(cognitive_memory)]` |

The `self_metrics` writers are reached only through the same-file
`with_temp_home` helper; the guard's two-pass call-graph propagation is what
surfaces them — a text scan of the test bodies would miss them.

### Provider surface (#2360 follow-up, demonstrated)

After the initial 19-writer migration, full-suite stress runs surfaced the
`SIMARD_LLM_PROVIDER` surface as a second demonstrated race — adjacent to
`SIMARD_STATE_ROOT` but driven by the provider-resolution path
(`RuntimeConfig::load` → `SIMARD_LLM_PROVIDER`, then `<state_root>/config.toml`).
These were demonstrated flakes, so they are migrated here and the guard is
extended to enforce them symmetrically.

| File | Test | Role | Now |
|------|------|------|-----|
| `ooda_actions/session.rs` | `dispatch_launch_session_fails_loud_on_unsupported_rustyclawd_1162` | writes `SIMARD_LLM_PROVIDER` (had a **false** "cargo is single-threaded" safety comment) | `#[serial_test::serial(cognitive_memory)]` |
| `disk_health.rs` | `run_returns_error_when_recipe_runner_unavailable_or_recipe_invalid` | writes `SIMARD_LLM_PROVIDER` | bare `#[serial]` → `#[serial(cognitive_memory)]` |
| `self_improve_executor/tests.rs` | `generate_patch_without_api_key_returns_unavailable` | writes `SIMARD_LLM_PROVIDER` / `ANTHROPIC_API_KEY` | `#[serial_test::serial(cognitive_memory)]` |
| `operator_commands_dashboard/chat.rs` | `open_agent_session_returns_none_without_provider_config` | **reads** the provider surface via `open_dashboard_agent_session()` | `HermeticState` + `#[serial_test::serial(cognitive_memory)]` |

The guard now watches `SIMARD_LLM_PROVIDER` on **both** sides (mutation and
direct read), so all four tests are enforced symmetrically.

**The chat reader needed more than a serial key.** Its assertion ("session is
`None` when `SIMARD_LLM_PROVIDER` is unset") silently assumed *no* provider was
configured anywhere — but `open_dashboard_agent_session()` also reads
`<state_root>/config.toml`, and a developer box's real `~/.simard/config.toml`
sets `llm_provider = "copilot"`. That made the test environment-dependent (it
passed in clean CI but flaked locally). The fix gives it a `HermeticState`
(fresh, empty state root → no `config.toml`), so the assertion is now
deterministic everywhere; the `cognitive_memory` key additionally prevents a
concurrent provider-env mutation from tearing the read. The assertion is
unchanged — only its environment is made hermetic.

**Why a serial key alone is not enough for a reader:** a keyed serial group only
serializes tests *within* the group. A plain (unannotated) reader in cargo's
parallel pool still overlaps the group's mutations, so the *readers* above must
share the key too — not just the writers.

### Meetings-persistence surface (#2360 follow-up, demonstrated in CI)

A later `verify` run flaked on
`meeting_backend::tests_persist_extra::write_auto_save_lands_under_simard_state_root`
with `autosave parent must be $SIMARD_STATE_ROOT/meetings (got
/tmp/bundle-stubs-…/…)`. Same root-cause class as #2360, a third variable pair:
the meeting-persistence resolver `meetings_dir()` (used by `write_auto_save` /
`write_transcript` / `write_meeting_bundle`) consults `SIMARD_MEETINGS_DIR`, then
`SIMARD_MEETINGS_ROOT`, then falls through to `SIMARD_STATE_ROOT`. The
`write_meeting_bundle_*` tests set `SIMARD_MEETINGS_ROOT` under their own
`simard_meetings_root_env` key — disjoint from `cognitive_memory` — so they ran
concurrently with the autosave reader and tore its narrow-override read, routing
the autosave into the bundle test's directory.

The guard had a blind spot: it watched `SIMARD_STATE_ROOT` but not the two
`SIMARD_MEETINGS_*` overrides that shadow it. The fix closes both the race and
the audit gap:

- `SIMARD_MEETINGS_DIR` and `SIMARD_MEETINGS_ROOT` are added to the guard's
  mutation watch **and** `READ_WATCHED_VARS`, and `write_auto_save` /
  `write_transcript` / `write_meeting_bundle` are added to the env-reading
  handler list, so every meetings reader/writer is enforced symmetrically.
- The ~30 meetings-surface tests are migrated into `cognitive_memory` (appended
  to their existing semantic key, never replacing it):

| File | Previous key | Now |
|------|--------------|-----|
| `meeting_backend/tests_persist_extra.rs` | `meeting_persist` | `simard_meetings_dir_env, cognitive_memory` (unified — see [Unifying the meetings-dir group](#unifying-the-meetings-dir-group-issue-4520)) |
| `meeting_backend/persist/mod.rs`, `persist/markdown.rs` | `simard_meetings_dir_env` | `simard_meetings_dir_env, cognitive_memory` |
| `meeting_facilitator/handoff/persistence.rs` | `simard_meetings_root_env` | `simard_meetings_root_env, cognitive_memory` |
| `engineer_loop/tests_meeting_decisions.rs` | bare `#[serial]` | `#[serial(cognitive_memory)]` |

Validation: serial_guard green; the autosave reader, the bundle writers, and the
persist mutators run together under the multi-threaded runner across 20 focused
high-concurrency runs and four full runs with zero failures.

#### Handoff-dir surface (completing the meetings migration)

Moving `tests_meeting_decisions` into `cognitive_memory` exposed a coupled gap:
those tests set **both** `SIMARD_MEETINGS_ROOT` **and** `SIMARD_HANDOFF_DIR`, and
read the latter back through `load_carried_meeting_decisions()`. Their
`SIMARD_HANDOFF_DIR` writers (`ooda_loop::tests_observe`,
`operator_cli::meeting`, `meeting_facilitator::handoff::default_handoff_dir`)
remained on a bare `#[serial]` / unkeyed — disjoint from `cognitive_memory` —
so the reader could be torn by a concurrent handoff-dir write (the exact
"cross-variable tear" formerly tracked as a blind spot). `SIMARD_HANDOFF_DIR` is
therefore now part of the watched surface too:

- It is added to the guard's mutation watch **and** `READ_WATCHED_VARS`, so the
  handoff resolver/writers are enforced symmetrically with the meetings surface.
- The remaining 10 handoff-dir writers (6 in `ooda_loop/tests_observe.rs`, 3 in
  `operator_cli/meeting.rs`, 1 in `meeting_facilitator/handoff/mod.rs`) are
  migrated into `cognitive_memory` (replacing bare `#[serial]` / adding the key
  to the previously-unkeyed `default_handoff_dir_returns_path`). The
  `meeting_backend` handoff writers already carried the key from the meetings
  migration above.

Validation: serial_guard green with all three meeting vars watched; 8 full
`cargo test --lib --test-threads=16` runs (5799 passed, 0 failed each).

> **Out of scope — `base_type_copilot` meeting tests.** These spawn the **real**
> `copilot` subprocess and only run when the binary is on `PATH` (they skip in
> CI). They exhibit two distinct intermittent failures: (a) "No authentication
> information found" — a `HOME`-tear that the migrated `HOME` writers above
> already prevent from overlapping; and (b) "Authentication failed (Request
> ID …)" — a **live GitHub Copilot API rejection** (rate-limit / token), which
> no test-isolation change can fix. Because they are live-integration tests that
> skip in CI and carry an irreducible external-service flake, they are left
> unannotated and out of #2360's scope.

**Audited but deliberately NOT migrated** (the guard correctly leaves them
unkeyed because they are isolated by construction, per the
[exclusions](#annotation-decision-rule)):

- `bootstrap/config.rs::test_goal_store_path` and
  `bootstrap/tests_config.rs::config_path_methods_use_state_root` — resolve
  their state root from the **compile-time** `env!("CARGO_MANIFEST_DIR")` (and
  explicit `state_root: Some(temp)` inputs), never the runtime global env.
- `engineer_loop/tests_goal_records_migration.rs::engineer_pipeline_returns_empty_top_5_when_no_snapshot`
  — opens cognitive memory at an **explicit** tempdir (`fresh_state_root`), no
  env access.
- `operator_commands_meeting/goal_curation.rs::goal_curation_read_probe_with_missing_directory`
  — passes an **explicit** `Some(path)`, so the resolver's env-default branch
  is never taken.

Tests that **were already** in the group (the `HermeticState` writers, the
cognitive-memory readers, the direct `SIMARD_STATE_ROOT` writers behind
`with_state_root`, and the full `tests_goals_crud` /
`tests_goal_records_migration` suites) are unchanged. Their bodies and
assertions — including the `full_goal_lifecycle_crud` lifecycle assertion — are
untouched.

The broader set of ~111 env-mutating tests across other semantic groups is
tracked as a follow-up. They are **theoretical** contributors (the race is
var-agnostic) but were not demonstrated to flake; they are migrated to
multi-key annotations on a rolling basis, and the `serial_guard` meta-test
makes any new offender fail immediately rather than waiting for a flake.

---

## Unifying the meetings-dir group (issue #4520)

> **Status:** specification of the target state. This section is written in
> finished-state tense (retcon convention) and defines exactly how the
> meetings-dir serial group is to be unified — the single source of truth the
> implementation must satisfy. The isolation edits described below (rename the
> eight `tests_persist_extra.rs` attributes; guard the two `tests_persist.rs`
> handoff-report tests) land in this same change; until they are merged the
> "now"/"unchanged" tenses describe the intended, not yet deployed, state.

### Symptom — a frozen self-deploy

The deploy unit-test canary refused every Simard self-deploy with **exit 101**.
Reproduced on `origin/main`:
`meeting_backend::tests_persist::handoff_report_omits_agenda_section_when_no_template_applied`
(`src/meeting_backend/tests_persist.rs:245`) panicked at `.expect("write
report")` under full-parallel `cargo test --lib` (9277 passed, **1 failed**),
yet **passed in isolation** — the classic signature of a non-hermetic,
load-dependent, process-global-env flake.

### Root cause — two serial groups guarding one global resource

`meetings_dir()` (`src/meeting_backend/persist/mod.rs:62`) resolves its target
directory from the **process-global** env vars `SIMARD_MEETINGS_DIR`, then
`SIMARD_MEETINGS_ROOT`, then `SIMARD_STATE_ROOT`. `serial_test` only serializes
tests that **share a group name**. Before the fix, tests touching this single
global resource were split across **two mutually non-exclusive** serial groups:

- `#[serial(simard_meetings_dir_env, cognitive_memory)]` in
  `persist/markdown.rs` and `persist/mod.rs`, and
- `#[serial(meeting_persist, cognitive_memory)]` in `tests_persist_extra.rs`.

Because `meeting_persist` and `simard_meetings_dir_env` are different names,
serial_test let those two groups **race each other**. Worse, the two
handoff-report tests at `tests_persist.rs:245` and `:271` — which call
`write_handoff_markdown_report` into the **default** `meetings_dir` — carried
**no serial guard at all**. Under load a concurrent test would `set_var`
`SIMARD_MEETINGS_DIR` to a `tempfile::tempdir()` that was then dropped and
removed; the unguarded test wrote into that deleted directory, got an `Err`,
and panicked — exit 101, red canary, every self-deploy refused.

### The fix — one group name for one global resource

Isolation-only. No production code, no assertion, and no timeout/sleep/retry was
changed. The finished state:

1. **One serial group name.** Every test that reads or writes the meetings dir,
   or mutates `SIMARD_MEETINGS_DIR` / `SIMARD_MEETINGS_ROOT` / `SIMARD_STATE_ROOT`,
   now shares the single key **`simard_meetings_dir_env`** (with
   `cognitive_memory` appended, per the multi-key rule above). The eight
   `#[serial(meeting_persist, cognitive_memory)]` attributes in
   `tests_persist_extra.rs` were renamed to
   `#[serial(simard_meetings_dir_env, cognitive_memory)]`. The `meeting_persist`
   name is retired.
2. **The two unguarded tests are guarded.**
   `handoff_report_omits_agenda_section_when_no_template_applied` and
   `handoff_report_includes_agenda_section_when_template_applied` in
   `tests_persist.rs` now carry
   `#[serial(simard_meetings_dir_env, cognitive_memory)]` (with a
   `use serial_test::serial;` import added to the module).
3. **The already-correct files are unchanged.** `persist/mod.rs` (9 guards) and
   `persist/markdown.rs` (5 guards) already used
   `simard_meetings_dir_env, cognitive_memory`; they were verified, not edited.

The invariant is now: **two group names may never guard the same process-global
resource.** One resource → one name.

| File | Previous key | Now |
|------|--------------|-----|
| `meeting_backend/tests_persist_extra.rs` (8 tests) | `meeting_persist, cognitive_memory` | `simard_meetings_dir_env, cognitive_memory` |
| `meeting_backend/tests_persist.rs` (2 handoff-report tests, L245 & L271) | *(unguarded)* | `simard_meetings_dir_env, cognitive_memory` |
| `meeting_backend/persist/mod.rs` (9), `persist/markdown.rs` (5) | `simard_meetings_dir_env, cognitive_memory` | *(unchanged — already correct)* |

**Audit closed.** The only tests in `tests_persist.rs` that touch
`meetings_dir` / `write_handoff_markdown_report` are the two at L245 and L271;
every other test in that module is a pure function (sanitize/extract) that never
reads the resolver. The `cognitive_memory`-only tests in `agent_proxy.rs` and
`tests_goal_records_migration.rs` never touch the meetings dir and are correctly
left unchanged.

### Assertions preserved

Isolation was fixed; behaviour was not. Both handoff-report tests keep their
original assertions verbatim:

```rust
// handoff_report_omits_agenda_section_when_no_template_applied
assert!(!body.contains("## Agenda"));

// handoff_report_includes_agenda_section_when_template_applied
assert!(body.contains("## Agenda"));
```

### Validation — de-flaking a load-dependent bug

A single green run does **not** clear a load-flaky test. The acceptance gate is:

```bash
# Full-parallel, five consecutive runs — every run must be green.
# Fail fast: abort the moment any run is red rather than swallowing the status.
for i in 1 2 3 4 5; do
  echo "=== run $i ==="
  cargo test --lib || { echo "FAILED on run $i"; exit 1; }
done

# Formatting and lint gates (fmt invoked directly, never piped).
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Acceptance gate:

- `cargo test --lib` full-parallel passes **five consecutive times** with zero
  failures.
- `cargo fmt --check` is clean (direct invocation, no pipe).
- `cargo clippy --all-targets -- -D warnings` is clean.
- The two handoff-report assertions above are intact — no assertion weakened,
  moved, or deleted; no wall-clock timeout, sleep, or retry introduced.

Shipping this unfreezes Simard self-deploy so the already-merged label fix
(#4525) can go live.

---

## Validation

The fix is correct only if the formerly-flaky tests pass **deterministically**
across repeated runs:

```bash
# 1. The targeted module, five release runs — expect zero failures.
for i in 1 2 3 4 5; do
  cargo test --release --lib operator_commands_dashboard 2>&1 | tail -3
done

# 2. A couple of full runs.
cargo test
cargo test --release

# 3. Stress the closed race directly: run the dashboard tests together with the
#    formerly-disjoint env mutators under the multi-threaded runner.
cargo test --release --lib \
  operator_commands_dashboard cmd_cleanup ooda_brain bootstrap goal_curation

# 4. The regression guard.
cargo test --lib serial_guard
```

Acceptance gate:

- `cargo build` and the full lib test suite are green.
- All five iterations of step 1 pass with **zero** failures.
- `full_goal_lifecycle_crud`, the `tests_goals_crud` siblings, and the
  `tests_goal_records_migration` siblings pass on every run.
- `serial_guard` reports no offenders.
- **No assertion is weakened, moved, or deleted.**

---

## Related reading

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how
  an individual test allocates an isolated state root with `HermeticState`. This
  page is its whole-binary counterpart.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — the original
  "serial env-var tests" pattern (issue #2197) that this fix generalizes from a
  handful of tests to the entire env-touching surface.
- [Goal board API](../reference/goal-board-api.md) — the handler surface
  (`save_goal_board`, `load_goal_board`, …) whose env-default reads are
  serialized here.
- [Cognitive memory client helpers](../reference/cognitive-memory-client-helpers.md)
  — `launch_writer_client` and the per-state-root socket path.
