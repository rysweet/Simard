#!/usr/bin/env bash
# Outside-in coverage for the `simard-atelier` identity, end-to-end.
#
# Proves the two acceptance criteria of the Atelier identity:
#   1. SELECTABLE  — the operator probe can bootstrap the `simard-atelier`
#      identity through the repo-grounded engineer-loop-run surface and drive
#      the session to completion.
#   2. BRIEF -> MODEL + RENDER — the `simard-atelier-build` tool takes a product
#      brief JSON to an exported model (STL) + render (SVG) plus a cut list,
#      BOM, and manifest, deterministically and with no external dependency.
#
# Hermetic: uses --no-cad so the run never shells out to openscad, making the
# assertions deterministic on any machine (with or without a CAD toolchain).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRIEF="examples/atelier/desk_brief.json"
OUT="$(mktemp -d "${TMPDIR:-/tmp}/atelier-gadugi.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT

echo "== 1. Atelier identity is selectable via the operator probe =="
PROBE_OUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-atelier local-harness single-process \
    "design a small oak side table"
)"
printf '%s\n' "$PROBE_OUT"
printf '%s\n' "$PROBE_OUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$PROBE_OUT" | grep -F "Identity: simard-atelier" >/dev/null
printf '%s\n' "$PROBE_OUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$PROBE_OUT" | grep -F "Shutdown: stopped" >/dev/null
echo "   OK: simard-atelier bootstrapped and completed."

echo "== 2. Product brief -> exported model + render (end-to-end) =="
BUILD_OUT="$(
  cargo run --quiet --bin simard_atelier_build -- \
    --brief "$BRIEF" --out "$OUT" --no-cad
)"
printf '%s\n' "$BUILD_OUT"
printf '%s\n' "$BUILD_OUT" | grep -F "Studio Writing Desk" >/dev/null

# Every fabrication artifact must exist and be non-empty.
for f in model.scad model.stl render.svg cutlist.csv bom.csv manifest.json; do
  test -s "$OUT/$f" || { echo "MISSING or empty artifact: $f" >&2; exit 1; }
done
echo "   OK: all six deterministic artifacts written."

# The STL must be a well-formed ASCII mesh.
head -1 "$OUT/model.stl" | grep -Eq '^solid ' || { echo "STL missing solid header" >&2; exit 1; }
tail -1 "$OUT/model.stl" | grep -Eq '^endsolid ' || { echo "STL missing endsolid footer" >&2; exit 1; }
grep -q "facet normal" "$OUT/model.stl" || { echo "STL has no facets" >&2; exit 1; }
echo "   OK: model.stl is a well-formed mesh."

# The render must be a valid SVG.
head -1 "$OUT/render.svg" | grep -q '<svg' || { echo "render.svg is not SVG" >&2; exit 1; }
grep -q '</svg>' "$OUT/render.svg" || { echo "render.svg not closed" >&2; exit 1; }
echo "   OK: render.svg is a valid render."

# Cut list and BOM must have headers + at least one data row.
head -1 "$OUT/cutlist.csv" | grep -Fq "part,quantity,length_mm,width_mm,thickness_mm,material" \
  || { echo "cutlist header mismatch" >&2; exit 1; }
test "$(wc -l < "$OUT/cutlist.csv")" -ge 2 || { echo "cutlist has no rows" >&2; exit 1; }
head -1 "$OUT/bom.csv" | grep -Fq "item,quantity,unit,notes" \
  || { echo "bom header mismatch" >&2; exit 1; }
grep -q "Sheet good" "$OUT/bom.csv" || { echo "bom missing sheet goods" >&2; exit 1; }
echo "   OK: cut list and BOM are well-formed."

# The manifest must record the product type and the artifact index.
grep -q '"product_type": "table"' "$OUT/manifest.json" || { echo "manifest product_type missing" >&2; exit 1; }
grep -q '"openscad_used": false' "$OUT/manifest.json" || { echo "manifest openscad flag wrong" >&2; exit 1; }
echo "   OK: manifest.json records the build."

echo "ALL ATELIER CHECKS PASSED"
