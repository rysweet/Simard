#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [ -n "$(git status --short --untracked-files=all)" ]; then
  EXPECTED_DIRTY=true
else
  EXPECTED_DIRTY=false
fi

OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    engineer-loop-run single-process "$ROOT" "$OBJECTIVE"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: engineer-loop-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Repo root: $ROOT" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Repo branch: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Worktree dirty: $EXPECTED_DIRTY" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Execution scope: local-only" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected action: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Action status: success" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Verification status: verified" >/dev/null
! printf '%s\n' "$OUTPUT" | grep -F "Azlin" >/dev/null

NON_REPO="$(mktemp -d /tmp/simard-engineer-loop-not-a-repo.XXXXXX)"
trap 'rm -rf "$NON_REPO"' EXIT

set +e
BAD_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    engineer-loop-run single-process "$NON_REPO" "$OBJECTIVE" 2>&1
)"
BAD_STATUS=$?
set -e

[ "$BAD_STATUS" -ne 0 ]
printf '%s\n' "$BAD_OUTPUT" | grep -F "NOT_A_REPO" >/dev/null
printf '%s\n' "$BAD_OUTPUT" | grep -F "$NON_REPO" >/dev/null
