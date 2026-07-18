---
title: De-flaking the dispatch-timing and cost-ledger tests (wall-clock ratio + HOME race)
description: >
  How two pre-existing flaky lib-binary tests were made deterministic under
  parallel `cargo test --lib` — the tests that intermittently failed the
  `pre-commit` verify gate and blocked a cluster of otherwise merge-ready PRs
  (#4322, #4324, #4325, #4328). `concurrent_dispatch_parallelizes_and_respects_cap`
  is de-flaked by replacing a brittle wall-clock ratio assertion with a
  directional check backed by the existing structural concurrency proofs;
  `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` is
  de-flaked by a `#[cfg(test)]` thread-local ledger-path override that isolates
  the cost-ledger lookup from concurrent `HOME` mutators. Both fixes are
  additive and test-scoped; the production cost-tracking path is byte-for-byte
  unchanged.
last_updated: 2026-07-18
review_schedule: when the cost-ledger path resolution changes, when the meeting turn stops running on the test thread, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
---

# De-flaking the dispatch-timing and cost-ledger tests

This page documents the finished state of the work that made two pre-existing
flaky lib-binary tests deterministic under parallel `cargo test --lib`. Both
tests are on `origin/main` (neither was introduced by a PR), and both
intermittently failed the CI check named `pre-commit` in
`.github/workflows/verify.yml` — which runs the cargo test gate directly, not a
Python pre-commit hook suite. Their shared *symptom* (a red `pre-commit` check)
blocked a cluster of otherwise merge-ready PRs
([#4322](https://github.com/rysweet/Simard/pull/4322),
[#4324](https://github.com/rysweet/Simard/pull/4324),
[#4325](https://github.com/rysweet/Simard/pull/4325),
[#4328](https://github.com/rysweet/Simard/pull/4328)); de-flaking on `main`
unblocks all four at once.

This is the test-author and reviewer contract for the two hardening mechanisms,
plus the verification gate that keeps them closed.

The flakes and their fixes:

| Flaky test | Location | Root cause | Fix |
| ---------- | -------- | ---------- | --- |
| `concurrent_dispatch_parallelizes_and_respects_cap` | `src/ooda_actions/tests_dispatch_concurrency.rs` | A wall-clock ratio assertion (`parallel_elapsed * 2 <= serial_elapsed`) that jitters under scheduler contention on a busy CI runner, even though concurrency is already proven structurally | Replace the ratio assertion with a directional check (`parallel_elapsed < serial_elapsed`); keep every structural assert |
| `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` | `src/base_type_copilot/tests.rs` | The cost ledger resolves its path from `HOME` **at call time**; a *concurrent* lib-binary test mutating `HOME` could tear that read, so the meeting turn wrote its entry under a different HOME than the assertion read | Add a `#[cfg(test)]` thread-local ledger-path override consumed by `cost_tracking::ledger_path()`, and point the test at an explicit, thread-owned ledger path |

Both fixes preserve full test parallelism and neither blanket-serializes the
suite. **No production behaviour changes.** The dispatch scheduler is untouched,
and the cost-tracking write path resolves its ledger path from the environment
exactly as before — the override seam is compiled out of non-test builds.

> **One coupled observability fix.** While isolating the ledger race, the
> silently-swallowed write failure in the meeting path
> (`src/base_type_copilot/mod.rs`) — previously an `eprintln!` — was converted to
> a structured `tracing::warn!` that logs the **error kind only**. A failed
> ledger write now surfaces as a warning instead of masquerading as "entry not
> recorded," and the change also satisfies the repo's no-`print!`/`println!`
> logging constraint. This is the only non-test edit, and it is additive.

> **TL;DR**
>
> - **Dispatch timing:** delete the brittle `parallel_elapsed * 2 <= serial_elapsed`
>   ratio. Concurrency is already proven by `peak_parallel >= 2`, `peak_serial <= 1`,
>   and `run_count == N`. The only timing claim retained is the directional
>   `parallel_elapsed < serial_elapsed`.
> - **Cost ledger:** `cost_tracking::ledger_path()` consults a `#[cfg(test)]`
>   thread-local override before falling back to `SIMARD_COST_LEDGER` (test builds
>   only) and finally `$HOME/.simard/costs/ledger.jsonl`. The meeting test sets the
>   override to a temp path via an RAII guard, so its ledger lookup never races a
>   concurrent `HOME` write.
> - **Production is byte-for-byte unchanged.** Both override branches are strictly
>   `#[cfg(test)]`-gated; a `--release` build resolves the ledger from `HOME`
>   exactly as it did before.

---

## Fix 1 — the dispatch-timing wall-clock flake

### The flake

`concurrent_dispatch_parallelizes_and_respects_cap`
(`src/ooda_actions/tests_dispatch_concurrency.rs`) exercises
`dispatch_actions_bounded` twice over the same four `AdvanceGoal` actions:

1. **Run 1** with `cap = N` — all four dispatch concurrently.
2. **Run 2** with `cap = 1` — dispatch is serialized.

Each fake `run_turn` sleeps for a fixed duration, so the concurrent run should
finish faster in wall-clock terms. The final assertion demanded a **hard 2×
speedup**:

```rust
// BEFORE — brittle:
assert!(
    parallel_elapsed * 2 <= serial_elapsed,
    "concurrent dispatch must be >=2x faster than serialized: \
     parallel={parallel_elapsed:?}, serial={serial_elapsed:?}"
);
```

On a loaded CI runner, scheduler contention inflates `parallel_elapsed` (or
compresses `serial_elapsed`) enough that the 2× margin is not always met — even
though the code is correct. **Proof it is a flake, not a regression:** PR #4328's
failing and passing CI runs are on the *identical* commit `842748390e`, 15
seconds apart. Same code, different outcome ⇒ non-determinism in the assertion,
not in the product.

### The rule

Wall-clock **ratios** are not a valid concurrency proof in a shared-runner test.
Concurrency in this test is already established **structurally**, and those
asserts are retained unchanged:

| Assertion | Line (approx.) | What it proves |
| --------- | -------------- | -------------- |
| `run_count == N` | ~144 | Each goal's `run_turn` is invoked exactly once |
| `peak_parallel >= 2` | ~150 | With `cap = N`, slow calls genuinely overlap |
| `peak_serial <= 1` | ~167 | With `cap = 1`, dispatch is genuinely serialized |

Given those, the only wall-clock claim worth making is **directional**: the
concurrent run is faster than the serialized run, not faster by a fixed factor.

```rust
// AFTER — directional, backed by the structural asserts above:
assert!(
    parallel_elapsed < serial_elapsed,
    "concurrent dispatch (cap=N) must be faster than serialized (cap=1): \
     parallel={parallel_elapsed:?}, serial={serial_elapsed:?}"
);
```

The per-call sleep already dominates per-call overhead (both runs build the same
`N` inputs; only the sleep parallelizes), so the directional check keeps
roughly the same headroom the 2× check intended while removing the ratio's
false-failure surface.

### Writing a new dispatch-concurrency test

- **Prove overlap with the instrumentation counters** (`peak`, `live`,
  `run_count`), never with a wall-clock ratio.
- If you must make a timing claim, keep it **directional** (`a < b`), and only as
  a secondary signal on top of the structural asserts.
- Keep the fake `run_turn` sleep comfortably larger than per-call overhead so the
  directional comparison stays stable.

---

## Fix 2 — the cost-ledger `HOME` race

### The flake

`meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective`
(`src/base_type_copilot/tests.rs`) verifies that a Copilot meeting turn records
its cost against the **full enriched prompt** rather than the bare objective
(issue [#4164](https://github.com/rysweet/Simard/issues/4164)). It does so by:

1. pointing `HOME` at a fresh `TempDir`,
2. running a fake meeting turn (which calls `cost_tracking::record_cost`),
3. reading `$HOME/.simard/costs/ledger.jsonl` and asserting the recorded
   `prompt_tokens_est` exceeds the bare-objective token estimate.

The cost ledger resolves its path from `HOME` **at call time**:

```rust
// src/cost_tracking.rs — BEFORE
fn ledger_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard").join("costs").join("ledger.jsonl")
}
```

The test carries `#[serial_test::serial(cognitive_memory)]`, which serializes it
against every *keyed* env mutator (see
[serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)).
But the race is fundamentally that `record_cost` reads a **process-global**
`HOME` mid-run: any lib-binary test that mutates `HOME` between step 2's write
and step 3's read can send the write to a different tree than the read, so the
assertion sees an empty or missing ledger. The failure then presents
misleadingly as "a copilot-meeting cost entry for this session must be recorded."

### The mechanism: a `#[cfg(test)]` thread-local ledger override

The de-flake removes the ambient dependency from the *exercised* path without
touching production. `ledger_path()` gains a test-only resolution prefix that
runs **before** the `HOME` read; the meeting turn executes synchronously on the
test thread (via the `SingleProcess` topology), so a **thread-local** override is
race-proof by construction — no other thread can observe or mutate it.

```rust
// src/cost_tracking.rs — AFTER (production behaviour unchanged)
#[cfg(test)]
thread_local! {
    static LEDGER_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

fn ledger_path() -> PathBuf {
    // Test-only seam. Compiled out of --release; production falls straight
    // through to the HOME resolution below, byte-for-byte as before.
    #[cfg(test)]
    {
        if let Some(path) = LEDGER_PATH_OVERRIDE.with(|c| c.borrow().clone()) {
            return path;
        }
        if let Ok(path) = std::env::var("SIMARD_COST_LEDGER") {
            return PathBuf::from(path);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard").join("costs").join("ledger.jsonl")
}
```

Tests set and clear the override through an RAII guard so it is always cleared,
even on panic:

```rust
// src/cost_tracking.rs — test-only API
#[cfg(test)]
pub(crate) struct LedgerPathGuard;

#[cfg(test)]
impl LedgerPathGuard {
    /// Redirect this thread's cost ledger to `path` until the guard drops.
    pub(crate) fn set(path: impl Into<PathBuf>) -> Self {
        LEDGER_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path.into()));
        LedgerPathGuard
    }
}

#[cfg(test)]
impl Drop for LedgerPathGuard {
    fn drop(&mut self) {
        LEDGER_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    }
}
```

### What the finished test looks like

The `LedgerPathGuard` is **additive to** the existing `HOME` redirection, not a
replacement for it. The guard's job is narrow: it closes the mid-run *ledger
lookup* race by making `ledger_path()` return a thread-owned path instead of
reading a process-global `HOME` that a concurrent test can tear between the
write and the read. The temp-`HOME` redirect is retained because the meeting
turn may touch **other** `HOME`-derived state (session/cognitive-memory
artifacts), and removing it would let the test write into the developer's real
`$HOME` — a hermeticity regression. So the finished test keeps its temp `HOME`
(and the `catch_unwind` restore + `#[serial(cognitive_memory)]` key that guard
it) and simply *adds* the ledger guard:

```rust
#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective() {
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    // SAFETY: serialised via #[serial(cognitive_memory)] — isolates any
    // HOME-derived writes the meeting turn makes beyond the cost ledger.
    unsafe { std::env::set_var("HOME", home.path()); }

    let result = std::panic::catch_unwind(|| {
        let ledger = home.path().join(".simard").join("costs").join("ledger.jsonl");

        // Thread-local: this turn runs synchronously on the test thread (via
        // SingleProcess), so the override cannot be observed or torn by any
        // concurrent test. This is what closes the mid-run ledger race — the
        // temp HOME above only handles the turn's *other* HOME-derived writes.
        let _ledger_guard = crate::cost_tracking::LedgerPathGuard::set(&ledger);

        let session_id = "session-00000000-0000-0000-0000-000000004164";
        let objective = "Meeting objective body for the #4164 cost-accounting regression.";
        let (_dir, bin) = fake_copilot("FAKE-COPILOT-OK: meeting reply");
        run_fake_meeting_turn_with_session("copilot-meeting-cost-4164", &bin, session_id, objective);

        let contents = std::fs::read_to_string(&ledger)
            .expect("meeting turn must write a cost ledger entry to the overridden path");
        let entry = contents
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|e| {
                e.get("session_id").and_then(|v| v.as_str()) == Some(session_id)
                    && e.get("model").and_then(|v| v.as_str()) == Some("copilot-meeting")
            })
            .expect("a copilot-meeting cost entry for this session must be recorded");

        let recorded = entry.get("prompt_tokens_est").and_then(|v| v.as_u64()).unwrap();
        let bare = crate::cost_tracking::estimate_tokens(objective.len());
        assert!(
            recorded > bare,
            "meeting prompt cost must reflect the full enriched prompt (issue #4164), \
             not the bare objective: recorded={recorded} bare_objective_tokens={bare}"
        );
    });

    // SAFETY: restore HOME before propagating any panic (same serial key).
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    if let Err(e) = result { std::panic::resume_unwind(e); }
}
```

The `#4164` assertion — that the recorded prompt cost exceeds the bare-objective
estimate — is **unchanged**. The only new element is the thread-local ledger
guard; the temp `HOME`, the `catch_unwind` restore, and the serial key all
remain. Point the guard at `home.path()/.simard/costs/ledger.jsonl` (as above)
so the guarded lookup and any residual `HOME`-relative lookup resolve to the
same tree.

### Production callers are unaffected

In any non-`test` build the two override branches do not exist: `cargo expand`
under `--release` shows `ledger_path()` reducing to the original `HOME`
resolution. There is no new environment variable read, file permission, or
process-global mutation on the production path, and no new dependency.

---

## Coupled fix — observable ledger-write failure

The meeting path previously swallowed a failed ledger write with an `eprintln!`,
which (a) violates the repo's structured-logging / no-`println!` constraint and
(b) made a genuine write failure look identical to "entry not recorded":

```rust
// src/base_type_copilot/mod.rs — BEFORE
if let Err(e) = crate::cost_tracking::record_cost( /* … */ ) {
    eprintln!("[simard] cost tracking write failed: {e}");
}
```

It now emits a structured warning that logs **only the error kind/`Display`** —
never ledger contents, token counts, session identifiers, API keys, or
`HOME`-derived absolute paths:

```rust
// src/base_type_copilot/mod.rs — AFTER
if let Err(e) = crate::cost_tracking::record_cost( /* … */ ) {
    tracing::warn!(error = %e, "cost tracking write failed");
}
```

This warning fires only on a genuine write failure (not on the hot path), so it
adds no log volume in the normal case and creates no information-disclosure
surface.

---

## API & configuration reference

### Test-only cost-ledger override (`src/cost_tracking.rs`)

| Symbol | Visibility | Purpose |
| ------ | ---------- | ------- |
| `LedgerPathGuard::set(path)` | `#[cfg(test)] pub(crate)` | Redirect the **current thread's** cost ledger to `path` until the returned guard drops. |
| `LedgerPathGuard` (`Drop`) | `#[cfg(test)] pub(crate)` | Clears the thread-local override on drop, including on panic. |
| `LEDGER_PATH_OVERRIDE` | `#[cfg(test)]` thread-local | Backing store consulted first by `ledger_path()`. Never present in `--release`. |

**Resolution order of `ledger_path()`** (first match wins):

1. `#[cfg(test)]` thread-local override, if set via `LedgerPathGuard` — **primary
   test seam, race-proof.**
2. `#[cfg(test)]` `SIMARD_COST_LEDGER` environment variable, if set —
   **documented fallback** for the rare case where a future ledger write happens
   on a thread *other than* the test thread (i.e., the meeting turn moves off the
   synchronous `SingleProcess` path). Because this is process-global, a test
   relying on it must also hold the `cognitive_memory` serial key.
3. `$HOME/.simard/costs/ledger.jsonl` — **the production path**, and the only
   branch present in non-test builds.

| Variable | Scope | Effect |
| -------- | ----- | ------ |
| `SIMARD_COST_LEDGER` | `#[cfg(test)]` only | Overrides the ledger path when no thread-local override is set. Absent from `--release`. |
| `HOME` | production + test | Base for the default ledger path (`$HOME/.simard/costs/ledger.jsonl`). |

> **Choosing a seam.** Prefer `LedgerPathGuard` (thread-local) whenever the code
> under test records cost **synchronously on the test thread** — it needs no
> serial key for the ledger lookup and cannot be torn by a concurrent `HOME`
> writer. Reach for `SIMARD_COST_LEDGER` only when the write demonstrably crosses
> a thread boundary, and pair it with `#[serial(cognitive_memory)]`.

---

## Verification gate

The fixes are correct only if the two formerly-flaky tests pass
**deterministically** across repeated runs.

### Targeted stress (the de-flake proof)

```bash
# Dispatch timing — 20 release iterations, expect zero failures.
for i in $(seq 1 20); do
  cargo test --release --lib \
    ooda_actions::tests_dispatch_concurrency::concurrent_dispatch_parallelizes_and_respects_cap \
    2>&1 | tail -1
done

# Cost-ledger isolation — 20 iterations under the multi-threaded runner, run
# alongside HOME mutators to exercise the closed race directly.
for i in $(seq 1 20); do
  cargo test --release --lib \
    base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective \
    2>&1 | tail -1
done
```

### Full suite

```bash
cargo test --lib
cargo test --release --lib
```

### Production-absence check

```bash
# The test-only override symbols must NOT appear in a release build.
cargo build --release
! nm -C target/release/libsimard.rlib 2>/dev/null | grep -i "LedgerPathGuard"
```

### Repo gates

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Acceptance gate:

- The `pre-commit` verify check passes **on re-run** for all four PRs' required
  checks (#4322, #4324, #4325, #4328).
- All 20 iterations of each targeted stress loop pass with **zero** failures.
- `cargo fmt --check` and `cargo clippy` are clean; no `print!`/`println!` in
  touched files.
- The `#4164` and structural concurrency assertions are **not** weakened, moved,
  or deleted — only the wall-clock ratio and the ledger *location* changed.
- The override symbols are absent from the `--release` build.

---

## Scope

**In scope:** de-flaking the two named tests on `main`; the `#[cfg(test)]` ledger
override in `cost_tracking.rs`; the meeting-test wiring; the directional dispatch
assertion; the `eprintln!` → `tracing::warn!` observability fix; this document.

**Out of scope:** merging any of the four blocked PRs; restructuring the CI hook
suite; bumping unrelated tooling; any change to production ledger resolution,
dispatch scheduling, or the `#4164` cost-accounting contract.

---

## Related reading

- [serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
  — the process-global env race and the serial key that guards the watched
  surface; the meeting test keeps its `cognitive_memory` key, and the
  `SIMARD_COST_LEDGER` fallback lives under that same contract.
- [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md) — the
  companion "ambient wrapper + explicit-path core" de-flake for the goal-board
  state-root race; the ledger override here is the same
  inject-the-path-instead-of-reading-the-env pattern applied to the cost ledger.
- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how an
  individual test allocates an isolated state root.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — why wall-clock
  ratios and ambient env reads are anti-patterns in the shared-runner suite.
