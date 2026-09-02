#!/usr/bin/env bash
# verify-docs.sh — Executable acceptance tests for the documentation audit (issue #2307).
#
# TDD gate for the "clean, update, and improve Simard docs to reflect current
# reality" goal. Each test encodes one acceptance criterion and prints PASS/FAIL.
# The script runs ALL tests (it does not stop at the first failure) and exits
# non-zero if any test fails, so CI and humans see the full picture at once.
#
# Scope: docs-only. This harness never mutates the repo; it only inspects the
# working tree, the git diff vs the base ref, and the native Rust
# docs-integrity gate.
#
# Usage:
#   scripts/verify-docs.sh                 # verify vs origin/main
#   BASE_REF=origin/main scripts/verify-docs.sh
#   SKIP_DOCS_INTEGRITY=1 scripts/verify-docs.sh   # skip the docs-integrity gate (T3)
#
# Env:
#   BASE_REF             base ref the docs branch was cut from (default: origin/main)
#   SKIP_DOCS_INTEGRITY  set to 1 to skip the native docs-integrity gate (T3)

set -uo pipefail

# --- Locate repo root --------------------------------------------------------
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 2
}
cd "$REPO_ROOT"

BASE_REF="${BASE_REF:-origin/main}"

# --- Result tracking ---------------------------------------------------------
FAILURES=0
PASSES=0

section() { printf '\n=== %s ===\n' "$1"; }
pass()    { PASSES=$((PASSES + 1));    printf '  PASS  %s\n' "$1"; }
fail()    { FAILURES=$((FAILURES + 1)); printf '  FAIL  %s\n' "$1"; }
info()    { printf '        %s\n' "$1"; }

# Resolve the base ref; if origin/main is not a local ref, fall back to HEAD~
# comparisons are skipped for scope tests but PRD/front-matter still run.
if ! git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
  echo "WARNING: base ref '$BASE_REF' not found locally; trying to fetch..." >&2
  git fetch --quiet origin main 2>/dev/null || true
fi
BASE_OK=0
if git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
  BASE_OK=1
fi

# Changed docs (vs base). Populated only when BASE_OK.
CHANGED_DOCS=()
if [ "$BASE_OK" -eq 1 ]; then
  while IFS= read -r f; do
    [ -n "$f" ] && CHANGED_DOCS+=("$f")
  done < <(git diff --name-only "$BASE_REF"...HEAD -- 'docs/**/*.md' 'docs/*.md' 2>/dev/null)
fi

# =============================================================================
# T1 — HARD CONSTRAINT: the PRD is preserved byte-for-byte.
# =============================================================================
section "T1  PRD preserved byte-for-byte (Specs/ProductArchitecture.md)"
PRD="Specs/ProductArchitecture.md"
if [ ! -s "$PRD" ]; then
  fail "PRD missing or empty at $PRD"
else
  pass "PRD present and non-empty"
  if [ "$BASE_OK" -eq 1 ]; then
    if git diff --quiet "$BASE_REF" -- "$PRD"; then
      pass "PRD is byte-for-byte identical to $BASE_REF"
    else
      fail "PRD DIFFERS from $BASE_REF — the PRD must not be edited"
      git --no-pager diff --stat "$BASE_REF" -- "$PRD" | sed 's/^/        /'
    fi
  else
    info "SKIP diff check: base ref $BASE_REF unavailable"
  fi
fi

