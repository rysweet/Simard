#!/usr/bin/env bash
#
# enable-merge-queue.sh — enable GitHub's native merge queue on a branch and
# relax strict up-to-date-before-merge (issue #1050).
#
# `main` branch protection is managed EXTERNALLY through the GitHub API (there
# is no settings-as-code file in this repo), so enabling the queue is an
# explicit, admin-run apply step rather than something CI can self-provision.
# This script is the audited, idempotent way to perform that apply: it creates
# a `required_merge_queue` ruleset if one is absent (an existing ruleset is
# treated as already-satisfied and left unchanged) and sets
# required_status_checks `strict: false` so the queue's merged-result testing —
# not per-PR re-runs —
# provides freshness. Re-running converges to the same state.
#
# Scope / contract note: the strict-relax step targets *classic* branch
# protection (repos/{repo}/branches/{branch}/protection/required_status_checks).
# If up-to-date-before-merge is instead enforced by a repository RULESET's
# required_status_checks (strict), this script does NOT relax it — the classic
# PATCH is an idempotent no-op against ruleset-based freshness. Relaxing a
# ruleset's strict flag must be done on that ruleset directly. See
# docs/howto/merge-queue.md for the full operator guide.
#
# Auth: uses gh-managed auth or a GITHUB_TOKEN from the environment. A token is
# NEVER accepted as a flag and NEVER echoed or logged.
#
# Exit codes:
#   0  success (queue enabled/converged, or --dry-run / --help)
#   1  generic failure (unexpected API error, malformed response)
#   2  invalid arguments / input validation failure
#   3  insufficient permission — token lacks repo admin (HTTP 403)
set -euo pipefail

# The exact, pinned API-version header sent with every call. Kept as a literal
# so the pin is greppable in source and can't drift silently.
readonly API_VERSION_HEADER="X-GitHub-Api-Version: 2022-11-28"
# Repo/branch identifiers are validated against a strict allowlist before they
# are ever placed into an API path, so a shell metacharacter can never be
# interpolated into a call. --repo must be exactly `owner/name` (one slash,
# each segment [A-Za-z0-9._-]); --branch allows the usual ref characters
# (slashes for namespaces like feat/foo). A literal `..` is rejected in either
# to foreclose any path-traversal (e.g. `--repo ../../x`) into the API path.
readonly REPO_RE='^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'
readonly BRANCH_RE='^[A-Za-z0-9._/-]+$'
readonly RULESET_NAME="required_merge_queue"
# Resilience for the external GitHub API: transient failures (rate limits, 5xx,
# transport blips) are retried with bounded exponential backoff. Terminal
# outcomes (403 no-admin, 404/422 idempotent-convergence) are NEVER retried.
readonly MAX_ATTEMPTS=3
readonly BACKOFF_BASE_SECONDS=2

REPO="rysweet/Simard"
BRANCH="main"
DRY_RUN=0

log() { echo "enable-merge-queue.sh: $*" >&2; }

usage() {
  cat <<'EOF'
Usage: enable-merge-queue.sh [--repo <owner/name>] [--branch <name>] [--dry-run] [-h|--help]

Enable GitHub's native merge queue (a required_merge_queue ruleset) on a branch
and relax strict up-to-date-before-merge (strict: false). Idempotent.

Flags:
  --repo <owner/name>   Target repository (default: rysweet/Simard).
  --branch <name>       Target branch (default: main).
  --dry-run             Print the HTTP method + path that would be called and
                        exit 0 without writing anything (no auth required).
  -h, --help            Print this usage and exit 0.

Exit codes:
  0  success / dry-run / help
  1  generic failure
  2  invalid arguments
  3  insufficient permission (repo admin / HTTP 403)

Auth: uses gh-managed auth or $GITHUB_TOKEN from the environment. A token is
never accepted as a flag and never echoed or logged.
EOF
}

# ── Argument parsing ────────────────────────────────────────────────────────
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      [ "$#" -ge 2 ] || { log "--repo requires a value"; exit 2; }
      REPO="$2"
      shift 2
      ;;
    --branch)
      [ "$#" -ge 2 ] || { log "--branch requires a value"; exit 2; }
      BRANCH="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      log "unknown argument: $1"
      usage >&2
      exit 2
      ;;
  esac
done

# ── Input validation (before any use in an API path) ────────────────────────
# Reject path-traversal outright, then enforce the per-field allowlists.
if [[ "$REPO" == *..* ]] || ! [[ "$REPO" =~ $REPO_RE ]]; then
  log "invalid --repo '$REPO' (must be owner/name matching ${REPO_RE}, no '..')"
  exit 2
fi
if [[ "$BRANCH" == *..* ]] || ! [[ "$BRANCH" =~ $BRANCH_RE ]]; then
  log "invalid --branch '$BRANCH' (must match ${BRANCH_RE}, no '..')"
  exit 2
fi

readonly RULESETS_PATH="repos/${REPO}/rulesets"
readonly STRICT_PATH="repos/${REPO}/branches/${BRANCH}/protection/required_status_checks"

# The required_merge_queue ruleset payload. Named so re-applies converge, and
# targeted at the requested branch only.
ruleset_payload() {
  cat <<EOF
{
  "name": "${RULESET_NAME}",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/heads/${BRANCH}"], "exclude": [] } },
  "rules": [
    {
      "type": "merge_queue",
      "parameters": {
        "merge_method": "SQUASH",
        "grouping_strategy": "ALLGREEN",
        "max_entries_to_build": 5,
        "min_entries_to_merge": 1,
        "max_entries_to_merge": 5,
        "min_entries_to_merge_wait_minutes": 5,
        "check_response_timeout_minutes": 60
      }
    }
  ]
}
EOF
}

