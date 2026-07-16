#!/usr/bin/env bash
# Outside-in scenario for the `simard-atelier` identity (industrial & furniture
# design). It proves the identity's headline "done" condition end-to-end: a
# product brief is taken to an EXPORTED MODEL plus a RENDER, alongside a cut
# list and a bill of materials — and that the pipeline DEGRADES GRACEFULLY when
# CAD tools are absent instead of failing the goal session.
#
# Two layers of proof:
#   (a) Name-pinned in-tree unit tests for the fabrication engine, the loader
#       identity, and the CLI subcommand. A cargo filter that matches zero
#       tests still exits 0, so we assert the NAMED tests actually ran and
#       passed — a future rename cannot silently turn this scenario into a
#       no-op.
#   (b) A real black-box run of the shipped `simard atelier demo` CLI into a
#       temp dir. We assert the deterministic artifacts (parametric OpenSCAD
#       source, cut list CSV, BOM JSON, manifest) are ALWAYS produced, and that
#       the model/render (STL/PNG) are either `produced` (when OpenSCAD is
#       installed — including headless via `xvfb-run`) or `skipped-tool-missing`
#       (when it is not). Either outcome is correct; a `failed` STL or a missing
#       deterministic artifact is not. This keeps the scenario hermetic and
#       green on any host, with or without a CAD toolchain.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-atelier-scenario.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/atelier-cargo-test.log"

echo "== (a) atelier fabrication-engine + identity + CLI unit tests =="
# Scope to the atelier module, the loader identity test, and the CLI dispatch
# tests. Hermetic: the pipeline tests inject a fake tool runner, so no CAD
# binary or network is required.
cargo test --lib --locked -- \
    atelier:: \
    identity::loader::tests::builtin_loader_loads_atelier_identity \
    operator_cli::atelier::tests:: \
    --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more atelier tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

# Pin the load-bearing tests so a rename can't silently no-op the proof.
for t in \
  "atelier::pipeline::tests::writes_all_deterministic_artifacts_without_tools" \
  "atelier::pipeline::tests::skips_tool_artifacts_when_tools_missing" \
  "atelier::pipeline::tests::produces_model_and_render_with_full_toolchain" \
  "atelier::pipeline::tests::render_uses_xvfb_wrapper_on_headless_host" \
  "atelier::pipeline::tests::step_skipped_when_stl_not_produced" \
  "atelier::pipeline::tests::records_tool_failure" \
  "atelier::pipeline::tests::rejects_invalid_brief_before_writing" \
  "atelier::fabrication::tests::table_cut_list_has_top_legs_and_aprons" \
  "identity::loader::tests::builtin_loader_loads_atelier_identity" \
  "operator_cli::atelier::tests::demo_runs_end_to_end_into_tempdir"
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done
echo "OK: fabrication engine, loader identity, and CLI subcommand are proven."

echo "== (b) black-box CLI: brief -> exported model + render (graceful degradation) =="
OUT="$WORK/out"
cargo run --quiet --locked --bin simard -- atelier demo --out "$OUT" \
    >"$WORK/demo.log" 2>&1 \
  || { echo "FAIL: 'simard atelier demo' exited non-zero" >&2; cat "$WORK/demo.log" >&2; exit 1; }

MANIFEST="$OUT/manifest.json"
[ -f "$MANIFEST" ] || { echo "FAIL: manifest.json not written" >&2; cat "$WORK/demo.log" >&2; exit 1; }

# Deterministic artifacts must ALWAYS exist regardless of installed tooling.
missing=0
for pat in '\.scad' 'cut_list\.csv' 'bom\.json'; do
  grep -qE "\"name\": \"[^\"]*${pat}\"" "$MANIFEST" \
    || { echo "FAIL: deterministic artifact matching /${pat}/ absent from manifest" >&2; missing=1; }
done
[ "$missing" -eq 0 ] || { cat "$MANIFEST" >&2; exit 1; }

# Files on disk for the deterministic artifacts.
ls "$OUT"/*.scad >/dev/null 2>&1 || { echo "FAIL: no .scad file on disk" >&2; exit 1; }
[ -s "$OUT/cut_list.csv" ] || { echo "FAIL: cut_list.csv missing/empty" >&2; exit 1; }
[ -s "$OUT/bom.json" ]     || { echo "FAIL: bom.json missing/empty" >&2; exit 1; }
echo "OK: parametric model source + cut list + BOM + manifest always produced."

# Extract the STL and PNG status from the manifest (portable: python3-free).
stl_status="$(awk '/\.stl"/{f=1} f&&/"status"/{gsub(/[",]/,"");print $2;exit}' "$MANIFEST")"
png_status="$(awk '/\.png"/{f=1} f&&/"status"/{gsub(/[",]/,"");print $2;exit}' "$MANIFEST")"
echo "model(STL) status = ${stl_status:-<none>} ; render(PNG) status = ${png_status:-<none>}"

for label in "STL:${stl_status}" "PNG:${png_status}"; do
  s="${label#*:}"
  case "$s" in
    produced|skipped-tool-missing) : ;;  # both are correct outcomes
    *) echo "FAIL: ${label%%:*} status '${s}' is neither produced nor gracefully skipped" >&2
       cat "$MANIFEST" >&2; exit 1 ;;
  esac
done

# When OpenSCAD IS installed, the end-to-end promise (model + render) must hold.
if command -v openscad >/dev/null 2>&1; then
  [ "$stl_status" = "produced" ] \
    || { echo "FAIL: openscad present but STL not produced (${stl_status})" >&2; cat "$MANIFEST" >&2; exit 1; }
  [ "$png_status" = "produced" ] \
    || { echo "FAIL: openscad present but PNG render not produced (${png_status}); headless xvfb fallback should cover CI" >&2; cat "$MANIFEST" >&2; exit 1; }
  ls "$OUT"/*.stl >/dev/null 2>&1 || { echo "FAIL: STL marked produced but no file on disk" >&2; exit 1; }
  ls "$OUT"/*.png >/dev/null 2>&1 || { echo "FAIL: PNG marked produced but no file on disk" >&2; exit 1; }
  echo "OK: with OpenSCAD present, brief -> exported model + render is closed end-to-end."
else
  echo "OK: OpenSCAD absent -> tool artifacts skipped gracefully; deterministic outputs intact."
fi

echo "PASS: atelier-brief-to-model-and-render scenario (identity selectable; brief -> model + render)"
