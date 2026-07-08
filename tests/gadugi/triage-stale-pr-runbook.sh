#!/usr/bin/env bash
# qa-team scenario for issue #810 — stale-PR triage runbook contract.
#
# Outside-in (TDD) verification that the evergreen runbook
# docs/howto/triage-stale-pull-requests.md encodes the ratified merge-gate
# behaviour and stays in sync with the LIVE CI workflow definitions it
# describes. The validation surface for a documentation deliverable is the doc
# text plus the workflow YAMLs it must mirror, so this scenario asserts directly
# against the checked-in files (it never reads a deployed copy).
#
# It pins, in particular, the two corrections the architect review demanded:
#   1. Check-name accuracy — the runbook must classify the ACTUAL CI rollup
#      checks enumerated live from the workflow YAMLs: `pre-commit`,
#      `cargo-audit`, `install-real`, `e2e-dashboard` (verify.yml), `coverage`
#      (coverage.yml), and `build` — the `mkdocs build --strict` job from
#      docs.yml that fires on docs/`mkdocs.yml`/`Specs/**` PRs. It must NOT
#      invent a Rust `fmt` job (fmt/clippy/compile run inside `pre-commit`) or
#      carry the mislabeled `lbug-clippy` prefix, and it must NOT deny the real
#      `build` check that docs.yml contributes on documentation PRs.
#   2. Honest gate framing — the runbook must describe the `gh pr merge` env-red
#      path as an AUDITED OPERATOR OVERRIDE layered on top of the gate, never as
#      a tooling-sanctioned bypass (the deterministic gate refuses on any
#      non-green check and never silently falls back).
# It also enforces durable doc hygiene: mkdocs nav registration, valid
# front-matter, real merge commands, the six merge-ready criteria, the evergreen
# "no point-in-time snapshot" invariant, and — the bug this suite was written to
# catch first — that every in-page anchor link actually resolves to a heading.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

DOC="docs/howto/triage-stale-pull-requests.md"
MKDOCS="mkdocs.yml"
VERIFY=".github/workflows/verify.yml"
COVERAGE=".github/workflows/coverage.yml"
DOCS=".github/workflows/docs.yml"

PASS=0
fail() {
  echo "[gadugi] FAIL: $1" >&2
  exit 1
}
ok() {
  PASS=$((PASS + 1))
  echo "[gadugi] ok: $1"
}
has() {  # has <needle> <message>  — fixed-string presence in the doc
  grep -Fq -- "$1" "$DOC" || fail "$2 (missing literal: $1)"
}
hasre() {  # hasre <regex> <message> — extended-regex presence in the doc
  grep -Eq -- "$1" "$DOC" || fail "$2 (missing pattern: $1)"
}
lacks() {  # lacks <needle> <message> — fixed-string MUST be absent
  if grep -Fq -- "$1" "$DOC"; then fail "$2 (forbidden literal present: $1)"; fi
}

# --- Preconditions -----------------------------------------------------------
[ -f "$DOC" ]      || fail "$DOC not found"
[ -f "$MKDOCS" ]   || fail "$MKDOCS not found"
[ -f "$VERIFY" ]   || fail "$VERIFY (CI source of truth) not found"
[ -f "$COVERAGE" ] || fail "$COVERAGE (CI source of truth) not found"
[ -f "$DOCS" ]     || fail "$DOCS (CI source of truth) not found"
ok "all ground-truth files present"

# --- Group 1: registration + front-matter -----------------------------------
grep -Eq "howto/triage-stale-pull-requests\.md" "$MKDOCS" \
  || fail "$DOC is not registered in the mkdocs nav ($MKDOCS)"
ok "doc is registered in the mkdocs nav"

head -1 "$DOC" | grep -Eq '^---$' \
  || fail "$DOC does not open with a YAML front-matter fence"
# Extract the front-matter block once, then validate keys against it in memory
# (avoids re-scanning the whole doc with a fresh awk pass for every key).
FRONT_MATTER="$(awk 'NR>1 && /^---$/{exit} NR>1' "$DOC")"
for key in title description owner doc_type; do
  grep -Eq "^${key}:" <<< "$FRONT_MATTER" \
    || fail "$DOC front-matter missing required key '${key}:'"
