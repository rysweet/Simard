#!/usr/bin/env bash
# memory-stats-edges.sh — outside-in check for issue #2331.
#
# `simard memory stats` gained a graph-edge / dedup ("edges / connections")
# section. This script drives the real binary at the process boundary and
# asserts the section renders in both the human and `--json` output, that the
# `--json` stdout stays pure JSON (diagnostics on stderr), and that the
# daemon-IPC tier surfaces the "run with daemon stopped for graph stats" note
# instead of misreporting zero edges. Every step opens a hermetic state root
# under a temp dir so the live ~/.simard store is never touched. (The seeded
# DERIVES_FROM / provenance / snapshot-dedup edge counts are proven end to end
# by the `bin_simard_memory_cli` integration suite and the
# `cognitive_memory::tests_graph_stats` unit suite that CI runs.)
set -euo pipefail

echo "[edges] building simard binary…"
cargo build --release --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/release/simard"
[[ -x "$BIN" ]] || { echo "[edges] FAIL: binary not found at $BIN" >&2; exit 1; }

fail() { echo "[edges] FAIL: $1" >&2; exit 1; }

WORK="$(mktemp -d -t memstats-edges.XXXXXX)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# A leaked socket override would force the daemon tier on every step.
unset SIMARD_MEMORY_SOCKET || true

# ── 1) Human, direct on-disk open of a fresh store ───────────────────────────
H="$("$BIN" memory stats "$WORK/direct" 2>/dev/null)"
for needle in "edges / connections" "DERIVES_FROM" "PROCEDURE_DERIVES_FROM" \
              "SIMILAR_TO" "SUPERSEDES" "facts with provenance:" "snapshot dedup:"; do
  grep -qF "$needle" <<<"$H" || fail "human edges section missing '$needle'"
done
grep -qF "via direct open" <<<"$H" || fail "human report must show the direct-open tier"
echo "[edges] ok: human direct-open section"

# ── 2) JSON, direct open: pure JSON on stdout + stable scripting keys ─────────
J="$("$BIN" memory stats "$WORK/direct" --json 2>/dev/null)"
[[ "${J:0:1}" == "{" ]] \
  || fail "json stdout must be pure JSON (logs belong on stderr); got: ${J:0:80}"
for key in '"edges"' '"derives_from"' '"procedure_derives_from"' '"similar_to"' \
           '"supersedes"' '"provenance"' '"facts_with_provenance"' '"snapshot_dedup"' \
           '"distinct_caller_keys"'; do
  grep -qF "$key" <<<"$J" || fail "json missing key $key"
done
grep -qF '"access_tier": "direct-open"' <<<"$J" || fail "json must report direct-open tier"
echo "[edges] ok: json direct-open keys + pure-JSON stdout"

# ── 3) Daemon-IPC tier (human): a present-but-unconnectable socket forces the
#       DaemonSocket tier, which has no graph reader -> note instead of zeros ──
D="$WORK/daemon"; mkdir -p "$D"
export SIMARD_MEMORY_SOCKET="$D/memory.sock"; : > "$SIMARD_MEMORY_SOCKET"
HN="$("$BIN" memory stats "$D" 2>/dev/null)"
grep -qF "edges / connections" <<<"$HN" || fail "daemon-note human output missing section header"
grep -qF "run with daemon stopped for graph stats" <<<"$HN" \
  || fail "daemon tier must print the run-with-daemon-stopped note"
echo "[edges] ok: daemon-tier human note path"

# ── 4) Daemon-IPC tier (JSON): note surfaces as edges_note, not fabricated zeros
JN="$("$BIN" memory stats "$D" --json 2>/dev/null)"
grep -qF '"edges_note"' <<<"$JN" || fail "daemon tier json must carry edges_note"
grep -qF "run with daemon stopped for graph stats" <<<"$JN" || fail "edges_note text missing"
unset SIMARD_MEMORY_SOCKET
echo "[edges] ok: daemon-tier json edges_note path"

# The seeded DERIVES_FROM / provenance / snapshot-dedup edge counts are proven
# end to end by the process-boundary integration test
# `bin_simard_memory_cli::stats_shows_edges_and_dedup_section_via_direct_open`
# (it spawns this same binary against a seeded store) and the
# `cognitive_memory::tests_graph_stats` unit suite, both run by CI. This
# scenario stays focused on the outside-in CLI output structure across tiers.

echo "[edges] PASS: edges/connections + dedup section holds (#2331)"
