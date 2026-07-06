# Investigation Report — Distill 100% Parse-Failure & Blocked kgpacks-rs Parity Goals

**Type:** Investigation (continuation / Round 2 — persists Round 1 synthesis)
**Date:** 2026-07-06
**Repo:** rysweet/Simard @ `92150406`
**Anomaly signal:** `overseer-obs:anomaly:distill parse-fail rate 100%`

> **⚠️ Read the "Consolidated Findings (Final)" section at the very bottom first.**
> It reconciles all parallel deep dives (Track A parse-fail, Track B parity
> goals, and the Overseer signal-emission/dedup model) into one self-contained,
> live-`main`-verified answer. Where it disagrees with anything above, **the
> Consolidated section wins.**
>
> Round 1/2 were written on the investigation branch base `92150406`, which is
> **92 commits behind `main` (946fe3ca)**. Several Round-1 claims were correct
> *for that stale base* but are **wrong for the live system (`main`)**. The
> Round-3 addendum re-verifies every criterion against `main` with file:line
> evidence; the Consolidated section folds those corrections in.

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

---

# Round-3 Verification Addendum — Live-`main` Re-Verification & Corrections

**Verifier:** primary investigator (Track A live parse-fail root cause + coupling verdict)
**Authoritative branch:** `main` @ `946fe3ca` (the daemon runs `main`).
**Investigation branch:** `investigation/distill-parsefail-kgpacks-parity-2658` @ `6cef8dbb`
(parent `92150406`) — **92 commits behind `main`** (`git rev-list --count HEAD..main` = 92).
Every Round-1/2 file:line that cites `src/…` from the investigation checkout must be
re-read on `main`, where `distillation.rs` is **3222 lines** (vs 1039 on the stale
branch) and `src/overseer/` **exists** (it does *not* exist in the investigation
checkout).

## What Round 1/2 got RIGHT (re-confirmed on `main`)

- **Track A root cause holds on the live branch.** The 100% distill parse-fail is the
  **#2658 residual: an LLM **trailing comma** makes strict `serde_json` reject the whole
  `{ "facts": [...] }` object.** Confirmed still-live on `main`:
  - Every parse site is **strict** `serde_json::from_str` / `from_value` (serde_json
    **1.0.149**, stock registry crate — `Cargo.lock`; no lenient fork):
    `distillation.rs:1219` (Tier-1a `RecipeRunnerEnvelope`), `:1382`/`:1392` (recovery
    candidate views), `:1473`/`:1494` (`scan_cleaned_for_facts`).
  - A trailing comma keeps braces balanced, so `recipe_output::balanced_objects` still
    yields the span (`:1490`), but strict parse rejects it → `recover_distill_output`
    returns `None` (`:1305-1340`) → `scan_for_facts_object` returns `None`
    (`:1420-1452`) → Tier-3 `Err` (`:1250-1254`) → batch deferred **every cycle** →
    `distill_parse_success_rate` → 0 → Overseer reports **100%**.
  - **No trailing-comma / json5 / lenient-JSON tolerance anywhere in `main` `src/`**:
    `git grep -in 'trailing_comma|json5|json_repair|jsonc|relaxed_json|sanitize_json'
    main -- src/` → **empty**. Verified independently of Round-1's grep.
  - `#2658` issue title (maintainer) literally: *"distill: residual 100% parse-failure —
    agent JSON trailing comma drops the whole batch"* — triangulates the code trace.
- **#2658 ≠ #2619 (causal distinction).** The banner/ANSI/pretty-envelope cause is
  **fixed and closed** (`#2619` CLOSED 2026-07-06) and its fixes are present on `main`
  (`recover_distill_output` + `strip_recipe_noise`/`strip_ansi` dual views + prefer-
  last/grounded object: `distillation.rs:1223-1452`). Because those fixes landed, the
  banner cause **cannot** be the live 100%; the residual **trailing comma** is.
  Representative failing input class (both reject on serde_json 1.0.149; braces stay
  balanced): `{"facts":[{"concept":"bug-pattern","content":"x","source_episode_id":"e1"},]}`
  and `{"facts":[{...,"source_episode_id":"e1",}]}`.
- **Track B classification holds** (issue-state verified via `gh`):
  kgpacks-rs **#12 CLOSED** 2026-07-05 (parity decision, intentional divergence) →
  goal `dbabd65f` board-block is **STALE**; **#16 OPEN** (WS1 CVE eval) = critical path
  (`0c0ada69`); **#17 OPEN** (WS2 int8/PQ, gated on eval recall parity) = gated on #16
  (`7f5afcca`); **#32 OPEN** (optional non-default semantic embeddings) = non-blocking;
  umbrella `f29bb15c` blocked transitively on #16/#17, **not** on #12 or on distill.
