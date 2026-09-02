---
title: Checkout-independent workdir tests
description: >
  The test-author contract that keeps the `resolve_agent_workdir` unit tests
  green whether the library test binary runs inside a git checkout (CI) or
  inside a non-git build directory (the self-deploy deploy gate). Documents the
  discoverable-root precondition (skip-on-absence) pattern, why redirecting the
  process cwd is forbidden, and the anti-hardcoded-path invariants that must
  never be weakened.
last_updated: 2026-07-23
review_schedule: when resolve_agent_workdir resolution order or the self-deploy gate cwd changes
owner: meeting-backend
doc_type: reference
related:
  - ./ci-resilient-test-patterns.md
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ../reference/meeting-backend-api.md
  - ../reference/argv-free-meeting-agent-proxy.md
  - ../operations/meeting-handoffs.md
  - ../safe-self-update.md
---

# Checkout-independent workdir tests

This page is the test-author contract for the two unit tests in
`src/meeting_backend/agent_proxy.rs` that exercise
[`resolve_agent_workdir()`](../reference/meeting-backend-api.md). Those tests
must pass **regardless of whether the ambient current working directory is a
git checkout**, because the library test binary runs in two structurally
different environments:

- **CI** runs `cargo test --lib` from inside the repository checkout, so the
  ambient cwd *is* a git repository root.
- **The self-deploy deploy gate** runs the compiled lib-test binary
  (`~/.simard/self-deploy-target/debug/deps/simard-*`) directly from a non-git
  build directory, so there is **no** discoverable `.git` root above the cwd.

