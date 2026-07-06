# Investigation Report — Distill 100% Parse-Failure & Blocked kgpacks-rs Parity Goals

**Type:** Investigation (continuation / Round 2 — persists Round 1 synthesis)
**Date:** 2026-07-06
**Repo:** rysweet/Simard @ `92150406`
**Anomaly signal:** `overseer-obs:anomaly:distill parse-fail rate 100%`

> **⚠️ Read the Round-3 verification addendum first (bottom of this file).**
> Round 1/2 were written on the investigation branch base `92150406`, which is
> **92 commits behind `main` (946fe3ca)**. Several Round-1 claims were correct
> *for that stale base* but are **wrong for the live system (`main`)**. The
> addendum re-verifies every criterion against `main` with file:line evidence,
> confirms the Track A root cause still holds, and **corrects** the coupling and
> branch/telemetry claims. Where the addendum and the body disagree, **the
> addendum wins.**

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
