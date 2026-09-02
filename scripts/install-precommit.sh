#!/usr/bin/env bash
#
# install-precommit.sh — Enroll Simard's native (Python-free) git hooks.
#
# Idempotent: safe to run repeatedly. Points `core.hooksPath` at the committed
# `hooks/` directory so `git commit` and `git push` run the same fmt / clippy /
# test / Rust-only fences that CI runs — with NO Python `pre-commit` framework,
# `pip`, or `python3` dependency (issue #3181). The hooks shell out to `cargo`
# directly.
#
# See CONTRIBUTING.md "Local Commit Gates" and
# docs/operations/pre-commit-setup.md for details.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

HOOKS_DIR="hooks"

# 1. Verify the committed hooks exist and are executable.
missing=0
for hook in "$HOOKS_DIR/pre-commit" "$HOOKS_DIR/pre-push"; do
  if [[ ! -f "$hook" ]]; then
    echo "[simard] ERROR: expected committed hook $hook is missing" >&2
    missing=1
  elif [[ ! -x "$hook" ]]; then
    echo "[simard] making $hook executable" >&2
    chmod +x "$hook"
  fi
done
if [[ "$missing" -ne 0 ]]; then
  echo "[simard] ERROR: native git hooks are not committed under $HOOKS_DIR/" >&2
  exit 1
fi

# 2. Wire git to the committed hooks. A single setting installs both the
#    pre-commit and pre-push stages (git dispatches by hook filename).
echo "[simard] wiring git core.hooksPath -> $HOOKS_DIR" >&2
git config core.hooksPath "$HOOKS_DIR"

# 3. Print verification commands.
cat <<'EOF'

[simard] ✓ native git hooks enrolled (Python-free).

Verification:
  # 1. Confirm the hooks path is wired:
  git config --get core.hooksPath        # -> hooks

  # 2. Run the commit-stage gate by hand:
  hooks/pre-commit

  # 3. Run the push-stage gate by hand:
  hooks/pre-push

  # 4. Or run the individual cargo gates directly:
  cargo fmt --all -- --check
  scripts/clippy-precommit-release.sh
  cargo clippy --all-targets --all-features --locked -- -D warnings

See CONTRIBUTING.md "Local Commit Gates" for the full reference.
EOF
