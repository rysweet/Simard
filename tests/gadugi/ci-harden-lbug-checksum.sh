#!/usr/bin/env bash
# qa-team scenario — the prebuilt liblbug archive is content-verified before it
# is linked (issue #2471).
#
# Outside-in verification of the CI-health supply-chain fix:
# scripts/provision-lbug-prebuilt.sh downloads a version-pinned
# `liblbug-static-*.tar.gz` release asset and statically links it into every CI
# build. TLS + version/repo pinning authenticate the transport and URL, but not
# the *content*; a release asset tampered with at rest would previously be
# linked unnoticed. The fix pins each asset's SHA-256 in the code-reviewed
# manifest scripts/lbug-prebuilt.sha256 and refuses (fail-closed) to extract any
# tarball whose hash does not match.
#
# This script asserts the operator-visible contract with NO network:
#   1. behavioural — sourcing the provisioner and driving the real verify gate
#      with a throwaway manifest: a matching hash is accepted, a mismatch and an
#      unknown (fail-closed) pin are both refused;
#   2. wiring — verification runs BEFORE extraction, against the in-repo
#      manifest, and the manifest pins the CI-consumed asset;
#   3. the Rust regression guard encodes the same invariant.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

SCRIPT="scripts/provision-lbug-prebuilt.sh"
MANIFEST="scripts/lbug-prebuilt.sha256"
[ -f "$SCRIPT" ] || fail "$SCRIPT missing"
[ -f "$MANIFEST" ] || fail "$MANIFEST missing"

# ---------------------------------------------------------------------------
# 1) Behavioural test of the real verify gate (no sudo, no network).
# ---------------------------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'pretend-tarball-bytes\n' > "$tmp/asset.tar.gz"
digest="$(sha256sum "$tmp/asset.tar.gz" | awk '{ print $1 }')"
printf '%s  9.9.9  asset.tar.gz\n' "$digest" > "$tmp/manifest.sha256"

export LBUG_CHECKSUM_MANIFEST="$tmp/manifest.sha256"
# Source the provisioner: the source-safe guard exposes verify_sha256 without
# triggering a download.
# shellcheck source=/dev/null
source "$SCRIPT"

verify_sha256 "$tmp/asset.tar.gz" "9.9.9" "asset.tar.gz" \
  || fail "verify_sha256 rejected an asset whose hash matches the pinned digest"
echo "OK: matching SHA-256 is accepted"

printf 'tampered-bytes\n' > "$tmp/asset.tar.gz"
if verify_sha256 "$tmp/asset.tar.gz" "9.9.9" "asset.tar.gz" 2>/dev/null; then
  fail "verify_sha256 accepted a tampered asset (hash no longer matches)"
fi
echo "OK: a SHA-256 mismatch is refused"

if verify_sha256 "$tmp/asset.tar.gz" "0.0.0" "unknown.tar.gz" 2>/dev/null; then
  fail "verify_sha256 accepted an asset with no pinned digest (not fail-closed)"
fi
echo "OK: an unknown (version, asset) is fail-closed"

# ---------------------------------------------------------------------------
# 2) Wiring — verification is ordered before extraction and pins the CI asset.
# ---------------------------------------------------------------------------
verify_line="$(grep -n 'verify_sha256 "' "$SCRIPT" | head -n1 | cut -d: -f1)"
extract_line="$(grep -n 'tar xzf "' "$SCRIPT" | head -n1 | cut -d: -f1)"
[ -n "$verify_line" ] || fail "provisioner does not verify the downloaded asset"
[ -n "$extract_line" ] || fail "provisioner does not extract the asset"
[ "$verify_line" -lt "$extract_line" ] \
  || fail "verification must run BEFORE extraction (verify@$verify_line, extract@$extract_line)"
echo "OK: SHA-256 verification runs before extraction"

ci_asset="liblbug-static-linux-x86_64-compat.tar.gz"
# Derive the resolved lbug version from Cargo.lock via the provisioner's own
# helper (sourced above) so this guard does not go stale on a version bump.
ci_version="$(lbug_version)"
[ -n "$ci_version" ] || fail "could not resolve lbug version from Cargo.lock/Cargo.toml"
grep -Eq "^[0-9a-f]{64}[[:space:]]+${ci_version}[[:space:]]+${ci_asset}\$" "$MANIFEST" \
  || fail "manifest does not pin the CI-consumed asset $ci_asset at lbug $ci_version"
echo "OK: manifest pins the CI-consumed asset ($ci_asset @ $ci_version)"

# ---------------------------------------------------------------------------
# 3) Rust regression guard encodes the same invariant — run it.
# ---------------------------------------------------------------------------
OUTPUT="$(cargo test --test issue_2471_lbug_provision_checksum 2>&1)"
printf '%s\n' "$OUTPUT"
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null \
  || fail "issue_2471_lbug_provision_checksum regression guard did not pass"
echo "OK: issue_2471_lbug_provision_checksum regression guard passed"

echo "PASS: prebuilt liblbug archive is content-verified before it is linked"
