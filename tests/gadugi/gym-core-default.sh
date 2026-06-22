#!/usr/bin/env bash
# Issue #2087: the default gym surface must resolve to the four spec-mandated
# core V1 classes only; the extra classes must be preserved but reachable only
# via an explicit `extended` opt-in. This is a fast, deterministic check that
# exercises only `gym list` (no scenario execution / real backends).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CORE_OUTPUT="$(cargo run --quiet --bin simard-gym -- list)"
printf '%s\n' "$CORE_OUTPUT"

# The default (core) list must contain exactly the four spec-mandated classes.
for class in repo-exploration documentation safe-code-change session-quality; do
  printf '%s\n' "$CORE_OUTPUT" | grep -F "class=${class}" >/dev/null \
    || { echo "FAIL: core list missing required class '${class}'" >&2; exit 1; }
done

CORE_CLASSES="$(
  printf '%s\n' "$CORE_OUTPUT" | grep -oE 'class=[a-z-]+' | sort -u
)"
EXPECTED_CLASSES=$'class=documentation\nclass=repo-exploration\nclass=safe-code-change\nclass=session-quality'
if [ "$CORE_CLASSES" != "$EXPECTED_CLASSES" ]; then
  echo "FAIL: core list classes are not exactly the four spec classes:" >&2
  printf '%s\n' "$CORE_CLASSES" >&2
  exit 1
fi

# No opt-in/extended class may leak into the default list.
for class in chaos-engineering event-sourcing rate-limiting knowledge-recall; do
  if printf '%s\n' "$CORE_OUTPUT" | grep -F "class=${class}" >/dev/null; then
    echo "FAIL: default gym list leaked extended class '${class}'" >&2
    exit 1
  fi
done

# The extended set must be reachable only via the explicit opt-in and must be a
# strict superset of the core set.
EXTENDED_OUTPUT="$(cargo run --quiet --bin simard-gym -- list extended)"
for class in chaos-engineering event-sourcing rate-limiting knowledge-recall; do
  printf '%s\n' "$EXTENDED_OUTPUT" | grep -F "class=${class}" >/dev/null \
    || { echo "FAIL: extended list missing opt-in class '${class}'" >&2; exit 1; }
done

CORE_COUNT="$(printf '%s\n' "$CORE_OUTPUT" | grep -c '^- ')"
EXTENDED_COUNT="$(printf '%s\n' "$EXTENDED_OUTPUT" | grep -c '^- ')"
[ "$CORE_COUNT" -gt 0 ] || { echo "FAIL: core list is empty" >&2; exit 1; }
[ "$EXTENDED_COUNT" -gt "$CORE_COUNT" ] \
  || { echo "FAIL: extended ($EXTENDED_COUNT) is not a strict superset of core ($CORE_COUNT)" >&2; exit 1; }

# The `--extended` flag form is also accepted.
cargo run --quiet --bin simard-gym -- list --extended | grep -F "class=chaos-engineering" >/dev/null

# An unknown selector is rejected (strict-arg contract preserved).
if cargo run --quiet --bin simard-gym -- list bogus >/dev/null 2>&1; then
  echo "FAIL: 'gym list bogus' should have errored" >&2
  exit 1
fi

echo "OK: gym default=core (${CORE_COUNT} scenarios, 4 classes), extended=${EXTENDED_COUNT}, opt-in gating verified"
