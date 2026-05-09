# Pre-Commit Setup

Step-by-step guide to installing and verifying the local pre-commit and
pre-push hooks that mirror Simard's CI `pre-commit` workflow.

> Quick reference: see the
> [Local Pre-Commit Workflow section in CONTRIBUTING.md](../../CONTRIBUTING.md#local-pre-commit-workflow).

---

## Prerequisites

- Linux or macOS development host (Windows via WSL2 is supported but
  untested by CI).
- `python3` (≥ 3.9) with `pip` or `pipx`.
- A working Rust toolchain (`rustc`, `cargo`, `rustfmt`, `clippy`)
  matching the version CI uses. The repo does **not** currently pin a
  toolchain via `rust-toolchain.toml`; install whatever stable Rust
  CI is currently building against (see
  [`.github/workflows/`](../../.github/workflows/)).
- Repo cloned and you are at the repo root.

---

## Install

```bash
./scripts/install-precommit.sh
```

The script (added under issue #1631 — if your branch does not yet
contain it, use the [Manual install](#manual-install) procedure below)
performs the following:

1. Verifies Python and Rust prerequisites.
2. Installs the `pre-commit` framework (`>=3.7,<4`):
   - `pipx install pre-commit` if `pipx` is on `PATH`, otherwise
   - `pip install --user pre-commit`.
3. Runs `pre-commit install --install-hooks`. The project pins
   `default_install_hook_types: [pre-commit, pre-push]` in
   [`.pre-commit-config.yaml`](../../.pre-commit-config.yaml), so a
   single `--install-hooks` invocation creates both
   `.git/hooks/pre-commit` and `.git/hooks/pre-push`.
4. Runs `pre-commit run --all-files` to populate caches and surface any
   immediate violations.

Re-running the script is safe; it short-circuits if hooks are already
installed.

### Manual install

```bash
pipx install 'pre-commit>=3.7,<4'      # or: pip install --user 'pre-commit>=3.7,<4'
pre-commit install --install-hooks
pre-commit run --all-files
```

---

## What the Hooks Check

The actual configuration lives in
[`.pre-commit-config.yaml`](../../.pre-commit-config.yaml); the table
below summarizes it.

### `pre-commit` stage (every `git commit`)

| Hook id | Command | Purpose |
|---|---|---|
| `cargo-fmt` | `cargo fmt --all -- --check` | Reject unformatted Rust code |
| `cargo-clippy-precommit` | `cargo clippy --release --no-deps -- -D warnings` | Fast incremental clippy on the workspace only |

Both hooks run with `always_run: true` so unstaged drift (e.g.,
introduced by a rebase) still fails the commit. `cargo fmt --check`
typically completes in under 2 seconds; `cargo clippy --release
--no-deps` is typically under 30 seconds incrementally.

### `pre-push` stage (every `git push`)

| Hook id | Command | Purpose |
|---|---|---|
| `cargo-fmt` | `cargo fmt --all -- --check` | Re-check fmt at push time (defense in depth) |
| `cargo-test-race-subset` | `cargo test --release --lib -- --test-threads=$(nproc) cognitive_memory bootstrap memory_ipc memory_consolidation` | Catch concurrency regressions in the modules where they actually live (issue #1631) |

The push-time gate is intentionally narrow on tests — full-suite
gating (and the deeper `--all-targets --all-features --locked` clippy
pass) belongs in CI, not in the local pre-push hook. Local pre-push
exists to catch the multi-thread race classes (writer-Arc lifecycle,
IPC bridge teardown, consolidation order-of-operations) **before**
they leave a developer machine, while staying inside a ≤ 90 second
budget on a dev host.

Realistic budgets (warm caches, dev host with the workspace already
built):

- `cargo fmt --check` — under 2 seconds.
- `cargo clippy --release --no-deps` (commit) — under 30 seconds.
- `cargo test --release --lib …` (push) — ≤ 90 seconds on a dev host.

---

## Configuration File

`.pre-commit-config.yaml` lives at the repo root. Consult that file
directly for the source of truth; the three hooks summarized above
(`cargo-fmt`, `cargo-clippy-precommit`, `cargo-test-race-subset`) are
wired with the stage tagging shown in the tables above. All hooks use
`language: system` (they shell out to the locally installed `cargo`),
so there are no upstream hook revisions to bump.

---

## Manual Invocation

```bash
# Run all hooks on all files (recommended before opening a PR)
pre-commit run --all-files

# Run a specific hook on all files
pre-commit run cargo-fmt              --all-files
pre-commit run cargo-clippy-precommit --all-files
pre-commit run cargo-test-race-subset --all-files --hook-stage pre-push

# Run only on staged files (default behavior at commit time)
pre-commit run

# Run all hooks at the manual stage
pre-commit run --hook-stage manual --all-files
```

---

## Bypass (Emergency Only)

```bash
# Skip a single hook (use only when actively debugging the hook
# itself, not the code under change)
SKIP=cargo-test-race-subset git push
SKIP=cargo-clippy-precommit,cargo-fmt git commit -m "WIP"
```

> **PRs pushed with `SKIP=` will be rejected at merge time.** CI runs
> the same checks and merge is blocked on red CI. There is no
> `--admin` override (see
> [CONTRIBUTING.md → Merge Policy](../../CONTRIBUTING.md#merge-policy-no---admin-merges)).

---

## Verifying Hooks Catch What CI Catches

To confirm your local install actually blocks each failure class CI
catches, intentionally introduce each failure once and verify the
matching hook fires.

### 1. Format failure (commit-time)

```bash
cat >> src/lib.rs <<'EOF'
fn   bad_fmt(  )  ->  i32{1}
EOF
git add src/lib.rs
git commit -m "test: fmt failure"
# Expected: cargo-fmt hook fails, commit blocked.
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
# Expected: cargo-clippy-precommit hook fails on commit, commit blocked.
git restore --staged src/lib.rs
git checkout -- src/lib.rs
```

### 3. Race-subset test failure (push-time)

Introduce a failing assertion in any cognitive_memory / bootstrap /
memory_ipc / memory_consolidation test, commit, then push. The
`cargo-test-race-subset` hook should fail and block the push. Revert
before continuing.

---

## Troubleshooting

### "pre-commit: command not found" after install

Ensure your install location is on `PATH`:

```bash
# If installed via pipx
pipx ensurepath
# If installed via pip --user
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### Hook is slow

```bash
# Show per-hook timing
pre-commit run --all-files --verbose
```

The race-subset test hook builds only the four target lib-tests in
release mode, but the first push from a cold workspace still pays the
release-profile compile cost. Subsequent pushes use the `target/`
cache and are typically much faster. If repeated push attempts exceed
your patience, keep `cargo` build state warm with
`cargo test --release --lib --no-run` periodically.

### "cargo: command not found" inside hook

The hooks invoke `cargo` from `PATH`. If you use `rustup` shims, ensure
your shell's `PATH` is exported in your shell rc file (not just the
interactive profile).

### Skip a hook permanently for a single file

Don't. If you have a real reason a file should not be linted, raise it
in the PR; the rules are universal so we can keep CI honest.

---

## Updating Pre-Commit Itself

```bash
pipx upgrade pre-commit                  # if installed via pipx
pip install --user --upgrade pre-commit  # if installed via pip
pre-commit install --install-hooks       # re-install hooks if framework upgraded
```

---

## See Also

- [`CONTRIBUTING.md`](../../CONTRIBUTING.md) — full contributor workflow
- [`.pre-commit-config.yaml`](../../.pre-commit-config.yaml) — hook
  configuration source of truth
- `scripts/install-precommit.sh` — installer (added under issue #1631)
