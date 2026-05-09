#!/usr/bin/env bash
#
# install-precommit.sh — Set up Simard's local pre-commit and pre-push hooks.
#
# Idempotent: safe to run repeatedly. Installs the `pre-commit` framework
# (>=3.7,<4) via pipx if available, otherwise via `pip install --user`, then
# runs `pre-commit install --install-hooks` so both `pre-commit` and `pre-push`
# git hook stages are wired up in one call (the project pins
# `default_install_hook_types: [pre-commit, pre-push]` in
# `.pre-commit-config.yaml`).
#
# Verification commands are printed at the end. See CONTRIBUTING.md
# "Local Pre-Commit Workflow" section for details.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [[ ! -f .pre-commit-config.yaml ]]; then
  echo "ERROR: .pre-commit-config.yaml not found in $REPO_ROOT" >&2
  exit 1
fi

# 1. Verify python3.
if ! command -v python3 >/dev/null 2>&1; then
  echo "ERROR: python3 is required to install pre-commit." >&2
  echo "       Install Python 3.9+ and rerun." >&2
  exit 1
fi

# 2. Install or upgrade pre-commit (>=3.7) using pipx if available.
PRE_COMMIT_VERSION_SPEC='pre-commit>=3.7'

if command -v pre-commit >/dev/null 2>&1; then
  echo "[install-precommit] pre-commit already on PATH: $(pre-commit --version)"
elif command -v pipx >/dev/null 2>&1; then
  echo "[install-precommit] Installing pre-commit via pipx..."
  pipx install "$PRE_COMMIT_VERSION_SPEC" || true
  # pipx may print a "already installed" warning and exit non-zero on
  # some versions; ensure the bin dir is on PATH for this shell.
  pipx_bin="$(pipx environment --value PIPX_BIN_DIR 2>/dev/null || echo "$HOME/.local/bin")"
  case ":$PATH:" in
    *":$pipx_bin:"*) ;;
    *) export PATH="$pipx_bin:$PATH" ;;
  esac
else
  echo "[install-precommit] pipx not found; installing pre-commit via pip --user..."
  python3 -m pip install --user --upgrade "$PRE_COMMIT_VERSION_SPEC"
  # Make sure ~/.local/bin is on PATH for this shell so `pre-commit` resolves.
  user_bin="$(python3 -m site --user-base)/bin"
  case ":$PATH:" in
    *":$user_bin:"*) ;;
    *) export PATH="$user_bin:$PATH" ;;
  esac
fi

if ! command -v pre-commit >/dev/null 2>&1; then
  echo "ERROR: pre-commit installed but not on PATH. Add ~/.local/bin to PATH and rerun." >&2
  exit 1
fi

# 3. Install both pre-commit and pre-push hooks. The config pins
#    default_install_hook_types so a single command wires up both stages.
echo "[install-precommit] Installing git hooks (pre-commit, pre-push)..."
pre-commit install --install-hooks

# 4. Print verification commands.
cat <<'EOF'

✓ pre-commit hooks installed.

Verification:
  # 1. Run all hooks against all files (recommended before opening a PR):
  pre-commit run --all-files

  # 2. Run a single hook:
  pre-commit run cargo-fmt --all-files
  pre-commit run cargo-clippy-precommit --all-files
  pre-commit run cargo-clippy --all-files --hook-stage pre-push
  pre-commit run cargo-test-race-subset --all-files --hook-stage pre-push

  # 3. Confirm hook scripts are present:
  ls -l .git/hooks/pre-commit .git/hooks/pre-push

See CONTRIBUTING.md "Local Pre-Commit Workflow" for the full reference.
EOF
