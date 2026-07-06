# Investigation Report — Distill 100% Parse-Failure & Blocked kgpacks-rs Parity Goals

**Type:** Investigation (continuation / Round 2 — persists Round 1 synthesis)
**Date:** 2026-07-06
**Repo:** rysweet/Simard @ `92150406`
**Anomaly signal:** `overseer-obs:anomaly:distill parse-fail rate 100%`

---

## Executive Summary

Two coincident-but-**independent** problems were surfaced together by one Overseer
Observe pass and were mistakenly read as one:

1. **Distill 100% parse-fail** is a *process-health* defect in the cognitive-memory
   distillation parser. The original cause (launcher-banner / ANSI / pretty-envelope
   noise) was fixed and closed as **#2619**. A **residual** cause is still open as
   **#2658**: a **trailing comma** in the distiller agent's JSON makes strict
   `serde_json` reject the *entire* `{ "facts": [...] }` object, so every pass returns
   `Err` and the whole batch is silently deferred → the rate collapses to the
   Overseer's "100%". There is currently **no** lenient-JSON recovery anywhere in the
   parse path (verified: `grep -rn 'trailing_comma|json5|lenient_json' src/` → empty).

2. **The four blocked kgpacks-rs parity goals are blocked by their own goal-level
   dependency structure**, not by the distill failure. #12 (parity decision) is
   *resolved* (decision recorded, issue CLOSED) and any remaining board-block on it is
   a **stale block**; #16 (WS1 CVE eval harness) and #17 (WS2 int8/PQ, *gated on* #16)
   are genuinely OPEN, and the umbrella goal cannot close until they land.

The linking mechanism between the process signals and the goals is **correlational at
the Observe layer** (the Overseer emits `process:distill_fail`,
`resource:engineer_spawn`, `quality:gym_skipped`, and `GoalBlocked` as *independent*
`Signal`s derived from one `ObservedState`) plus a **systemic causal** relationship:
the distill failure starves the learning loop that would otherwise help advance goals,
which keeps engineer spawn elevated and the gym self-eval skipped.

**Top remediation:** land the **#2658 trailing-comma recovery** (P0) to restore the
learning loop, then unblock the parity chain in order **#12 (self-heal stale block) →
#16 (WS1) → #17 (WS2) → f29bb15c (umbrella)**.

---

## Criterion 1 — Root cause of "distill parse-fail rate 100%"

### 1a. Original cause — **CLOSED (#2619)**
Production distill is a `type: agent` step; recipe-runner captures the Copilot CLI's
full stdout into `step_results[].output`, prefixed with launcher banners
(`launching copilot`, `NODE_OPTIONS…`), ANSI-dimmed tracing timestamps, and a leading
`{"level":"info",…}` log object. The tolerant scanner took the **first** balanced
`{...}` span → it isolated the log object, never the real (last) `{"facts":…}` →
Tier-3 `Err` on every pass. Fixed by **#2504 / #2512 / #2517 / #2570** (ANSI strip,
launcher-preamble skip, prefer-last-facts), which closed #2619.

### 1b. Residual cause — **OPEN (#2658)** — *this is the live 100%*
**Evidence (`src/memory_consolidation/distillation.rs`):**

- `parse_recipe_output_full` (line ~710) runs three tolerant tiers. Tier 2 and the
  envelope path both terminate in `scan_for_facts_object` (line ~740).
- `scan_for_facts_object` balanced-brace scans for a `{...}` span and parses each
  candidate with **strict** `serde_json::from_str::<RecipeEnvelope>` (lines 743, 757).
- A **trailing comma** before a `}` / `]` — the single most common real-world LLM JSON
  defect, which the recipe prompt ("no surrounding prose") cannot fully prevent — is
  *invalid JSON*. `serde_json` rejects the **whole** object. The balanced-brace scan
  still finds the span (a trailing comma does not unbalance braces), but **every**
  candidate parse fails → `parse_recipe_output_full` returns the Tier-3 `Err`
  (line ~727) → caller treats `Err` as the retry-safe "no markers set / batch
  deferred" path (this is correct-by-design; there is never a hollow `Ok`).
