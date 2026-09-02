#!/usr/bin/env bash
# qa-team scenario: Simard's RustyClawd dependency pins stay on the upstream
# RUSTSEC-2026-0204 fixed commit and Cargo.lock matches Cargo.toml.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

FIXED_REV="dcccad80ed381c66a7728565be5cb84120aacbed"
OLD_REV="ddae0fdf1b922ae8604b0a4e178a3634ada7e350"
REPO_URL="https://github.com/rysweet/RustyClawd.git"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

for crate in rustyclawd-core rustyclawd-tools; do
  grep -F "${crate} = { git = \"${REPO_URL}\", rev = \"${FIXED_REV}\" }" Cargo.toml >/dev/null \
    || fail "Cargo.toml does not pin ${crate} to ${FIXED_REV}"
  echo "OK: Cargo.toml pins ${crate} to ${FIXED_REV}"
done

if grep -F "$OLD_REV" Cargo.toml Cargo.lock >/dev/null; then
  fail "old vulnerable RustyClawd rev ${OLD_REV} is still present"
fi
echo "OK: old RustyClawd rev ${OLD_REV} is absent from Cargo.toml and Cargo.lock"

lock_count="$(
  grep -F "source = \"git+${REPO_URL}?rev=${FIXED_REV}#${FIXED_REV}\"" Cargo.lock | wc -l | tr -d ' '
)"
[ "$lock_count" = "2" ] \
  || fail "expected 2 Cargo.lock RustyClawd entries for ${FIXED_REV}, found ${lock_count}"
echo "OK: Cargo.lock has both RustyClawd package sources at ${FIXED_REV}"

cargo metadata --locked --format-version 1 --no-deps >/dev/null
echo "OK: cargo metadata --locked accepts the manifest and lockfile"

work="$(mktemp -d /tmp/simard-rustyclawd-pin.XXXXXX)"
trap 'rm -rf "$work"' EXIT
git -C "$work" init -q
git -C "$work" remote add origin "$REPO_URL"
git -C "$work" fetch --depth=1 origin "$FIXED_REV" >/dev/null 2>&1 \
  || fail "RustyClawd fixed rev ${FIXED_REV} is not fetchable from ${REPO_URL}"
git -C "$work" cat-file -e "${FIXED_REV}^{commit}" \
  || fail "RustyClawd fixed rev ${FIXED_REV} is not a commit"
echo "OK: RustyClawd fixed rev ${FIXED_REV} is fetchable from ${REPO_URL}"

echo "PASS: RustyClawd dependency pins are fixed and resolvable"
