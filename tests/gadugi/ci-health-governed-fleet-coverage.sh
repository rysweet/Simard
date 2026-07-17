#!/usr/bin/env bash
# qa-team scenario for goal steward-ci-github-actions-health-across-all-gov.
#
# Outside-in verification that "across all governed repos" is complete,
# enforced, and auditable: the shipped `simard ci-health --list-repos` surface
# must print exactly the governed fleet documented in the ecosystem table of
# prompt_assets/simard/engineer_system.md — no more, no less. This is the
# outside-in complement to the in-crate drift-guard test
# (ci_health::tests::governed_fleet_coverage): here we drive the real binary and
# cross-check its output against the checked-in prompt doc, so a governed repo
# onboarded to the doc but dropped from the swept const (which would silently
# narrow coverage) is caught from the outside too.
#
# Read-only and network-free: --list-repos never calls `gh`. It must run against
# the in-repo prompt_assets/, never the deployed ~/.simard copy.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

ENGINEER="prompt_assets/simard/engineer_system.md"

fail() {
  echo "[gadugi] FAIL: $1" >&2
  exit 1
}

[ -f "$ENGINEER" ] || fail "$ENGINEER (governed-fleet source of truth) not found"

# ── Extract the governed slugs from the ecosystem table ─────────────────────
# Scope strictly to the section between `## Your Ecosystem` and the next H2, so
# `rysweet/...` slugs elsewhere in the prompt (e.g. the frozen upstream-pin
# table) cannot leak in. Within that section, a table row's second content cell
# is the GitHub `owner/repo` slug; the `GitHub` header and `---` separator cells
# fail the owner/repo shape and are skipped.
DOC_SLUGS="$(awk '
  /^## / { insec = ($0 == "## Your Ecosystem"); next }
  insec && /^[[:space:]]*\|/ {
    n = split($0, a, "|")
    cell = a[3]; gsub(/^[[:space:]]+|[[:space:]]+$/, "", cell)
    if (cell ~ /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/) print cell
  }
' "$ENGINEER" | sort -u)"

[ -n "$DOC_SLUGS" ] || fail "parsed zero governed slugs from the $ENGINEER ecosystem table"
DOC_COUNT="$(printf '%s\n' "$DOC_SLUGS" | grep -c .)"
[ "$DOC_COUNT" -ge 5 ] || fail "parsed only $DOC_COUNT slugs — the ecosystem table shape likely changed"

echo "[gadugi] ecosystem-table governed slugs ($DOC_COUNT):"
printf '  %s\n' $DOC_SLUGS

# ── 1. Human --list-repos prints exactly the documented fleet ───────────────
HUMAN_OUT="$(cargo run --quiet --bin simard -- ci-health --list-repos 2>/dev/null)"
printf '%s\n' "$HUMAN_OUT"
CLI_SLUGS="$(printf '%s\n' "$HUMAN_OUT" | grep -E '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$' | sort -u)"

if [ "$DOC_SLUGS" != "$CLI_SLUGS" ]; then
  echo "[gadugi] doc vs CLI slug mismatch" >&2
  echo "--- in doc, not CLI (add to GOVERNED_REPOS):" >&2
  comm -23 <(printf '%s\n' "$DOC_SLUGS") <(printf '%s\n' "$CLI_SLUGS") >&2 || true
  echo "--- in CLI, not doc (add to the ecosystem table or drop from the const):" >&2
  comm -13 <(printf '%s\n' "$DOC_SLUGS") <(printf '%s\n' "$CLI_SLUGS") >&2 || true
  fail "simard ci-health --list-repos drifted from the ecosystem table"
fi

# Simard itself must be in the fleet (it is a governed repo too).
printf '%s\n' "$CLI_SLUGS" | grep -Fx "rysweet/Simard" >/dev/null \
  || fail "rysweet/Simard missing from --list-repos output"

# ── 2. --list-repos --json is valid JSON with count == fleet size ───────────
JSON_OUT="$(cargo run --quiet --bin simard -- ci-health --list-repos --json 2>/dev/null)"
printf '%s\n' "$JSON_OUT"
if command -v python3 >/dev/null 2>&1; then
  echo "$JSON_OUT" | python3 -c '
import json, sys
d = json.load(sys.stdin)
repos = d["repos"]
count = d["count"]
assert count == len(repos), "count != len(repos)"
assert len(repos) == len(set(repos)), "duplicate slugs in --list-repos --json"
assert "rysweet/Simard" in repos, "Simard missing from JSON fleet"
print("[gadugi] --list-repos --json OK:", count, "repos")
' || fail "--list-repos --json failed shape validation"
  # The JSON fleet must equal the documented fleet exactly.
  JSON_SLUGS="$(echo "$JSON_OUT" | python3 -c 'import json,sys; print("\n".join(json.load(sys.stdin)["repos"]))' | sort -u)"
  [ "$DOC_SLUGS" = "$JSON_SLUGS" ] || fail "--list-repos --json fleet differs from the ecosystem table"
fi

# ── 3. --list-repos is standalone: it ignores the writing sweep flags ───────
# A stray --file-issues must NOT turn a coverage query into a live, issue-filing
# sweep. --list-repos short-circuits before any sweep and exits 0, printing the
# same fleet (no network, no writes).
GUARD_OUT="$(cargo run --quiet --bin simard -- ci-health --list-repos --file-issues 2>/dev/null)"
GUARD_SLUGS="$(printf '%s\n' "$GUARD_OUT" | grep -E '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$' | sort -u)"
[ "$DOC_SLUGS" = "$GUARD_SLUGS" ] \
  || fail "--list-repos --file-issues did not short-circuit to the plain fleet listing"

# ── 4. --list-repos is advertised in help ───────────────────────────────────
HELP_OUT="$(cargo run --quiet --bin simard -- ci-health --help 2>/dev/null)"
printf '%s\n' "$HELP_OUT" | grep -F -- "--list-repos" >/dev/null \
  || fail "--list-repos is not advertised in `simard ci-health --help`"

echo "ci-health-governed-fleet-coverage: PASS"
