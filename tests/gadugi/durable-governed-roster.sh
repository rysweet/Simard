#!/usr/bin/env bash
# Outside-in coverage for the governed-repo roster as identity-curated state.
#
# The governed roster used to be a git-tracked framework file
# (prompt_assets/simard/ecosystem_repos.toml) that every self-deploy clobbered.
# It is now Simard's identity-scoped, mutable, deploy-durable curated state: the
# `governed_repos` dataset for the `simard` identity, stored under the durable
# state root that `install` never overwrites.
#
# This scenario drives the shipping `simard ci-health` binary (which resolves the
# governed roster via `governed_repos()` before any network call) against an
# isolated SIMARD_STATE_ROOT and a fast-failing fake `gh`, and asserts the whole
# contract end-to-end:
#   1. SEEDED FROM IDENTITY — first use seeds the durable dataset from Simard's
#      default roster (10 repos; includes rysweet/Simard; excludes the deprecated
#      Python rysweet/amplihack).
#   2. NO FRAMEWORK FILE — the retired prompt_assets TOML is gone and is not read.
#   3. DURABLE + MUTABLE — an agentic curation (add + remove a stewarded repo)
#      survives a simulated redeploy: a later resolve returns the CURATED copy,
#      never the seed, so the edit is not clobbered.
#   4. SINGLE SOURCE OF TRUTH — the CI-health sweep resolves the SAME durable
#      dataset, so there is no second hardcoded roster to drift.
#
# Deterministic + offline: SIMARD_STATE_ROOT is a tempdir and a fake `gh` on PATH
# fails fast, so the sweep errors immediately AFTER seeding the roster — no
# network, no real gh, no real issues filed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

export SIMARD_STATE_ROOT="$WORK/state-root"
ROSTER_FILE="$SIMARD_STATE_ROOT/state/identity_state/simard/governed_repos.toml"

# A fake `gh` that fails fast, so the live sweep errors on its first workflow
# query — immediately AFTER `governed_repos()` has seeded the durable roster.
# This keeps the scenario hermetic (no network, no auth, no real gh) while still
# exercising the real roster-resolution code path in the shipping binary.
FAKE_BIN="$WORK/bin"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/gh" <<'EOF'
#!/usr/bin/env bash
echo "fake gh: offline scenario, no network" >&2
exit 1
EOF
chmod +x "$FAKE_BIN/gh"
export PATH="$FAKE_BIN:$PATH"

# Build once so the `cargo run` invocations below are fast and quiet.
cargo build --quiet --bin simard

run_ci_health() {
  # The sweep resolves (and on first use seeds) the governed roster, then hits
  # the fake gh and errors. We only care about the roster side effect, so the
  # non-zero exit is expected and ignored.
  cargo run --quiet --bin simard -- ci-health >/dev/null 2>&1 || true
}

# ── 0. The retired framework file must be gone ──────────────────────────────
if [ -f "prompt_assets/simard/ecosystem_repos.toml" ]; then
  echo "FAIL: retired framework file prompt_assets/simard/ecosystem_repos.toml still exists" >&2
  exit 1
fi

# ── 1. SEEDED FROM IDENTITY — first resolve seeds the durable dataset ────────
[ -f "$ROSTER_FILE" ] && { echo "FAIL: roster present before first run" >&2; exit 1; }

run_ci_health

if [ ! -f "$ROSTER_FILE" ]; then
  echo "FAIL: first ci-health run did not seed the durable governed roster at $ROSTER_FILE" >&2
  exit 1
fi
echo "seeded durable roster: $ROSTER_FILE"
cat "$ROSTER_FILE"

# The seed is Simard's default roster: 10 repos, includes Simard herself, and
# deliberately EXCLUDES the deprecated Python rysweet/amplihack.
SEED_COUNT="$(grep -c '^value = ' "$ROSTER_FILE")"
if [ "$SEED_COUNT" -ne 10 ]; then
  echo "FAIL: seeded roster has $SEED_COUNT repos, expected 10" >&2
  exit 1
fi
grep -F 'value = "rysweet/Simard"' "$ROSTER_FILE" >/dev/null \
  || { echo "FAIL: seed must include rysweet/Simard (Simard stewards her own CI)" >&2; exit 1; }
grep -F 'value = "rysweet/amplihack-rs"' "$ROSTER_FILE" >/dev/null \
  || { echo "FAIL: seed must include rysweet/amplihack-rs" >&2; exit 1; }
if grep -F 'value = "rysweet/amplihack"' "$ROSTER_FILE" >/dev/null; then
  echo "FAIL: seed must NOT include the deprecated Python rysweet/amplihack" >&2
  exit 1
fi

# ── 2. DURABLE + MUTABLE — curate the roster, then simulate a redeploy ───────
# Agentic curation: remove a stewarded repo and add a new one, editing the
# durable dataset directly (as Simard would, or an operator on the deployed
# host). `install` never overwrites the state root, so this edit is durable.
python3 - "$ROSTER_FILE" <<'PY'
import sys, re
path = sys.argv[1]
text = open(path).read()
# Remove the rysweet/azlin item block (an [[item]] with value + note lines).
text = re.sub(
    r'\n\[\[item\]\]\nvalue = "rysweet/azlin"\nnote = "[^"]*"\n',
    "\n",
    text,
)
# Append a freshly-stewarded repo.
text = text.rstrip() + '\n\n[[item]]\nvalue = "rysweet/new-steward"\nnote = "added by curation test"\n'
open(path, "w").write(text)
PY

# Simulate a redeploy: `install` replaces bin/prompt_assets but NEVER the state
# root, so the curated roster is still on disk. Re-running ci-health resolves the
# CURATED copy — it must NOT re-seed over the edit.
run_ci_health

grep -F 'value = "rysweet/new-steward"' "$ROSTER_FILE" >/dev/null \
  || { echo "FAIL: added repo did not survive the simulated redeploy (roster was clobbered)" >&2; exit 1; }
if grep -F 'value = "rysweet/azlin"' "$ROSTER_FILE" >/dev/null; then
  echo "FAIL: removed repo reappeared after redeploy (roster was re-seeded, not curated)" >&2
  exit 1
fi

# The curated roster is authoritative: still exactly one edit-net repo change
# (10 - 1 removed + 1 added = 10), and Simard herself is still stewarded.
CURATED_COUNT="$(grep -c '^value = ' "$ROSTER_FILE")"
if [ "$CURATED_COUNT" -ne 10 ]; then
  echo "FAIL: curated roster has $CURATED_COUNT repos, expected 10 after one remove + one add" >&2
  exit 1
fi
grep -F 'value = "rysweet/Simard"' "$ROSTER_FILE" >/dev/null \
  || { echo "FAIL: Simard fell off her own curated roster" >&2; exit 1; }

echo "durable-governed-roster: PASS"
