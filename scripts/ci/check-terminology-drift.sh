#!/usr/bin/env bash
# Anti-drift terminology gate for the unified "Brain" cognition model (#2419).
#
# Enforces the behavior-preserving terminology law from
# docs/reference/brain-terminology-migration.md:
#
#   1. "Brain" = the whole cognition (scheduler + threads + memory). LEGAL.
#   2. A single OODA phase is a "reasoner" — never a phase-level "brain".
#   3. Nothing is named "Bridge" (any case) — the sole survivor is the frozen
#      JSON-RPC wire method literal "bridge.health".
#   4. The scheduler executive is `Brain`, never `Mind`.
#
# The gate scans for RETIRED IDENTIFIER TOKENS, not English prose. It runs
# alongside `cargo build` + `cargo test` + `mkdocs build --strict` so a green
# pipeline proves: no dangling old names.
#
# Exit codes: 0 = clean, 1 = drift found (offending lines printed).
#
# NOTE: This gate mirrors the pure-Rust scan in tests/terminology_drift.rs.
# Keep the two lists in sync when the migration map changes.
set -uo pipefail

# ── locate repo root (this script lives in scripts/ci/) ──────────────────────
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
ROOT="$(cd -- "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd -P)"
cd "${ROOT}" || exit 2

VIOLATIONS="$(mktemp)"
trap 'rm -f "${VIOLATIONS}"' EXIT

# Frozen wire values whose retired spelling legitimately survives (see the
# migration map's frozen-value allow-list). A line is exempt if, after these
# literals are stripped, no retired token remains.
FROZEN_CODE_CARVEOUTS='bridge\.health|FROZEN WIRE VALUE|SIMARD_MIND_MAX_NONCRITICAL_PER_TICK|"brain_judgments"'

# "Hive Mind" / "hive-mind" is a DISTINCT product concept (shared cross-agent
# memory in src/memory_hive.rs) — NOT the cognitive-thread scheduler. It keeps
# its name; only the scheduler `Mind` becomes `Brain`.
HIVE_CARVEOUT='[Hh]ive[ -][Mm]ind'

# Docs allowed to spell retired identifiers (a migration map and a changelog must).
DOC_ALLOWLIST='docs/reference/brain-terminology-migration.md|docs/whats-changed.md'

# Curated phase-level "brain" identifiers (retired). "brain" for the WHOLE
# cognition stays legal (Brain, BrainIntrospection*, brain_introspection,
# brain-model/executive/terminology docs, kept ooda-brain-*/recipe-brain-* FILENAMES).
PHASE_BRAIN_IDENTS=(
  'OodaBrain' 'OodaOrientBrain' 'OodaDecideBrain'
  'RustyClawdBrain' 'RustyClawdDecideBrain' 'RustyClawdOrientBrain'
  'DeterministicFallbackBrain' 'DeterministicLifecycleBrain'
  'DeterministicDecideBrain' 'DeterministicFallbackDecideBrain'
  'DeterministicOrientBrain' 'DeterministicFallbackOrientBrain'
  'RecipeBrain' 'RecipeEngineerLifecycleBrain'
  'BrainPhase' 'BrainParseSource' 'BrainResponseUnparseable'
  'BrainJudgmentRecord' 'BrainsLlmBackedProbe'
  'decide_brain' 'orient_brain' 'act_brain'
  'build_act_brain' 'build_decide_brain' 'build_orient_brain'
  'build_rustyclawd_brain' 'build_rustyclawd_orient_brain'
  'fallback_brain_count' 'FALLBACK_BRAIN_COUNT'
  'clear_brain_judgments' 'take_brain_judgments'
)

# Retired lowercase doc-link slugs that must vanish (renamed docs).
RETIRED_DOC_SLUGS=('bridge-pattern' 'bridge-wire-protocol' 'cognitive-memory-bridge-helpers')

fail=0
record() { # label, matches
  local label="$1"; local matches="$2"
  if [ -n "${matches}" ]; then
    fail=1
    {
      echo "### DRIFT: ${label}"
      printf '%s\n' "${matches}"
      echo
    } >>"${VIOLATIONS}"
  fi
}

# ── CODE scan: {src,tests}/**/*.{rs,py} ──────────────────────────────────────
# The two enforcement test files carry the denylist as data, so they are the
# only code excluded (analogous to the migration doc being allow-listed).
CODE_ROOTS=(src tests)
CODE_EXCLUDES=(--exclude='terminology_drift.rs' --exclude='frozen_wire_values.rs')