done
# Validate the front-matter block structurally with awk (native, no Python):
# there must be a closing fence and every non-blank line must be a YAML mapping
# key or list item. (The required keys are already asserted above.)
[ "$(grep -c '^---$' "$DOC")" -ge 2 ] || fail "front-matter has no closing '---' fence"
awk '
  /^[[:space:]]*$/ { next }
  /^#/ { next }
  /^[A-Za-z0-9_.-]+:([[:space:]].*)?$/ { next }
  /^[[:space:]]+- / { next }
  /^[[:space:]]+[A-Za-z0-9_.-]+:([[:space:]].*)?$/ { next }
  { print "invalid front-matter line: " $0 > "/dev/stderr"; exit 1 }
' <<< "$FRONT_MATTER" || fail "front-matter is not valid YAML"
ok "front-matter is valid YAML with the required keys"

# --- Group 2: check-name accuracy, derived from the LIVE workflows -----------
# The set of real CI checks is read from the workflow files themselves so the
# runbook fails this gate the moment CI and the doc drift apart. Job names are
# the 2-space-indented keys under `jobs:` — enumerated with awk (native, no
# Python).
enum_jobs() {
  awk '
    /^jobs:[[:space:]]*$/ { inj=1; next }
    inj && /^[^[:space:]]/ { inj=0 }
    inj && /^  [A-Za-z0-9_-]+:/ { l=$0; sub(/^  /,"",l); sub(/:.*/,"",l); print l }
  ' "$1"
}
LIVE_CHECKS="$(enum_jobs "$VERIFY"; enum_jobs "$COVERAGE"; enum_jobs "$DOCS")"
[ -n "$LIVE_CHECKS" ] || fail "could not enumerate live CI job names from the workflows"
while IFS= read -r check; do
  [ -n "$check" ] || continue
  has "$check" "runbook does not mention the live CI check '$check'"
done <<< "$LIVE_CHECKS"
ok "runbook mentions every live CI check ($(echo "$LIVE_CHECKS" | tr '\n' ' '))"

# Environmental (candidate non-blocking) set is EXACTLY cargo-audit + install-real.
has "cargo-audit" "runbook must name cargo-audit as a candidate environmental red"
has "install-real" "runbook must name install-real as a candidate environmental red"
# pre-commit must be classified as a REAL hard gate, never environmental.
hasre "pre-commit.*(real|hard gate)|(real|hard gate).*pre-commit" \
  "runbook must classify pre-commit as a REAL hard gate"
# Correction #1 (refined): the runbook must (a) explain that there is no SEPARATE
# Rust build/fmt check — fmt/clippy/compile all run inside `pre-commit` — and
# (b) correctly document the docs.yml `build` job (`mkdocs build --strict`) that
# DOES surface as a real gate on docs/`Specs` PRs. The mislabeled `lbug-clippy`
# prefix must be gone, and the doc must NOT deny the real docs `build` check.
lacks "lbug-clippy" "stale 'lbug-clippy' check name must not appear"
lacks "no \`build\` or \`fmt\` check" \
  "runbook must not deny the real docs.yml 'build' check"
hasre "[Nn]o separate Rust .build. or .fmt." \
  "runbook must state there is no SEPARATE Rust build/fmt check (folded into pre-commit)"
has "docs.yml" "runbook must attribute the 'build' check to docs.yml"
hasre "mkdocs build --strict" \
  "runbook must document the docs.yml 'build' job as 'mkdocs build --strict'"
ok "check-name classification matches the live workflows incl. docs.yml build (correction #1)"

# --- Group 3: honest operator-override framing (correction #2) ---------------
hasre "never silently fall back|never silently falls back" \
  "runbook must state the merge authority never silently falls back"
hasre "no flag, env var, or allow-list|There is no flag" \
  "runbook must state no flag/env/allow-list bypasses the gate"
has "operator override" "runbook must frame the gh pr merge env-red path as an operator override"
hasre "audited (human )?override|audited human override" \
  "runbook must call the override audited"
