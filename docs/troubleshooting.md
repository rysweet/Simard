# Troubleshooting

Common failure modes and how to recover.

## `npx github:rysweet/Simard` fails to download the binary

- Confirm `gh auth status` shows an authenticated account with access to `rysweet/Simard`.
- Confirm your platform is one of `linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-aarch64`, `windows-x86_64`. If not, build from source.
- Re-run with verbose: `GH_DEBUG=1 npx github:rysweet/Simard install`.

See [reference/npx-npm-install.md](reference/npx-npm-install.md).

## `copilot-sdk` base type fails: "amplihack: command not found"

The `copilot-sdk` adapter spawns `amplihack copilot` via PTY. Install amplihack:

```bash
cargo install --git https://github.com/rysweet/amplihack.git
```

or set `SIMARD_COPILOT_GH_ACCOUNT` and run `simard ensure-deps`. This is a known runtime dependency — tracked for removal in [amplihack-comparison.md](amplihack-comparison.md).

## `simard gym run ...` fails importing `amplihack.eval`

The gym bridge at `python/simard_gym_bridge.py` imports `amplihack.eval.progressive_test_suite` and `amplihack.eval.long_horizon_memory`. Install `amplihack-agent-eval` from amplihack until Simard ships native Rust gym eval.

## Dashboard shows nothing

- Confirm `simard dashboard serve` is running.
- Check port 8080 is not blocked.
- Confirm there is at least one goal / metric / cost record to render — a fresh install looks empty.

See [howto/run-dashboard-e2e-tests.md](howto/run-dashboard-e2e-tests.md).

## Low-disk `cargo build` OOMs on LadybugDB / lbug

Use `CARGO_TARGET_DIR` on a larger partition and set `CARGO_BUILD_JOBS=4 CMAKE_BUILD_PARALLEL_LEVEL=4`. See [howto/reclaim-disk-space-and-run-low-space-rust-builds.md](howto/reclaim-disk-space-and-run-low-space-rust-builds.md).

## Pre-push hook fails on `cargo clippy`

The pre-push hook runs `cargo clippy --all-targets`, which may OOM-link on disk-pressured machines. Use `git push --no-verify` sparingly, or set up a separate build target directory. Do not disable clippy in CI.

## Meeting REPL hangs after a decision

Likely a meeting-daemon IPC issue. Check `~/.simard/meetings/*/daemon.log`. Recent fix merged on `main` as of April 2026; ensure you are on the latest release (`simard update`).

## Memory bridge errors

`bridge_launcher.rs` prepends `~/.amplihack/src` and related paths to `PYTHONPATH`. If you run without amplihack installed, knowledge-pack operations that go through the Python bridge may fail. The cognitive memory core (backed by the `amplihack-memory` Rust crate) is native and does not require Python.

## Filing a bug

- Reproduce with the minimum command.
- Capture stderr and the session record.
- Open an issue at [github.com/rysweet/Simard/issues](https://github.com/rysweet/Simard/issues).
