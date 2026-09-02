#!/usr/bin/env bash
# qa-release-create-idempotent.sh
#
# Regression gate for the recurring red `release` workflow (root cause: a
# non-idempotent "Create release" step racing on an unbumped version — see
# scripts/release-create.sh header for the full narrative).
#
# It exercises the REAL scripts/release-create.sh against a hermetic, on-PATH
# mock `gh` (no network, no real GitHub) and asserts the four load-independent
# outcomes that must hold regardless of scheduling:
#
#   1. Release already published (probe hit)      -> exit 0, "already exists" skip
#   2. Fresh publish succeeds                      -> exit 0
#   3. Create/create race ("already exists" err)   -> exit 0, treated idempotent
#   4. Genuine `gh` failure (e.g. HTTP 500)        -> non-zero, fails loudly
#
# A non-zero exit on any failed assertion is a real gate for the gadugi `cli`
# agent runner (and can be wired into a workflow step), so this is a true
# regression lock, not a cosmetic check.
set -uo pipefail

fail() {
  echo "QA-RELEASE-CREATE: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$REPO_ROOT/scripts/release-create.sh"
[ -x "$SCRIPT" ] || fail "scripts/release-create.sh missing or not executable"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ── Hermetic mock `gh` ───────────────────────────────────────────────────────
# Behaviour is driven by env vars set per-case:
#   MOCK_VIEW_EXISTS = 1  -> `gh release view` succeeds (release present)
#   MOCK_CREATE_MODE = ok|exists|fail
mkdir -p "$WORK/bin"
cat > "$WORK/bin/gh" <<'MOCK'
#!/usr/bin/env bash
if [ "$1" = "release" ] && [ "$2" = "view" ]; then
  [ "${MOCK_VIEW_EXISTS:-0}" = "1" ] && exit 0 || exit 1
fi
if [ "$1" = "release" ] && [ "$2" = "create" ]; then
  case "${MOCK_CREATE_MODE:-ok}" in
    ok)     echo "https://github.com/rysweet/Simard/releases/tag/${TAG:-vX}"; exit 0;;
    exists) echo "a release with the same tag name already exists: ${TAG:-vX}" >&2; exit 1;;
    fail)   echo "HTTP 500: something transient went wrong" >&2; exit 1;;
  esac
fi
# Any other gh subcommand is unexpected in this gate.
echo "unexpected gh invocation: $*" >&2
exit 99
MOCK
chmod +x "$WORK/bin/gh"

# Dummy asset so the "no assets" guard is not tripped; the mock never reads it.
: > "$WORK/asset.tar.gz"

run_case() {
  # $1=view_exists $2=create_mode ; prints exit code
  env PATH="$WORK/bin:$PATH" \
      TAG="v9.9.9" VERSION="9.9.9" GITHUB_REPOSITORY="rysweet/Simard" \
      MOCK_VIEW_EXISTS="$1" MOCK_CREATE_MODE="$2" \
      bash "$SCRIPT" "$WORK/asset.tar.gz" >"$WORK/out.log" 2>&1
  echo "$?"
}

# 1. Pre-existing release: probe hits -> idempotent skip, exit 0.
rc="$(run_case 1 ok)"
[ "$rc" = "0" ] || fail "pre-existing release must exit 0 (got $rc): $(cat "$WORK/out.log")"
grep -qi "already exists; skipping" "$WORK/out.log" \
  || fail "pre-existing release must log an idempotent skip notice"

# 2. Fresh publish succeeds -> exit 0.
rc="$(run_case 0 ok)"
[ "$rc" = "0" ] || fail "fresh successful publish must exit 0 (got $rc): $(cat "$WORK/out.log")"

# 3. Create/create race: probe misses, create says already-exists -> exit 0.
rc="$(run_case 0 exists)"
[ "$rc" = "0" ] || fail "already-exists create race must exit 0 (got $rc): $(cat "$WORK/out.log")"
grep -qi "concurrently; treating as idempotent success" "$WORK/out.log" \
  || fail "create race must log the concurrent idempotent-success notice"

# 4. Genuine failure must NOT be swallowed -> non-zero (fail-closed).
rc="$(run_case 0 fail)"
[ "$rc" != "0" ] || fail "a genuine gh failure must NOT be treated as success"
grep -qi "gh release create failed" "$WORK/out.log" \
  || fail "a genuine failure must surface a loud ::error:: line"

# 5. Missing-assets guard.
rc="$(env PATH="$WORK/bin:$PATH" TAG=v9.9.9 VERSION=9.9.9 \
        MOCK_VIEW_EXISTS=0 MOCK_CREATE_MODE=ok bash "$SCRIPT" >/dev/null 2>&1; echo $?)"
[ "$rc" != "0" ] || fail "calling with no assets must fail (guard)"

echo "QA-RELEASE-CREATE: PASS - idempotent publish holds across all 5 cases"
