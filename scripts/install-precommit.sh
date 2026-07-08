#!/usr/bin/env bash
#
# install-precommit.sh — wire Simard's committed native git hooks.
#
# Simard is a pure-Rust, Python-free daemon (issue #3181). Local commit/push
# gating is provided by committed scripts under `hooks/` (hooks/pre-commit and
# hooks/pre-push), run by git via `core.hooksPath`. There is no Python
# `pre-commit` framework, no `pip`, and no `pipx`.
#
# Idempotent: safe to run repeatedly. See CONTRIBUTING.md "Local Pre-Commit
# Workflow" for details.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [[ ! -f hooks/pre-commit ]]; then
  echo "ERROR: hooks/pre-commit not found in $REPO_ROOT" >&2
  echo "       This repo must ship committed native hooks under hooks/." >&2
  exit 1
fi

# Point git at the committed hooks/ dir. This wires BOTH stages
# (hooks/pre-commit and hooks/pre-push) in one setting.
echo "[install-precommit] wiring core.hooksPath -> hooks"
git config --local core.hooksPath hooks

# Ensure the committed hooks are executable in this checkout.
chmod +x hooks/pre-commit hooks/pre-push 2>/dev/null || true

cat <<'EOF'

✓ Native git hooks wired (core.hooksPath=hooks). No Python required.

Verification:
  # 1. Confirm the setting:
  git config --local --get core.hooksPath        # -> hooks

  # 2. Run the full commit gate (rust-only + fmt + release clippy):
  hooks/pre-commit

  # 3. Run the full push gate (rust-only + fmt + race tests + full clippy):
  hooks/pre-push

  # 4. Or run a single gate directly:
  cargo fmt --all -- --check
  scripts/clippy-precommit-release.sh
  cargo clippy --all-targets --all-features --locked -- -D warnings

See CONTRIBUTING.md "Local Pre-Commit Workflow" for the full reference.
EOF
