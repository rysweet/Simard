#!/usr/bin/env bash
# pr-illustrated-guide-skill.sh — qa-team contract check for the
# `pr-illustrated-guide` declarative skill (issue amplihack-rs#810).
#
# The skill is documentation an agent follows, so the outside-in test asserts
# the documented contract is complete: required frontmatter, the ten procedure
# steps, the configurable filter constants, dual-platform support, deep-link
# formats, conditional screenshots with announced degradation, the output
# contract, and the default-workflow integration hook.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SKILL_DIR="amplifier-bundle/skills/pr-illustrated-guide"
SKILL="$SKILL_DIR/SKILL.md"
README="$SKILL_DIR/README.md"

fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }

# Assert a fixed (literal) string is present in a file.
must_contain() {
  local file="$1" needle="$2"
  grep -Fq -- "$needle" "$file" || fail "missing in $file: $needle"
}

# Assert a case-insensitive extended-regex pattern matches in a file.
must_match() {
  local file="$1" pattern="$2"
  grep -Eiq -- "$pattern" "$file" || fail "pattern not found in $file: $pattern"
}

# ── Files exist ──────────────────────────────────────────────────────────────
[ -f "$SKILL" ]  || fail "SKILL.md not found at $SKILL"
[ -f "$README" ] || fail "README.md not found at $README"

# ── Rich frontmatter ─────────────────────────────────────────────────────────
must_match   "$SKILL" '^name:[[:space:]]*pr-illustrated-guide'
must_match   "$SKILL" '^version:[[:space:]]*1\.0\.0'
must_match   "$SKILL" '^description:'
must_match   "$SKILL" '^invokes:'
must_contain "$SKILL" "mermaid-diagram-generator"   # invoked skill
must_contain "$SKILL" "visualization-architect"     # invoked agent

# ── Step 1: platform detection (GitHub gh vs Azure DevOps az) ────────────────
must_contain "$SKILL" "git remote get-url origin"
must_contain "$SKILL" "github.com"
must_match   "$SKILL" 'dev\.azure\.com|visualstudio\.com'
must_contain "$SKILL" "azure-devops"

# ── Step 2: trivial-PR filter, OR-logic, named overridable constants ─────────
must_contain "$SKILL" "MIN_FILES_CHANGED"
must_contain "$SKILL" "MIN_LINES_CHANGED"
must_contain "$SKILL" "TRIVIAL_PATH_GLOBS"
must_match   "$SKILL" 'MIN_FILES_CHANGED.*3'
must_match   "$SKILL" 'MIN_LINES_CHANGED.*30'
must_match   "$SKILL" 'OR'                          # OR logic
must_match   "$SKILL" 'skip'                        # explicit skip notice
must_match   "$SKILL" 'overridable|override'        # constants are overridable

# ── Step 3: metadata + diff fetch on both platforms ──────────────────────────
must_contain "$SKILL" "gh pr view"
must_contain "$SKILL" "gh pr diff"
must_contain "$SKILL" "az repos pr show"
must_match   "$SKILL" 'linked issue|work item'

# ── Step 4: three-part document structure + mermaid delegation ───────────────
must_match "$SKILL" 'what problem'                  # problem
must_match "$SKILL" 'approach'                      # approach
must_match "$SKILL" 'step-by-step'                  # step-by-step
must_match "$SKILL" 'code snippet'
must_match "$SKILL" 'mermaid'

# ── Step 5/6: plain language + smart exemplar content ────────────────────────
must_match "$SKILL" 'plain language|jargon-free'
must_match "$SKILL" 'exemplar'
must_match "$SKILL" 'configurable constant'
must_match "$SKILL" 'non-obvious decision'

# ── Step 7: deep links for both GitHub and Azure DevOps ──────────────────────
must_contain "$SKILL" "/pull/"                      # GitHub anchor
must_match   "$SKILL" 'diff-.*R'                    # GitHub line anchor
must_contain "$SKILL" "pullrequest/"                # ADO anchor
must_match   "$SKILL" 'line='                       # ADO line param

# ── Step 8: conditional GUI/TUI screenshots with announced graceful skip ─────
must_contain "$SKILL" "Playwright"
must_match   "$SKILL" 'asciinema|terminal capture'
must_match   "$SKILL" 'screenshots unavailable|not applicable'
must_match   "$SKILL" 'never silent|visibly|announced'

# ── Step 9: markdown file + stdout, optional posting ─────────────────────────
must_contain "$SKILL" ".amplihack/pr-illustrated-guide.md"
must_match   "$SKILL" 'stdout'
must_contain "$SKILL" "gh pr comment"
must_match   "$SKILL" 'az devops invoke|pullrequestthreads|threads api'

# ── Step 10: integration hook, no recipe YAML changes ────────────────────────
must_contain "$SKILL" "default-workflow"
must_match   "$SKILL" 'phase 5/6|phase 5|phase 6'
must_match   "$SKILL" 'no recipe yaml'

# ── README mirrors key surfaces ──────────────────────────────────────────────
must_contain "$README" "pr-illustrated-guide"
must_contain "$README" "MIN_FILES_CHANGED"
must_match   "$README" 'github'
must_match   "$README" 'azure devops'

echo "PASS: pr-illustrated-guide skill contract satisfied"
