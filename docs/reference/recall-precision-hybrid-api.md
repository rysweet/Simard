---
title: Recall-precision hybrid measurement API reference
description: The authoritative API for the G1 hybrid measurement surface wired for recall precision@k — the upstream amplihack-memory measurement primitive, the Simard adapter, the fixed-corpus benchmark and its ScoreRecord, the shared gym_history path, the `simard gym recall-precision` command, and the read-only GET /api/cognition/recall-precision correlation endpoint with its schema, validation, auth, and error contract.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/hybrid-cognition-measurement.md
  - ../howto/measure-recall-precision-hybrid.md
  - ./telemetry-metrics.md
  - ./status-snapshot-api.md
---

# Recall-precision hybrid measurement API reference

This page is the authoritative catalog for the hybrid measurement surface,
wired end-to-end for the metric **`recall_precision_at_k`**. Read the
[concept](../concepts/hybrid-cognition-measurement.md) first for the *why*.

> **Modules:** `amplihack-memory::measurement` (upstream primitive),
> `src/cognitive_memory/metrics.rs` (Simard adapter + live rail),
> `src/cognitive_memory/recall_precision_bench.rs` (benchmark rail),
> `src/gym_history/mod.rs` (`ScoreHistory`, `default_db_path`),
> `src/operator_commands_gym/` + `src/operator_cli/gym.rs` (operator command),
> `src/operator_commands_dashboard/metrics.rs` + `routes.rs` (correlation
> endpoint).

## Naming and the shared join key

Everything hangs off one stable metric name — the benchmark's `scenario_id`,
the live `metric_name`, and the endpoint's `metric` field are all the **same
constant**, which is what lets the correlation join the two rails:

| Constant | Value | Defined at |
|---|---|---|
| `RECALL_PRECISION_METRIC` | `"recall_precision_at_k"` | `cognitive_memory::metrics` |
| `RECALL_PRECISION_SITE` | `"recall_facts_ranked"` | `cognitive_memory::metrics` (live aggregate site) |
| benchmark `suite_id` | `"cognition"` | `recall_precision_bench` (compile-time constant) |
| benchmark `scenario_id` | `"recall_precision_at_k"` | `recall_precision_bench` (== `RECALL_PRECISION_METRIC`) |

`suite_id` and `scenario_id` are **compile-time constants**, never
request-derived, so no untrusted value ever reaches a SQL `WHERE` clause.

## The measurement primitive (upstream, G2)

