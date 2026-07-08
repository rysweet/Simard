#!/usr/bin/env bash
# check-rust-only-gate.sh — Enforce the Rust-only policy (issues #2155, #3181).
#
# Fails if ANY .py file is found anywhere in the repo, or any .js/.ts file
# outside exempted directories, that is NOT in the allow-list of pre-existing
# files. Simard is a pure-Rust daemon (issue #3181) — no Python may be
# reintroduced under any path.
#
# Usage:
#   scripts/check-rust-only-gate.sh          # check working tree
#   scripts/check-rust-only-gate.sh --staged # check only staged files (pre-commit)

set -euo pipefail

# ── Allow-list: pre-existing files that are permitted until migrated ──────────
# Each entry is tracked by the Rust-only epic (#2155). Remove entries as the
# corresponding rewrite issues (#2156, #2157) are completed.
#
# Python bridge files (bridge_server.py, simard_gym_bridge.py,
# simard_knowledge_bridge.py) removed in #2181 — native Rust transport complete.
ALLOWED_PY_FILES=(
)

ALLOWED_JS_TS_FILES=(
  "bin.js"
  "bin.test.js"
  "readme.test.js"
)

# Directories where JS/TS is permitted (e.g., npm wrapper, e2e test fixtures).
EXEMPT_JS_TS_DIRS=(
  "npm/"
  "tests/e2e-dashboard/"
)

# ── Collect file list ─────────────────────────────────────────────────────────
MODE="${1:-}"

if [[ "$MODE" == "--staged" ]]; then
  # Pre-commit: only check staged additions/modifications.
  FILE_LIST=$(git diff --cached --name-only --diff-filter=ACR 2>/dev/null || true)
else
  # CI / manual: scan the full tree.
  FILE_LIST=$(git ls-files 2>/dev/null || find . -type f | sed 's|^\./||')
fi

# ── Helper: check if a value is in an array ──────────────────────────────────
in_array() {
  local needle="$1"; shift
  for item in "$@"; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

# ── Helper: check if a path starts with any exempt prefix ────────────────────
in_exempt_dir() {
  local path="$1"; shift
  for prefix in "$@"; do
    [[ "$path" == "$prefix"* ]] && return 0
  done
  return 1
}

# ── Check for prohibited Python files ────────────────────────────────────────
violations=()

while IFS= read -r file; do
  [[ -z "$file" ]] && continue

  # Python files ANYWHERE in the repo (issue #3181: Simard is Python-free).
  if [[ "$file" == *.py ]]; then
    if ! in_array "$file" "${ALLOWED_PY_FILES[@]}"; then
      violations+=("PYTHON  $file")
    fi
  fi

  # JS/TS files outside exempted directories
  if [[ "$file" == *.js || "$file" == *.ts ]]; then
    if ! in_exempt_dir "$file" "${EXEMPT_JS_TS_DIRS[@]}"; then
      if ! in_array "$file" "${ALLOWED_JS_TS_FILES[@]}"; then
        violations+=("JS/TS   $file")
      fi
    fi
  fi
done <<< "$FILE_LIST"

# ── Report ───────────────────────────────────────────────────────────────────
if [[ ${#violations[@]} -gt 0 ]]; then
  echo "❌ Rust-only policy violation detected (see https://github.com/rysweet/Simard/issues/2155)"
  echo ""
  echo "The following non-Rust files are not in the allow-list:"
  echo ""
  for v in "${violations[@]}"; do
    echo "  • $v"
  done
  echo ""
  echo "This project is Rust-only (issue #3181). New .py files anywhere in the"
  echo "repo, and new .js/.ts files outside npm/ and tests/e2e-dashboard/, are"
  echo "not permitted."
  echo ""
  echo "If this file is intentionally needed, add it to the allow-list in"
  echo "scripts/check-rust-only-gate.sh and document the reason."
  exit 1
fi

echo "✅ Rust-only gate passed — no prohibited non-Rust files found."
exit 0