- Result: the batch is deferred **every cycle**, `distill_parse_success_rate`
  collapses toward 0, and the Overseer reports **100%** parse-fail.

**Confirmed absence of recovery:** `grep -rn "trailing_comma\|json5\|lenient_json\|strip_json_trailing" src/` → **empty**. No lenient path exists.

### 1c. Secondary (distinct) symptom — surfaced, not conflated
Per #2619's ambiguity resolution: an *off-spec concept-label* filter in `into_facts`
can yield **zero facts on a successful parse** (successful parse, empty result). This
is a different failure mode from parse-fail and should emit a distinct "valid parse
yielded zero facts" log so it is never counted as parse-fail. Taxonomy redesign is out
of scope.

**Verdict:** Root cause of the current 100% = **#2658 trailing-comma → strict-parse
rejection of the whole facts object** (residual on the same distillation fact-yield
axis as the already-fixed #2619 banner cause).

---

## Criterion 2 — Dependency chain of the four blocked kgpacks-rs parity goals

| Goal ID | Goal name | Issue (rysweet/agent-kgpacks-rs) | Issue state |
|---|---|---|---|
| `f29bb15c` | advance-rysweet-agent-kgpacks-rs-to-full-parity | *umbrella* | — |
| `dbabd65f` | fix-agent-kgpacks-rs-issue-12-parity-decision | **#12** parity decision (hash vs BGE embeddings) | **CLOSED (resolved)** |
| `0c0ada69` | fix-agent-kgpacks-rs-issue-16-ws1-full-pack-cve | **#16** WS1 full-pack CVE eval | **OPEN** |
| `7f5afcca` | fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed | **#17** WS2 int8/PQ quantization | **OPEN** |

### Dependency graph
```
                       f29bb15c  advance-to-full-parity  (umbrella)
                          │  closes only when WS1 + WS2 land
          ┌───────────────┼────────────────────────────┐
          ▼               ▼                             ▼
    dbabd65f/#12     0c0ada69/#16                  7f5afcca/#17
   parity-decision   WS1: CVE eval harness  ◄────  WS2: int8/PQ quant
   CLOSED/RESOLVED    + eval questions       gate   GATED on WS1 eval
   (decision:         (OPEN — critical       (recall-  harness (recall
    ACCEPT            path prerequisite)      parity)   parity >= -0.02)
    intentional
    divergence;
    opt-in parity
    tracked in #32)
```

### Findings
- **#12 (dbabd65f) is DONE, not blocking.** The issue is CLOSED with an owner decision:
  *"ACCEPT the intentional divergence"* — the Rust port ships `deterministic-hash-v1`
  (768-d unit-norm bag-of-words) by design for hermeticity; true semantic BGE parity is
  an **opt-in, non-default** feature tracked separately in **#32** and explicitly
  *"does not block any current gate."* If the goal board still lists `dbabd65f`
  **Blocked**, that is a **stale / false-parked block** (a `GoalBlocked` with
  `perpetual` semantics) and should be self-healed/closed, not worked.
- **#16 (0c0ada69) is the true critical path.** WS1 must add a directory-confined
  `eval_questions.json` loader, commit ≥12 CVE questions (≥6 real 2024/2025 CVEs with
  reference answers), and produce `data/packs/cve/eval-results.{md,json}`. It is OPEN,
  no PR.
- **#17 (7f5afcca) is hard-gated on #16.** WS2 int8/PQ adoption is gated on running
  **"the WS1 eval harness"** and requires `delta_accuracy >= -0.02` + hit@k recall
  parity. It **cannot be validated until #16 lands the harness.** OPEN, no PR.
- **f29bb15c (umbrella) is blocked transitively** on #16 and #17. It is not blocked by
  #12 (resolved) and not blocked by the distill failure.

---

## Criterion 3 — Relationship: distill_fail / gym_skipped / engineer_spawn / workstream-gap ↔ blocked goals

The signals are produced by the Overseer's Observe→Orient model
(`src/overseer/signal.rs`, `src/overseer/mod.rs`, `src/overseer/observer.rs` — present
today **only in worktree** `feat/issue-2619-telemetry-anomaly-distill-parse-fail-rate-100`,
not yet in `main`).

`signals_from(state: &ObservedState)` derives each signal **independently** from durable
fields, then `classify_signal` maps each to a deduplicated `Problem`:

| Signal (enum) | ObservedState field | dedup_key | Kind / Priority |
|---|---|---|---|
| `DistillFailureRate{pct}` | `distill_fail_pct` | `process:distill_fail` | ProcessHealth / **High** |
| `EngineerSpawnRate{live}` | `live_engineers` | `resource:engineer_spawn` | ResourcePressure / Normal |
| `GymSkipped` | `gym_skipped` | `quality:gym_skipped` | QualityRegression / Low |
| `GoalBlocked{…}` | `blocked_goals` | `goal:stale:{id}` / hygiene | GoalHygiene |
| `Anomaly{detail}` | `TelemetrySignals.anomalies[]` | `anomaly:{detail}` | ProcessHealth / Normal |

**Two truths, kept distinct:**

1. **At the code layer these signals are correlated, not causally chained.** They are
   parallel Observe outputs folded into ranked `Problem`s. "workstream-gap" is the
   Overseer's M2+ observation that in-flight workstreams don't cover an open High/
   Critical problem (here: `process:distill_fail`). `GoalBlocked` for the parity goals
   is derived from the goal board's own `blocked_goals`, **not** from `distill_fail`.
   So: **the parity goals are NOT blocked *because of* the distill failure.**

2. **At the systems layer there is a real causal coupling through the learning loop.**
   Distill is the episode→fact consolidation that feeds cognitive memory. At 100%
   parse-fail:
   - zero new facts are stored each cycle (batch deferred) → **memory goes stale**;
   - engineers/brain that consult memory for enriched context advance goals more
     slowly → **`engineer_spawn` stays elevated** (re-spawning against the same
     unadvanced goals) and **workstream-gap** persists;
   - the **gym self-eval is skipped** (`gym_skipped`) because there is no fresh
     distilled signal to evaluate.
   This is why `distill_fail` is **High** priority: it starves the loop that would
   otherwise help unblock work. But it is a *systemic drag*, not the *proximate* blocker
   of the four parity goals.

**Net:** treat `process:distill_fail` (#2658) as the high-priority systemic fix, and
treat the parity `GoalBlocked`s as a **separate** goal-dependency problem. Fixing
distill will not by itself flip #16/#17 to done; fixing #16/#17 will not lower the
distill parse-fail rate.

---

## Criterion 4 — Prioritized remediation plan (with target files)

### P0 — Restore the learning loop: fix residual distill parse (#2658)
- **What:** Add a string-aware, last-resort `strip_json_trailing_commas` recovery and
  retry the facts-object parse **only after** the strict parse fails. A trailing comma
  is never valid JSON, so the stripper is a provable no-op on well-formed input (clean
  path byte-identical, zero-alloc) → no precision loss.
- **Target files:**
  - `src/memory_consolidation/distillation.rs` — `scan_for_facts_object` (line ~740):
    on strict-parse miss, retry `serde_json::from_str` against a comma-stripped view.
    Prefer a shared `recipe_output::extract` helper so OODA/brain parse paths can reuse
    it.
  - `src/memory_consolidation/distillation_tests.rs` — regression fixtures: a
    trailing-comma `{ "facts": [...] }` both bare and inside a `--output-format json`
    envelope `step_results[].output`; assert ≥1 fact recovered; assert a comma **inside**
    a string is untouched; assert a genuinely malformed (non-trailing-comma) object
    still `Err`s.
  - Telemetry: ensure `distill_parse` success/fail is recorded per pass via
    `self_metrics::record_metric` so the rate is observable driving toward 1.0.
  - A3 log: emit a distinct "valid parse yielded zero facts" warning in `into_facts`.
- **Status:** branches exist (`feat/issue-2658-distill-tolerate-trailing-comma`,
  `feat/issue-2658-distillation-parse-failure-rate-100`) but carry **no diff vs main**
  and there is **no open PR** — the fix is *not yet implemented*.
- **Done when:** `cargo build` + `cargo test memory_consolidation::distillation` pass;
  before/after benchmark records 0.000 → 1.000 recovery; `distill_parse_success_rate`
  trends to ~1.0.

### P1 — Promote the Overseer signal taxonomy to `main`
- **What:** The `src/overseer/*` Observe→Orient model (`signal.rs`, `observer.rs`,
  `sensor.rs`, `mod.rs`) that emits `process:distill_fail`, `resource:engineer_spawn`,
  `quality:gym_skipped`, and `GoalBlocked` lives only in the issue-2619 worktree. Land
  it on `main` so these signals become first-class and can auto-open/auto-route
  remediation (and self-heal stale `GoalBlocked`s).

### P1 — Unblock the kgpacks-rs parity chain (correct order)
1. **`dbabd65f` / #12 — self-heal the stale block.** Decision is recorded and #12 is
   CLOSED; mark the goal done / close the board-block. Do **not** re-litigate the
   embeddings decision (opt-in semantic parity already tracked in #32).
2. **`0c0ada69` / #16 (WS1) — critical path.** In `crates/kgpacks-eval` + `data/`:
   add the dir-confined `eval_questions.json` loader; commit ≥12 CVE questions
   (≥6 real, verifiable 2024/2025 CVEs with reference answers); run the eval where an
   LLM transport exists and commit `data/packs/cve/eval-results.{md,json}` (else commit
   deterministic hit@k recall + document the transport blocker); keep CI offline via a
   mock transport.
3. **`7f5afcca` / #17 (WS2) — after #16.** In `crates/kgpacks-embeddings`: implement
   `quantize_int8`/`dequantize_int8` (scale = max|v|/127, bound-checked, all-zero safe,
   cosine > 0.999 on L2-normalized vectors), additive pack format only; **gate adoption**
   on the WS1 harness (`delta_accuracy >= -0.02` + hit@k parity). Ship behind a flag
   only if parity holds; otherwise leave DISABLED and commit spike findings.
4. **`f29bb15c` (umbrella)** closes automatically once #16 and #17 land.

### Sequencing rationale
P0 is independent and highest systemic value (unstarves memory for *all* goals). The
parity chain is strictly ordered by its own gates: #12 (unblock/close) → #16 (build the
harness) → #17 (needs the harness) → umbrella. Distill and parity are parallel tracks;
neither blocks the other.

---

## Evidence Index
- `src/memory_consolidation/distillation.rs` — `parse_recipe_output_full` (~710),
  `scan_for_facts_object` (~740), `RecipeRunnerEnvelope::into_distill_output` (~800).
- `grep -rn "trailing_comma|json5|lenient_json|strip_json_trailing" src/` → empty
  (no lenient recovery exists).
- Issues: **#2619** (CLOSED, banner cause), **#2658** (OPEN, residual trailing-comma),
  history #2401/#2461/#2468/#2504/#2512/#2517/#2570.
- Worktree `feat/issue-2619-telemetry-anomaly-distill-parse-fail-rate-100`:
  `src/overseer/signal.rs` (Signal enum + `signals_from`), `src/overseer/mod.rs`
  (`classify_signal`: `process:distill_fail`/`resource:engineer_spawn`/
  `quality:gym_skipped`), `src/overseer/sensor.rs` (`ObservedState` fields).
- rysweet/agent-kgpacks-rs issues **#12** (CLOSED, decision recorded; parity opt-in in
  #32), **#16** (OPEN, WS1 CVE eval), **#17** (OPEN, WS2 int8/PQ gated on WS1).