The scoring primitive lives in `amplihack-memory-lib` so both rails share one
implementation (see [G2 in the concept](../concepts/hybrid-cognition-measurement.md#g2-the-measurement-primitive-lives-upstream)).

```rust
// amplihack-memory::measurement
/// Precision@k for a ranked recall result over **decoupled `(concept, content)`
/// pairs** (no dependency on any Simard type): of the top-`k` items, the
/// fraction that are query-relevant under the keyword proxy (a query token is a
/// case-insensitive substring of `concept` or `content`). Returns `None`
/// (undefined, NOT 0.0) when the query has no usable tokens or the result set is
/// empty; `k` is clamped to the item count.
pub fn precision_at_k(query: &str, items: &[(&str, &str)], k: usize) -> Option<f64>;
```

The primitive takes plain `(concept, content)` pairs so it stays free of any
Simard type — that decoupling is the whole point of hosting it upstream (G2).

Semantics (unchanged from the pre-de-fork Simard implementation, moved verbatim
with its 9 pure-math tests as the parity gate):

- **Relevance** is a coarse keyword proxy: a query token (whitespace-split,
  lowercased, punctuation-only tokens dropped) is a substring of the item's
  lowercased `concept` **or** `content`. Deliberately broader than the ranker's
  exact-token score — the query is its own relevance oracle, so no external
  ground-truth labels are needed. This substring judgment is **definition #3** of
  three; it deliberately differs from the served word-boundary gate — see
  [Relationship to the served word-boundary gate and the ranker](#relationship-to-the-served-word-boundary-gate-and-the-ranker).
- Returns `None` for a wildcard/empty query (`*`, `""`, whitespace) or an empty
  result set, so callers **skip** emitting a meaningless sample rather than
  dragging the mean toward zero.
- `k` is clamped to `items.len()`.

### Simard adapter

`cognitive_memory::metrics::precision_at_k` is a thin adapter that delegates to
the upstream primitive — no scoring math is forked into Simard:

```rust
// src/cognitive_memory/metrics.rs
pub fn precision_at_k(query: &str, facts: &[CognitiveFact], k: usize) -> Option<f64> {
    // Map Simard's CognitiveFact onto the upstream primitive's decoupled
    // (concept, content) pairs — the only Simard-side glue; no scoring math.
    let pairs: Vec<(&str, &str)> = facts
        .iter()
        .map(|f| (f.concept.as_str(), f.content.as_str()))
        .collect();
    amplihack_memory::measurement::precision_at_k(query, &pairs, k)
}
```

A Simard-side delegation test asserts the adapter's output equals the upstream
primitive's for the shared corpus (parity gate).

## Live rail

Unchanged behaviour; now sourced from the upstream primitive.

| Item | Signature / value | Notes |
|---|---|---|
| `observe_recall_precision(site, value)` | `fn(&str, f64)` | Folds one precision@k observation into the running per-site aggregate. Cheap, lock-guarded, called on every ranked recall. |
| `drain_recall_precision(site)` | `fn(&str) -> Option<(f64, u64)>` | Drains and resets; returns `(mean, samples)`. |
| `flush_recall_precision_metric()` | `fn()` | Drains `RECALL_PRECISION_SITE` and, if any recall ran this cycle, emits **one** aggregated `recall_precision_at_k` sample to `metrics.jsonl`. Called once per OODA cycle, unconditionally. **Skipped under `cfg!(test)`** (aggregate still drained). Best-effort: a write failure is logged, never propagated. |

The emitted `metrics.jsonl` record is a standard
[`MetricEntry`](./telemetry-metrics.md) (`{timestamp, metric_name, value,
context}`); `value` is the windowed cross-source mean precision@k, `context`
carries `{"site":"recall_facts_ranked","samples":N}`.

## Benchmark rail

`recall_precision_bench` runs a fixed, in-repo corpus through the same
primitive and persists one comparable score.

```rust
// src/cognitive_memory/recall_precision_bench.rs
/// Score the fixed recall-precision corpus: for each (query, ranked facts, k)
/// case, compute precision@k via the upstream primitive; the benchmark score is
/// the deterministic mean over all cases. The corpus is a non-empty in-repo
/// constant, so the score is reproducible and comparable across runs.
pub fn score_recall_precision_corpus() -> f64;

/// Run the benchmark and append one ScoreRecord to the shared gym history,
/// returning the recorded score. `commit_hash` stamps the record for lineage.
pub fn run_recall_precision_bench(
    history: &ScoreHistory,
    commit_hash: Option<String>,
) -> SimardResult<ScoreRecord>;
```

The written record is:

```rust
ScoreRecord {
    suite_id:    "cognition".into(),
    scenario_id: "recall_precision_at_k".into(),
    score:       /* mean precision@k over the fixed corpus, 0.0..=1.0 */,
    timestamp:   /* unix epoch seconds */,
    commit_hash: /* Some(hash) for lineage */,
}
```

It flows through the **existing** gym signal machinery unchanged:
`generate_signals(&history, "cognition")` returns an `Improvement` /
`Regression` / `Stable` / `Promoted` [`GymSignal`](../reference/telemetry-metrics.md)
for the `recall_precision_at_k` scenario using the same `0.01` regression
threshold and 3-run promotion streak as every other suite.

### `ScoreRecord`

```rust
// src/gym_history/mod.rs
pub struct ScoreRecord {
    pub suite_id: String,
    pub scenario_id: String,
    pub score: f64,      // 0.0..=1.0
    pub timestamp: i64,  // unix epoch seconds
    pub commit_hash: Option<String>,
}
```

Persisted via `ScoreHistory::record(&ScoreRecord)`; read via
`ScoreHistory::latest(suite, scenario)` and
`ScoreHistory::history(suite, scenario, limit)` (parameterized SQL only).

### Shared score-history path

To guarantee the benchmark **writer**, the OODA cycle, and the correlation
**reader** all point at one database (design fix R5/DATA-3 — no writer/reader
drift), all three resolve it through a single helper:

```rust
// src/gym_history/mod.rs
/// The one canonical gym score-history database path, shared by the benchmark
/// writer, the OODA gym step, and the correlation reader. Resolved relative to
/// the process working directory: `<cwd>/gym_history.db`.
pub fn default_db_path() -> PathBuf;
```

There is exactly one such path; no request input ever reaches it.

## Operator command

A new `simard gym` subcommand runs the benchmark on demand and appends one
score. It reuses the existing operator-command auth path (no new unauthenticated
trigger).

```text
simard gym recall-precision      Run the fixed recall-precision benchmark,
                                  append one score to gym history, and print
                                  the score plus the gym signal.
```

Dispatch: `operator_cli::gym::dispatch_gym_command` → `run_gym_recall_precision`
(`operator_commands_gym`). The command rejects trailing arguments like the other
gym subcommands. Example output:

```text
$ simard gym recall-precision
cognition/recall_precision_at_k: score=0.8333 signal=improvement(+0.0500) samples=6
```

## Correlation endpoint

Read-only, query-time join of the latest benchmark score and the recent live
trend on the shared metric name.

```text
GET /api/cognition/recall-precision
```

**Authentication.** Registered **inside** the `require_auth` middleware layer in
`routes.rs` (fail-closed) — identical coverage to every other `/api/*` route. A
route-scope test asserts the path is auth-covered.

### Query parameters

All parameters are optional and **clamped, never rejected** (`unwrap_or(default)
.clamp(min, max)`), so a malformed or hostile value degrades to a safe bound
rather than erroring:

| Parameter | Type | Default | Clamp | Meaning |
|---|---|---|---|---|
| `bench_limit` | uint | `20` | `1..=200` | How many recent benchmark records to load for the run-over-run delta. |
| `live_limit` | uint | `200` | `1..=2000` | Max live `metrics.jsonl` samples to scan within the window. |
| `window_hours` | uint | `168` | `1..=8760` | Live look-back window (hours) for the trend. |

### Response schema (200)

```json
{
  "metric": "recall_precision_at_k",
  "benchmark": {
    "suite_id": "cognition",
    "scenario_id": "recall_precision_at_k",
    "score": 0.8333,
    "timestamp": 1751771000,
    "commit_hash": "2c32fe65",
    "signal": "improvement(+0.0500)",
    "previous_score": 0.7833
  },
  "live": {
    "window_hours": 168,
    "samples": 42,
    "first": 0.80,
    "latest": 0.82,
    "mean": 0.81,
    "trend_delta": 0.02,
    "series": [
      { "timestamp": "2026-07-05T12:00:00Z", "value": 0.80, "samples": 6 },
      { "timestamp": "2026-07-06T00:00:00Z", "value": 0.82, "samples": 9 }
    ]
  },
  "correlation": {
    "verdict": "confirmed",
    "benchmark_delta": 0.05,
    "live_trend_delta": 0.02,
    "threshold": 0.01,
    "explanation": "Benchmark and live trend both improved beyond the 0.01 threshold."
  },
  "generated_at": "2026-07-06T03:20:00Z"
}
```

### Correlation verdict

`correlation.verdict` compares `benchmark_delta` (latest vs previous benchmark
score) and `live_trend_delta` (live `latest - first` in-window), each against
`threshold` (`0.01`, the gym regression threshold). Each rail is first classified
by direction — **up** (`delta > +t`), **flat** (`|delta| <= t`), or **down**
(`delta < -t`) — and the verdict is a **total** function of the two directions.

Ordered rules (evaluated top-to-bottom; first match wins; `b` = `benchmark_delta`,
`l` = `live_trend_delta`):

| # | Verdict | Condition |
|---|---|---|
| 1 | `insufficient` | fewer than 2 benchmark records **or** fewer than 2 in-window live samples |
| 2 | `confirmed` | `b > +t` **and** `l > +t` |
| 3 | `diverging` | one rail up while the other is down: `(b > +t and l < -t)` **or** `(b < -t and l > +t)` |
| 4 | `regressed` | either rail down — `b < -t` **or** `l < -t` (rule 3 already consumed the "offset by a rise on the other rail" cases) |
| 5 | `benchmark-only` | `b > +t` (live is necessarily flat here) |
| 6 | `live-only` | `l > +t` (benchmark is necessarily flat here) |
| 7 | `stable` | otherwise — both within `±t` |

Equivalently, as a direction matrix (rows = benchmark, columns = live trend):

| bench ＼ live | up | flat | down |
|---|---|---|---|
| **up** | `confirmed` | `benchmark-only` | `diverging` |
| **flat** | `live-only` | `stable` | `regressed` |
| **down** | `diverging` | `regressed` | `regressed` |

The mapping is **total**: all nine direction combinations — plus `insufficient`
when a rail lacks history — resolve to exactly one verdict, so the handler never
returns an unclassified result. `diverging` is a first-class verdict, not a
catch-all: one rail improving while the other regresses is a stronger distrust
signal than a plain regression, so it is reported as its own contradiction rather
than folded into `regressed` or mislabelled as a one-rail improvement.

Only `confirmed` should back a "cognition improved" claim.

### Failure contract

The handler **degrades, never panics or leaks**:

- A missing/empty rail yields `benchmark: null` or `live: null` and
  `correlation.verdict: "insufficient"`; HTTP stays `200`.
- On a read error (corrupt DB, unreadable JSONL) the affected section is `null`
  and a top-level `"error"` field carries a **generic** message; specifics
  (paths, SQL, env, stack traces) go only to `tracing::warn!`, never into the
  JSON (design requirement DATA-1).

  > **Do not copy `memory_metrics` here.** The neighbouring
  > `native_memory_error` field puts the raw `e.to_string()` (and several
  > absolute `*_path` fields) straight into its JSON — a pre-existing DATA-1
  > leak. This endpoint deliberately **diverges** from that precedent: generic
  > message out, specifics to `tracing::warn!` only. The `memory_metrics` leak
  > is out of scope here; file it as a follow-up finding on
  > [#2491](https://github.com/rysweet/Simard/issues/2491) rather than
  > propagating the pattern.
- Corrupt `metrics.jsonl` lines and unreadable score rows are **skipped**, not
  fatal (no `.unwrap()` on external data).

## Relationship to the served word-boundary gate and the ranker

> **Interpretation caveat (issue [#4378](https://github.com/rysweet/Simard/issues/4378)).**
> `recall_precision_at_k` scores the **ungated ranker** with the **substring**
> relevance proxy — a definition that is *deliberately different* from the
> relevance gate a user is actually served. Read the metric as "of the ranker's
> top-`k`, the fraction relevant **under the substring proxy**", NOT as "the
> fraction of served facts that were relevant".

Three definitions of "is this fact relevant to the query?" coexist across the
cognition recall/measurement stack. They are individually deliberate but give
different answers for the same `(query, fact)` pair, so the metric can diverge
from both the ranker it measures and the gate that serves users:

| # | Layer | Where | Relevance definition | Gated? |
|---|---|---|---|---|
| 1 | **Served recall gate** | `LibraryCognitiveMemory::search_facts` (`fact_shares_query_relevance` / `needle_matches_word`) | **Word boundary** — a clean query token must prefix a whole word (plus conservative singular/plural folds). An interior/suffix hit (`act` in "re*act*or") is **not** relevant. | Yes |
| 2 | **Ranked recall** | `LibraryCognitiveMemory::recall_facts_ranked` | **Ungated keyword-Jaccard-dominated** weighted score over **every** live fact. | No |
| 3 | **Precision metric** | `metrics::precision_at_k` → `amplihack_memory::measurement` | **Substring** — a query token is a case-insensitive substring of `concept`/`content`. | n/a (oracle) |

The `recall_precision_at_k` self-metric is computed with definition **#3** over
the candidate set produced by **#2**, while the production recall path a user hits
is gated by **#1**. So a fact the word-boundary gate (#1) would exclude — because
the query token only appears in the interior of an unrelated word — can still
count as relevant under the substring proxy (#3), letting the metric read **higher
than served precision**.

### Why the divergence is intentional (not a bug to "fix" here)

- **Guideline G2** hosts the measurement primitive upstream in
  `amplihack-memory-lib` so the benchmark and live rails share one implementation
  (see [the primitive section](#the-measurement-primitive-upstream-g2)).
  Re-pointing #3 at #1's word-boundary definition would **fork** that upstream
  math into Simard, which G2 forbids.
- The **#2 ranker must stay ungated**: `precision_at_k < 1.0` is a meaningful
  ranking-quality signal *only* because the measured set includes lower-relevance
  facts the ranker floated into the top-`k`. Gating #2 would collapse
  `tests_ranked_recall`'s `recall_precision_isolates_text_relevance` /
  `recall_precision_at_k_baseline` measurement infrastructure.

Converging the three definitions is a **relevance-definition change**, which
`USER_PREFERENCES` routes to `CONSENSUS_WORKFLOW`; it is intentionally **out of
scope** for this reference and is not done implicitly. What *is* pinned here:

- The divergence and the agreement case are executable invariants in
  `src/cognitive_memory/tests_relevance_definition_divergence.rs`, so the three
  definitions cannot silently drift further apart and any future convergence is a
  deliberate, test-visible edit.
- The code sites cross-reference each other and this section
  (`metrics::precision_at_k`, `fact_shares_query_relevance`), so the "individually
  deliberate but collectively invisible" trap the divergence created is closed.

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `SIMARD_STATE_ROOT` | Root under which the live rail's `metrics/metrics.jsonl` lives (`<root>/metrics/metrics.jsonl`). | `$HOME/.simard` |
| working directory | `default_db_path()` resolves `gym_history.db` relative to the process CWD; the benchmark, OODA gym step, and correlation reader must share a CWD (the daemon's repo root) so all three agree. | `<cwd>/gym_history.db` |

No new configuration knob is introduced; the metric name, suite, scenario,
thresholds, and corpus are all compile-time constants.

## Security properties

- **AUTH** — endpoint inside `require_auth` (fail-closed); the bench command
  reuses existing operator-command auth. No new unauthenticated trigger.
- **VAL** — all query params clamped (never rejected); no unbounded/negative
  `LIMIT` or window. All SQL is parameterized; `suite`/`scenario` are
  compile-time constants.
- **DATA** — responses carry only aggregates, timestamps, and verdicts; the
  fixed corpus and the single `default_db_path()` mean no user-controlled path
  reaches the filesystem or SQL (no traversal). Error bodies are generic.

## See also

- [Concept: hybrid cognition measurement](../concepts/hybrid-cognition-measurement.md)
- [How to measure recall precision on both rails](../howto/measure-recall-precision-hybrid.md)
- [Telemetry metrics reference](./telemetry-metrics.md) — the live `metrics.jsonl` plumbing.
- [Tokenized fact recall in preparation](./cognitive-memory-fact-recall.md) — the served **word-boundary** recall gate (definition #1 above).
- [How to self-maintain dependency pins](../howto/self-maintain-dependency-pins.md)
  — the G2 lockstep pin-bump for the upstream primitive.
