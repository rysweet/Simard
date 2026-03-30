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
NON_REPO=""

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
printf '%s\n' "$OUTPUT" | grep -F "Carried meeting decisions: 0" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected action: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Action plan: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Verification steps: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Action status: success" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Changed files after action: <none>" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Verification status: verified" >/dev/null
! printf '%s\n' "$OUTPUT" | grep -F "Azlin" >/dev/null

EDIT_REPO="$(mktemp -d /tmp/simard-engineer-loop-edit-fixture.XXXXXX)"
EDIT_STATE_ROOT="$(mktemp -d /tmp/simard-engineer-loop-edit-state.XXXXXX)"
trap 'rm -rf "${NON_REPO:-}" "$EDIT_REPO" "$EDIT_STATE_ROOT"' EXIT

cd "$EDIT_REPO"
git init -b main >/dev/null
git config user.name "Simard Test"
git config user.email "simard-tests@example.com"
cat > README.md <<'EOF'
# Demo Repo

Current status: TODO
EOF
git add README.md
git commit -m "initial fixture" >/dev/null
cd "$ROOT"

EDIT_OBJECTIVE="$(cat <<'EOF'
edit-file: README.md
replace: Current status: TODO
with: Current status: DONE
verify-contains: Current status: DONE
EOF
)"

EDIT_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    engineer-loop-run single-process "$EDIT_REPO" "$EDIT_OBJECTIVE" "$EDIT_STATE_ROOT"
)"

printf '%s\n' "$EDIT_OUTPUT"

printf '%s\n' "$EDIT_OUTPUT" | grep -F "Selected action: structured-text-replace" >/dev/null
printf '%s\n' "$EDIT_OUTPUT" | grep -F "Changed files after action: README.md" >/dev/null
printf '%s\n' "$EDIT_OUTPUT" | grep -F "Verification status: verified" >/dev/null
grep -F "Current status: DONE" "$EDIT_REPO/README.md" >/dev/null

NON_REPO="$(mktemp -d /tmp/simard-engineer-loop-not-a-repo.XXXXXX)"

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
