#!/usr/bin/env bash
# Outside-in scenario for the Gastronome culinary / menu & event-design
# identity. It proves the headline "Done when" contract end to end: the
# `simard gastronome` kitchen app takes an event/menu brief to a COSTED,
# SCHEDULED menu plan — menu chosen, recipes scaled to the guest count,
# nutrition + cost rolled up, and a prep schedule that finishes at service
# time — with dietary restrictions enforced fail-closed.
#
# The engine is pure and deterministic (no network, no clock), so this runs
# hermetically. It exercises BOTH surfaces:
#   (1) the in-tree unit + integration tests (name-pinned so a rename cannot
#       silently no-op the scenario), and
#   (2) the real `simard gastronome` CLI: demo, plan-from-file (JSON), a vegan
#       + gluten-free plan, and the fail-closed dietary guard.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-gastronome.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/gastronome-cargo-test.log"

echo "== gastronome domain + end-to-end tests (unit + integration) =="
# Scope to the module (unit tests) and the integration test separately: a
# shared positional filter would also filter the integration binary, so run
# each and concatenate the logs for name-pinning below. Hermetic and fast.
cargo test --locked --lib gastronome -- --nocapture >"$TEST_LOG" 2>&1
cargo test --locked --test gastronome_end_to_end -- --nocapture >>"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more gastronome tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

echo "== name-pinned end-to-end tests actually ran =="
for t in \
  "brief_to_costed_scheduled_plan_end_to_end" \
  "dietary_restrictions_are_enforced_fail_closed" \
  "budget_overage_is_surfaced_not_hidden" \
  "plan_scales_linearly_with_guest_count"
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done
echo "OK: brief -> costed, scheduled plan is proven at the library level."

echo "== build the simard binary once for CLI checks =="
cargo build --locked --bin simard >"$WORK/build.log" 2>&1 \
  || { echo "FAIL: binary build failed" >&2; cat "$WORK/build.log" >&2; exit 1; }
BIN="$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | grep -oE '"target_directory":"[^"]+"' | head -1 | cut -d'"' -f4)/debug/simard"
[ -x "$BIN" ] || { echo "FAIL: simard binary not found at $BIN" >&2; exit 1; }

echo "== (1) CLI demo plans end to end (costed + scheduled) =="
DEMO="$("$BIN" gastronome demo 2>/dev/null)"
echo "$DEMO" | grep -q "per guest:" || { echo "FAIL: demo missing per-guest cost" >&2; echo "$DEMO" >&2; exit 1; }
echo "$DEMO" | grep -q "Nutrition (per guest)" || { echo "FAIL: demo missing nutrition" >&2; exit 1; }
echo "$DEMO" | grep -qE "service 18:00" || { echo "FAIL: demo schedule not anchored to service" >&2; exit 1; }
echo "$DEMO" | grep -qE "^  17:5[0-9]–18:00 " || { echo "FAIL: demo schedule does not finish at service time" >&2; echo "$DEMO" >&2; exit 1; }
echo "OK: demo produced a costed, per-guest-priced, service-anchored plan."

echo "== (2) CLI plans a brief file (JSON) into a scaled, costed plan =="
cat >"$WORK/brief.json" <<'JSON'
{"event_name":"Client lunch","guest_count":16,"menu_id":"vegan-gf-lunch",
 "dietary_restrictions":["vegan","gluten-free"],"budget_per_guest":8.0,
 "service_time_min":750}
JSON
PLAN_JSON="$("$BIN" gastronome plan "$WORK/brief.json" --json 2>/dev/null)"
echo "$PLAN_JSON" | grep -q '"guest_count": 16' || { echo "FAIL: plan not scaled to 16 guests" >&2; echo "$PLAN_JSON" >&2; exit 1; }
echo "$PLAN_JSON" | grep -q '"per_guest"' || { echo "FAIL: plan missing per-guest cost" >&2; exit 1; }
echo "$PLAN_JSON" | grep -q '"tasks"' || { echo "FAIL: plan missing prep schedule tasks" >&2; exit 1; }
echo "OK: JSON brief -> scaled, costed, scheduled plan."

echo "== (3) dietary guard fails CLOSED (vegan on a dairy+gluten menu) =="
cat >"$WORK/bad.json" <<'JSON'
{"event_name":"Impossible","guest_count":10,"menu_id":"italian-dinner",
 "dietary_restrictions":["vegan"],"service_time_min":1080}
JSON
if "$BIN" gastronome plan "$WORK/bad.json" >/dev/null 2>&1; then
  echo "FAIL: vegan restriction on the Italian menu should have failed closed" >&2
  exit 1
fi
echo "OK: an unsatisfiable dietary restriction is rejected, not silently served."

echo "PASS: gastronome-menu-plan scenario (brief -> costed, scheduled menu plan; fail-closed dietary guard)"
