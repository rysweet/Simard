# Pre-Commit Setup

Step-by-step guide to wiring and verifying the local commit and push gates
that mirror Simard's CI `pre-commit` workflow.

Simard is a **pure-Rust, Python-free daemon** (issue #3181). Local gating is
provided by committed native git hooks (`hooks/pre-commit`, `hooks/pre-push`)
that shell out to `cargo` directly — there is no Python `pre-commit` framework,
no `pip`, and no `pipx`.

> Quick reference: see the
> [Local Pre-Commit Workflow section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#local-pre-commit-workflow).

---

## Prerequisites

- Linux or macOS development host (Windows via WSL2 is supported but
  untested by CI).
- A working Rust toolchain (`rustc`, `cargo`, `rustfmt`, `clippy`) matching the
  version CI uses. The repo does **not** currently pin a toolchain via
  `rust-toolchain.toml`; install whatever stable Rust CI is currently building
  against (see
  [`.github/workflows/`](https://github.com/rysweet/Simard/tree/main/.github/workflows)).
- `git`, and `jq` for the manual QA harnesses under `tests/gadugi/`.
- Repo cloned and you are at the repo root.

No Python runtime is required.

---

## Install

```bash
./scripts/install-precommit.sh
```

The script:

1. Verifies the repo ships committed hooks (`hooks/pre-commit`).
2. Runs `git config --local core.hooksPath hooks`, so git runs the committed
   `hooks/pre-commit` and `hooks/pre-push` scripts (both stages in one setting).
3. Ensures the committed hooks are executable and prints verification commands.

Re-running the script is safe; it simply re-asserts the setting.

### Manual install

```bash
git config --local core.hooksPath hooks
```

Per-engineer worktrees created by the daemon wire this automatically via
`crate::engineer_worktree::precommit::install_hooks`.

---

## What the Hooks Check

The committed hooks are the source of truth:
[`hooks/pre-commit`](https://github.com/rysweet/Simard/blob/main/hooks/pre-commit)
and [`hooks/pre-push`](https://github.com/rysweet/Simard/blob/main/hooks/pre-push).
The tables below summarize them.

### `pre-commit` stage (every `git commit`)

| Gate | Command | Purpose |
|---|---|---|
| Rust-only gate | `scripts/check-rust-only-gate.sh --staged` | Reject any new `.py` (anywhere) or `.js`/`.ts` outside the allow-list |
| `cargo fmt` | `cargo fmt --all -- --check` | Reject unformatted Rust code |
| release clippy | `cargo clippy --release --no-deps -- -D warnings` (via `scripts/clippy-precommit-release.sh`) | Fast incremental clippy on the workspace only |

`cargo fmt --check` typically completes in under 2 seconds; the release clippy
gate is fully incremental on a warm `target/`.

### `pre-push` stage (every `git push`)

| Gate | Command | Purpose |
|---|---|---|
| Rust-only gate | `scripts/check-rust-only-gate.sh` | Re-check the tracked tree at push time |
| `cargo fmt` | `cargo fmt --all -- --check` | Re-check fmt at push time (defense in depth) |
| race-subset tests | `cargo test --release --lib -- --test-threads=$(nproc) cognitive_memory bootstrap memory_ipc memory_consolidation` | Catch concurrency regressions in the modules where they actually live (issue #1631) |
| full clippy | `cargo clippy --all-targets --all-features --locked -- -D warnings` | The heavy pass CI runs, mirrored exactly |

Realistic budgets (warm caches, dev host with the workspace already built):

- `cargo fmt --check` — under 2 seconds.
- `cargo clippy --release --no-deps` (commit) — under 30 seconds.
- `cargo test --release --lib …` (push) — the race-catching subset, ≤ 90 seconds.

---

## The lbug link-path wrapper

The release-clippy gate runs through
[`scripts/clippy-precommit-release.sh`](https://github.com/rysweet/Simard/blob/main/scripts/clippy-precommit-release.sh),
which guarantees the `lbug` (LadybugDB) native static library is on the linker
search path before invoking `cargo clippy --release` (issue #2426). It is a
no-op for warm local checks, so the budgets above still hold. CI runs the same
wrapper and asserts on its success marker (the `#2426` regression guard in
`.github/workflows/verify.yml`).

---

## Manual Invocation

The hooks are plain scripts — run them directly, or run the individual gates:

```bash
# Run the full commit gate
hooks/pre-commit

# Run the full push gate
hooks/pre-push

# Or run a single gate directly:
cargo fmt --all -- --check
scripts/check-rust-only-gate.sh
scripts/clippy-precommit-release.sh
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --release --lib -- --test-threads="$(nproc)" cognitive_memory bootstrap memory_ipc memory_consolidation
```

---

## Bypass — Prohibited

Do **not** bypass the hooks.

> **`git commit --no-verify` / `git push --no-verify` are prohibited.** CI runs
> the same checks and merge is blocked on red CI. There is no `--admin`
> override (see
> [CONTRIBUTING.md → Merge Policy](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#merge-policy-no---admin-merges)).

---

## Verifying Hooks Catch What CI Catches

To confirm your local install actually blocks each failure class CI catches,
intentionally introduce each failure once and verify the matching gate fires.

### 1. Format failure (commit-time)

```bash
cat >> src/lib.rs <<'EOF'
fn   bad_fmt(  )  ->  i32{1}
EOF
git add src/lib.rs
git commit -m "test: fmt failure"
# Expected: cargo fmt gate fails, commit blocked.
git restore --staged src/lib.rs
git checkout -- src/lib.rs
```

### 2. Clippy failure (commit-time)

```bash
# Append a clippy violation in non-test code
cat >> src/lib.rs <<'EOF'
pub fn clippy_test() { let unused = 2; }
EOF
git add src/lib.rs
git commit -m "test: clippy failure"
# Expected: release clippy gate fails on commit, commit blocked.
git restore --staged src/lib.rs
git checkout -- src/lib.rs
```

### 3. Race-subset test failure (push-time)

Introduce a failing assertion in any cognitive_memory / bootstrap /
memory_ipc / memory_consolidation test, commit, then push. The race-subset
test gate should fail and block the push. Revert before continuing.

---

## Troubleshooting

### Hooks are not running

Confirm the setting is wired:

```bash
git config --local --get core.hooksPath        # -> hooks
```

If it prints nothing, run `./scripts/install-precommit.sh` (or
`git config --local core.hooksPath hooks`). Note that `core.hooksPath` makes
git run hooks **only** from `hooks/`, ignoring `.git/hooks/`.

### "cargo: command not found" inside a hook

The hooks invoke `cargo` from `PATH`. If you use `rustup` shims, ensure your
shell's `PATH` is exported in your shell rc file (not just the interactive
profile).

### Hook is slow

The race-subset test gate builds only the four target lib-tests in release
mode, but the first push from a cold workspace still pays the release-profile
compile cost. Subsequent pushes use the `target/` cache and are typically much
faster. Keep `cargo` build state warm with
`cargo test --release --lib --no-run` periodically.

### Skip a gate permanently for a single file

Don't. If you have a real reason a file should not be linted, raise it in the
PR; the rules are universal so we can keep CI honest.

---

## See Also

- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — full contributor workflow
- [`hooks/README.md`](https://github.com/rysweet/Simard/blob/main/hooks/README.md) — committed native hooks
- `scripts/install-precommit.sh` — hook installer (wires `core.hooksPath`)