- **Recurrence = unremediated re-emission, NOT a dedup defect** (confirmed in code):
  `signals_from(&ObservedState)` is **stateless/threshold-based**, regenerated fresh each
  Observe pass (`src/overseer/signal.rs`); the only dedup is **within-pass** merge on
  `dedup_key` in `orient()` (`src/overseer/mod.rs`) and an **intervention-level** 15-min
  `WhisperGate` (`src/overseer/guardrails.rs`). There is **no inter-pass signal dedup**,
  so a persistent condition (distill 100% + blocked goals) is re-emitted every pass. The
  2× asymmetry (2nd pass **adds** `resource:engineer_spawn`, **drops** the issue-12
  goal-block) is explained by **state evolution** between passes — `live_engineers`
  crossed its threshold and kgpacks-rs **#12 closed 2026-07-05** clearing its
  `goal:blocked` — which *confirms* re-emission and *rules out* an emit-duplicate defect.

## What Round 1/2 got WRONG (corrected against `main`)

1. **"Overseer signal taxonomy lives only in the #2619 worktree, not yet in `main`"
   (Criterion 3 / P1) — FALSE.** The taxonomy **is on `main`**:
   `src/overseer/mod.rs` emits `"process:distill_fail"` (`:714`), `"resource:engineer_spawn"`
   (`:741`), `"quality:gym_skipped"` (`:753`), `format!("goal:blocked:{goal_id}")` (`:807`),
   `format!("anomaly:{detail}")` (`:777`). The investigation *checkout* lacks `src/overseer/`
   only because it is 92 commits stale. **P1 "promote the taxonomy to `main`" is MOOT —
   already landed.**