# ── Dry-run: print method + path, write nothing, require no auth ─────────────
if [ "$DRY_RUN" -eq 1 ]; then
  echo "DRY-RUN: would POST ${RULESETS_PATH}"
  echo "  creates the '${RULESET_NAME}' ruleset (merge queue) on branch '${BRANCH}' if absent; an existing ruleset is left unchanged"
  echo "  header ${API_VERSION_HEADER}"
  echo "DRY-RUN: would PATCH ${STRICT_PATH}"
  echo "  sets required_status_checks strict: false (relax up-to-date-before-merge)"
  echo "DRY-RUN: no writes performed."
  exit 0
fi

command -v gh >/dev/null 2>&1 || { log "the 'gh' CLI is required to apply changes"; exit 1; }

# Call the GitHub API with the pinned version header. stdout carries the
# response body; stderr (captured by the caller) carries any gh error text so
# the HTTP status can be classified.
gh_api() {
  local method="$1" path="$2" payload="${3:-}"
  if [ -n "$payload" ]; then
    printf '%s' "$payload" | gh api --method "$method" \
      -H "Accept: application/vnd.github+json" \
      -H "$API_VERSION_HEADER" \
      "$path" --input -
  else
    gh api --method "$method" \
      -H "Accept: application/vnd.github+json" \
      -H "$API_VERSION_HEADER" \
      "$path"
  fi
}

# Map a captured gh stderr blob to an action code:
#   3  -> HTTP 403 (missing repo admin)
#   10 -> HTTP 404/422 (already-satisfied / benign convergence, idempotent)
#   1  -> anything else (unexpected)
classify_error() {
  local err="$1"
  if printf '%s' "$err" | grep -q 'HTTP 403'; then
    return 3
  fi
  if printf '%s' "$err" | grep -qE 'HTTP (404|422)'; then
    return 10
  fi
  return 1
}

# A transient failure worth retrying: GitHub rate-limit (429), a server-side
# 5xx, or a transport-level error where gh never received an HTTP status at all
# (DNS/connection/timeout/reset). Terminal statuses (403/404/422) return 1 here
# so they fall straight through to classify_error without wasting retries.
is_transient() {
  local err="$1"
  if printf '%s' "$err" | grep -qE 'HTTP (429|500|502|503|504)'; then
    return 0
  fi
  if ! printf '%s' "$err" | grep -qE 'HTTP [0-9]{3}'; then
    if printf '%s' "$err" \
      | grep -qiE 'timeout|timed out|connection|could not resolve|network|temporary failure|reset by peer|EOF'; then
      return 0
    fi
  fi
  return 1
}

# Resilient wrapper around gh_api: retries transient failures with bounded
# exponential backoff. On success returns 0 and passes the response through on
# stdout. On terminal failure returns 1 with gh's error text left in $err_file
# for the caller to classify (403 / 404 / 422 / generic). Only gh's own error
# text is ever logged, which never contains a token.
gh_api_resilient() {
  local method="$1" path="$2" payload="${3:-}"
  local attempt=1 delay="$BACKOFF_BASE_SECONDS"
  while :; do
    if gh_api "$method" "$path" "$payload" 2>"$err_file"; then
      return 0
    fi
    if [ "$attempt" -ge "$MAX_ATTEMPTS" ] || ! is_transient "$(cat "$err_file")"; then
      return 1
    fi
    log "transient GitHub API error on ${method} ${path} (attempt ${attempt}/${MAX_ATTEMPTS}); retrying in ${delay}s"
    sleep "$delay"
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
}

err_file="$(mktemp)"
trap 'rm -f "$err_file"' EXIT

# 1) Create the merge-queue ruleset (an existing one is a convergent no-op).
if gh_api_resilient POST "$RULESETS_PATH" "$(ruleset_payload)" >/dev/null; then
  log "applied '${RULESET_NAME}' ruleset on ${REPO}@${BRANCH}"
else
  rc=0
  classify_error "$(cat "$err_file")" || rc=$?
  case "$rc" in
    3)
      log "insufficient permission: enabling a merge-queue ruleset requires repo admin (HTTP 403)"
      exit 3
      ;;
    10)
      log "'${RULESET_NAME}' ruleset already satisfied on ${REPO}@${BRANCH} (idempotent no-op)"
      ;;
    *)
      log "unexpected GitHub API error creating the ruleset:"
      cat "$err_file" >&2
      exit 1
      ;;
  esac
fi

# 2) Relax strict up-to-date-before-merge. Uses the targeted
#    required_status_checks endpoint so existing required contexts are
#    preserved — only the `strict` flag changes. NOTE: this targets *classic*
#    branch protection only; if freshness is enforced by a ruleset's
#    required_status_checks (strict), this PATCH does not touch it and the
#    below 404/422 idempotent no-op will silently apply. See script header.
if gh_api_resilient PATCH "$STRICT_PATH" '{"strict":false}' >/dev/null; then
  log "relaxed strict up-to-date-before-merge on ${REPO}@${BRANCH} (strict: false)"
else
  rc=0
  classify_error "$(cat "$err_file")" || rc=$?
  case "$rc" in
    3)
      log "insufficient permission: relaxing strict requires repo admin (HTTP 403)"
      exit 3
      ;;
    10)
      log "required_status_checks already relaxed or no protection to patch on ${REPO}@${BRANCH} (idempotent no-op)"
      ;;
    *)
      log "unexpected GitHub API error relaxing strict:"
      cat "$err_file" >&2
      exit 1
      ;;
  esac
fi

log "merge queue enabled and strict up-to-date-before-merge relaxed on ${REPO}@${BRANCH}"
exit 0