# =============================================================================
# T2 — Change scope is docs-only (no source/behavior changes).
# =============================================================================
section "T2  Change scope is docs-only vs $BASE_REF"
if [ "$BASE_OK" -eq 1 ]; then
  OUT_OF_SCOPE=()
  while IFS= read -r path; do
    [ -z "$path" ] && continue
    case "$path" in
      docs/*) ;;
      mkdocs.yml) ;;
      .github/workflows/docs.yml) ;;
      scripts/verify-docs.sh) ;;
      *) OUT_OF_SCOPE+=("$path") ;;
    esac
  done < <(git diff --name-only "$BASE_REF"...HEAD)
  if [ "${#OUT_OF_SCOPE[@]}" -eq 0 ]; then
    pass "all changed paths are within docs-only allowlist"
  else
    fail "out-of-scope changes detected (docs-only constraint violated):"
    for p in "${OUT_OF_SCOPE[@]}"; do info "$p"; done
  fi
else
  info "SKIP: base ref $BASE_REF unavailable"
fi

# =============================================================================
# T3 — Native docs-integrity gate (link + nav integrity, Python-free).
#      Replaces the former `mkdocs build --strict` gate (issue #3181): the same
#      broken-link / broken-nav integrity now runs as a std-only Rust test,
#      `tests/docs_integrity.rs`, under `cargo test`. No Python `mkdocs`.
# =============================================================================
section "T3  native docs-integrity gate (tests/docs_integrity.rs)"
if [ "${SKIP_DOCS_INTEGRITY:-0}" = "1" ]; then
  info "SKIPPED via SKIP_DOCS_INTEGRITY=1"
else
  DI_LOG="$(mktemp)"
  if cargo test --test docs_integrity --quiet >"$DI_LOG" 2>&1; then
    pass "docs-integrity passed (0 broken links, 0 broken nav entries)"
  else
    fail "docs-integrity FAILED"
    grep -iE 'docs-integrity|dead|missing|panicked|FAILED' "$DI_LOG" | tail -30 | sed 's/^/        /'
  fi
  rm -f "$DI_LOG"
fi

# =============================================================================
# T4 — No orphaned docs: every docs/**/*.md is referenced in mkdocs.yml nav.
#      Explicit belt-and-suspenders check independent of the mkdocs config,
#      so flipping validation.nav.omitted_files to "ignore" cannot hide orphans.
# =============================================================================
section "T4  No orphaned docs (every docs/*.md appears in mkdocs.yml nav)"
if [ ! -f mkdocs.yml ]; then
  fail "mkdocs.yml not found"
else
  # Extract docs-relative *.md tokens referenced anywhere in mkdocs.yml.
  NAV_REFS="$(grep -oE '[A-Za-z0-9_./-]+\.md' mkdocs.yml | sort -u)"
  ORPHANS=()
  while IFS= read -r doc; do
    rel="${doc#docs/}"
    if ! printf '%s\n' "$NAV_REFS" | grep -qxF "$rel"; then
      ORPHANS+=("$rel")
    fi
  done < <(find docs -type f -name '*.md' | sort)
  if [ "${#ORPHANS[@]}" -eq 0 ]; then
    pass "0 orphaned pages — all docs reachable from nav"
  else
    fail "${#ORPHANS[@]} orphaned doc(s) not referenced in mkdocs.yml nav:"
    for o in "${ORPHANS[@]}"; do info "$o"; done
  fi
fi

# =============================================================================
# T5 — De-fork accuracy: the removed NativeCognitiveMemory symbol may appear in
#      docs ONLY when framed as history (de-fork #2307), never as current fact.
# =============================================================================
section "T5  Removed symbol NativeCognitiveMemory is framed as history"
STALE_SYMBOL="NativeCognitiveMemory"
MARKER='#2307|de-fork|de-forked|removed|deleted|superseded|no longer|former|formerly|historical|history|replaced|retired'
UNFRAMED=()
while IFS= read -r doc; do
  [ -z "$doc" ] && continue
  if ! grep -qiE "$MARKER" "$doc"; then
    UNFRAMED+=("$doc")
  fi
done < <(grep -rlF "$STALE_SYMBOL" docs --include='*.md' 2>/dev/null | sort)
MENTION_COUNT="$(grep -rlF "$STALE_SYMBOL" docs --include='*.md' 2>/dev/null | wc -l | tr -d ' ')"
if [ "${#UNFRAMED[@]}" -eq 0 ]; then
  pass "all $MENTION_COUNT page(s) mentioning $STALE_SYMBOL frame it as removed history"
else
  fail "${#UNFRAMED[@]} page(s) mention $STALE_SYMBOL without history framing:"
  for u in "${UNFRAMED[@]}"; do info "$u"; done
fi

# =============================================================================
# T6 — Source-claim verification: the reality the docs describe holds in src.
#      Guards against docs referencing deleted things or missing shipped ones.
# =============================================================================
section "T6  Documented claims verify against the source tree"

