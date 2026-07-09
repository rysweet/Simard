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

while [[ $# -gt 0 ]]; do
  case "$1" in
    -b|--branch) BRANCH="$2"; shift 2 ;;
    --simard-home) INSTALL_ARGS+=("--simard-home" "$2"); shift 2 ;;
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
