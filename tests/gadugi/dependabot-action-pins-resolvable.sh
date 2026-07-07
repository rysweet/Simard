#!/usr/bin/env bash
# qa-team scenario — taiki-e/install-action pins stay Dependabot-resolvable.
#
# Outside-in verification of the CI-health fix for the failing default-branch
# `dependabot/dependabot-updates` (github_actions) run on rysweet/Simard:
#
#   Error processing taiki-e/install-action (HelperSubprocessFailed)
#   error: no such commit 754bf4dbae00ad1b16b244717154b96ba27d2416
#
# The action was SHA-pinned to its per-tool tags (`# cargo-audit`, ...), whose
# commits live on a lineage diverged from the action's default branch, so
# Dependabot's shallow clone could not resolve them and the whole update job
# failed. The fix pins the reachable `v2` release SHA and selects each tool via
# the `with: tool:` input — SHA-hardened AND Dependabot-updatable.
#
# This script asserts the operator-visible contract with file-shaped checks
# across ALL workflow files (no network, deterministic) plus the Rust guard.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Every workflow file (so a newly-added workflow with a bad pin is caught too).
mapfile -t WORKFLOWS < <(find .github/workflows -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)
[ "${#WORKFLOWS[@]}" -ge 1 ] || fail "no workflow files found under .github/workflows"

# 1) The previously-dead per-tool commit must be gone everywhere.
if grep -rn "754bf4dbae00ad1b16b244717154b96ba27d2416" .github/ >/dev/null 2>&1; then
  fail "the dead cargo-audit per-tool SHA 754bf4db... is still pinned (breaks Dependabot)"
fi
echo "OK: dead per-tool SHA 754bf4db... is absent"

# 2) Every taiki-e/install-action `uses:` pin must be a 40-hex SHA carrying a
#    version-tag comment (# v...), never a per-tool tag name.
found=0
while IFS= read -r line; do
  # Only real `uses:` lines (skip prose/comment lines).
  case "$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//')" in
    \#*) continue ;;
  esac
  printf '%s' "$line" | grep -q "uses:" || continue

  found=$((found + 1))
  after="${line#*taiki-e/install-action@}"
  sha="$(printf '%s' "$after" | sed -e 's/[[:space:]].*$//')"
  comment="$(printf '%s' "$after" | sed -n 's/.*#[[:space:]]*//p')"

  printf '%s' "$sha" | grep -Eq '^[0-9a-f]{40}$' \
    || fail "taiki-e/install-action is not SHA-pinned (got '$sha')"
  printf '%s' "$comment" | grep -Eq '^v[0-9]' \
    || fail "taiki-e/install-action@$sha has non-version comment '# ${comment:-<none>}' (per-tool tags break Dependabot; pin the v2 release SHA + 'with: tool:')"
  echo "OK: taiki-e/install-action@${sha:0:12} # $comment"
done < <(grep -h "taiki-e/install-action@" "${WORKFLOWS[@]}")

[ "$found" -ge 5 ] || fail "expected >=5 taiki-e/install-action pins (audit/deny/vet/llvm-cov/cyclonedx), found $found"
echo "OK: all $found taiki-e/install-action pins are SHA+version-tag pinned"

# 3) The Rust regression guard encodes the same invariant — run it.
OUTPUT="$(cargo test --test dependabot_action_pins 2>&1)"
printf '%s\n' "$OUTPUT"
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null \
  || fail "dependabot_action_pins regression guard did not pass"
echo "OK: dependabot_action_pins regression guard passed"

echo "PASS: taiki-e/install-action pins are Dependabot-resolvable"
