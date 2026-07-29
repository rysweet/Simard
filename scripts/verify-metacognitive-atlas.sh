#!/usr/bin/env bash
# verify-metacognitive-atlas.sh — Executable acceptance tests (TDD) for the
# code-derived metacognitive atlas deliverable (issue #4982).
#
# Each test encodes ONE acceptance criterion from the approved design spec and
# prints PASS/FAIL. The harness runs ALL tests (it never stops at the first
# failure) and exits non-zero if any fail, so CI and humans see the full picture
# at once. It mirrors the style of the sibling `scripts/verify-docs.sh` gate.
#
# Scope: docs-only inspection. This harness NEVER mutates the repo; it only
# reads the working tree, the source of truth in `src/**`, and (when a base ref
# is available) the git diff to enforce the additive-only constraint.
#
# Contract (what "done" means for this deliverable):
#   A1  Atlas page exists, is non-empty, and carries valid front-matter.
#   A2  Atlas renders exactly 5 inline Mermaid diagrams.
#   A3  All 5 Graphviz `.dot` sources exist and are well-formed digraphs.
#   A4  The 13-thread roster in the diagrams matches `ThreadName::ALL` in source
#       (code-first: no drift, no missing/extra threads).
#   A5  The Overseer layer is labeled a DESIGN SKETCH and drawn dashed — never
#       implied to be a live, wired-into-main loop.
#   A6  The metacognition data-flow carries the full canonical path tokens.
#   A7  Reciprocal cross-links exist between the atlas and the model doc
#       (both front-matter `related:` and an in-body "See also" link).
#   A8  The atlas is reachable from the mkdocs nav (no orphan page).
#   A9  Additive-only: the PRD is byte-for-byte preserved; changes stay within
#       the docs/mkdocs allowlist; no forbidden constructs are introduced.
#
# Usage:
#   scripts/verify-metacognitive-atlas.sh
#   BASE_REF=origin/main scripts/verify-metacognitive-atlas.sh

set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 2
}
cd "$REPO_ROOT"

BASE_REF="${BASE_REF:-origin/main}"

# --- Paths under test --------------------------------------------------------
ATLAS="docs/architecture/metacognitive-atlas.md"
MODEL="docs/architecture/metacognitive-model.md"
DIAGRAMS_DIR="docs/architecture/diagrams"
DOT_FILES=(
  "$DIAGRAMS_DIR/system-map.dot"
  "$DIAGRAMS_DIR/thread-drilldown.dot"
  "$DIAGRAMS_DIR/ooda-loop.dot"
  "$DIAGRAMS_DIR/overseer-sketch.dot"
  "$DIAGRAMS_DIR/metacognition-flow.dot"
)
ROSTER_SRC="src/ooda_brain/thread_reasoning_record.rs"

# --- Result tracking ---------------------------------------------------------
FAILURES=0
PASSES=0
section() { printf '\n=== %s ===\n' "$1"; }
pass()    { PASSES=$((PASSES + 1));    printf '  PASS  %s\n' "$1"; }
fail()    { FAILURES=$((FAILURES + 1)); printf '  FAIL  %s\n' "$1"; }
info()    { printf '        %s\n' "$1"; }

BASE_OK=0
if git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
  BASE_OK=1
fi

# =============================================================================
# A1 — Atlas page exists, is non-empty, and has valid front-matter.
# =============================================================================
section "A1  Atlas page present with valid front-matter ($ATLAS)"
if [ ! -s "$ATLAS" ]; then
  fail "atlas page missing or empty at $ATLAS"
else
  pass "atlas page present and non-empty"
  if [ "$(head -1 "$ATLAS")" != "---" ]; then
    fail "atlas is missing a leading YAML front-matter block"
  else
    for key in title last_updated owner doc_type; do
      if grep -qE "^${key}:[[:space:]]*\S" "$ATLAS"; then
        pass "front-matter has '$key'"
      else
        fail "front-matter missing '$key'"
      fi
    done
    if grep -qE '^last_updated:[[:space:]]*[0-9]{4}-[0-9]{2}-[0-9]{2}' "$ATLAS"; then
      pass "front-matter last_updated is an ISO date"
    else
      fail "front-matter last_updated is not an ISO (YYYY-MM-DD) date"
    fi
  fi
