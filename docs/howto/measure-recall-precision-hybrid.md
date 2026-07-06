---
title: "How to measure recall precision on both rails (benchmark + live)"
description: Run the fixed-corpus recall-precision benchmark, watch the live per-cycle trend accumulate, and read the hybrid correlation verdict on the dashboard — the operator playbook for the G1 hybrid measurement surface wired for recall_precision_at_k.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/hybrid-cognition-measurement.md
  - ../reference/recall-precision-hybrid-api.md
  - ../tutorials/run-your-first-benchmark-gym.md
  - ../howto/simard-status.md
---

# How to measure recall precision on both rails (benchmark + live)

This guide walks through the hybrid measurement surface for the cognition metric
**`recall_precision_at_k`**: run the fixed benchmark, let the live rail
accumulate, and read the correlation verdict that says whether an improvement
holds on **both** rails. For the design rationale see the
[concept](../concepts/hybrid-cognition-measurement.md); for the full API see the
[reference](../reference/recall-precision-hybrid-api.md).

## Prerequisites

- A built `simard` binary (or run through `cargo run --`).
- For the dashboard step: the OODA daemon / dashboard running (see
  [Run the OODA daemon](./run-ooda-daemon.md)).
- Run the benchmark and the daemon from the **same working directory** (the
  repo root) so both share `<cwd>/gym_history.db` — see
  [why](../reference/recall-precision-hybrid-api.md#shared-score-history-path).

## Step 1: Run the benchmark

The benchmark scores a fixed, in-repo corpus deterministically and appends one
comparable score to the shared gym history.

```console
$ simard gym recall-precision
cognition/recall_precision_at_k: score=0.8333 signal=stable samples=6
```

- `score` is the mean precision@k over the fixed corpus (`0.0..=1.0`).
- `signal` is the gym signal versus the previous run (`stable` on the first run;
  `improvement(+Δ)` / `regression(Δ)` / `promoted` thereafter).

Run it again after a cognition change to get a second, comparable point:

```console
$ simard gym recall-precision
cognition/recall_precision_at_k: score=0.8833 signal=improvement(+0.0500) samples=6
```

Because the corpus is frozen, any score change is attributable to the change
under test — not to a shift in inputs. This is the **benchmark** half of the
hybrid.

## Step 2: Let the live rail accumulate

The **live** half needs no operator action — it is produced by normal operation.
Each OODA cycle, the ranked fact-recall path folds a precision@k observation per
recall and drains **one** aggregated `recall_precision_at_k` sample into
`metrics.jsonl` at cycle end.

Confirm live samples are landing:

```console
$ grep '"recall_precision_at_k"' "${SIMARD_STATE_ROOT:-$HOME/.simard}/metrics/metrics.jsonl" | tail -3
{"timestamp":"2026-07-06T00:10:00Z","metric_name":"recall_precision_at_k","value":0.80,"context":"{\"site\":\"recall_facts_ranked\",\"samples\":6}"}
{"timestamp":"2026-07-06T00:15:00Z","metric_name":"recall_precision_at_k","value":0.82,"context":"{\"site\":\"recall_facts_ranked\",\"samples\":9}"}
{"timestamp":"2026-07-06T00:20:00Z","metric_name":"recall_precision_at_k","value":0.83,"context":"{\"site\":\"recall_facts_ranked\",\"samples\":7}"}
```

Each line is one cycle's cross-source mean precision@k with the sample count in
`context`. A cycle with no ranked recall emits nothing, so the series carries
signal only.

> **Note.** Live samples only appear from a real daemon run. Unit tests never
> write to the operator's `metrics.jsonl` (the flush is skipped under
> `cfg!(test)`), so a fresh checkout shows an empty live rail until the daemon
> has run at least a couple of cycles.

## Step 3: Read the hybrid correlation

Query the read-only correlation endpoint (authenticated, like every `/api/*`
route):

```console
$ curl -s --cookie "$SIMARD_DASH_COOKIE" \
    'http://127.0.0.1:8080/api/cognition/recall-precision?window_hours=168' | jq .
```

```json
{
  "metric": "recall_precision_at_k",
  "benchmark": { "score": 0.8833, "previous_score": 0.8333, "signal": "improvement(+0.0500)" },
  "live":      { "first": 0.80, "latest": 0.83, "trend_delta": 0.03, "samples": 42 },
  "correlation": {
    "verdict": "confirmed",
    "benchmark_delta": 0.05,
    "live_trend_delta": 0.03,
    "threshold": 0.01,
    "explanation": "Benchmark and live trend both improved beyond the 0.01 threshold."
  },
  "generated_at": "2026-07-06T03:20:00Z"
}
```

Tune the window and limits as needed (all values are clamped to safe bounds, not
rejected):

```console
# Narrow the live window to the last 24h and cap the sample scan.
$ curl -s --cookie "$SIMARD_DASH_COOKIE" \
    'http://127.0.0.1:8080/api/cognition/recall-precision?window_hours=24&live_limit=500'
```

## Step 4: Read it on the dashboard

Open the dashboard and go to the **System Status** tab (the same surface that
hosts the #2491/#2494 competency scorecard). The recall-precision panel shows:

- the latest **benchmark** score and its gym signal,
- the recent **live** trend (sparkline: first → latest, sample count),
- the **correlation verdict** badge.

No new tab is added; the panel sits alongside the existing status tiles.

## Interpret the verdict

| Verdict | What it means | What to do |
|---|---|---|
| `confirmed` | Improved on the fixed corpus **and** live. | Trust the "cognition improved" claim. |
| `benchmark-only` | Benchmark up, production flat. | Suspect overfit or an unrepresentative corpus; broaden the corpus (follow-up issue). |
| `live-only` | Production up, benchmark flat. | The frozen corpus may be missing the improved case; add a case. |
| `diverging` | One rail up while the other is **down** — the rails contradict each other. | Do **not** trust the gain; find why the rails disagree before claiming anything. |
| `regressed` | A drop on at least one rail with no offsetting rise (both down, or one down and one flat). | A real regression — investigate the change under test. |
| `stable` | Neither moved beyond `±0.01`. | No measurable change. |
| `insufficient` | <2 benchmark runs or <2 live samples in-window. | Run the benchmark again (Step 1) and/or widen `window_hours`. |

Only `confirmed` should back a cognition-improvement claim.

## Troubleshooting

- **`benchmark: null` / verdict `insufficient`.** Fewer than two benchmark
  records exist. Run `simard gym recall-precision` again (Step 1). Confirm the
  benchmark and dashboard share a CWD so both use the same
  [`gym_history.db`](../reference/recall-precision-hybrid-api.md#shared-score-history-path).
- **`live: null` / no live samples.** The daemon has not produced ranked-recall
  cycles yet, or `SIMARD_STATE_ROOT` differs between the daemon and your shell.
  Verify `metrics.jsonl` (Step 2) under the daemon's state root.
- **`error` field present.** A rail failed to read; the affected section is
  `null` and the message is intentionally generic. Check the daemon logs
  (`tracing::warn!`) for specifics — the endpoint never leaks paths, SQL, or env
  into the JSON.
- **401 / redirected to login.** The endpoint is behind `require_auth`; supply a
  valid dashboard session cookie.

## See also

- [Concept: hybrid cognition measurement](../concepts/hybrid-cognition-measurement.md)
- [Recall-precision hybrid measurement API reference](../reference/recall-precision-hybrid-api.md)
- [Tutorial: run your first benchmark gym suite](../tutorials/run-your-first-benchmark-gym.md)
- [How to read `simard status`](./simard-status.md)
