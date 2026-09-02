# Local Commit Gates (Native Git Hooks)

Step-by-step guide to enrolling and verifying the local **commit** and
**push** gates that mirror Simard's CI `verify` workflow.

Simard has **no Python runtime requirement**. The local gates are plain
committed shell scripts wired through Git's native
[`core.hooksPath`](https://git-scm.com/docs/githooks) mechanism — there is
**no `pre-commit` framework, no `pip install`, and no `python3` dependency**.
Every gate shells out to the locally installed `cargo`.

> Quick reference: see the
> [Local Commit Gates section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#local-commit-gates).

---

## How It Works

The repository commits its own Git hooks under `hooks/` and points Git at
that directory with `core.hooksPath`. Two thin dispatcher hooks delegate to
shared gate scripts under `scripts/gates/`, and CI runs **the exact same
`cargo` commands** directly. There is a single source of truth for every
check, and it is Rust/`cargo` — never Python.

```text
hooks/pre-commit  ──▶ scripts/gates/pre-commit.sh ─┐
hooks/pre-push    ──▶ scripts/gates/pre-push.sh   ─┤──▶ cargo fmt / clippy / test
.github/workflows/verify.yml  ────────────────────┘     (same commands, same flags)
```

- **`hooks/pre-commit`, `hooks/pre-push`** — committed, executable
  (`100755`) dispatchers. They are tiny, reviewed, and hermetic: no network,
  no `curl`/`wget`, no auto-update, no telemetry.
- **`scripts/gates/pre-commit.sh`, `scripts/gates/pre-push.sh`** — the shared
  gate bodies. Each uses `#!/usr/bin/env bash`, `set -euo pipefail`, and a
  strict `IFS`; all path expansions are quoted and `--` precedes any file
  list so a filename beginning with `-` cannot inject options.
- **`core.hooksPath = hooks`** — enrollment is a single `git config` write.
  Git resolves the relative path per worktree, so linked worktrees pick up
  the committed hooks automatically.

Because the hooks are committed to the repo, every checkout already has them;
enrollment only tells *your* clone to use them.

---

## Prerequisites

- Linux or macOS development host (Windows via WSL2 is supported but
  untested by CI).
- A POSIX `bash` and `git` (both already required to build Simard). **No
  Python.**
- A working Rust toolchain (`rustc`, `cargo`, `rustfmt`, `clippy`)
  matching the version CI uses. The repo does **not** currently pin a
  toolchain via `rust-toolchain.toml`; install whatever stable Rust CI is
  currently building against (see
  [`.github/workflows/`](https://github.com/rysweet/Simard/tree/main/.github/workflows)).
- Repo cloned and you are at the repo root.

---

## Enroll (One-Time)

```bash
git config core.hooksPath hooks
```

That is the entire mechanism. To confirm:

```bash
git config --get core.hooksPath          # → hooks
git ls-files -s hooks/                    # both entries show mode 100755
```

A convenience wrapper is provided that does exactly this and verifies the
hooks are executable:

```bash
./scripts/install-precommit.sh
```

The wrapper is **Python-free**: it runs `git config core.hooksPath hooks`,
checks that `hooks/pre-commit` and `hooks/pre-push` exist and are
executable, and prints the verification commands below. It is idempotent —
re-running it is a no-op once you are enrolled.

> **Fresh clones and CI do not auto-enroll.** CI never relies on
> `core.hooksPath`; it runs the gate commands directly in
> [`verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml).
> Local enrollment is a fast mirror for developers, not the security
> boundary — see [Relationship to CI](#relationship-to-ci).

### Engineer worktrees

When the OODA daemon allocates a per-engineer git worktree it enrolls the
hooks automatically via `engineer_worktree::install_hooks`, which runs
`git config core.hooksPath hooks` in the new worktree (no subprocess spawn,
no Python). If `hooks/` is missing or not executable, it emits a loud but
non-fatal `[simard]` diagnostic and worktree allocation still succeeds — the
gates are a productivity mirror, and CI remains the authoritative gate.

---

## What the Gates Check

The gate bodies live in
[`scripts/gates/pre-commit.sh`](https://github.com/rysweet/Simard/blob/main/scripts/gates/pre-commit.sh)
and
[`scripts/gates/pre-push.sh`](https://github.com/rysweet/Simard/blob/main/scripts/gates/pre-push.sh);
the tables below summarize them. CI runs the **same** commands with the
**same** flags.

### `pre-commit` stage (every `git commit`)

| Gate | Command | Purpose |
|---|---|---|
| Rust-only gate | `scripts/check-rust-only-gate.sh --staged` | Reject any new `.py` (anywhere) and `.js`/`.ts` outside the allow-list |
| Format | `cargo fmt --all -- --check` | Reject unformatted Rust code |
| Clippy (fast) | `scripts/clippy-precommit-release.sh` → `cargo clippy --release --no-deps -- -D warnings` | Fast incremental clippy on the workspace only |

`cargo fmt --check` typically completes in under 2 seconds; the fast clippy
pass is typically under 30 seconds incrementally. The clippy wrapper
guarantees the `lbug` (LadybugDB) native static library is on the linker
search path before invoking `cargo clippy --release` (issue #2426), so the
gate never reds on a cold registry cache.

### `pre-push` stage (every `git push`)

| Gate | Command | Purpose |
|---|---|---|
| Format | `cargo fmt --all -- --check` | Re-check fmt at push time (defense in depth) |
| Race-subset tests | `cargo test --release --lib -- --test-threads=$(nproc) cognitive_memory bootstrap memory_ipc memory_consolidation` | Catch concurrency regressions where they live (issue #1631) |
| Clippy (full) | `cargo clippy --all-targets --all-features --locked -- -D warnings` | Full-surface clippy, mirroring CI exactly |

The push-time test gate is intentionally narrow — full-suite gating belongs
in CI, where the runner has more cores and isolated caches. Local pre-push
exists to catch the multi-thread race classes (writer-`Arc` lifecycle, IPC
client teardown, consolidation order-of-operations) **before** they leave a
developer machine, while staying inside a ≤ 90 second budget on a dev host.

Realistic budgets (warm caches, dev host with the workspace already built):

- `cargo fmt --check` — under 2 seconds.
- `cargo clippy --release --no-deps` (commit) — under 30 seconds.
- `cargo test --release --lib …` (push) — ≤ 90 seconds on a dev host.
- `cargo clippy --all-targets --all-features --locked` (push) — reuses the
  warm `target/` after the race-test compile; the incremental delta is small.

---

## Gate Scripts (Source of Truth)

`hooks/pre-commit` and `hooks/pre-push` are two-line dispatchers; the real
logic is in `scripts/gates/*.sh`. Consult those files directly — the tables
above are a summary, not the source of truth. Every script:

- starts with `#!/usr/bin/env bash` and `set -euo pipefail` + strict `IFS`;
- quotes all path expansions and passes `--` before any file list;
- performs **no** `eval`, `sh -c`, or command substitution on git-controlled
  paths;
- is hermetic — it never touches the network, so the local gate can never
  auto-update itself out from under you.

Because the hooks shell out to the locally installed `cargo`, there are no
upstream hook revisions to bump and nothing to `pip install`.

---

## Manual Invocation

You can run any gate on demand without committing:

```bash
# Run the full commit-stage gate against the working tree
./scripts/gates/pre-commit.sh

# Run the full push-stage gate
./scripts/gates/pre-push.sh

# Or run the underlying commands directly (identical to CI)
cargo fmt --all -- --check
scripts/clippy-precommit-release.sh
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --release --lib -- --test-threads="$(nproc)" \
  cognitive_memory bootstrap memory_ipc memory_consolidation
scripts/check-rust-only-gate.sh          # full-tree Rust-only scan
```

---

## Bypass (Emergency Only)

Native git hooks are bypassed with Git's built-in `--no-verify`:

```bash
git commit --no-verify -m "WIP"   # DISCOURAGED
git push   --no-verify            # DISCOURAGED
```

> **`--no-verify` is forbidden for anything you intend to merge.** CI re-runs
> the identical gate commands, and merge is blocked on red CI. There is no
> `--admin` override (see
> [CONTRIBUTING.md → Merge Policy](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#merge-policy-no---admin-merges)).
> Because the gates are shared scripts invoked identically locally and in CI,
> skipping them locally only defers — never avoids — the failure.

There is no per-hook `SKIP=` variable (that was a `pre-commit`-framework
feature). If you need to iterate on the gate scripts themselves, run them
directly (see [Manual Invocation](#manual-invocation)) rather than bypassing.

---

## Verifying Gates Catch What CI Catches

To confirm your local enrollment actually blocks each failure class CI
catches, intentionally introduce each failure once and verify the matching
gate fires.

### 1. Rust-only violation (commit-time)

```bash
touch tests/should_not_exist.py
git add tests/should_not_exist.py
git commit -m "test: rust-only gate"
# Expected: rust-only gate fails, commit blocked (a .py ANYWHERE is rejected).
git restore --staged tests/should_not_exist.py && rm tests/should_not_exist.py
```

### 2. Format failure (commit-time)

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

### 3. Clippy failure (commit-time)

```bash
cat >> src/lib.rs <<'EOF'
pub fn clippy_test() { let unused = 2; }
EOF
git add src/lib.rs
git commit -m "test: clippy failure"
# Expected: fast clippy gate fails on commit, commit blocked.
git restore --staged src/lib.rs
git checkout -- src/lib.rs
```

### 4. Race-subset test failure (push-time)

Introduce a failing assertion in any cognitive_memory / bootstrap /
memory_ipc / memory_consolidation test, commit, then push. The push-stage
gate should fail and block the push. Revert before continuing.

---

## Relationship to CI

Local hooks are a **fast mirror**, not the trust boundary. The authoritative
enforcement is server-side in
[`.github/workflows/verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml),
which runs the identical `cargo` commands directly — no `actions/setup-python`,
no `pip`, no `pre_commit`:

| Check | Local gate | CI job/step |
|---|---|---|
| Rust-only (no new `.py` anywhere) | `hooks/pre-commit` | `verify` → *Rust-only gate* step |
| `cargo fmt --check` | `hooks/pre-commit`, `hooks/pre-push` | `verify` → *fmt* step |
| `cargo clippy --release --no-deps` | `hooks/pre-commit` | `verify` → *clippy (precommit-release)* step |
| `cargo clippy --all-targets --all-features --locked` | `hooks/pre-push` | `verify` → *clippy (full)* step |
| `cargo test` (race subset local / full in CI) | `hooks/pre-push` | `verify` → *cargo test* step |
| `cargo deny check` (licenses, bans, sources) | — (CI-only) | `cargo-deny` job |
| `cargo audit` (offline, pinned DB) | — (CI-only) | `cargo-audit` job |
| `cargo vet` | — (CI-only) | `cargo-vet` job |
| `npm audit` (dev-dep surface) | — (CI-only) | `npm-audit` job |

The Rust-only gate is enforced in CI (not only in the `--no-verify`-bypassable
local hook) and fails on a tracked or staged `.py` file **anywhere** in the
tree — the hardening that keeps Python from silently returning.

> **Note on secret scanning.** Simard does not currently ship a dedicated
> secret-scanning gate (e.g. gitleaks/trufflehog) in CI or the hooks. This is
> stated here truthfully rather than implied; the local hooks are hermetic and
> carry no secrets. Adding a pinned gitleaks CI job is tracked as optional
> hardening.

---

## Troubleshooting

### Hooks don't run on commit/push

Confirm enrollment and executability:

```bash
git config --get core.hooksPath        # must print: hooks
git ls-files -s hooks/                 # both entries must be mode 100755
```

If `core.hooksPath` is empty, run `git config core.hooksPath hooks` (or
`./scripts/install-precommit.sh`). If the mode is `100644`, the exec bit was
lost on checkout; restore it with `chmod +x hooks/pre-commit hooks/pre-push`
and report it — the committed entries are stored as `100755`.

### Gate is slow

The race-subset test gate builds only the four target lib-tests in release
mode, but the first push from a cold workspace still pays the release-profile
compile cost. Subsequent pushes reuse the `target/` cache. Keep build state
warm with `cargo test --release --lib --no-run` periodically.

### "cargo: command not found" inside a gate

The gates invoke `cargo` from `PATH`. If you use `rustup` shims, ensure your
shell's `PATH` is exported in your shell rc file (not just the interactive
profile). The hooks intentionally do **not** modify your environment.

### Skip a gate permanently for a single file

Don't. The rules are universal so CI stays honest. If a file genuinely must be
exempt, raise it in the PR and (for `.js`/`.ts`) add it to the allow-list in
`scripts/check-rust-only-gate.sh` with a justifying comment.

---

## See Also

- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — full contributor workflow
- [`hooks/pre-commit`](https://github.com/rysweet/Simard/blob/main/hooks/pre-commit) and
  [`hooks/pre-push`](https://github.com/rysweet/Simard/blob/main/hooks/pre-push) — committed dispatcher hooks
- [`scripts/gates/pre-commit.sh`](https://github.com/rysweet/Simard/blob/main/scripts/gates/pre-commit.sh) and
  [`scripts/gates/pre-push.sh`](https://github.com/rysweet/Simard/blob/main/scripts/gates/pre-push.sh) — gate bodies (source of truth)
- [`scripts/check-rust-only-gate.sh`](https://github.com/rysweet/Simard/blob/main/scripts/check-rust-only-gate.sh) — Rust-only enforcement
- `scripts/install-precommit.sh` — Python-free enrollment convenience wrapper