fi

# =============================================================================
# A2 — Exactly five inline Mermaid diagrams (one per required diagram).
# =============================================================================
section "A2  Atlas renders exactly 5 inline Mermaid diagrams"
if [ -f "$ATLAS" ]; then
  MERMAID_COUNT="$(grep -c '```mermaid' "$ATLAS")"
  if [ "$MERMAID_COUNT" -eq 5 ]; then
    pass "found exactly 5 mermaid fences"
  else
    fail "expected 5 mermaid fences, found $MERMAID_COUNT"
  fi
  # Fences must be balanced (opening ``` count is even).
  FENCE_TOTAL="$(grep -cE '^```' "$ATLAS")"
  if [ $(( FENCE_TOTAL % 2 )) -eq 0 ]; then
    pass "code fences are balanced ($FENCE_TOTAL delimiters)"
  else
    fail "unbalanced code fences ($FENCE_TOTAL delimiters) — will break rendering"
  fi
else
  fail "cannot check mermaid: atlas missing"
fi

# =============================================================================
# A3 — All five .dot sources exist and are well-formed digraphs.
# =============================================================================
section "A3  Five Graphviz .dot sources exist and are well-formed"
for dot in "${DOT_FILES[@]}"; do
  if [ ! -s "$dot" ]; then
    fail "missing/empty .dot source: $dot"
    continue
  fi
  if ! grep -qE '\bdigraph\b' "$dot"; then
    fail "$dot does not declare a 'digraph'"
    continue
  fi
  opens="$(tr -cd '{' < "$dot" | wc -c | tr -d ' ')"
  closes="$(tr -cd '}' < "$dot" | wc -c | tr -d ' ')"
  if [ "$opens" != "$closes" ]; then
    fail "$dot has unbalanced braces ({=$opens, }=$closes)"
    continue
  fi
  pass "$(basename "$dot") is a well-formed digraph"
done
# If the Graphviz `dot` binary is available, run its real parser (CI-safe: only
# when present — the .dot source is the mandatory deliverable, not the SVG).
if command -v dot >/dev/null 2>&1; then
  for dot in "${DOT_FILES[@]}"; do
    [ -f "$dot" ] || continue
    if dot -Tcanon "$dot" >/dev/null 2>&1; then
      pass "graphviz parses $(basename "$dot")"
    else
      fail "graphviz FAILED to parse $dot"
    fi
  done
else
  info "SKIP graphviz parse: 'dot' binary not installed (source is the deliverable)"
fi

# =============================================================================
# A4 — The 13-thread roster matches ThreadName::ALL in source (code-first).
# =============================================================================
section "A4  Diagram thread roster matches ThreadName::ALL in $ROSTER_SRC"
if [ ! -f "$ROSTER_SRC" ]; then
  fail "roster source of truth not found: $ROSTER_SRC"