assert_path_exists() {  # $1 path  $2 description
  if [ -e "$1" ]; then pass "$2 ($1 exists)"; else fail "$2 — expected path missing: $1"; fi
}
assert_grep_present() { # $1 pattern  $2 pathspec  $3 description
  if git grep -qE "$1" -- $2 2>/dev/null; then pass "$3"; else fail "$3 — pattern not found: /$1/ in $2"; fi
}
assert_grep_absent() {  # $1 pattern  $2 pathspec  $3 description
  if git grep -qE "$1" -- $2 2>/dev/null; then fail "$3 — pattern unexpectedly present: /$1/ in $2"; else pass "$3"; fi
}

# self-deploy (#2467) and self-relaunch
assert_path_exists "src/self_deploy"            "self-deploy module present (#2467)"
assert_path_exists "src/self_relaunch"          "self-relaunch module present"
# shared recipe_output extractor chokepoint (#2504 launch-preamble stripping)
assert_path_exists "src/recipe_output/extract.rs" "shared recipe_output extractor present (#2504)"
# base-type adapters shipping
assert_grep_present "base_type_rustyclawd" "src"       "rusty-clawd base-type adapter present"
assert_grep_present "base_type_copilot"    "src"       "copilot-sdk base-type adapter present"
# goal-board cross-process write-lock (#2511 / #2514)
assert_grep_present "libc::flock" "src/goals/store.rs" "goal-store cross-process flock present (#2511/#2514)"
# de-fork #2307: native fork deleted, amplihack-memory-lib + lbug 0.17.1 is sole backend
assert_grep_absent  "struct[[:space:]]+NativeCognitiveMemory" "src" "NativeCognitiveMemory fork deleted from src (#2307)"
assert_grep_present "amplihack-memory"   "Cargo.toml"          "amplihack-memory-lib dependency pinned"
assert_grep_present 'lbug[[:space:]]*=[[:space:]]*"=0\.17\.1"' "Cargo.toml" "lbug pinned to =0.17.1"
# RecipeBrain abstraction present
assert_grep_present "struct[[:space:]]+RecipeBrain" "src" "RecipeBrain abstraction present"

# =============================================================================
# T7 — Front-matter correctness on every touched doc page.
#      Requires last_updated (ISO date), owner, and doc_type. The site home
#      (docs/index.md) is exempt from doc_type by Diataxis landing-page convention.
# =============================================================================
section "T7  Front-matter on touched pages (last_updated, owner, doc_type)"
if [ "$BASE_OK" -eq 1 ]; then
  if [ "${#CHANGED_DOCS[@]}" -eq 0 ]; then
    info "no changed docs vs $BASE_REF"
  fi
  FM_BAD=0
  for f in "${CHANGED_DOCS[@]}"; do
    [ -f "$f" ] || continue   # skip deleted files
    problems=""
    # Front-matter must be a leading YAML block delimited by --- on line 1.
    if [ "$(head -1 "$f")" != "---" ]; then
      problems="${problems} missing-frontmatter-block"
    else
      grep -qE '^last_updated:[[:space:]]*[0-9]{4}-[0-9]{2}-[0-9]{2}' "$f" || problems="${problems} last_updated"
      grep -qE '^owner:[[:space:]]*\S' "$f" || problems="${problems} owner"
      if [ "$f" != "docs/index.md" ]; then
        grep -qE '^doc_type:[[:space:]]*\S' "$f" || problems="${problems} doc_type"
      fi
    fi
    if [ -n "$problems" ]; then
      fail "$f — missing/invalid:${problems}"
      FM_BAD=$((FM_BAD + 1))
    fi
  done
  if [ "$FM_BAD" -eq 0 ] && [ "${#CHANGED_DOCS[@]}" -gt 0 ]; then
    pass "all ${#CHANGED_DOCS[@]} touched page(s) have valid front-matter"
  fi
else
  info "SKIP: base ref $BASE_REF unavailable"
fi

# =============================================================================
# Summary
# =============================================================================
section "Summary"
printf '  %d passed, %d failed\n' "$PASSES" "$FAILURES"
if [ "$FAILURES" -gt 0 ]; then
  printf '\nDOCS VERIFICATION FAILED (%d check[s]).\n' "$FAILURES"
  exit 1
fi
printf '\nDOCS VERIFICATION PASSED.\n'
exit 0