# (1) Total case-insensitive "bridge" ban (carve-outs stripped per line).
code_bridge="$(grep -rInE --include='*.rs' --include='*.py' "${CODE_EXCLUDES[@]}" -i 'bridge' "${CODE_ROOTS[@]}" 2>/dev/null \
  | grep -vE "${FROZEN_CODE_CARVEOUTS}" || true)"
record "code: retired 'Bridge' (only 'bridge.health' may survive)" "${code_bridge}"

# (2) Scheduler *type* `Mind` (capital-M word token). Case-sensitive: lowercase
#     "mind" is English prose ("keep in mind"); the type is always `Mind`.
code_mind="$(grep -rInE --include='*.rs' --include='*.py' "${CODE_EXCLUDES[@]}" '\bMind\b' "${CODE_ROOTS[@]}" 2>/dev/null \
  | grep -vE "${FROZEN_CODE_CARVEOUTS}" | grep -vE "${HIVE_CARVEOUT}" || true)"
record "code: retired scheduler name 'Mind' (rename to 'Brain')" "${code_mind}"

# (3) Curated phase-level "brain" identifiers.
brain_alt="$(IFS='|'; echo "${PHASE_BRAIN_IDENTS[*]}")"
code_brain="$(grep -rInE --include='*.rs' --include='*.py' "${CODE_EXCLUDES[@]}" "(${brain_alt})" "${CODE_ROOTS[@]}" 2>/dev/null \
  | grep -vE "${FROZEN_CODE_CARVEOUTS}" || true)"
record "code: retired phase-level 'brain' identifiers (rename to 'reasoner')" "${code_brain}"

# ── DOCS scan: docs/**/*.md + mkdocs.yml ─────────────────────────────────────
# Docs forbid retired IDENTIFIER TOKENS, not English prose (migration-map rule).
# A prose law statement that quotes a forbidden word to forbid it — e.g.
# `Nothing is ever named "Bridge"` or ``No `Bridge` in any name`` — is ALLOWED
# because the quoted word is not identifier-adjacent.
#
# (1) `Bridge`/`bridge` only when part of an identifier token (adjacent to an
#     identifier char). Catches `BridgeTransport`, `OodaBridges`, `memory_bridge`;
#     spares `"Bridge"`, `` `Bridge` ``, the frozen `bridge.health`, and prose.
doc_bridge="$(grep -rInE --include='*.md' '[A-Za-z0-9_]Bridge|Bridge[A-Za-z0-9_]|[a-z0-9_]bridge|bridge[a-z0-9_]' docs/ 2>/dev/null \
  | grep -vE "${DOC_ALLOWLIST}" || true)"
record "docs: retired 'Bridge' identifier token" "${doc_bridge}"

# (2) Retired lowercase doc-link slugs.
slug_alt="$(IFS='|'; echo "${RETIRED_DOC_SLUGS[*]}")"
doc_slug="$(grep -rInE --include='*.md' "(${slug_alt})" docs/ mkdocs.yml 2>/dev/null \
  | grep -vE "${DOC_ALLOWLIST}" || true)"
record "docs: retired doc-link slugs (renamed docs)" "${doc_slug}"

# (3) Standalone scheduler type `Mind` in docs (capital-M word token; Hive Mind spared).
doc_mind="$(grep -rInE --include='*.md' '\bMind\b' docs/ 2>/dev/null \
  | grep -vE "${DOC_ALLOWLIST}" | grep -vE "${HIVE_CARVEOUT}" || true)"
record "docs: retired scheduler name 'Mind'" "${doc_mind}"

# (4) Curated phase-level "brain" identifiers in docs (content reframed to reasoner).
doc_brain="$(grep -rInE --include='*.md' "(${brain_alt})" docs/ 2>/dev/null \
  | grep -vE "${DOC_ALLOWLIST}" || true)"
record "docs: retired phase-level 'brain' identifiers" "${doc_brain}"

# ── verdict ──────────────────────────────────────────────────────────────────
if [ "${fail}" -ne 0 ]; then
  echo "TERMINOLOGY DRIFT DETECTED — the unified Brain model is not yet coherent." >&2
  echo "See docs/reference/brain-terminology-migration.md for the old->new map." >&2
  echo >&2
  cat "${VIOLATIONS}" >&2
  exit 1
fi

echo "terminology gate: clean — no retired Bridge/Mind/phase-brain identifiers."
exit 0
