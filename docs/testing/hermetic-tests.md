---
title: Writing hermetic tests against cognitive memory
description: Test-author contract for the SIMARD_STATE_ROOT / SIMARD_MEMORY_SOCKET hermeticity guard, the helper APIs that satisfy it, and the regression check that prevents leaks into ~/.simard.
last_updated: 2026-05-19
review_schedule: at every cognitive-memory schema or socket-path change
owner: simard
doc_type: reference
related:
  - ../reference/simard-cli.md
  - ../reference/goal-board-api.md
  - ../reference/cognitive-memory-bridge-helpers.md
  - ../howto/clean-fixture-leaks.md
  - ./COVERAGE_BASELINE.md
---

# Writing hermetic tests against cognitive memory

This page is the test-author contract for any test — unit, integration,
or end-to-end — that exercises code which can write to cognitive
memory. The contract exists because, before issues
[#1923](https://github.com/rysweet/Simard/issues/1923) and
[#1925](https://github.com/rysweet/Simard/issues/1925), tests that
*looked* hermetic (constructed a `TempDir`, set `SIMARD_STATE_ROOT`)
silently wrote into the operator's live `~/.simard/cognitive_memory.ladybug`
because the IPC socket path was hard-coded to `~/.simard/memory.sock`
and the test's bridge-launch picked up the running daemon's socket
instead of opening its own DB.

The fix has two parts that test authors must understand:

1. **Socket path follows the state root.** `memory_ipc::socket_path_for(state_root)`
   resolves to `<state_root>/memory.sock` unless `$SIMARD_MEMORY_SOCKET`
   is set. Pointing `SIMARD_STATE_ROOT` at a `TempDir` is now sufficient
   to isolate from the live daemon — the test's writer bridge cannot
   even *find* the operator's socket.
2. **A hermetic-state-root guard runs in tests.** Every code path that
   reaches `save_goal_board` / `save_goal_board_with_removals` /
   `store_fact` under `#[cfg(test)]` asserts that the resolved state
   root is hermetic. The guard is the regression prevention for the
   *class* of mistake.

The rest of this page documents the contract and the helpers that make
satisfying it a one-liner.

## The contract — what every test must guarantee

A test that triggers a cognitive-memory write MUST guarantee, at the
moment of the write:

- (H1) `memory_ipc::default_state_root()` returns a path under
  `std::env::temp_dir()`.
- (H2) `memory_ipc::default_state_root()` is not equal to, and is not
  a descendant of, `$HOME/.simard`. Tests run under a shared CI account
  or a developer workstation must not be able to touch the operator's
  durable state, even when `$TMPDIR` is mis-configured to be inside
  `$HOME`.
- (H3) `memory_ipc::socket_path_for(default_state_root())` is under the
  same `TempDir`. (This holds automatically from (H1) when
  `SIMARD_MEMORY_SOCKET` is unset — which it must be in tests.)
- (H4) The `TempDir` outlives every bridge handle the test opens.
  Dropping the `TempDir` while a bridge is alive triggers WAL-write
  errors on Linux, which are easy to misread as schema bugs.

A test that needs to talk to the operator's daemon by design (the only
class is the install-real / install-fake harnesses for the npm wrapper)
sets the explicit opt-out env var `SIMARD_TEST_ALLOW_LIVE_STATE=1`
inside its own `TestSetup` and is documented in
`docs/reference/dashboard-e2e-tests.md` style. The hermetic guard
short-circuits when that env var is `1`. Use of this opt-out outside
the install harnesses requires a code-review acknowledgement.

## The hermetic-state-root guard (regression prevention)

The guard is invoked unconditionally inside cognitive-memory writers
under `cfg(test)`. Pseudocode:

```rust
#[cfg(test)]
fn assert_hermetic_state_root_for_tests() {
    if std::env::var("SIMARD_TEST_ALLOW_LIVE_STATE").as_deref() == Ok("1") {
        return;
    }
    let root = memory_ipc::default_state_root();
    let tmp = std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir());
    let home_simard = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".simard"))
        .unwrap_or_default();

    let canon = root.canonicalize().unwrap_or_else(|_| root.clone());

    assert!(
        canon.starts_with(&tmp),
        "hermetic-test guard tripped: state root {} is not under temp_dir {}. \
         Set SIMARD_STATE_ROOT to a TempDir before invoking any cognitive-memory writer. \
         See docs/testing/hermetic-tests.md.",
        canon.display(), tmp.display(),
    );
    assert!(
        !home_simard.as_os_str().is_empty() && !canon.starts_with(&home_simard),
        "hermetic-test guard tripped: state root {} is under HOME/.simard ({}). \
         A test is about to write into operator state. \
         See docs/testing/hermetic-tests.md.",
        canon.display(), home_simard.display(),
    );
}
```

Tripping the guard fails the test with a message that names the offending
state root and points to this page. The assertion message is
intentionally verbose because a tripped guard always indicates the test
needs a code change — there is no scenario in which retrying or
ignoring it is correct.

The guard runs at three sites, chosen to cover every path that can
mutate persisted state:

- `cognitive_memory::native::NativeCognitiveMemory::store_fact` (and its
  `store_episode` / `store_procedure` siblings).
- `goals::persistence::save_goal_board` and
  `goals::persistence::save_goal_board_with_removals` (immediately
  before the first `store_fact` call).
- `memory_ipc::launcher::launch_writer_bridge` (immediately before
  returning a writer bridge, regardless of which tier was selected).

Three independent enforcement points means deleting one of them does
not silently disable the guard — at least one will still fire.

## Helpers that satisfy the contract

You should rarely need to set the env vars by hand. The shipped helpers
do it correctly and clean up after themselves.

### `simard::test_support::HermeticState`

The recommended entry point. Construct one at the top of every test
that touches cognitive memory:

```rust
use simard::test_support::HermeticState;

#[test]
fn my_persistence_test() {
    let state = HermeticState::new();
    // state.state_root() — &Path under env::temp_dir()
    // state.socket_path() — <state_root>/memory.sock
    // SIMARD_STATE_ROOT is set for the duration of `state`
    // SIMARD_MEMORY_SOCKET is unset for the duration of `state`
    // (so socket_path_for follows the state root automatically)

    let bridge = launch_writer_bridge(state.state_root()).expect("bridge");
    save_goal_board(&board, bridge.ops()).expect("save");
    // bridge dropped, then state dropped — TempDir reaped last
}
```

`HermeticState` is a thin wrapper around `tempfile::TempDir` plus an
RAII env-var guard. The `Drop` impl restores the previous env-var
values, so two `HermeticState` instances in the same test file do not
cross-contaminate.

For tests that exercise multi-process daemon/client interactions in the
same temp root, use `HermeticState::shared_for_subprocess()` — same
contract, plus it exposes the socket path as an env var to spawned
children so the child's bridge picks up the same socket.

### `#[serial_test::serial(cognitive_memory)]`

Cognitive-memory tests run under the `cognitive_memory` serial group.
Always annotate:

```rust
#[test]
#[serial_test::serial(cognitive_memory)]
fn …
```

The serial group exists because `HermeticState` mutates process-wide
env vars (`SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`). Two parallel
tests in the same process would race on those vars and one would write
into the other's TempDir, silently passing the hermetic guard while
producing nonsense results.

## What NOT to do

The following patterns *look* hermetic but trip the guard or leak. Each
one is a known failure mode from the #1923 / #1925 forensics:

- **Setting `SIMARD_STATE_ROOT` without unsetting `SIMARD_MEMORY_SOCKET`.**
  If the caller's shell had `SIMARD_MEMORY_SOCKET=/some/path` exported,
  the test still talks to that socket — which may be the live daemon's.
  `HermeticState` explicitly unsets it.
- **Constructing `TempDir` inside `HOME`.** If `$TMPDIR` is unset and
  `env::temp_dir()` resolves to `/tmp` (the usual case), this is fine;
  but a developer with `$TMPDIR=~/tmp` set will trip the (H2) check.
  Either fix your shell, or use `HermeticState::new_in("/tmp")`.
- **Writing through a daemon socket "to test the IPC path".** If your
  test needs to verify daemon-side behaviour, start a daemon yourself
  inside the `HermeticState`'s temp root — don't reuse the operator's.
  `HermeticState::spawn_isolated_daemon()` is the supported helper.
- **Re-using `~/.simard` "because it's just a unit test".** The
  `cargo test` runner has no isolation from the operator's home
  directory. The whole point of #1923 / #1925 is that this assumption
  was wrong even when the test author *thought* they were hermetic.

## Migrating an existing test

The cheapest correct migration:

1. Add `let _hermetic = HermeticState::new();` as the first line of the
   test body.
2. Annotate the function with `#[serial_test::serial(cognitive_memory)]`
   if it isn't already.
3. Replace any explicit `SIMARD_STATE_ROOT` / `SIMARD_MEMORY_SOCKET`
   manipulation with reads from `_hermetic`.
4. Run `cargo test --lib <module>` and confirm the hermetic guard does
   not fire. If it does, the message names the offending path and the
   fix is usually #1 (constructing `HermeticState` later than the
   first cognitive-memory write).

A grep that finds candidate tests:

```bash
rg --type rust -l 'save_goal_board|store_fact|launch_writer_bridge' \
   src/ tests/ | xargs rg -L 'HermeticState'
```

Every match in that list is a test (or test-adjacent helper) that
needs migration. The PR closing #1923 / #1925 migrates the known
historical offenders in `src/operator_cli/tests_goal.rs`,
`src/ooda_actions/tests_advance_goal.rs`, and
`src/goal_curation/tests_*.rs`. New code should never appear on this
list.

## Related reading

- [How to clean a fixture leak from the live goal board](../howto/clean-fixture-leaks.md)
  — the operator remediation for a leak that nevertheless reached
  production.
- [CLI reference — Shared socket-path contract](../reference/simard-cli.md#shared-socket-path-contract)
  — the operator-visible surface of the socket-path fix.
- [Goal board API — save_goal_board_with_removals](../reference/goal-board-api.md#save_goal_board_with_removals)
  — the removal API exercised by the cleanup commands; tests of it
  must use `HermeticState`.
- [Cognitive memory bridge helpers](../reference/cognitive-memory-bridge-helpers.md)
  — the `launch_writer_bridge` tier table now reflects the
  per-state-root socket path.