hasre "record(s|ed|ing)? .*(PR comment|decision)|PR comment" \
  "runbook must require recording the override decision in a PR comment"
# The override is explicitly NOT a path past a real failure / conflict / pending.
hasre "never.*real fail|not .*real fail|never a way past a real" \
  "runbook must state the override is never a path past a real failure"
ok "gate framing is an audited operator override, not a tooling bypass (correction #2)"

# --- Group 4: real commands, six criteria, disposition surface ---------------
has "simard merge-pr" "runbook must use 'simard merge-pr' for Simard merges"
has "gh pr merge" "runbook must reference 'gh pr merge'"
has "--squash --delete-branch" "runbook must use squash + delete-branch for cross-repo merges"
has "gh pr checkout" "runbook must isolate work via 'gh pr checkout' (existing branch only)"
has "gh pr close" "runbook must document the CLOSE path via 'gh pr close'"
has "gh pr comment" "runbook must document the LEAVE-OPEN triage-comment path"
# The six MERGE-READY evidence criteria must all be named.
for crit in "QA-team" "Documentation" "Quality-audit" "Scope" "Verdict"; do
  has "$crit" "six merge-ready criteria incomplete — missing '$crit'"
done
hasre "three .*(cycle|SEEK)|at least .*three" \
  "Quality-audit criterion must require at least three clean cycles"
ok "real merge commands + the six merge-ready criteria are present"

# --- Group 5: evergreen + conservative-safety invariants ---------------------
hasre "no point-in-time|never .*snapshot|re-?enumerate|Live enumeration is authoritative" \
  "runbook must be evergreen (live enumeration; no point-in-time snapshot)"
hasre "never push from .*(dirty|top-level)|never push from a dirty" \
  "runbook must forbid pushing from the dirty/shared tree"
hasre "never .*force-merge|No force-merge|never merge past a .*real" \
  "runbook must forbid force-merging past a real failure/conflict"
hasre "48[ -]?h(our)?s?" \
  "runbook must define the in-flight (do-not-touch) window"
ok "evergreen + conservative-safety invariants are stated"

# --- Group 6: internal anchors resolve (the bug this suite catches first) -----
# Every in-page (#slug) link must point at a real heading. Slugs are computed
# with the same default algorithm mkdocs' toc extension uses (awk port, native,
# no Python), so this matches the rendered HTML ids exactly.
awk '
function slugify(s,   t) {
  t=s
  gsub(/[^A-Za-z0-9_ \t-]/,"",t)
  gsub(/^[ \t]+|[ \t]+$/,"",t)
  t=tolower(t)
  gsub(/[- \t]+/,"-",t)
  return t
}
{
  line=$0
  stripped=line; sub(/^[ \t]+/,"",stripped)
  if (stripped ~ /^```/) { infence=!infence; next }
  if (infence) next
  if (match(line, /^#{1,6}[ \t]+/)) {
    h=substr(line, RLENGTH+1); sub(/[ \t]+$/,"",h); slugs[slugify(h)]=1
  }
  s=line
  while (match(s, /\]\(#[A-Za-z0-9_-]+\)/)) {
    tok=substr(s, RSTART, RLENGTH); a=tok; sub(/^\]\(#/,"",a); sub(/\)$/,"",a)
    targets[a]=1; s=substr(s, RSTART+RLENGTH)
  }
}
END {
  broken=""
  for (t in targets) if (!(t in slugs)) broken=broken " " t
  if (broken != "") {
    print "Unresolved in-page anchors:" broken > "/dev/stderr"
    m=""; for (g in slugs) m=m " " g
    print "Known heading anchors:" m > "/dev/stderr"
    exit 1
  }
  n=0; for (t in targets) n++
  print "all " n " in-page anchor link(s) resolve to headings"
}
' "$DOC" || fail "one or more in-page anchor links do not resolve to a heading"
ok "every in-page anchor link resolves to a real heading"

echo "[gadugi] stale-PR triage runbook contract (#810): all ${PASS} checks passed"