Before this contract existed, the two tests assumed the CI environment
unconditionally: they called `resolve_agent_workdir()` and `.expect()`-panicked
when it returned `None`. That produced a **CI-green / deploy-gate-red gap** — the
tests passed on every PR check yet failed on every self-deploy cycle, which
manifested as a persistent red-canary crash-loop that blocked self-deploy for
6+ hours (running binary stuck two commits behind merged `main`, recurring
`DeployDrift`, `test result: FAILED. 9256 passed; 2 failed`, process exit
status `101`). See issues
[#4505](https://github.com/rysweet/Simard/issues/4505) (the gap and this fix)
and [#2549](https://github.com/rysweet/Simard/issues/2549) (the underlying
repo-derived-workdir feature these tests protect).

The fix makes the two tests **tolerate the absence of a discoverable git
checkout**: they assert the full invariants whenever a repository root *is*
discoverable (always true in CI) and skip cleanly when it is not (the deploy
gate), without ever weakening the anti-hardcoded-path assertions and without
mutating any process-global state beyond the already-serialized `WORKDIR_ENV`.

---

## What `resolve_agent_workdir()` guarantees (unchanged)

The production function is **not** modified by this contract. It resolves the
directory a meeting agent operates in, in this order:

1. `SIMARD_MEETING_AGENT_DIR` (the `WORKDIR_ENV` constant) when it names an
   existing directory.
2. The repository root of the current working directory, via
   `git rev-parse --show-toplevel`.

It returns `None` when neither source yields a repository — for example when a
meeting is launched outside any git checkout. Returning `None` is **correct
behavior**, not a bug: callers then no-op the `--add-dir` grant and let the
agent inherit the process cwd instead of pointing it at some other operator's
worktree. The self-deploy gate reaching branch 2 with no discoverable root is
exactly the `None` path, and the tests — not the function — were wrong to treat
it as fatal.

---

## Why not just construct a temp git checkout?

The obvious alternative — have each test `git init` a throwaway `TempDir` and
point resolution at it — is **rejected for branch-2 (cwd-derivation) tests**,
because the only way to drive `git rev-parse --show-toplevel` at a chosen
directory is to change the process's current working directory with
`std::env::set_current_dir`.

**The current working directory is process-global and is read by many tests
that run concurrently.** `cargo test` executes the ~9 200 unit tests in one
multi-threaded process. Redirecting cwd — even briefly, even with
restore-before-assert — races against every concurrent cwd reader and corrupts
them non-deterministically. This was observed empirically: adding a serialized
`set_current_dir` to these two tests turned a green suite into runs failing
`worktree_gc::liveness::tests::procfs_probe_detects_self_cwd`,
`terminal_session::*`, and other cwd-sensitive tests — up to two dozen unrelated
failures in a single run, appearing and disappearing between runs.

This is why **`set_current_dir` appears nowhere in the `src/` tree**: the codebase
deliberately never mutates process cwd in tests. The `cognitive_memory` serial
key only serializes tests that *share that key*; it cannot protect the hundreds
of other tests that read cwd without it. A per-test cwd redirect is therefore
fundamentally unsafe here, regardless of serialization.

> Branch **1** (explicit override) tests *may* still use a `TempDir`, because
> they point `SIMARD_MEETING_AGENT_DIR` at it and never touch cwd — see
> `resolve_agent_workdir_honors_explicit_override`. The restriction is specific
> to driving branch-2 cwd-derivation.

---

## The contract — what every workdir test must guarantee

A branch-2 (cwd-derivation) test MUST:

- **(W1)** Never mutate the process current working directory. Do **not** call
  `std::env::set_current_dir`. Resolution derives from the *ambient* cwd.
- **(W2)** Force the branch under test. To exercise cwd-derivation, clear
  `WORKDIR_ENV` so branch 1 cannot short-circuit; to exercise the
  bogus-override fall-through, set `WORKDIR_ENV` to a non-existent path.
- **(W3)** Serialize any process-global env mutation. Clearing or setting
  `WORKDIR_ENV` is process-wide, so the test carries the shared
  `cognitive_memory` serial key. See
  [Pattern 3 in ci-resilient-test-patterns.md](./ci-resilient-test-patterns.md#pattern-3-serial-for-env-var-mutating-tests).
- **(W4)** Restore `WORKDIR_ENV` **before asserting**, so a failing assertion
  cannot leak the mutated env into other tests (the same restore-before-assert
  discipline used by `resolve_agent_workdir_honors_explicit_override`).
- **(W5)** Guard on a discoverable root. Bind the result with `let Some(resolved)
  = ... else { <trace + return> }`. When resolution yields `None` (no `.git`
  discoverable from cwd — the deploy gate), skip cleanly: the decision is traced
  via `debug!`, never printed, and never a silent fallback.
- **(W6)** Preserve the anti-hardcoded-path invariants verbatim. Whenever a path
  *is* resolved, it must be a real directory, must contain `.git`, and must
  **never** equal the hardcoded operator path
  `/home/azureuser/src/Simard/worktrees/main` (issue #2549). These assertions
  MUST NOT be weakened, relaxed, or removed — the skip in (W5) only applies when
  there is no path at all to assert against.

> **Skip-on-absence is not the same as weakening the invariants.** The invariant
> assertions still run in full on every CI PR check (where cwd is a checkout).
> The skip only fires in the deploy gate, where `resolve_agent_workdir()`
> correctly returns `None` and there is genuinely no resolved path to test. The
> anti-hardcoded-path assertion is never bypassed for a real path.

---

## The two tests

### Test A — `resolve_agent_workdir_derives_repo_root_from_cwd`

Verifies **branch 2** (cwd-derivation via `git rev-parse --show-toplevel`)
against the ambient cwd. It clears `WORKDIR_ENV` (branch-1 override) so branch 2
is the path under test, and is serialized under the shared `cognitive_memory`
key because that clear is a process-global env mutation.

```rust
#[test]
#[serial_test::serial(cognitive_memory)]
fn resolve_agent_workdir_derives_repo_root_from_cwd() {
    // Clear any explicit override so resolution must derive from the ambient
    // cwd; do NOT mutate cwd (that corrupts concurrent cwd readers).
    let prev = std::env::var_os(WORKDIR_ENV);
    // SAFETY: env mutation is serialised via the serial key above.
    unsafe { std::env::remove_var(WORKDIR_ENV) };

    let resolved = resolve_agent_workdir();

    // Restore before asserting so a panic cannot leak the cleared override.
    unsafe {
        if let Some(v) = &prev {
            std::env::set_var(WORKDIR_ENV, v);
        }
    }

    let Some(resolved) = resolved else {
        // No git checkout discoverable from cwd (the self-deploy gate). Nothing
        // to assert; skip cleanly (traced, not a silent fallback).
        debug!(
            "resolve_agent_workdir_derives_repo_root_from_cwd: no repo root \
             discoverable from cwd — skipping repo-root assertions (issue #4505)"
        );
        return;
    };

    assert!(resolved.is_dir(), "resolved workdir must exist");
    assert!(
        resolved.join(".git").exists(),
        "resolved workdir must be a git repository root: {}",
        resolved.display()
    );
    assert_ne!(
        resolved,
        PathBuf::from("/home/azureuser/src/Simard/worktrees/main"),
        "workdir must never be the hardcoded operator path (issue #2549)"
    );
}
```

Key points:

- The `#[serial_test::serial(cognitive_memory)]` attribute is **added** so this
  test never runs concurrently with the override tests that mutate
  `WORKDIR_ENV`; without it, a leaked override could send this test down
  branch 1 and defeat its purpose.
- Clearing `WORKDIR_ENV` (not setting cwd) is what forces *cwd-derivation*. The
  override branch is already covered by
  `resolve_agent_workdir_honors_explicit_override`.
- The three invariant assertions (W6) are byte-for-byte the originals; only the
  `.expect()` became a discoverable-root guard (W5).

### Test B — `resolve_agent_workdir_ignores_nonexistent_override`

Verifies that a **bogus** `SIMARD_MEETING_AGENT_DIR` is ignored and resolution
**falls through** to branch 2 rather than a hardcoded path. It keeps its
existing bogus-override set/restore and adds the discoverable-root guard so the
fall-through is tolerated in every environment.

```rust
#[test]
#[serial_test::serial(cognitive_memory)]
fn resolve_agent_workdir_ignores_nonexistent_override() {
    let prev = std::env::var_os(WORKDIR_ENV);
    // SAFETY: env mutation is serialised via the serial key above.
    unsafe { std::env::set_var(WORKDIR_ENV, "/nonexistent/simard/meeting/dir") };

    let resolved = resolve_agent_workdir();

    // Restore before asserting (restore-before-assert).
    unsafe {
        match &prev {
            Some(v) => std::env::set_var(WORKDIR_ENV, v),
            None => std::env::remove_var(WORKDIR_ENV),
        }
    }

    let Some(resolved) = resolved else {
        // Bogus override correctly ignored, and no repo root discoverable from
        // cwd (the self-deploy gate). Nothing to assert; skip cleanly.
        debug!(
            "resolve_agent_workdir_ignores_nonexistent_override: bogus override \
             ignored and no repo root discoverable from cwd — skipping (issue #4505)"
        );
        return;
    };
    assert_ne!(
        resolved,
        PathBuf::from("/home/azureuser/src/Simard/worktrees/main"),
        "must not resolve to the hardcoded operator path"
    );
}
```

Key points:

- `WORKDIR_ENV` is restored before the assertions (W4).
- The bogus-override string and the `assert_ne!` invariant are unchanged; only
  the `.expect()` became a discoverable-root guard (W5).

> The sibling test `resolve_agent_workdir_honors_explicit_override` (branch 1)
> is **not** changed by this contract: it already points `WORKDIR_ENV` at a
> `TempDir` and never depends on the ambient cwd.

---

## Configuration

| Knob | Where | Effect |
| --- | --- | --- |
| `SIMARD_MEETING_AGENT_DIR` (`WORKDIR_ENV`) | Process env | Branch-1 explicit override. Tests set/clear and restore it under the serial key; production reads it first. |
| Serial key `cognitive_memory` | `#[serial_test::serial(cognitive_memory)]` | Shared across all three env-mutating workdir tests so none run concurrently. Reuse this exact key — do not introduce a new one. |
| `git` CLI | `PATH` | Required by production branch 2. Present in CI and the deploy gate. |

No production code, `Cargo.toml`, or dependency changes are required. The fix is
confined to the `#[cfg(test)]` block of `src/meeting_backend/agent_proxy.rs`.

---

## How to verify

The gate runs the **compiled lib-test binary directly** from a non-git build
directory. `cargo test --manifest-path <checkout>/Cargo.toml` does **not**
reproduce it, because cargo sets the test process's cwd to the manifest
directory (the checkout) — so run the built binary yourself from a non-git cwd.

```bash
# 1. Build the lib-test binary (path is printed after "Executable unittests").
cd /home/azureuser/src/Simard
bin="$(cargo test --lib --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys,json;
[print(json.loads(l)["executable"]) for l in sys.stdin
 if l.strip().startswith("{") and json.loads(l).get("target",{}).get("name")=="simard"
 and json.loads(l).get("executable")]' | tail -1)"

# 2. Reproduce the gate: run the binary from a directory with no .git above it.
workdir="$(mktemp -d)"; cd "$workdir"
"$bin" resolve_agent_workdir
# Expect: test result: ok. 3 passed; 0 failed  — shell exit status 0.
# (The two branch-2 tests skip cleanly here; honors_explicit_override runs.)
echo "exit=$?"
```

```bash
# 3. Regression check from inside the checkout (CI environment): the branch-2
#    invariant assertions actually execute here.
cd /home/azureuser/src/Simard
cargo test --lib resolve_agent_workdir
# Expect: all three workdir tests pass; no deadlock, no env leak.
```

```bash
# 4. Full-suite gate reproduction: the whole library from a non-git cwd.
workdir="$(mktemp -d)"; cd "$workdir"
"$bin"                       # parallel (default)
"$bin" --test-threads=1      # serial: deterministic exit 0
# Expect: test result: ok. ... 0 failed. Serial is the authoritative check;
# rare parallel-only failures in unrelated modules (e.g. an install-lock flock
# race) are pre-existing contention flakes, not caused by these tests.
```

The three workdir tests that must pass in both environments:

| Test | Branch exercised | Deploy gate (non-git) |
| --- | --- | --- |
| `resolve_agent_workdir_derives_repo_root_from_cwd` | 2 (cwd-derivation) | skips (resolution `None`) |
| `resolve_agent_workdir_honors_explicit_override` | 1 (explicit override) | asserts (uses its own `TempDir`) |
| `resolve_agent_workdir_ignores_nonexistent_override` | 1 → 2 (bogus override falls through) | skips (resolution `None`) |

---

## Relationship to the other test-isolation patterns

This contract is another face of the same root cause — `cargo test` runs tests
in parallel within one process, so any process-global mutation must be isolated
*or avoided entirely*:

- **`HermeticState`** isolates cognitive-memory *state roots*
  ([hermetic-tests.md](./hermetic-tests.md)).
- **`#[serial]` on env-var tests** isolates process-wide *environment variables*
  ([Pattern 3](./ci-resilient-test-patterns.md#pattern-3-serial-for-env-var-mutating-tests)).
- **This contract** handles the process-wide *current working directory* by
  **not mutating it at all**: it forces the branch under test via the serialized
  `WORKDIR_ENV` env var and tolerates a missing checkout via a discoverable-root
  precondition, so cwd-derived resolution is deterministic in CI and skips
  cleanly in the self-deploy gate — without ever redirecting the shared cwd.

Never call `set_current_dir` in a test. If a test must drive
`git rev-parse --show-toplevel` at a specific repository, it must do so through
the ambient cwd (CI) and skip when no root is discoverable, per (W5) above.