else
  # Derive the snake_case labels straight from the label() match arms so the
  # test tracks source drift automatically rather than hard-coding names. Scope
  # extraction to the `fn label(self)` body so unrelated arms (e.g. the
  # `expected_domain()` "notes" bucket) are never miscounted as threads.
  LABEL_FN_BODY="$(awk '
      /pub fn label\(self\)/ {inb=1}
      inb {print}
      inb && /^    }/ && seen {exit}
      inb && /match self/ {seen=1}
    ' "$ROSTER_SRC")"
  LABELS="$(printf '%s\n' "$LABEL_FN_BODY" \
              | grep -oE 'Self::[A-Za-z]+ => "[a-z_]+"' \
              | sed -E 's/.*"([a-z_]+)".*/\1/' | sort -u)"
  LABEL_COUNT="$(printf '%s\n' "$LABELS" | grep -c .)"
  if [ "$LABEL_COUNT" -ge 13 ]; then
    pass "derived $LABEL_COUNT snake_case thread labels from source"
  else
    fail "expected >=13 thread labels from source, derived $LABEL_COUNT"
  fi
  # Confirm the closed roster size the atlas claims equals the source array.
  if grep -qE 'pub const ALL: \[ThreadName; 13\]' "$ROSTER_SRC"; then
    pass "source declares 'pub const ALL: [ThreadName; 13]'"
  else
    fail "source no longer declares a 13-element ThreadName::ALL (roster drift)"
  fi
  DRILL="$DIAGRAMS_DIR/thread-drilldown.dot"
  MISS_DOT=0
  MISS_ATLAS=0
  while IFS= read -r label; do
    [ -z "$label" ] && continue
    if [ -f "$DRILL" ] && ! grep -qw "$label" "$DRILL"; then
      fail "thread '$label' missing from thread-drilldown.dot"
      MISS_DOT=$((MISS_DOT + 1))
    fi
    if [ -f "$ATLAS" ] && ! grep -qw "$label" "$ATLAS"; then
      fail "thread '$label' missing from atlas page"
      MISS_ATLAS=$((MISS_ATLAS + 1))
    fi
  done <<< "$LABELS"
  [ "$MISS_DOT" -eq 0 ]   && pass "all $LABEL_COUNT threads present in thread-drilldown.dot"
  [ "$MISS_ATLAS" -eq 0 ] && pass "all $LABEL_COUNT threads present in the atlas page"
fi

# =============================================================================
# A5 — Overseer is labeled a DESIGN SKETCH and drawn dashed (never live).
# =============================================================================
section "A5  Overseer rendered as an unwired DESIGN SKETCH (dashed)"
OVERSEER_DOT="$DIAGRAMS_DIR/overseer-sketch.dot"
if [ -f "$OVERSEER_DOT" ]; then
  if grep -qi 'dashed' "$OVERSEER_DOT"; then
    pass "overseer-sketch.dot uses dashed styling"
  else
    fail "overseer-sketch.dot is not drawn dashed"
  fi
  if grep -qiE 'design sketch|not wired|dead_code' "$OVERSEER_DOT"; then
    pass "overseer-sketch.dot labels itself an unwired design sketch"
  else
    fail "overseer-sketch.dot lacks a 'design sketch / not wired' label"
  fi
else
  fail "overseer-sketch.dot missing"
fi
# The atlas prose must also frame Overseer as an unwired sketch.
if [ -f "$ATLAS" ] && grep -qiE 'design sketch' "$ATLAS" \
   && grep -qiE 'not wired' "$ATLAS"; then
  pass "atlas prose frames Overseer as an unwired design sketch"
else
  fail "atlas prose does not clearly frame Overseer as an unwired sketch"
fi
# The claim is grounded in source: overseer module carries #![allow(dead_code)].
if grep -qE '#!\[allow\(dead_code\)\]' src/overseer/mod.rs 2>/dev/null; then
  pass "source claim verified: src/overseer/mod.rs has #![allow(dead_code)]"
else
  fail "src/overseer/mod.rs no longer has #![allow(dead_code)] — re-check the sketch framing"
fi

# =============================================================================
# A6 — Metacognition data-flow carries the full canonical path tokens.
# =============================================================================
section "A6  Metacognition flow encodes recipe -> record -> rail -> summary"
FLOW_DOT="$DIAGRAMS_DIR/metacognition-flow.dot"
FLOW_TOKENS=(
  "RecipeRunnerInvoker"
  "run_reflective_thread"
  "ThreadReasoningRecord"
  "thread-reasoning/v1"
  "rail"
  "ThreadOutcome"
)
if [ -f "$FLOW_DOT" ]; then
  for tok in "${FLOW_TOKENS[@]}"; do
    if grep -qF "$tok" "$FLOW_DOT"; then
      pass "flow contains '$tok'"
    else
      fail "metacognition-flow.dot missing token '$tok'"
    fi
  done
else
  fail "metacognition-flow.dot missing"
fi