2. **Coupling story is OVER-STATED; the gym link is REFUTED.** Round-1 claimed the gym
   self-eval is skipped *because* distill produces no fresh signal. **Code refutes this:**
   `gym_skipped` is driven by the **manual env flag** `SIMARD_SKIP_GYM`
   (`src/status/provider.rs:61`; also `src/overseer/sensor.rs:125-126`) — it has **zero**
   dependence on distill. `git grep distill main -- src/gym/` → **empty** (gym never reads
   distilled facts). Likewise `live_engineers` = live worktree-claim count
   (`provider.rs:177`, `count_live_engineer_claims`, #2432) and `distill_fail_pct` =
   `DISTILL_RUNS{result=parse_fail}/total` (`provider.rs:490-497`). **All three signals are
   independent `ObservedState` fields from independent subsystems**, read in one Observe
   pass → the coupling is **CORRELATIONAL at the code layer, with no causal chain.**

3. **Coupling verdict (my primary deliverable): Track A and Track B are INDEPENDENT.**
   `GoalBlocked` for the parity goals derives from the goal board's own `blocked_goals`
   (`sensor.rs`), never from `distill_fail`. Distilled facts feed **cognitive memory**
   (`src/cognitive_memory/*`, `memory_consolidation/mod.rs` "episodic→procedural learning
   loop"), **not** the gym and **not** goal advancement — no code path makes a parity
   `GoalBlocked` depend on distill output. **Verdict: fixing #2658 is neither necessary
   nor sufficient to unblock #16/#17 — it is INDEPENDENT.** It is necessary-and-sufficient
   only for the `process:distill_fail` signal itself (and the memory-staleness it causes).
   The residual "systemic drag" hypothesis (stale semantic memory slows brain/engineers)
   is *plausible but unproven* and must not be stated as causation.

4. **"#2658 branches carry no diff vs `main`" — HALF WRONG.**
   `feat/issue-2658-distill-tolerate-trailing-comma` **is at `main` tip `946fe3ca`** →
   truly empty, no fix (Round-1 correct here). But
   `feat/issue-2658-distillation-parse-failure-rate-100` (`db117b98`) is `main` **plus**
   divergent work that **does** modify `distillation.rs` — however its approach is
   **retry + JSON-format reinforcement** (`DISTILL_PARSE_RETRY_MAX`, `run_all_reinforced`,
   #2468) and **facts-file capture** (#2622/#2619), **not** trailing-comma stripping. So
   **no branch and no open PR adds trailing-comma tolerance.** Open PRs are docs-only:
   **#2668** (this investigation) and **#2657** (stale-deploy runbook). The #2658 code fix
   is genuinely **not yet implemented anywhere.**

5. **`workstream-gap` is NOT a code-emitted token.** `git grep -in
   'workstream.gap|workstream_gap|coverage' main -- src/overseer/` and `-- src/` →
   **empty**. `provider.rs` anomaly assembly only emits `"distill parse-fail rate N%"`,
   `"telemetry cardinality overflow (…)"`, `"daily LLM budget exceeded"`,
   `"brain decide ladder exhausted (…)"` (`provider.rs:~510-537`). Round-1's attribution
   of `workstream-gap` to an "Overseer M2+ in-flight-coverage observation" is **unverified
   — no such emitter exists on `main`.** It most likely originates in the Overseer
   brief/whisper narrative or the #2669 author's synthesis, not `signals_from`.

## Telemetry recording semantics (Criterion / status audit)

`distill_success_rate` and `distill_parse_success_rate` are recorded **per distill run**
via `record_distill_success_metric` (`distillation.rs:888`, emits at `:908` and `:929`;
failure path `:369`, success path `:573`); the Overseer's `distill_fail_pct` is then
computed per Observe pass from the `DISTILL_RUNS{result=ok|parse_fail}` counters
(`status/provider.rs:490-497`), with `parse_fix_holding = (pct == 0.0)` (`:540`).

## Corrected bottom line

- **Track A (live 100% parse-fail):** root cause = **#2658 trailing-comma → strict
  serde_json rejection of the whole facts object**, still live on `main`; **no** lenient
  recovery exists (serde_json 1.0.149 stock). Distinct from the already-fixed **#2619**
  banner cause. **OPEN, no fix on any branch/PR.**
- **Coupling verdict:** **correlational only** (independent Observe signals); the gym link
  is **refuted** (env-flag driven). **Track A remediation does NOT unblock Track B — they
  are independent tracks.**
- **Track B:** unblock order `#12` (self-heal stale block / close goal `dbabd65f`) →
  `#16` (WS1, critical path) → `#17` (WS2, gated on #16) → umbrella `f29bb15c`. Unaffected
  by the distill fix.
- **P1 "promote signal taxonomy to `main`" is obsolete** (already on `main`).

## Evidence index (Round-3, all on `main` @ 946fe3ca unless noted)
- `src/memory_consolidation/distillation.rs`: `parse_recipe_output_full` (1212-1255),
  `recover_distill_output` (1305-1341), `scan_for_facts_object` (1420-1453),
  `scan_cleaned_for_facts` (1471-…, strict parses at 1473/1494), `de_lenient_string`
  (field-value coercion only, 1645-1661), telemetry (888/908/929/369/573).
- `src/overseer/mod.rs` dedup keys: 714/741/753/777/807. `src/overseer/signal.rs`
  `signals_from` (stateless per-pass). `src/overseer/guardrails.rs` `WhisperGate` (15-min).
- `src/status/provider.rs`: `gym_skipped` env-flag (61), `distill_fail_pct` (490-497/540),
  `live_engineers` (177). `src/gym/` — no distill reference (no code coupling).
- `git grep` (main): trailing-comma/json5 tolerance → empty; `workstream-gap`/coverage →
  empty. `Cargo.lock`: serde_json 1.0.149.
- Issues (verified via `gh`): Simard **#2619 CLOSED**, **#2658 OPEN**, **#2669 OPEN**;
  kgpacks-rs **#12 CLOSED**, **#16 OPEN**, **#17 OPEN**, **#32 OPEN**.
- Open PRs (docs-only): Simard **#2668**, **#2657**. No code fix for #2658.

---

# Tertiary (Track B) Deep-Dive Addendum — Parity Goal Dependency Graph & Stale-vs-Open Classification

**Investigator:** tertiary (architect) — *kgpacks-rs parity goal dependency graph and
stale-vs-open block classification with issue-state evidence.*
**Verification base:** `main` @ `946fe3ca` (the branch the daemon Observes) + live `gh`
issue/PR state for `rysweet/agent-kgpacks-rs`. Confirms and **mechanistically explains**
the Round-1/Round-3 Track B classification; adds the completion-gate root mechanism and
the full workstream landscape.

## 1. The block set is RE-DERIVED every pass (stateless), not a persisted stale list

`sensor::blocked_goals_from_board(board)` is a **pure, stateless projection** re-computed
each Observe pass: it yields one `BlockedGoal` per `board.active` goal whose status is
`GoalProgress::Blocked(reason)` (`src/overseer/sensor.rs:188-205`). There is **no
persisted "blocked list"** — a goal appears in the signature *only while* its live board
status is `Blocked`. This is the architectural reason the 2× signature is asymmetric: the
second pass **drops `dbabd65f`/#12** because #12 closed (2026-07-05) and the goal stopped
projecting as `Blocked`. So "recurrence" of the parity blocks = **honest per-pass
re-derivation of a still-true board state**, and the #12 drop = **the board reconciling a
now-resolved node** — not a dedup defect (consistent with the secondary track).

## 2. Two distinct goal-hygiene signals — all four goals emitted `goal:blocked`, none `goal:stale`

The classifier (`src/overseer/mod.rs`) emits **two different** goal-hygiene dedup keys:
- `Signal::StaleGoal` → `goal:stale:{id}` — "re-litigated / stale-complete" (`:767-771`).
- `Signal::GoalBlocked` → `goal:blocked:{id}` — GoalHygiene, `High` iff `needs_review`
  else `Normal` (`:795-815`).

Every goal in the observed signature carries `goal:blocked:{id}`, so at Observe time each
of the four (incl. #12) was a live `GoalProgress::Blocked` on the active board — **not**
yet reclassified as `StaleGoal`. The stewardship router `decide_blocked_goal`
(`src/overseer/mod.rs:955-968`) then routes each block: `perpetual && no_progress_marker`
→ `UnblockGoal` (self-heal false-park); `needs_review` → `EscalateBlockedGoal`; plain
dependency/operator block → `Report` (respect it, leave untouched).

## 3. Root mechanism of "stale vs open" — the deploy-aware done-gate needs THREE proofs

`goal_curation::completion_gate` (`src/goal_curation/completion_gate.rs`) certifies a goal
`Complete` **only** with `pr_merged` **AND** `issue_closed` **AND** `deployed`
(`:369-393`; `deployed` is auto-true for non-self-affecting goals). Any missing proof →
`Blocked{ missing:[…] }`, and the goal is **retained** on the active board with the
missing-evidence annotation (`archive_completed_with_evidence`, `:493-520`). Perpetual/
standing goals **never archive** (#2580): if driven to a terminal-looking status they
`roll_to_new_cycle()` in place (`:508-515`). This gate is what mechanically separates a
*stale* block from a *genuine* one, per the table below.

## 4. Per-goal classification (issue-state + PR-state + gate evidence)

| Goal | Issue | Issue state | Merged PR? | Done-gate verdict | Classification |
|---|---|---|---|---|---|
| `f29bb15c` advance-to-full-parity (umbrella) | — | standing umbrella | n/a | never archives; rolls each cycle | **STANDING (by design)** — active until all parity WSs land; not a stuck defect |
| `dbabd65f` | **#12** parity-decision | **CLOSED** 2026-07-05 | **NONE** (decision-only; no code PR) | `issue_closed=✓`, `pr_merged=✗` → Blocked[PrNotMerged] | **STALE / false-park** — work done, gate can't certify (no merge artifact) |
| `0c0ada69` | **#16** WS1 CVE eval | **OPEN** | **NONE** | Blocked[PrNotMerged, IssueOpen] | **GENUINELY-OPEN — critical path** |
| `7f5afcca` | **#17** WS2 int8/PQ | **OPEN** | **NONE** | Blocked[PrNotMerged, IssueOpen] | **GENUINELY-OPEN — hard-gated on #16** |

**Key mechanism for #12/`dbabd65f`:** #12 was closed as a **decision** ("ACCEPT the
intentional deterministic-hash divergence; opt-in semantic BGE tracked in **#32 OPEN,
non-default**"). No code PR references #12 (PR audit below). The completion gate's
`pr_merged` requirement therefore can **never** be satisfied for this decision-only node,
so a non-perpetual `dbabd65f` would pin as `Blocked[PrNotMerged]` forever — the textbook
false-park the Overseer's `UnblockGoal` self-heal exists to clear. **Remediation: complete/
`simard goal unblock dbabd65f`; do NOT re-litigate the embeddings decision (#32 owns it).**

**#17 gate on #16 is explicit, not inferred:** #17's own body — *"Parity gate: run **the
WS1 eval harness** on a quantized pack; adopt only if `delta_accuracy >= -0.02` AND
retrieval hit@k parity."* #16 (WS1) *builds* that harness. #17 cannot be validated until
#16 lands → hard sequence `#16 → #17`.

## 5. Full workstream landscape — #16/#17 are the ONLY stalled parity sub-chain

The umbrella `f29bb15c` spans far more than the three nodes in the signature. Live
`rysweet/agent-kgpacks-rs` state:

| WS | Issue | State | PR | Note |
|---|---|---|---|---|
| WS1 | **#16** | OPEN | **none** | eval harness — **critical path**, zero work in flight |
| WS2 | **#17** | OPEN | **none** | int8/PQ — gated on #16 |
| WS3 | #18 | CLOSED | #34 merged | versioned release tags |
| WS4 | #19 | CLOSED | #35 merged | XDG data dir |
| WS5 | #20 | CLOSED | #33 merged | CI >2GiB coverage |
| WS6 | #21 | OPEN | none | resumable pipelined build |
| WS7 | #22 | OPEN | **#36 open** | sign release index |
| WS8 | #23 | OPEN | none | ENTITY_RELATION bulk load |
| — | #25 | OPEN | none | fetch CVE corpus (cvelistV5) |
| decision follow-up | #32 | OPEN | none | optional semantic embeddings (non-default, non-blocking) |

**Architectural read:** the parity effort is *actively advancing* on WS3/4/5 (merged) and
WS7 (PR #36), while **WS1/WS2 (#16/#17) — the eval+quant sub-chain — sat untouched with no
PRs.** So #16 is not merely "next in the queue"; it is a **genuinely stalled critical-path
node** whose stall also holds #17. The blocked signature surfaced a coherent, correct
board picture: the stuck sub-chain (#16→#17) + the stale decision node (#12) + the standing
umbrella. **No spurious blocks; no missing genuine blocks.**

## 6. Re-derived-vs-stale verdict & coupling

- **All four are re-derived each cycle** by the stateless projection (§1) — none is a
  persisted phantom.
- Only **`dbabd65f`/#12 is content-stale** (resolved issue + no merge artifact → an
  ungate-able false-park); **`f29bb15c` persists by design** (standing goal); **#16 and
  #17 are correctly, genuinely blocked** until real work + merged PRs land.
- **Coupling to Track A: independent (re-confirmed).** `blocked_goals_from_board` reads
  only `GoalBoard.active` goal *status*; nothing reads `distill_fail_pct`. Fixing #2658
  neither unblocks #16/#17 nor un-stales #12. The two tracks share only the single Observe
  pass that surfaced them together.

## 7. Track-B remediation (ordered; unchanged by the distill fix)

1. **`dbabd65f`/#12 — self-heal the stale block:** complete/unblock the goal (decision
   recorded, issue CLOSED, opt-in tracked in #32). Do not re-open the embeddings decision.
2. **`0c0ada69`/#16 (WS1) — critical path:** add the dir-confined `eval_questions.json`
   loader, commit ≥12 CVE questions (≥6 real 2024/2025 CVEs w/ reference answers), commit
   `data/packs/cve/eval-results.{md,json}` (or documented hit@k), CI offline via mock.
3. **`7f5afcca`/#17 (WS2) — after #16:** implement `quantize_int8`/`dequantize_int8`
   (scale=max|v|/127, bound-checked, all-zero safe, cosine>0.999 on L2-norm), additive
   format; **gate adoption on the WS1 harness** (`delta_accuracy >= -0.02` + hit@k parity);
   ship behind a flag only if parity holds, else DISABLED + spike findings.
4. **`f29bb15c` (umbrella)** rolls to a fresh cycle and only retires once the parity
   workstreams (#16/#17 and siblings #21/#22/#23/#25; #32 is non-default/non-blocking) land.

## 8. Tertiary evidence index
- `src/overseer/sensor.rs:188-205` `blocked_goals_from_board` (stateless per-pass projection);
  `capabilities.rs:132-158` `BlockedGoal` struct (perpetual/needs_review/consecutive_no_action).
- `src/overseer/mod.rs:767-771` `goal:stale`, `:795-815` `goal:blocked`, `:955-968`
  `decide_blocked_goal` (UnblockGoal / EscalateBlockedGoal / Report).
- `src/goal_curation/completion_gate.rs:1-18` gate doc, `:347-393` `evaluate`
  (pr_merged ∧ issue_closed ∧ deployed), `:475-520` perpetual-never-archive + retain-blocked.
- `gh` (rysweet/agent-kgpacks-rs): **#12 CLOSED** 2026-07-05 (decision, no PR), **#16 OPEN**
  (no PR), **#17 OPEN** (no PR, gated on WS1 harness per body), **#32 OPEN** (non-default).
  Sibling PRs: #33→#20, #34→#18, #35→#19 MERGED; #36→#22 OPEN. **No PR references #16 or #17.**

---

# Consolidated Findings (Final) — All Parallel Deep Dives Reconciled

**Status:** AUTHORITATIVE. Supersedes Round 1/2 body and folds in the Round-3 +
Tertiary addenda. Every claim below re-verified against **`main` @ `946fe3ca`**
(the branch the daemon Observes) and live `gh` state on 2026-07-06.

## 0. One-line verdict

The recurring 2× signature is **two coincident-but-independent conditions**
surfaced together by one Overseer Observe pass and honestly **re-emitted** each
pass (there is no dedup defect): **(A)** a live 100% distill parse-failure caused
by an **LLM trailing comma** that makes stock `serde_json` reject the whole facts
object (**#2658 OPEN, no fix on any branch/PR**), and **(B)** a parity-goal
dependency cluster that is **correctly blocked** — a stale decision node (#12) +
a genuinely-stalled critical sub-chain (#16 → #17) under a standing umbrella
(f29bb15c). **The two tracks are independent**; fixing one does not affect the
other.

## 1. The signature decoded (each token, verified)

| Signature token | Meaning | Emitter (main) | Verified |
|---|---|---|---|
| `anomaly:distill parse-fail rate 100%` | 100% of distill runs return parse `Err` | `status/provider.rs` anomaly assembly → `Signal::Anomaly` → `format!("anomaly:{detail}")` `mod.rs:777` | ✅ live |
| `process:distill_fail` | `DistillFailureRate` classified (High/ProcessHealth) | `mod.rs:714` | ✅ live |
| `resource:engineer_spawn` | `EngineerSpawnRate` (`live_engineers ≥ 8`) | `mod.rs:741`; field `provider.rs:177` | ✅ live |
| `quality:gym_skipped` | `GymSkipped` (Low/QualityRegression) | `mod.rs:753`; driven by **`SIMARD_SKIP_GYM`** env flag `provider.rs:61` + `sensor.rs:125-126` | ✅ live |
| `goal:blocked:{id}` ×4 (f29bb15c, dbabd65f, 0c0ada69, 7f5afcca) | one per `GoalProgress::Blocked` active goal | `mod.rs:807`; projection `sensor.rs:188-205` | ✅ live |
| `workstream-gap` | **NOT a code-emitted token** — no emitter in `src/`; originates in the Overseer brief/whisper narrative or #2669 author synthesis | `git grep workstream.gap main -- src/` → **empty** | ✅ refuted as code token |

## 2. Track A — root cause of "distill parse-fail rate 100%"

**Root cause (live on `main`):** an LLM **trailing comma** before `}`/`]` in the
distiller agent's `{ "facts": [...] }`. A trailing comma keeps braces balanced,
so `recipe_output::balanced_objects` still finds the span, but **stock
`serde_json` 1.0.149 rejects the whole object** → every candidate parse fails →
`recover_distill_output` returns `None` (`distillation.rs:1305-1341`) →
`scan_for_facts_object` returns `None` (`:1420-1453`) → `parse_recipe_output_full`
returns Tier-3 `Err` (`:1212-1255`) → batch deferred **every cycle** →
`distill_parse_success_rate → 0` → Overseer reports **100%**.

- **No lenient-JSON recovery exists on `main`:** `git grep -in
  'trailing_comma|json5|json_repair|jsonc|relaxed_json|sanitize_json|
  strip_json_trailing|lenient_json' main -- src/` → **empty** (verified now).
- **Distinct from #2619 (CLOSED).** The banner/ANSI/pretty-envelope cause was
  fixed (`recover_distill_output` + ANSI-strip dual views + prefer-last-facts,
  `distillation.rs:1223-1452`) and closed. Because those fixes landed, the banner
  cause **cannot** be the live 100%; the residual **trailing comma** is.
- **Issue triangulation:** `#2658 OPEN` title (maintainer) = *"distill: residual
  100% parse-failure — agent JSON trailing comma drops the whole batch."*
- **No fix in flight:** `feat/issue-2658-distill-tolerate-trailing-comma` = `main`
  tip (empty). `feat/issue-2658-distillation-parse-failure-rate-100` (`db117b98`)
  diverges but does **retry + JSON-format reinforcement**, *not* comma stripping.
  Open PRs are **docs-only** (#2668 this investigation, #2657 runbook).

**Secondary, distinct symptom (do not conflate):** a valid parse can yield **zero
facts** via the `into_facts` concept-label filter — a different mode from
parse-fail; should log "valid parse yielded zero facts" separately.

## 3. Track B — the four blocked parity goals (dependency + stale/open)

Block set is **re-derived every pass** by the stateless projection
`blocked_goals_from_board` (`sensor.rs:188-205`) — no persisted "stale list." The
deploy-aware done-gate `completion_gate` certifies `Complete` only with
`pr_merged ∧ issue_closed ∧ deployed` (`goal_curation/completion_gate.rs:347-393`).

| Goal | Issue | Live state | Merged PR | Classification |
|---|---|---|---|---|
| `f29bb15c` umbrella | — | standing | n/a | **STANDING by design** — rolls each cycle; retires only when parity WSs land |
| `dbabd65f` | **#12** | **CLOSED** (intentional divergence; opt-in BGE → **#32 OPEN**, non-default) | none | **STALE / false-park** — decision done, gate can't certify (no merge artifact). Self-heal via `UnblockGoal`; do **not** re-litigate |
| `0c0ada69` | **#16** | **OPEN** | none | **GENUINELY OPEN — critical path** (WS1 CVE eval harness) |
| `7f5afcca` | **#17** | **OPEN** | none | **GENUINELY OPEN — hard-gated on #16** (WS2 int8/PQ; adopt only if `delta_accuracy ≥ -0.02` + hit@k parity, run on the WS1 harness) |

Full board is *advancing* elsewhere (WS3/#18, WS4/#19, WS5/#20 CLOSED+merged;
WS7/#22 PR #36 open) — so #16→#17 is a **genuinely stalled sub-chain**, not merely
"next in queue." No spurious blocks, no missing genuine blocks.

## 4. Overseer signal-emission & dedup model — WHY it recurs 2×

**Emission is stateless/threshold-based.** `signals_from(&ObservedState)`
(`signal.rs:122`) regenerates every signal fresh each Observe pass from durable
`ObservedState` fields; there is **no memory of prior passes** in emission.

**Dedup is only two-layered, both narrow:**
1. **Within-pass merge** in `orient()` (`mod.rs:680-698`): signals sharing a
   `dedup_key` fold into one `Problem` (`classify_signal`, `mod.rs:709`). This
   dedups *within a single pass*, never across passes.
2. **Intervention-level `WhisperGate`** (`guardrails.rs:286`) with **900 s
   (15-min)** windows (`whisper_gate = WhisperGate::new(900, 5)`,
   `blocked_goal_gate = WhisperGate::new(900, 20)`, `mod.rs:220/226`) — rate-limits
   *actions*, not signal emission.

**⇒ There is no inter-pass signal dedup.** A persistent condition (distill 100% +
blocked goals) is therefore **honestly re-emitted every pass** → the 2×
recurrence. **This is expected behavior, not a bug.**

**The 2× asymmetry is explained by state evolution, which *proves* re-emission
and *rules out* an emit-duplicate defect:**
- Pass 2 **adds** `resource:engineer_spawn` → `live_engineers` crossed its `≥ 8`
  threshold between passes.
- Pass 2 **drops** `goal:blocked:dbabd65f` (#12) → **#12 closed 2026-07-05**, so
  the board stopped projecting it as `Blocked`.

## 5. Coupling verdict — Track A ⟂ Track B (INDEPENDENT)

- **Code layer: correlational only.** `distill_fail_pct`, `live_engineers`,
  `gym_skipped`, and `blocked_goals` are **independent `ObservedState` fields from
  independent subsystems**, read in one Observe pass. `blocked_goals_from_board`
  reads only goal *status*; **nothing reads `distill_fail_pct`.**
- **The gym link is REFUTED.** `gym_skipped` is the manual **`SIMARD_SKIP_GYM`**
  env flag (`provider.rs:61`) — zero dependence on distill (`git grep distill
  main -- src/gym/` → empty).
- **Distilled facts feed cognitive memory** (`cognitive_memory/*`,
  `memory_consolidation/mod.rs`), **not** the gym and **not** goal advancement.
- **Net:** fixing **#2658** is **neither necessary nor sufficient** to unblock
  #16/#17, and unblocking #16/#17 does not lower the parse-fail rate. The
  "systemic drag" (stale memory slows brain/engineers) is *plausible but
  unproven* — must not be stated as causation.

## 6. Corrections that supersede Round 1/2 (consolidated)

1. **Overseer signal taxonomy IS on `main`** (dedup keys `mod.rs:714/741/753/771/
   777/807`). The old P1 "promote taxonomy to `main`" is **MOOT** — the
   investigation *checkout* merely lacked `src/overseer/` because it is 92 commits
   stale.
2. **gym↔distill causal link:** REFUTED (env-flag driven).
3. **Coupling:** Track A and Track B are **INDEPENDENT**, not causally chained.
4. **`workstream-gap`:** **not** a code-emitted signal — narrative/synthesis
   artifact only.
5. **#2658 fix:** genuinely **not implemented** on any branch or open PR.

## 7. Prioritized remediation (final, ordered)

- **P0 — Fix residual distill parse (#2658).** In
  `distillation.rs::scan_for_facts_object`/`recover_distill_output`, after strict
  parse fails, retry `serde_json::from_str` against a **string-aware
  trailing-comma-stripped** view (provably a no-op on well-formed JSON → clean
  path byte-identical, no precision loss). Prefer a shared `recipe_output::extract`
  helper reusable by OODA/brain. Add regression fixtures (bare + `--output-format
  json` envelope; assert comma *inside a string* untouched; genuinely malformed
  still `Err`s). Emit a distinct "valid parse yielded zero facts" log in
  `into_facts`. **Done when** `cargo test memory_consolidation::distillation`
  passes and `distill_parse_success_rate` trends 0.0 → ~1.0.
- **P1 — Track B, strictly ordered by its own gates (unchanged by P0):**
  1. `dbabd65f`/#12 — **self-heal the stale block** (`simard goal unblock
     dbabd65f`); decision recorded, issue CLOSED, opt-in tracked in #32. Do **not**
     re-open the embeddings decision.
  2. `0c0ada69`/#16 (WS1, **critical path**) — dir-confined `eval_questions.json`
     loader; ≥12 CVE questions (≥6 real 2024/2025 w/ reference answers); commit
     `data/packs/cve/eval-results.{md,json}` (or documented hit@k); CI offline via
     mock transport.
  3. `7f5afcca`/#17 (WS2, **after #16**) — `quantize_int8`/`dequantize_int8`
     (scale = max|v|/127, bound-checked, all-zero safe, cosine > 0.999 on L2-norm),
     additive format; **gate adoption on the WS1 harness**; flag-off/DISABLED
     unless parity holds.
  4. `f29bb15c` (umbrella) — rolls each cycle; retires only once #16/#17 (and
     siblings) land. #32 is non-default/non-blocking.
- **Signal hygiene (optional):** if the 2× re-emission is noisy, add an
  *inter-pass* suppression window for unchanged `dedup_key`s (mirroring
  `WhisperGate`) — but the current re-emission is **correct**, not a defect.

## 8. Consolidated evidence index (all on `main` @ 946fe3ca unless noted)

- **Parse chain:** `distillation.rs` `parse_recipe_output_full` (1212-1255),
  `recover_distill_output` (1305-1341), `scan_for_facts_object` (1420-1453),
  `scan_cleaned_for_facts` (strict parses 1473/1494); telemetry
  `record_distill_success_metric` (888/908/929/369/573).
- **Signal model:** `signal.rs` `signals_from` (122, stateless per-pass);
  `mod.rs` `orient` (680-698, within-pass dedup), `classify_signal` (709), dedup
  keys 714/741/753/771/777/807; `guardrails.rs` `WhisperGate` (286),
  instantiated `mod.rs:220/226` (900 s windows).
- **Independence proof:** `sensor.rs` `blocked_goals_from_board` (188-205),
  `gym_skipped` (125-126); `provider.rs` `SIMARD_SKIP_GYM` (61),
  `distill_fail_pct` (490-497/540), `live_engineers` (177); `src/gym/` — no
  distill reference.
- **Gate:** `goal_curation/completion_gate.rs` evaluate (347-393),
  perpetual-never-archive + retain-blocked (475-520).
- **`git grep` (main):** lenient-JSON tolerance → empty; `workstream-gap` → empty.
  `Cargo.lock`: serde_json **1.0.149** (stock).
- **Live issues (`gh`):** Simard **#2619 CLOSED**, **#2658 OPEN**; kgpacks-rs
  **#12 CLOSED**, **#16 OPEN**, **#17 OPEN**, **#32 OPEN**. Open PRs docs-only:
  Simard #2668, #2657. **No code fix for #2658 anywhere.**
