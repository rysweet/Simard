#!/usr/bin/env bash
# Outside-in coverage for `simard roster`: the runtime CURATION surface for
# Simard's stewarded-repo roster. Storage/seeding/resolution already live in the
# framework (`identity_curated_state` + `ecosystem_observe::load_stewarded_roster`,
# seeded from the committed `prompt_assets/simard/identity/stewarded_repos.seed.toml`);
# this scenario proves the missing agentic verb — actually curating that durable,
# identity-scoped state at runtime — works end to end and is deploy-durable.
#
# Drives the real `simard` binary against a throwaway SIMARD_STATE_ROOT (the tree
# `install` NEVER overwrites) and asserts:
#   1. `roster list` seeds on first use from committed identity data: 10 repos,
#      including rysweet/Simard, EXCLUDING the deprecated Python rysweet/amplihack.
#   2. `roster add <slug> [note]` upserts a stewarded repo (now 11) and persists it
#      to <state_root>/identity-state/simard/stewarded_repos.toml.
#   3. A SEPARATE process (same state root) still sees the curated 11 — the edit
#      survived a process restart, the proxy for surviving a self-deploy since the
#      state root is untouched by install.
#   4. `roster remove <slug>` drops it back to 10 and rewrites the durable file.
#   5. A malformed slug is rejected with a NON-ZERO exit and NEVER mutates state.
#   6. `roster --help` advertises list/add/remove.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STATE_ROOT="$(mktemp -d)"
trap 'rm -rf "$STATE_ROOT"' EXIT
export SIMARD_STATE_ROOT="$STATE_ROOT"
# Suppress the "update available" stdout banner so assertions see clean output.
export SIMARD_NO_UPDATE_CHECK=1

ROSTER_FILE="$STATE_ROOT/identity-state/simard/stewarded_repos.toml"

roster() {
  cargo run --quiet --bin simard -- roster "$@" 2>/dev/null
}

# ── 1. First `list` seeds the roster from committed identity data ────────────
LIST_OUT="$(roster list)"
printf '%s\n' "$LIST_OUT"

printf '%s\n' "$LIST_OUT" | grep -F "Stewarded roster for identity 'simard' (10 repos):" >/dev/null
printf '%s\n' "$LIST_OUT" | grep -F "rysweet/Simard" >/dev/null
printf '%s\n' "$LIST_OUT" | grep -F "rysweet/gadugi-agentic-test" >/dev/null
# The deprecated Python repo must NOT be stewarded (amplihack-rs is, amplihack is not).
if printf '%s\n' "$LIST_OUT" | grep -Eq '(^|[^-])rysweet/amplihack($|[^-])'; then
  echo "FAIL: deprecated rysweet/amplihack must not be on the roster" >&2
  exit 1
fi
# Seeding must have written the durable file under the install-safe state root.
test -f "$ROSTER_FILE" || { echo "FAIL: durable roster file not created at $ROSTER_FILE" >&2; exit 1; }

# ── 2. `add` curates a new stewarded repo (durable) ──────────────────────────
ADD_OUT="$(roster add rysweet/qa-curated-repo "added by the roster curation scenario")"
printf '%s\n' "$ADD_OUT"
printf '%s\n' "$ADD_OUT" | grep -F "roster: stewarding rysweet/qa-curated-repo" >/dev/null
printf '%s\n' "$ADD_OUT" | grep -F "11 repos total" >/dev/null
grep -F 'key = "rysweet/qa-curated-repo"' "$ROSTER_FILE" >/dev/null
grep -F 'note = "added by the roster curation scenario"' "$ROSTER_FILE" >/dev/null

# ── 3. A separate process (same state root) sees the curated edit ────────────
LIST2_OUT="$(roster list)"
printf '%s\n' "$LIST2_OUT" | grep -F "(11 repos):" >/dev/null
printf '%s\n' "$LIST2_OUT" | grep -F "rysweet/qa-curated-repo — added by the roster curation scenario" >/dev/null

# ── 4. `remove` drops the stewardship durably ────────────────────────────────
REMOVE_OUT="$(roster remove rysweet/qa-curated-repo)"
printf '%s\n' "$REMOVE_OUT"
printf '%s\n' "$REMOVE_OUT" | grep -F "roster: no longer stewarding rysweet/qa-curated-repo" >/dev/null
printf '%s\n' "$REMOVE_OUT" | grep -F "10 repos total" >/dev/null
if grep -F 'key = "rysweet/qa-curated-repo"' "$ROSTER_FILE" >/dev/null; then
  echo "FAIL: removed repo still present in durable roster file" >&2
  exit 1
fi

# ── 5. A malformed slug is rejected and never mutates state ──────────────────
BEFORE="$(cat "$ROSTER_FILE")"
if cargo run --quiet --bin simard -- roster add "not a slug" >/dev/null 2>&1; then
  echo "FAIL: malformed slug should have produced a non-zero exit" >&2
  exit 1
fi
AFTER="$(cat "$ROSTER_FILE")"
test "$BEFORE" = "$AFTER" || { echo "FAIL: rejected slug must not mutate the roster file" >&2; exit 1; }

# ── 6. Help advertises the curation verbs ────────────────────────────────────
HELP_OUT="$(roster --help)"
printf '%s\n' "$HELP_OUT" | grep -F "Usage: simard roster <command>" >/dev/null
printf '%s\n' "$HELP_OUT" | grep -F "add <owner/name>" >/dev/null
printf '%s\n' "$HELP_OUT" | grep -F "remove <owner/name>" >/dev/null

echo "ROSTER-CURATION: PASS"
