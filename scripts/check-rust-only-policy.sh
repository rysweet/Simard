#!/usr/bin/env bash
# Rust-only policy gate — see https://github.com/rysweet/Simard/issues/2155
#
# Blocks non-Rust files in restricted locations. Invoked by pre-commit with
# the list of staged (or --all-files) paths matching *.py / *.js / *.ts.
#
# Rules:
#   1. No new .py files under src/ or python/
#   2. No new .js/.ts files outside npm/ and tests/e2e-dashboard/
#
# Existing files pending migration are allow-listed below.

set -euo pipefail

# ── Allow-list (existing files, tracked for migration in #2155) ──────────
ALLOWLIST=(
  # Python bridges (migration tracked in #2157)
  python/bridge_server.py
  python/simard_gym_bridge.py
  python/simard_knowledge_bridge.py
  # Python audit scripts (migration tracked in #2156)
  scripts/dashboard_audit/audit_dashboard.py
  scripts/dashboard_audit/audit_pass_01.py
  # Root-level JS (disposition tracked in #2159)
  bin.js
  bin.test.js
  readme.test.js
)

violations=()

for file in "$@"; do
  # Normalise path: strip leading ./
  file="${file#./}"

  # Skip allow-listed files
  for allowed in "${ALLOWLIST[@]}"; do
    if [[ "$file" == "$allowed" ]]; then
      continue 2
    fi
  done

  # Rule 1: .py files under src/ or python/
  if [[ "$file" == *.py ]]; then
    if [[ "$file" == src/* || "$file" == python/* ]]; then
      violations+=("$file")
    fi
    continue
  fi

  # Rule 2: .js/.ts files outside exempt directories
  if [[ "$file" == *.js || "$file" == *.ts ]]; then
    if [[ "$file" != npm/* && "$file" != tests/e2e-dashboard/* ]]; then
      violations+=("$file")
    fi
  fi
done

if (( ${#violations[@]} )); then
  echo "❌ Rust-only policy violation (see #2155)"
  echo ""
  echo "The following files violate the Rust-only CLI policy:"
  for v in "${violations[@]}"; do
    echo "  - $v"
  done
  echo ""
  echo "Policy: all new CLI code must be written in Rust."
  echo "  • No new .py files under src/ or python/"
  echo "  • No new .js/.ts files outside npm/ and tests/e2e-dashboard/"
  echo ""
  echo "If migrating an existing file, add it to the allow-list in"
  echo "scripts/check-rust-only-policy.sh and reference the tracking issue."
  echo ""
  echo "Epic: https://github.com/rysweet/Simard/issues/2155"
  exit 1
fi
