#!/usr/bin/env bash
# Outside-in coverage for `simard roster`: the agentic curation surface for
# Simard's governed-fleet roster, now IDENTITY-scoped, mutable, deploy-durable
# state rather than a committed framework file.
#
# Drives the CLI against an isolated temp state root (SIMARD_STATE_ROOT) so the
# verdict is deterministic and never touches the real ~/.simard. Verifies:
#   1. `roster list` seeds from Simard's identity default on first use (the 10
#      stewarded slugs), and the deprecated Python rysweet/amplihack is absent.
#   2. `roster add owner/name` persists a curated repo to durable state under
#      <state_root>/identity/simard/curated/stewarded_repos.toml — the location
#      install/self-deploy never rewrites, so the edit is deploy-durable.
#   3. The added repo is visible on the next `roster list` (one source of truth).
#   4. Re-adding a listed repo is an idempotent no-op.
#   5. `roster remove owner/name` durably removes it; removal is idempotent.
#   6. Removing the LAST repo is refused — an empty roster is a fail-loud error
#      (an empty fleet would report GREEN), never a silent empty pass.
#   7. A malformed slug is rejected (never persisted) with a non-zero exit.
#   8. The curation surface is advertised in `simard --help` and `roster --help`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STATE_ROOT="$(mktemp -d)"
export SIMARD_STATE_ROOT="$STATE_ROOT"
trap 'rm -rf "$STATE_ROOT"' EXIT

CURATED="$STATE_ROOT/identity/simard/curated/stewarded_repos.toml"

roster() {
  cargo run --quiet --bin simard -- roster "$@" 2>/dev/null
}

# ── 1. First `list` seeds from the identity default (10 stewarded slugs) ──────
LIST_OUT="$(roster list)"
printf '%s\n' "$LIST_OUT"
for slug in \
  rysweet/Simard \
  rysweet/RustyClawd \
  rysweet/amplihack-rs \
  rysweet/azlin \
  rysweet/amplihack-memory-lib \
  rysweet/amplihack-agent-eval \
  rysweet/agent-kgpacks \
  rysweet/amplihack-recipe-runner \
  rysweet/amplihack-xpia-defender \
  rysweet/gadugi-agentic-test; do
  printf '%s\n' "$LIST_OUT" | grep -Fxq "$slug" \
    || { echo "FAIL: seeded roster missing $slug" >&2; exit 1; }
done
# The deprecated Python rysweet/amplihack must NOT be on the roster.
if printf '%s\n' "$LIST_OUT" | grep -Fxq "rysweet/amplihack"; then
  echo "FAIL: deprecated rysweet/amplihack present on roster" >&2
  exit 1
fi

# Seeding wrote the durable curated document under the state root.
[ -f "$CURATED" ] || { echo "FAIL: curated roster not persisted at $CURATED" >&2; exit 1; }

# ── 2 & 3. `add` persists a repo and it shows up on the next list ────────────
ADD_OUT="$(roster add rysweet/new-tool "trial stewardship")"
printf '%s\n' "$ADD_OUT"
printf '%s\n' "$ADD_OUT" | grep -F "added 'rysweet/new-tool'" >/dev/null \
  || { echo "FAIL: add did not report success" >&2; exit 1; }
roster list | grep -Fxq "rysweet/new-tool" \
  || { echo "FAIL: added repo not visible on list (one source of truth broken)" >&2; exit 1; }
# The durable document on disk contains it too (deploy-durable state).
grep -F "rysweet/new-tool" "$CURATED" >/dev/null \
  || { echo "FAIL: added repo not durably persisted" >&2; exit 1; }

# ── 4. Re-adding a listed repo is an idempotent no-op ────────────────────────
READD_OUT="$(roster add rysweet/new-tool "again")"
printf '%s\n' "$READD_OUT"
printf '%s\n' "$READD_OUT" | grep -F "already on" >/dev/null \
  || { echo "FAIL: re-add was not reported as a no-op" >&2; exit 1; }
# Still exactly one occurrence.
COUNT="$(roster list | grep -Fxc "rysweet/new-tool")"
[ "$COUNT" -eq 1 ] || { echo "FAIL: re-add duplicated the repo ($COUNT copies)" >&2; exit 1; }

# ── 5. `remove` durably removes it; removal is idempotent ────────────────────
RM_OUT="$(roster remove rysweet/new-tool)"
printf '%s\n' "$RM_OUT"
printf '%s\n' "$RM_OUT" | grep -F "removed 'rysweet/new-tool'" >/dev/null \
  || { echo "FAIL: remove did not report success" >&2; exit 1; }
if roster list | grep -Fxq "rysweet/new-tool"; then
  echo "FAIL: removed repo still on roster" >&2
  exit 1
fi
# Idempotent: removing it again is a no-op, not an error.
RM2_OUT="$(roster remove rysweet/new-tool)"
printf '%s\n' "$RM2_OUT" | grep -F "not on" >/dev/null \
  || { echo "FAIL: idempotent remove not reported as a no-op" >&2; exit 1; }

# ── 6. Removing the LAST repo is refused (empty roster = fail-loud) ──────────
# Drain a fresh single-repo roster for an isolated identity, then attempt to
# remove its last entry.
SOLO_ROOT="$(mktemp -d)"
printf 'schema_version = 1\n[[repo]]\nslug = "only/one"\nnote = ""\n' \
  > "$SOLO_ROOT/roster.toml"
mkdir -p "$SOLO_ROOT/identity/simard/curated"
cp "$SOLO_ROOT/roster.toml" "$SOLO_ROOT/identity/simard/curated/stewarded_repos.toml"
set +e
LAST_OUT="$(SIMARD_STATE_ROOT="$SOLO_ROOT" cargo run --quiet --bin simard -- roster remove only/one 2>&1)"
LAST_CODE=$?
set -e
rm -rf "$SOLO_ROOT"
printf '%s\n' "$LAST_OUT"
if [ "$LAST_CODE" -eq 0 ]; then
  echo "FAIL: removing the last repo was accepted (must be refused)" >&2
  exit 1
fi
printf '%s\n' "$LAST_OUT" | grep -F "last stewarded repo" >/dev/null \
  || { echo "FAIL: last-repo refusal did not explain the empty-roster guard" >&2; exit 1; }

# ── 7. A malformed slug is rejected and never persisted ──────────────────────
set +e
BAD_OUT="$(roster add "not-a-slug" 2>&1)"
BAD_CODE=$?
set -e
printf '%s\n' "$BAD_OUT"
if [ "$BAD_CODE" -eq 0 ]; then
  echo "FAIL: malformed slug was accepted (must be rejected)" >&2
  exit 1
fi
if roster list | grep -Fxq "not-a-slug"; then
  echo "FAIL: malformed slug leaked onto the roster" >&2
  exit 1
fi

# ── 8. The curation surface is advertised in help ───────────────────────────
TOP_HELP="$(cargo run --quiet --bin simard -- --help 2>/dev/null)"
printf '%s\n' "$TOP_HELP" | grep -F "roster list" >/dev/null \
  || { echo "FAIL: 'roster' not advertised in top-level help" >&2; exit 1; }
ROSTER_HELP="$(roster --help)"
printf '%s\n' "$ROSTER_HELP" | grep -F "deploy-durable" >/dev/null \
  || { echo "FAIL: roster help does not describe deploy-durable state" >&2; exit 1; }
printf '%s\n' "$ROSTER_HELP" | grep -F "fail-loud" >/dev/null \
  || { echo "FAIL: roster help does not describe the empty-roster fail-loud guard" >&2; exit 1; }

echo "roster-curation: PASS"
