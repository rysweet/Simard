#!/usr/bin/env bash
# qa-team scenario — taiki-e/install-action AND dtolnay/rust-toolchain pins
# stay Dependabot-resolvable.
#
# Outside-in verification of the CI-health fix for the failing default-branch
# `dependabot/dependabot-updates` (github_actions) run on rysweet/Simard, which
# failed to update two actions with the same root cause:
#
#   Error processing taiki-e/install-action (HelperSubprocessFailed)
#   error: no such commit 754bf4dbae00ad1b16b244717154b96ba27d2416
#   ... plus dtolnay/rust-toolchain (unknown_error)
#
# taiki-e/install-action was SHA-pinned to its per-tool tags (`# cargo-audit`,
# ...) and dtolnay/rust-toolchain to its per-channel branch HEADs (`# stable`,
# `# nightly`) — commits that live on lineages *diverged* from each action's
# default branch, so Dependabot's shallow clone could not resolve them and the
# whole update job failed. The fix pins each action's reachable release SHA
# (`taiki-e`'s `v2`, `dtolnay`'s `v1`) and selects the tool / toolchain channel
# via the `with:` input — SHA-hardened AND Dependabot-updatable.
#
# This script asserts the operator-visible contract with file-shaped checks
# across ALL workflow files and in-repo composite `action.yml` definitions (no
# network, deterministic) plus the Rust guard.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Every workflow file plus in-repo composite action.yml (so a newly-added file
# with a bad pin is caught too). Dependabot reads composite actions as well.
mapfile -t FILES < <(
  {
    find .github/workflows -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \)
    find .github/actions -type f \( -name 'action.yml' -o -name 'action.yaml' \) 2>/dev/null
  } | sort
)
[ "${#FILES[@]}" -ge 1 ] || fail "no workflow/action files found under .github"

# 1) The previously-dead diverged commits must be gone everywhere.
for dead in \
  754bf4dbae00ad1b16b244717154b96ba27d2416 \
  29eef336d9b2848a0b548edc03f92a220660cdb8 \
  5b842231ba77f5c045dba54ac5560fed2db780e2 \
; do
  if grep -rn "$dead" .github/ >/dev/null 2>&1; then
    fail "dead diverged-branch SHA ${dead:0:12}... is still pinned (breaks Dependabot)"
  fi
  echo "OK: dead diverged SHA ${dead:0:12}... is absent"
done

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
done < <(grep -h "taiki-e/install-action@" "${FILES[@]}")

[ "$found" -ge 5 ] || fail "expected >=5 taiki-e/install-action pins (audit/deny/vet/llvm-cov/cyclonedx), found $found"
echo "OK: all $found taiki-e/install-action pins are SHA+version-tag pinned"

# 3) Every dtolnay/rust-toolchain `uses:` pin must be a 40-hex SHA carrying a
#    version-tag comment (# v...), never a per-channel branch name (stable/nightly).
found_rt=0
while IFS= read -r line; do
  case "$(printf '%s' "$line" | sed -e 's/^[[:space:]]*//')" in
    \#*) continue ;;
  esac
  printf '%s' "$line" | grep -q "uses:" || continue

  found_rt=$((found_rt + 1))
  after="${line#*dtolnay/rust-toolchain@}"
  sha="$(printf '%s' "$after" | sed -e 's/[[:space:]].*$//')"
  comment="$(printf '%s' "$after" | sed -n 's/.*#[[:space:]]*//p')"

  printf '%s' "$sha" | grep -Eq '^[0-9a-f]{40}$' \
    || fail "dtolnay/rust-toolchain is not SHA-pinned (got '$sha')"
  printf '%s' "$comment" | grep -Eq '^v[0-9]' \
    || fail "dtolnay/rust-toolchain@$sha has non-version comment '# ${comment:-<none>}' (per-channel branch HEADs break Dependabot; pin the v1 release SHA + 'with: toolchain:')"
  echo "OK: dtolnay/rust-toolchain@${sha:0:12} # $comment"
done < <(grep -h "dtolnay/rust-toolchain@" "${FILES[@]}")

[ "$found_rt" -ge 3 ] || fail "expected >=3 dtolnay/rust-toolchain pins (rust-runner-prep/coverage/release), found $found_rt"
echo "OK: all $found_rt dtolnay/rust-toolchain pins are SHA+version-tag pinned"

# 4) The Rust regression guard encodes the same invariant — run it.
OUTPUT="$(cargo test --test dependabot_action_pins 2>&1)"
printf '%s\n' "$OUTPUT"
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null \
  || fail "dependabot_action_pins regression guard did not pass"
echo "OK: dependabot_action_pins regression guard passed"

echo "PASS: taiki-e/install-action and dtolnay/rust-toolchain pins are Dependabot-resolvable"
