#!/usr/bin/env bash
# Outside-in QA contract for the canonical `simard install` deployment rail.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release --no-default-features --features signal >/tmp/simard-installer-canonical-build.log

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

DRY_RUN_OUTPUT="$(
  ./target/release/simard install \
    --dry-run \
    --simard-home "$TMP_DIR/simard-home" \
    --systemd-user-dir "$TMP_DIR/systemd-user" \
    --systemctl /bin/true \
    2>&1
)"

printf '%s\n' "$DRY_RUN_OUTPUT"

printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "[dry-run] Would install current binary:" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "target/release/simard -> $TMP_DIR/simard-home/bin/simard" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "[dry-run] Would install prompt_assets:" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "[dry-run] Would write user systemd units:" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "$TMP_DIR/systemd-user/simard-ooda.service" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "$TMP_DIR/systemd-user/simard-signal.service" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "/bin/true --user daemon-reload" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "/bin/true --user enable simard-ooda.service" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "/bin/true --user enable simard-signal.service" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "/bin/true --user restart simard-ooda.service" >/dev/null
printf '%s\n' "$DRY_RUN_OUTPUT" | grep -F "/bin/true --user restart simard-signal.service" >/dev/null

INSTALL_REAL_OUTPUT="$(
  cargo test --test install_real -- --test-threads=1 2>&1
)"

printf '%s\n' "$INSTALL_REAL_OUTPUT"
printf '%s\n' "$INSTALL_REAL_OUTPUT" | grep -F "simard_home_with_spaces_fails_before_any_mutation_or_systemctl_call ... ok" >/dev/null
printf '%s\n' "$INSTALL_REAL_OUTPUT" | grep -F "unsafe_systemd_path_characters_fail_closed_before_any_live_swap ... ok" >/dev/null
printf '%s\n' "$INSTALL_REAL_OUTPUT" | grep -F "prompt_asset_staging_failure_fails_closed_without_partial_binary_or_units ... ok" >/dev/null
printf '%s\n' "$INSTALL_REAL_OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

echo "simard-installer-canonical: canonical install dry-run and fail-closed contracts passed"
