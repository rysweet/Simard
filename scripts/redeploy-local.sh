#!/usr/bin/env bash
# redeploy-local.sh — rebuild the simard binary, swap it into ~/.simard/bin/,
# restart the user systemd daemon, and tail the log until the next cycle starts.
#
# Use this after merging changes to main (or after explicitly opting into a
# branch build) to bring the running OODA daemon up to date.
#
# Flags:
#   -b, --branch <name>   Build from a specific git branch (default: main)
#   -n, --no-restart      Build + swap binary only; do not restart daemon
#   -h, --help            Show this help

set -euo pipefail

BRANCH="main"
RESTART=1
SIMARD_REPO="${SIMARD_REPO:-/home/azureuser/src/Simard}"
SHARED_TARGET="${SIMARD_SHARED_TARGET:-/home/azureuser/src/Simard/worktrees/meeting-ux-762/target}"
INSTALL_BIN="${HOME}/.simard/bin/simard"
LOG_TAIL_SECS=15

while [[ $# -gt 0 ]]; do
  case "$1" in
    -b|--branch) BRANCH="$2"; shift 2 ;;
    -n|--no-restart) RESTART=0; shift ;;
    -h|--help)
      sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

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

NEW_SHA=$(sha256sum "$NEW_BIN" | awk '{print $1}')
OLD_SHA=$(sha256sum "$INSTALL_BIN" 2>/dev/null | awk '{print $1}' || echo "<absent>")
if [[ "$NEW_SHA" == "$OLD_SHA" ]]; then
  echo "[redeploy] binary unchanged (sha=${NEW_SHA:0:12}); nothing to do"
  exit 0
fi
echo "[redeploy] new sha=${NEW_SHA:0:12} (was ${OLD_SHA:0:12})"

mkdir -p "$(dirname "$INSTALL_BIN")"
cp -f "$NEW_BIN" "${INSTALL_BIN}.new"
mv -f "${INSTALL_BIN}.new" "$INSTALL_BIN"
echo "[redeploy] installed → ${INSTALL_BIN}"

if [[ "$RESTART" -eq 0 ]]; then
  echo "[redeploy] --no-restart given; daemon NOT restarted"
  exit 0
fi

echo "[redeploy] restarting simard-ooda.service (user) ..."
systemctl --user restart simard-ooda

sleep 2
if systemctl --user is-active simard-ooda >/dev/null; then
  echo "[redeploy] daemon active; tailing daemon.log for ${LOG_TAIL_SECS}s ..."
  timeout "$LOG_TAIL_SECS" tail -f -n 5 "$HOME/.simard/daemon.log" || true
  echo "[redeploy] done"
else
  echo "[redeploy] FATAL: daemon failed to start" >&2
  systemctl --user status simard-ooda --no-pager | head -20 >&2
  exit 1
fi
