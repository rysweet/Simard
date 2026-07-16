#!/usr/bin/env bash
# qa-team scenario — the flaky packages.microsoft.com apt source stays hardened.
#
# Outside-in verification of the CI-health fix for the intermittently-red
# default-branch `verify` jobs (issue #2975): GitHub runner images ship
# packages.microsoft.com apt sources that periodically serve an invalid
# InRelease ("NOSPLIT" / "no longer signed"), so every `apt-get update` exits
# 100 and fails the job with a spurious red. `scripts/ci-harden-apt.sh` strips
# those Microsoft sources before any apt-get update.
#
# This script asserts the operator-visible contract with no network:
#   1. behavioural — the script removes ONLY Microsoft sources, preserves the
#      rest, and is idempotent (running twice makes no further change);
#   2. wiring — both apt-consuming CI jobs invoke the script before apt-get;
#   3. the Rust regression guard encodes the same invariant.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

SCRIPT="scripts/ci-harden-apt.sh"
[ -x "$SCRIPT" ] || fail "$SCRIPT missing or not executable"

# ---------------------------------------------------------------------------
# 1) Behavioural test in an isolated fake apt tree (no sudo, no network).
# ---------------------------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/sources.list.d"

printf 'deb [arch=amd64] https://packages.microsoft.com/ubuntu/24.04/prod noble main\n' \
  > "$tmp/sources.list.d/microsoft-prod.list"
printf 'Types: deb\nURIs: https://packages.microsoft.com/repos/azure-cli/\nSuites: noble\nComponents: main\n' \
  > "$tmp/sources.list.d/azure-cli.sources"
printf 'deb https://apt.example.org/ubuntu noble main\n' \
  > "$tmp/sources.list.d/keep-me.list"
cat > "$tmp/sources.list" <<EOF
deb http://azure.archive.ubuntu.com/ubuntu noble main restricted
deb https://packages.microsoft.com/ubuntu/24.04/prod noble main
# deb https://packages.microsoft.com/old noble main
EOF

export APT_SOURCES_LIST_D="$tmp/sources.list.d" APT_SOURCES_LIST="$tmp/sources.list" CI_HARDEN_APT_SUDO=""

bash "$SCRIPT" >/dev/null

[ ! -f "$tmp/sources.list.d/microsoft-prod.list" ] || fail "Microsoft .list not removed"
[ ! -f "$tmp/sources.list.d/azure-cli.sources" ]  || fail "Microsoft .sources not removed"
[ -f "$tmp/sources.list.d/keep-me.list" ]         || fail "non-Microsoft source was removed"
grep -q '^deb http://azure.archive.ubuntu.com' "$tmp/sources.list" || fail "Ubuntu archive line was altered"
grep -q '^# disabled-by-ci:.*packages.microsoft.com/ubuntu/24.04/prod noble main' "$tmp/sources.list" \
  || fail "active Microsoft line in sources.list was not disabled"
echo "OK: removes only Microsoft sources; preserves Ubuntu + third-party"

before="$(cat "$tmp/sources.list")"
bash "$SCRIPT" >/dev/null
after="$(cat "$tmp/sources.list")"
[ "$before" = "$after" ]                                   || fail "second run mutated sources.list (not idempotent)"
[ "$(grep -c 'disabled-by-ci' "$tmp/sources.list")" -eq 1 ] || fail "idempotency: expected exactly one disabled marker"
echo "OK: idempotent (second run is a no-op)"

# ---------------------------------------------------------------------------
# 2) Wiring — both apt-consuming CI jobs run the hardening step first.
# ---------------------------------------------------------------------------
grep -q 'scripts/ci-harden-apt.sh' .github/actions/rust-runner-prep/action.yml \
  || fail "rust-runner-prep/action.yml does not invoke ci-harden-apt.sh"
grep -q 'scripts/ci-harden-apt.sh' .github/workflows/verify.yml \
  || fail "verify.yml does not invoke ci-harden-apt.sh"
echo "OK: both apt-consuming CI jobs invoke ci-harden-apt.sh"

# ---------------------------------------------------------------------------
# 3) Rust regression guard encodes the same invariant — run it.
# ---------------------------------------------------------------------------
OUTPUT="$(cargo test --test ci_harden_apt_wiring 2>&1)"
printf '%s\n' "$OUTPUT"
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null \
  || fail "ci_harden_apt_wiring regression guard did not pass"
echo "OK: ci_harden_apt_wiring regression guard passed"

echo "PASS: flaky packages.microsoft.com apt source stays hardened"
