#!/usr/bin/env bash
# qa-team scenario for issue #809 — Azure DevOps ACL self-escalation guard.
#
# Outside-in verification of the security fix that prevents the default-workflow
# from self-granting itself an Azure DevOps `ForcePush` ACL to bypass a push
# denial, and that makes any (opt-in) temporary grant crash-safe so an
# interrupted/failed run can never leak elevated permissions on a shared repo.
#
# The `ado_acl_guard` unit tests exercise the real chokepoints:
#   - detection of ACL-mutation commands (and that read-only ops are allowed),
#   - the default-deny policy that surfaces the missing permission, and
#   - the crash-safe, idempotent grant/revoke (revoke runs on Ok / Err / panic).
# This script also asserts the operator-facing surfaces (prompt rule + doc) are
# present, since they are part of the behavioral contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Run the guard unit tests. No `--quiet` so per-test `... ok` lines are emitted
# for the behavior assertions below.
OUTPUT="$(cargo test --lib ado_acl_guard 2>&1)"
printf '%s\n' "$OUTPUT"

# Assert the suite passed with zero failures.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

# --- Crash-safety regressions (the heart of issue #809) ---------------------
# The elevated ACL must be revoked even when the push step fails mid-run.
printf '%s\n' "$OUTPUT" | grep -F "revokes_when_body_returns_err ... ok" >/dev/null
# ...and even when the push step panics / is killed mid-run (Drop revokes).
printf '%s\n' "$OUTPUT" | grep -F "revokes_when_body_panics ... ok" >/dev/null
# Early-return / scope-exit still revokes via Drop.
printf '%s\n' "$OUTPUT" | grep -F "drop_revokes_on_early_return ... ok" >/dev/null
# Revoke is idempotent (a re-run cannot leave permissions elevated).
printf '%s\n' "$OUTPUT" | grep -F "revoke_is_idempotent ... ok" >/dev/null
# A failed grant schedules no revoke.
printf '%s\n' "$OUTPUT" | grep -F "no_revoke_when_grant_fails ... ok" >/dev/null

# --- Authorization-boundary policy ------------------------------------------
# Self-escalation is refused by default and the missing permission is surfaced.
printf '%s\n' "$OUTPUT" | grep -F "blocks_self_escalation_by_default ... ok" >/dev/null
# Read-only inspection is NOT blocked.
printf '%s\n' "$OUTPUT" | grep -F "allows_readonly_even_without_optin ... ok" >/dev/null
# Mutation detection covers az CLI and REST ACE writes.
printf '%s\n' "$OUTPUT" | grep -F "detects_az_security_permission_update ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_rest_ace_post ... ok" >/dev/null
# Bypass-resistant detection (fail-closed): implicit-POST-via-body, short/long
# method aliases, curl forms, and transitive group-membership escalation.
printf '%s\n' "$OUTPUT" | grep -F "detects_rest_ace_implicit_post_via_body ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_curl_ace_write_forms ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_curl_glued_short_options ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_rest_graph_membership_write ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_curl_upload_and_form_writes ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_curl_grouped_short_options ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "grouped_boolean_only_read_is_not_flagged ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_repeated_method_decoy ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "proxy_short_option_is_not_a_method ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "detects_group_membership_add ... ok" >/dev/null

# --- Operator-facing surfaces -----------------------------------------------
# The engineer system prompt forbids ACL self-escalation and tells the agent to
# surface the missing permission on a push denial.
grep -qi "Never modify a repository's security ACLs" prompt_assets/simard/engineer_system.md
grep -qi "SIMARD_ALLOW_ADO_ACL_ESCALATION" prompt_assets/simard/engineer_system.md
# The reference doc describing the permission behavior exists.
test -f docs/reference/ado-acl-self-escalation-guard.md

echo "[gadugi] ADO ACL self-escalation guard (#809): all behaviors verified"