# =============================================================================
# A7 — Reciprocal cross-links between the atlas and the model doc.
# =============================================================================
section "A7  Reciprocal cross-links (atlas <-> model)"
if [ -f "$ATLAS" ] && grep -qF "metacognitive-model.md" "$ATLAS"; then
  pass "atlas links to metacognitive-model.md"
else
  fail "atlas does not link to metacognitive-model.md"
fi
if [ -f "$MODEL" ] && grep -qF "metacognitive-atlas.md" "$MODEL"; then
  pass "model doc links back to metacognitive-atlas.md"
else
  fail "model doc does not link back to metacognitive-atlas.md"
fi
# Both directions must also appear in the front-matter `related:` block.
if [ -f "$ATLAS" ] && awk '/^---$/{n++} n==1 && /related:/{f=1} n==1 && f && /metacognitive-model\.md/{print; exit}' "$ATLAS" | grep -q .; then
  pass "atlas front-matter 'related:' references the model doc"
else
  fail "atlas front-matter 'related:' does not reference the model doc"
fi
if [ -f "$MODEL" ] && awk '/^---$/{n++} n==1 && /related:/{f=1} n==1 && f && /metacognitive-atlas\.md/{print; exit}' "$MODEL" | grep -q .; then
  pass "model front-matter 'related:' references the atlas"
else
  fail "model front-matter 'related:' does not reference the atlas"
fi

# =============================================================================
# A8 — Atlas is reachable from the mkdocs nav (no orphan page).
# =============================================================================
section "A8  Atlas wired into mkdocs.yml nav (no orphan)"
if [ -f mkdocs.yml ] && grep -qF "architecture/metacognitive-atlas.md" mkdocs.yml; then
  pass "mkdocs.yml nav references the atlas page"
else
  fail "mkdocs.yml nav does not reference architecture/metacognitive-atlas.md"
fi

# =============================================================================
# A9 — Additive-only constraint.
# =============================================================================
section "A9  Additive-only constraint"
PRD="Specs/ProductArchitecture.md"
if [ "$BASE_OK" -eq 1 ]; then
  if git diff --quiet "$BASE_REF" -- "$PRD"; then
    pass "PRD byte-for-byte preserved vs $BASE_REF"
  else
    fail "PRD DIFFERS from $BASE_REF — must not be edited"
  fi
  OUT_OF_SCOPE=()
  while IFS= read -r path; do
    [ -z "$path" ] && continue
    case "$path" in
      docs/*) ;;
      mkdocs.yml) ;;
      scripts/verify-metacognitive-atlas.sh) ;;
      tests/metacognitive_atlas.rs) ;;
      *) OUT_OF_SCOPE+=("$path") ;;
    esac
  done < <(git diff --name-only "$BASE_REF"...HEAD)
  if [ "${#OUT_OF_SCOPE[@]}" -eq 0 ]; then
    pass "all changed paths are within the docs/test allowlist"
  else
    fail "out-of-scope changes detected (additive-only violated):"
    for p in "${OUT_OF_SCOPE[@]}"; do info "$p"; done
  fi
else
  info "SKIP diff checks: base ref $BASE_REF unavailable"
fi
# Forbidden constructs must not appear in the new artifacts.
NEW_ARTIFACTS=("$ATLAS" "${DOT_FILES[@]}")
if grep -RInE 'Bridge' "${NEW_ARTIFACTS[@]}" >/dev/null 2>&1; then
  fail "forbidden 'Bridge' naming present in a new artifact"
else
  pass "no forbidden 'Bridge' naming in new artifacts"
fi

# =============================================================================
# Summary
# =============================================================================
section "Summary"
printf '  %d passed, %d failed\n' "$PASSES" "$FAILURES"
if [ "$FAILURES" -gt 0 ]; then
  printf '\nMETACOGNITIVE-ATLAS VERIFICATION FAILED (%d check[s]).\n' "$FAILURES"
  exit 1
fi
printf '\nMETACOGNITIVE-ATLAS VERIFICATION PASSED.\n'
exit 0
