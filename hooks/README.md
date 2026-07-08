# Simard native git hooks

These committed hooks replace the former Python `pre-commit` framework
(`.pre-commit-config.yaml`, removed in issue #3181). They shell out to `cargo`
directly — Simard is a pure-Rust daemon and does not depend on a Python runtime.

## Enable them

Point git at this directory (one-time, per clone):

```bash
git config core.hooksPath hooks
```

Per-engineer worktrees created by the daemon wire this automatically via
`crate::engineer_worktree::precommit::install_hooks`.

## What runs

| Hook         | Gates (identical strictness to CI `verify.yml`)                                                                   |
| ------------ | ---------------------------------------------------------------------------------------------------------------- |
| `pre-commit` | Rust-only gate · `cargo fmt --all -- --check` · `cargo clippy --release --no-deps -- -D warnings` (lbug wrapper)  |
| `pre-push`   | Rust-only gate · `cargo fmt --check` · race-subset `cargo test --release --lib` · `cargo clippy --all-targets --all-features --locked -- -D warnings` |

Do not bypass with `--no-verify`; CI enforces the same fences on every push.
