#!/usr/bin/env bash
# dashboard-cluster-clarity.sh — outside-in qa-team check for the Overview
# "Machines & Memory Sharing" de-jargon pass.
#
# The live Playwright audit found the densest cluster of machine jargon on the
# Overview landing page's cluster/event-bus card: "Cluster Topology",
# "DHT+bloom gossip", "Hive Status", the terse "Event Bus / Subscribers /
# Events/min" labels, and raw snake_case event-topic enum names
# (fact_imported, node_joined, …). This script boots the real dashboard binary,
# logs in with the dashkey, and asserts — against the served HTML — that the
# plain-language rewrite is present and the opaque labels are gone from the
# visible copy (the raw enum ids survive only inside data-testid / title
# tooltips, which we assert stay intact so power users lose nothing and the
# structural e2e contract keeps passing).
set -euo pipefail

PORT="${DASH_PORT:-8141}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-cluster.XXXXXX.log)"
CJ="$(mktemp -t dash-cluster-cookies.XXXXXX)"

echo "[cluster] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[cluster] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[cluster] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ"; }
trap cleanup EXIT

up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[cluster] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[cluster] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[cluster] FAIL: $1" >&2; exit 1; }

# ── Card header + plain-English description ──────────────────────────────────
grep -qF 'Machines &amp; Memory Sharing' <<<"$HTML" \
  || fail "card header must be the plain-English 'Machines & Memory Sharing'"
grep -qF '>Cluster Topology <' <<<"$HTML" \
  && fail "opaque 'Cluster Topology' header must be gone"
grep -qF "how those machines share what they've learned" <<<"$HTML" \
  || fail "card must carry a plain-English description of what it shows"

# ── Plain-English humanizers are wired in ────────────────────────────────────
grep -qF 'function humanizeEventTopic(' <<<"$HTML" \
  || fail "humanizeEventTopic must map raw event-topic enum names to plain text"
grep -qF 'function humanizeSyncProtocol(' <<<"$HTML" \
  || fail "humanizeSyncProtocol must translate the DHT/gossip protocol string"
grep -qF 'function humanizeHiveStatus(' <<<"$HTML" \
  || fail "humanizeHiveStatus must translate the hive-mind status enum"
grep -qF 'function humanizeTopology(' <<<"$HTML" \
  || fail "humanizeTopology must translate the topology value"

# ── Opaque row/section labels are replaced with plain language ───────────────
grep -qF '>Live internal signals<' <<<"$HTML" \
  || fail "'Event Bus' must be relabeled to 'Live internal signals'"
grep -qF '>Event Bus<' <<<"$HTML" \
  && fail "insider 'Event Bus' label must be gone from the visible copy"
grep -qF '>How memory is shared<' <<<"$HTML" \
  || fail "'Memory Sync' row must be relabeled 'How memory is shared'"
grep -qF '>Sharing status<' <<<"$HTML" \
  || fail "'Hive Status' row must be relabeled 'Sharing status'"
grep -qF '>Multi-machine mode<' <<<"$HTML" \
  || fail "'Topology' row must be relabeled 'Multi-machine mode'"

# The visible protocol value must be the plain phrase, not the raw protocol.
grep -qF 'Peer-to-peer (machines share facts directly)' <<<"$HTML" \
  || fail "the memory-sharing value must render as the plain peer-to-peer phrase"
grep -qF 'Facts received from other machines' <<<"$HTML" \
  || fail "raw topic 'fact_imported' must render as a plain-English label"

# ── Machine ids survive only as data-testids / title tooltips (contract) ─────
# The per-topic testid is built at runtime from the topic name, so the served
# HTML carries the template-literal form; assert that pattern is intact.
grep -qF 'data-testid="event-bus-topic-${escAttr(name)}"' <<<"$HTML" \
  || fail "stable per-topic event-bus testid pattern must remain for the structural e2e spec"
grep -qF 'title="internal signal id: ${escAttr(name)}"' <<<"$HTML" \
  || fail "raw topic enum id must survive as a hover tooltip so power users lose nothing"
grep -qF 'data-testid="event-bus-total-subscribers"' <<<"$HTML" \
  || fail "event-bus-total-subscribers testid must remain"

echo "[cluster] PASS: Overview machines/memory-sharing card is jargon-free"
