#!/usr/bin/env bash
# qa-team scenario: meeting REPL color helpers have a single source of truth.
#
# The meeting REPL's ANSI color helpers (green/yellow/cyan, NO_COLOR-aware)
# must live in exactly one module — `src/meeting_repl/color.rs` — which is the
# only one wired into the module tree via `mod color;`. A stray
# `src/meeting_repl/colors.rs` duplicate was committed by mistake in #2294: it
# was never declared in `mod.rs`, never referenced, and never compiled, so it
# silently drifted from the wired helpers. This scenario is the regression
# guard that keeps the dead duplicate from coming back and proves the wired
# module still works.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Contract 1: the wired color module exists and is declared in mod.rs.
test -f src/meeting_repl/color.rs
grep -Eq '^[[:space:]]*mod color;' src/meeting_repl/mod.rs

# Contract 2: the orphan duplicate must NOT exist (regression guard for #2294).
if [ -e src/meeting_repl/colors.rs ]; then
  echo "[gadugi] FAIL: orphan duplicate src/meeting_repl/colors.rs is back" >&2
  exit 1
fi
# And nothing should ever declare a plural `mod colors;`.
if grep -Eq '^[[:space:]]*mod colors;' src/meeting_repl/mod.rs; then
  echo "[gadugi] FAIL: src/meeting_repl/mod.rs declares an orphan 'mod colors;'" >&2
  exit 1
fi

# Contract 3: the single wired color module actually works — run its unit tests
# (green/yellow/cyan, with and without NO_COLOR). No `--quiet` so the per-test
# `... ok` lines are emitted for the assertions below.
OUTPUT="$(cargo test --lib -- meeting_repl::color:: 2>&1)"
printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "green_with_no_color_returns_plain ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "green_without_no_color_contains_escape ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "yellow_with_no_color_returns_plain ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "cyan_without_no_color_contains_escape ... ok" >/dev/null

echo "[gadugi] meeting REPL color single-source: all contracts verified"
