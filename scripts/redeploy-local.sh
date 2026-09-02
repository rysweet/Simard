#!/usr/bin/env bash
# redeploy-local.sh — legacy wrapper that builds Simard and delegates deployment
# to the canonical `simard install` transaction.
#
# Flags:
#   -b, --branch <name>       Build from a specific git branch (default: main)
#   --simard-home <path>      Pass an explicit install root to simard install
#   --systemd-user-dir <path> Pass an explicit user unit directory
#   --systemctl <path|name>   Pass an explicit systemctl executable
#   --dry-run                 Validate and print the installer plan only
#   -n, --no-restart          Deprecated alias for --dry-run
#   -h, --help                Show this help

set -euo pipefail

BRANCH="main"
SIMARD_REPO="${SIMARD_REPO:-/home/azureuser/src/Simard}"
SHARED_TARGET="${SIMARD_SHARED_TARGET:-${SIMARD_REPO}/target}"
DRY_RUN=0
INSTALL_ARGS=()
# Install root the parity gate checks against. Tracks --simard-home / SIMARD_HOME.
SIMARD_HOME_DIR="${SIMARD_HOME:-${HOME}/.simard}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -b|--branch) BRANCH="$2"; shift 2 ;;
    --simard-home) INSTALL_ARGS+=("--simard-home" "$2"); SIMARD_HOME_DIR="$2"; shift 2 ;;
    --systemd-user-dir) INSTALL_ARGS+=("--systemd-user-dir" "$2"); shift 2 ;;
    --systemctl) INSTALL_ARGS+=("--systemctl" "$2"); shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -n|--no-restart)
      echo "[redeploy] --no-restart is deprecated; using --dry-run because canonical installs restart services" >&2
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

if [[ "$DRY_RUN" -eq 1 ]]; then
  INSTALL_ARGS+=("--dry-run")
fi

echo "[redeploy] repo=${SIMARD_REPO} branch=${BRANCH} target=${SHARED_TARGET}"
cd "$SIMARD_REPO"

ORIG_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" != "$ORIG_BRANCH" ]]; then
  echo "[redeploy] checking out ${BRANCH} (was ${ORIG_BRANCH})"
  git checkout "$BRANCH"
fi

echo "[redeploy] building simard (release) ..."
CARGO_TARGET_DIR="$SHARED_TARGET" cargo build --release --bin simard

NEW_BIN="${SHARED_TARGET}/release/simard"
if [[ ! -x "$NEW_BIN" ]]; then
  echo "[redeploy] FATAL: built binary missing: $NEW_BIN" >&2
  exit 1
fi

echo "[redeploy] delegating install to ${NEW_BIN} install"
"$NEW_BIN" install "${INSTALL_ARGS[@]}"

# ---------------------------------------------------------------------------
# Post-deploy PATH-entrypoint parity gate (issue #4460).
#
# A deploy must NEVER leave a stale operator `simard` shadowing the freshly
# installed binary on PATH. Assert PATH identity first (the check that catches a
# same-version stale *file* a version-string comparison alone would miss), then
# version equality. Skipped under --dry-run because no live install happened.
# See docs/reference/simard-installer.md#redeploy-localsh-parity-gate.
# ---------------------------------------------------------------------------
if [[ "$DRY_RUN" -eq 0 ]]; then
  echo "[redeploy] verifying PATH-entrypoint parity ..."

  INSTALLED_BIN="${SIMARD_HOME_DIR}/bin/simard"
  if [[ ! -x "$INSTALLED_BIN" ]]; then
    echo "[simard] FATAL: installed binary missing after install: $INSTALLED_BIN" >&2
    exit 1
  fi

  # Resolve `simard` the way the systemd unit renders PATH: ~/.local/bin first,
  # then ~/.cargo/bin, then $SIMARD_HOME/bin. Drop any shell command-cache entry
  # so a cached path cannot mask a real skew.
  export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${SIMARD_HOME_DIR}/bin:${PATH}"
  hash -r 2>/dev/null || true

  PATH_BIN="$(command -v simard || true)"
  if [[ -z "$PATH_BIN" ]]; then
    echo "[simard] FATAL: no 'simard' resolved on PATH after install" >&2
    echo "[simard]   expected an owned entrypoint at ${HOME}/.local/bin/simard -> ${INSTALLED_BIN}" >&2
    exit 1
  fi

  PATH_CANON="$(readlink -f "$PATH_BIN")"
  INSTALLED_CANON="$(readlink -f "$INSTALLED_BIN")"
  if [[ "$PATH_CANON" != "$INSTALLED_CANON" ]]; then
    echo "[simard] FATAL: PATH-entrypoint parity check failed after install" >&2
    echo "[simard]   PATH-resolved: ${PATH_BIN} -> ${PATH_CANON}" >&2
    echo "[simard]   expected:      -> ${INSTALLED_CANON}" >&2
    echo "[simard]   the 'simard' on PATH is not the installed entrypoint (stale file or foreign shadow)" >&2
    exit 1
  fi

  INSTALLED_VERSION="$("$INSTALLED_BIN" --version)"
  PATH_VERSION="$("$PATH_BIN" --version)"
  if [[ "$INSTALLED_VERSION" != "$PATH_VERSION" ]]; then
    echo "[simard] FATAL: version parity check failed after install" >&2
    echo "[simard]   installed:     ${INSTALLED_VERSION}  (${INSTALLED_BIN})" >&2
    echo "[simard]   PATH-resolved: ${PATH_VERSION}  (${PATH_BIN})" >&2
    echo "[simard]   a stale 'simard' is still shadowing the freshly installed binary on PATH" >&2
    exit 1
  fi

  echo "[redeploy] PATH-resolved:  ${PATH_BIN} -> ${PATH_CANON}"
  echo "[redeploy] installed:      ${INSTALLED_VERSION}"
  echo "[redeploy] PATH-version:   ${PATH_VERSION}"
  echo "[redeploy] parity OK (path identity + version match)"
fi
