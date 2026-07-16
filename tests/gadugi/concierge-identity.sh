#!/usr/bin/env bash
# Outside-in verification for the simard-concierge hospitality identity.
#
#   1. the builtin loader bootstraps the identity through the operator probe
#      (the repo-grounded engineer-loop-run surface), and
#   2. the bundled reservations/PMS reference prototype actually runs its
#      end-to-end smoke test green (design -> runnable software, for real).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# --- 1. Identity bootstraps via the operator probe -------------------------
OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-concierge local-harness single-process \
    "design a boutique hotel and scaffold its reservations/PMS prototype"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-concierge" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

# --- 2. The runnable reservations/PMS prototype passes its self-verifying demo
PROTO_OUT="$(cargo run --quiet --example concierge_reservations_pms)"
printf '%s\n' "$PROTO_OUT"
printf '%s\n' "$PROTO_OUT" | grep -E "result: ok\. [1-9][0-9]* checks passed; 0 failed" >/dev/null

# The same prototype's invariant tests are wired into cargo test (test = true).
cargo test --locked --example concierge_reservations_pms 2>&1 | tee /tmp/concierge-proto.log
grep -E "test result: ok\. [1-9][0-9]* passed" /tmp/concierge-proto.log >/dev/null

# --- 3. The Rust asset/identity contract holds -----------------------------
cargo test --locked --test concierge_identity_assets 2>&1 | tee /tmp/concierge-assets.log
grep -E "test result: ok\. [1-9][0-9]* passed" /tmp/concierge-assets.log >/dev/null

echo "concierge-identity: PASS"
