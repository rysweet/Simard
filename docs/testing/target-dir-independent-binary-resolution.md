# Target-Dir-Independent Binary Resolution in E2E Tests

Integration tests that shell out to the compiled `simard` binary resolve its
path through Cargo's compile-time `CARGO_BIN_EXE_simard` environment variable
rather than a hardcoded `<manifest>/target/debug/simard` path. This makes the
tests correct under any active target directory — including the **redirected
target dir** the self-deploy canary uses.

## Why this matters

The self-deploy deploy gate compiles and runs the test suite inside a
redirected workspace:

| Location            | Path                                          |
| ------------------- | --------------------------------------------- |
| Source              | `~/.simard/self-deploy-src`                   |
| Target dir          | `~/.simard/self-deploy-target`                |
| Compiled binary     | `~/.simard/self-deploy-target/debug/simard`   |

A test that hardcodes `env!("CARGO_MANIFEST_DIR")/target/debug/simard` looks for
the binary in the *manifest's* `target/debug`, which does not exist in the
redirected workspace. The `assert!(binary.exists(), ...)` guard then panics with
`exit status: 101`, turning the canary **red** and blocking every self-deploy.

Resolving the path via `CARGO_BIN_EXE_simard` instead points at the active
target dir automatically, so the same tests pass in both a normal workspace and
the redirected self-deploy workspace.

## The pattern

Cargo sets `CARGO_BIN_EXE_<name>` at **compile time** for every integration
test, where `<name>` is the binary's name. Here that is `simard`, the crate's
default binary (package `name = "simard"` built from `src/main.rs`), not one of
the auxiliary `[[bin]]` gym targets. The variable always points at the binary
under the *active* `CARGO_TARGET_DIR`, and Cargo guarantees the binary is built
before the test runs.

The snippet below shows the helper in isolation. In
`tests/e2e_engineer_external_repo.rs` the rewritten body only references
`PathBuf`, but the existing `use std::path::{Path, PathBuf};` import **stays as
is** — `Path` is still used by `engineer_loop_inspects_external_repo`
(`Path::new(env!("CARGO_MANIFEST_DIR"))`). Do not narrow the import to
`PathBuf` alone.

```rust
use std::path::PathBuf;

/// Resolve the Simard binary path from the active Cargo target directory.
///
/// Uses Cargo's compile-time `CARGO_BIN_EXE_simard`, which Cargo builds and
/// populates for integration tests. This resolves correctly under a redirected
/// `CARGO_TARGET_DIR` (e.g. the self-deploy canary workspace), unlike a
/// manifest-relative `target/debug/simard` path.
fn simard_binary() -> PathBuf {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_simard"));
    assert!(
        binary.exists(),
        "Simard binary not found at {}. Cargo should build it automatically \
         for integration tests; a missing binary indicates a broken build.",
        binary.display()
    );
    binary
}
```

### Do

- Resolve binaries with `env!("CARGO_BIN_EXE_<binname>")`.
- Keep a loud `assert!(binary.exists(), ...)` so a genuinely missing binary
  fails **closed** with an actionable message.
- Pass the resulting `PathBuf` directly to `Command::new(...)` (or via
  `.to_str().unwrap()` when composing arguments for `timeout`).

### Don't

- Don't join `env!("CARGO_MANIFEST_DIR")` with `target/debug/<bin>` — this
  ignores `CARGO_TARGET_DIR` and breaks under redirected workspaces.
- Don't require callers to `cargo build` first — Cargo builds the bin
  dependency for integration tests automatically.
- Don't add a manifest-relative fallback. A fallback would only ever re-point
  at the exact broken path, reintroducing the crash-loop without benefit.
- Don't downgrade the missing-binary `assert!` to a silent skip or fallback.

## Anti-pattern (removed)

```rust
// BROKEN under a redirected CARGO_TARGET_DIR — do not use.
fn simard_binary() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = manifest_dir.join("target/debug/simard");
    assert!(
        binary.exists(),
        "Simard binary not found at {}. Run `cargo build` first.",
        binary.display()
    );
    binary
}
```

This form is what caused the `deploy_gate: red canary` failures across 13+
self-deploy commits, panicking on `gym_list_shows_all_scenarios` and
`meeting_repl_shows_greeting` because the manifest-relative binary path did not
exist in the redirected self-deploy target dir.

## Verification

Confirm the tests resolve the binary under an arbitrary/redirected target dir,
mirroring what the self-deploy canary does:

```bash
# Normal workspace (non-regression)
cargo test --test e2e_engineer_external_repo \
  gym_list_shows_all_scenarios meeting_repl_shows_greeting

# Redirected target dir (reproduces the self-deploy canary environment)
CARGO_TARGET_DIR="$(mktemp -d)/self-deploy-target" \
  cargo test --test e2e_engineer_external_repo \
  gym_list_shows_all_scenarios meeting_repl_shows_greeting
```

Both invocations must pass. In the redirected case, Cargo builds `simard` under
the temporary target dir and `CARGO_BIN_EXE_simard` points at it, so
`simard_binary()` resolves correctly and the canary no longer exits `101`.

Confirm no hardcoded paths remain:

```bash
# Expect: no matches
rg 'target/debug/simard' tests/
```

## Scope

- **In scope:** the `simard_binary()` helper in
  `tests/e2e_engineer_external_repo.rs` (the sole hardcoded-path consumer) and
  its stale module doc-comment (lines 7–9) telling callers to `cargo build`
  first and use `target/debug/simard`.
- **Unchanged imports:** keep `use std::path::{Path, PathBuf};` — `Path`
  remains in use at `Path::new(env!("CARGO_MANIFEST_DIR"))` in
  `engineer_loop_inspects_external_repo`. Only the helper body stops using
  `Path`.
- **Already compliant:** `tests/simard_cli.rs`, `tests/issue_1909_state_root_required.rs`,
  and other suites already use `CARGO_BIN_EXE_simard` / `CARGO_BIN_EXE_*`.
- **Out of scope:** production code, tracing/OTel, canary/self-deploy
  orchestration, and `#[ignore]`d LLM-dependent tests.

## Security notes

- The binary path is a compile-time literal supplied by Cargo, not built from
  user or runtime input — no path- or command-injection surface.
- The `PathBuf` is passed directly to `Command::new(...)`; the invocation is
  never routed through a shell string.
- No new runtime env-var trust is introduced (e.g. no user-supplied
  `SIMARD_BIN`); resolution relies solely on Cargo's compile-time
  `CARGO_BIN_EXE_simard`. A redirected `CARGO_TARGET_DIR` sits within the same
  trust boundary as every existing `CARGO_BIN_EXE_*` sibling test.
