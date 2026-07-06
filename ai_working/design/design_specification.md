# Design Specification — Issue #2669

**Recurring signature:** `overseer-obs:anomaly:distill parse-fail rate 100%` +
blocked-goal (kgpacks parity) + `process:distill_fail` + `quality:gym_skipped`
+ `resource:engineer_spawn` + `workstream-gap`, seen 2×.

**Repo:** `rysweet/Simard` · **Branch:** `feat/issue-2669-recurring-signature-seen-2-in-cognitive-memory-ove`
· **Base:** `origin/main`

This document turns the Step-2c requirements into an implementable
architecture. It supersedes two Step-2c assumptions that were **falsified by
source in this worktree** (see §0).

---

## 0. Corrections to Step-2c requirements (grounded in this worktree)

Step-2c was written before the overseer taxonomy landed. Two premises changed:

| Step-2c premise | Verified reality in this base | Impact |
|---|---|---|
| A4/P1.1: overseer signal taxonomy is NOT in `main`; must land from `feat/issue-2619` before recurrence is observable | `src/overseer/{signal,observer,sensor,wiring}.rs` **are present in base**, emitting `Signal::Anomaly`, `Signal::GoalBlocked`, `Signal::WorkstreamGap`, `Signal::DistillFailureRate`, dedup_key `process:distill_fail` | **P1.1 is already satisfied.** Acceptance criterion 2 ("no 3rd recurrence") is assertable in mainline now; nothing to land. |
| A5/P0: root-cause parser is `scan_for_facts_object` (~L740), strict `from_str::<RecipeEnvelope>` scanning recipe **stdout** | Post-#2622/#2619 the parser is `parse_facts_document` (L1257) → `scan_cleaned_for_facts` (L1289), reading the agent's **dedicated facts file**, using strict `serde_json::from_str::<RecipeEnvelope>` at L1291/L1312 over spans from the shared string-aware `recipe_output::balanced_objects` | The **root cause is unchanged** (strict serde rejects trailing commas) but the **fix site moved** to `scan_cleaned_for_facts` + the shared `recipe_output` helper. |

Everything else in Step-2c stands (cross-repo boundary A1, two-track split A2,
#12 closed A3, trailing-comma unimplemented A5).

---

## 1. Components involved

```
┌─────────────────────────── TRACK P0 (in-repo, code) ───────────────────────────┐
│                                                                                  │
│  distill agent → facts file ──► parse_facts_document (distillation.rs:1257)      │
│                                    └─► scan_cleaned_for_facts (1289)             │
│                                          │  strict from_str::<RecipeEnvelope>    │
│                                          │  (fast L1291 / per-span L1312)        │
│                                          ▼                                        │
│                     ★ NEW recovery tier: strip_json_trailing_commas ★            │
│                        (src/recipe_output/extract.rs — shared, string-aware)     │
│                                          │                                        │
│                                          ▼                                        │
│                     RecipeEnvelope::into_output / into_facts (1461)              │
│                        ★ NEW: distinct "valid parse, 0 facts" warn ★             │
│                                          │                                        │
│                                          ▼                                        │
│         record_distill_success_metric (882) + ★ NEW distill_parse class ★        │
│                        (self_metrics::record_metric)                             │
└──────────────────────────────────────────────────────────────────────────────┘

┌───────────────────── TRACK P1 (telemetry/orchestration state) ─────────────────┐
│  overseer::sensor (distill_fail_pct) → observer (Anomaly/GoalBlocked/           │
│  WorkstreamGap, dedup process:distill_fail) → wiring (gap-scan, dedup window)   │
│  Goal store: stale blocked goals (#12 closed, f29bb15c/0c0ada69/7f5afcca)       │
│  Gym/quality gate: ooda_actions::simple_actions::dispatch_run_gym_eval          │
│  Cross-repo hand-off: rysweet/agent-kgpacks-rs #16 (WS1), #17 (WS2)             │
└──────────────────────────────────────────────────────────────────────────────┘
```

**P0 touches (code):**
- `src/recipe_output/extract.rs` — new shared `strip_json_trailing_commas`.
- `src/memory_consolidation/distillation.rs` — wire recovery into
  `scan_cleaned_for_facts`; distinct zero-facts warn; per-pass parse metric.
- `src/memory_consolidation/distillation_tests.rs` and in-module `#[cfg(test)]` —
  regression tests.
- `src/recipe_output/extract.rs` `#[cfg(test)]` — unit tests for the stripper.

**P1 touches (mostly non-code / operational + one possible code gate):**
- Overseer telemetry: **no landing needed** (already in base). Optional: verify
  the anomaly self-heals once P0 drives parse-fail to 0%.
- Goal store self-heal: expire stale blocks (operational, or a self-heal code
  path if one exists — to confirm during impl).
- Gym gate: `src/ooda_actions/simple_actions.rs::dispatch_run_gym_eval` — confirm
  whether `quality:gym_skipped` is a *regression* (restore) or intended skip
  (document). In-repo code change only if a regression.
- kgpacks #16/#17: cross-repo hand-off (issue assignment), **no code here**.

---

## 2. Module boundaries (the "bricks")

### Brick A — `strip_json_trailing_commas` (NEW, shared)
- **Location:** `src/recipe_output/extract.rs`.
- **Contract (stud):**
  ```rust
  /// Remove JSON-illegal trailing commas (a comma immediately before a
  /// closing `}` or `]`, ignoring intervening whitespace) that make strict
  /// `serde_json` reject an otherwise well-formed object. STRING-AWARE:
  /// commas and brackets inside JSON string literals are never touched.
  /// Returns `Cow::Borrowed` unchanged when no trailing comma is present, so
  /// the common (clean) path allocates nothing.
  pub fn strip_json_trailing_commas(s: &str) -> std::borrow::Cow<'_, str>;
  ```
- **Implementation:** single forward pass mirroring `scan_balanced`'s
  `in_string`/`escaped` state machine (L233–264). Outside strings, when a `,`
  is seen, look ahead past whitespace (` \t\r\n`); if the next non-ws byte is
  `}` or `]`, drop the comma. Inside a string literal, copy verbatim.
  Pure/total; never errors; never widens acceptance beyond comma removal.
- **Why here, not in distillation:** the same strict-serde-vs-noisy-agent-JSON
  problem exists for every recipe parse path (brain decide/orient, verdicts).
  Co-locating with `balanced_objects`/`strip_ansi` makes it reusable and keeps
  the string-aware invariant in one audited place. **Zero coupling** to
  distillation types.

### Brick B — recovery tier in `scan_cleaned_for_facts` (MODIFY)
- **Location:** `distillation.rs:1289`.
- **Boundary rule:** recovery is attempted **only after** the strict parse of a
  given text/span returns `Err`. Order per span: (1) strict
  `from_str::<RecipeEnvelope>(span)`; (2) on `Err`, strict
  `from_str::<RecipeEnvelope>(strip_json_trailing_commas(span).as_ref())`.
  The existing three preference tiers (grounded-capable → non-empty → empty)
  are unchanged; recovery only changes whether a span *parses at all*.
- **Non-goals:** no json5, no comment stripping, no quote-fixing — trailing
  commas only. Genuinely malformed input still yields no parseable span →
  `scan_cleaned_for_facts` returns `None` → `parse_facts_document` returns `Err`
  → caller defers (retry-safe; **never a hollow `Ok`**).

### Brick C — zero-facts-after-filter signal (MODIFY)
- **Location:** `RecipeEnvelope::into_facts` / `into_output` (L1461/1497).
- **Boundary:** a **valid parse that yields 0 facts after the
  `pr-pattern|bug-pattern|lesson-learned` category filter** must emit a
  **distinct** `tracing::warn!(target: "simard::distill", …)` ("parsed OK,
  zero facts survived category filter") so it is never conflated with a
  parse-failure. Pure logging side-channel; return type unchanged.

### Brick D — per-pass parse-outcome metric (MODIFY/EXTEND)
- **Location:** distillation.rs metric block (L882–931) + call sites.
- **Boundary:** distinguish four parse outcomes in the metric context so
  "recovered via trailing-comma strip" is observably different from "strict-ok":
  `strict-ok`, `recovered` (trailing-comma), `deferred` (still unparseable),
  `zero-facts` (parsed, filtered to empty). Reuse the existing
  `record_metric("distill_parse_success_rate"/"distill_success_rate", …)`
  channel by threading a `parse_recovery` discriminator into the context string
  (via `build_distill_success_context`), OR add a dedicated
  `record_metric("distill_parse", …)` event — **decision D-1 below**. Metric
  writes stay best-effort (log-on-error, never propagate) and no-op under
  `cfg!(test)`, matching current behavior.

### Brick E — regression tests (NEW)
- **Locations:** `extract.rs` `#[cfg(test)]` (Brick A unit tests);
  `distillation.rs` in-module tests + `distillation_tests.rs` (Bricks B/C).
- Uses existing fixtures/patterns (`REAL_RUNNER_ENVELOPE_VERBATIM`,
  `parse_facts_document(...)` assertions already present at L1994+, L2159+).

---

## 3. Implementation approach (sequenced)

**P0 (blocks everything; pure in-repo):**
1. **Brick A** — add `strip_json_trailing_commas` + its unit tests. Land/verify
   in isolation (`cargo test -p … recipe_output`). String-aware invariant is
   the highest-risk detail, so test it first and standalone.
2. **Brick B** — wire the recovery tier into `scan_cleaned_for_facts` (both the
   fast path L1291 and the per-span loop L1312). Keep strict-first ordering.
3. **Brick C** — add the distinct zero-facts warn in `into_facts`/`into_output`.
4. **Brick D** — thread the parse-outcome discriminator into the metric.
5. **Brick E** — regression tests (see §5 acceptance mapping). Run full
   `cargo test` + `cargo clippy` + `cargo fmt`.

**P1 (after P0 lands; ordered):**
6. Confirm the overseer anomaly self-heals: because the taxonomy is already in
   base and dedups on `process:distill_fail`, once P0 drives the observed
   `distill_fail_pct` below the anomaly threshold, the Observe pass stops
   re-emitting. **No code landing** (contra Step-2c P1.1). Add/confirm an
   overseer-side test asserting "0% parse-fail telemetry ⇒ no Anomaly signal".
7. Stale blocked-goal self-heal: #12 (CLOSED) → expire; re-evaluate
   f29bb15c/0c0ada69/7f5afcca to unblocked/in-progress **or** documented
   actionable root cause. Prefer an existing self-heal/expiry path; if none,
   this is operational goal-store maintenance, not new subsystem code.
8. Gym/quality gate: determine whether `quality:gym_skipped` is a regression in
   `dispatch_run_gym_eval` (restore) or an intended skip (document
   justification). Code change **only** if a regression.
9. kgpacks #16 (WS1 full-pack CVE) / #17 (WS2 int8-PQ, hard-gated on #16):
   assign cross-repo owners in `rysweet/agent-kgpacks-rs`. **No code in this
   repo.** Clears `resource:engineer_spawn` / `workstream-gap` once owned.

**Decision D-1 (metric shape):** thread a `parse_recovery` field into the
existing `distill_success_context` rather than adding a new metric name.
Rationale: keeps one denominator for `distill_parse_success_rate`, avoids a new
metric consumers must learn, and the recovery/deferred/zero-facts split is a
context attribute of the *same* pass. Minimal, back-compatible.

---

## 4. Module specifications (interfaces)

### `recipe_output::strip_json_trailing_commas`
- **Input:** `&str` (a candidate JSON object/array span, already brace-balanced
  and noise-stripped by upstream helpers).
- **Output:** `Cow<str>` — borrowed & identical if no trailing comma; owned with
  offending commas removed otherwise.
- **Invariants:** (I1) bytes inside `"…"` literals are preserved exactly,
  honoring `\"` escapes; (I2) only a `,` whose next non-whitespace byte is `}`
  or `]` is removed; (I3) total/pure — no panics, no `Result`; (I4) idempotent:
  `f(f(x)) == f(x)`.
- **Errors:** none by construction. Downstream strict `serde_json` remains the
  sole arbiter of validity — a still-malformed result simply fails to parse.

### `scan_cleaned_for_facts` (revised contract)
- Unchanged signature `fn(&str) -> Option<DistillOutput>`.
- New internal step: for the fast path and each balanced span, if strict parse
  fails, retry strict parse on the trailing-comma-stripped view before moving
  on. Preference-tier selection and the string-aware span iteration are
  unchanged. Returns `None` iff no view (strict or recovered) parses — preserving
  the retry-safe `Err` contract in `parse_facts_document`.

### `RecipeEnvelope::into_facts` (revised behavior)
- Same signature/return. Adds a distinct `warn!` when the input envelope parsed
  but every fact was filtered out by `canonical_distill_concept`. No control-flow
  change.

### Metric context (revised)
- `build_distill_success_context(...)` gains a `parse_recovery: ParseRecovery`
  input where `ParseRecovery ∈ {StrictOk, Recovered, Deferred, ZeroFacts}`,
  serialized as a stable `parse_recovery=<label>` key in the context string.
  Consumers that ignore the key are unaffected (append-only context).

---

## 5. Acceptance-criteria → verification mapping

| Criterion (Step-2c) | Verified by |
|---|---|
| C1: previously-100%-failing inputs → 0% parse-fail | Brick E tests: bare + enveloped trailing-comma inputs recover ≥1 fact (`parse_facts_document` returns `Ok` with facts); Brick D metric shows `Recovered`. |
| C2: signature does not re-emit on next observe cycle (no 3rd recurrence) | Overseer test: `distill_fail_pct` below threshold ⇒ no `Signal::Anomaly` / `process:distill_fail`. Assertable **now** (taxonomy already in base). |
| C3: blocked goals advanced/unblocked or stale-expired (#12) or documented | Step 7: #12 expired (closed); others re-evaluated with documented remediation. |
| C4: gym/quality gate executed or documented skip | Step 8: restore `dispatch_run_gym_eval` (if regression) or record justification. |
| C5: WS1/WS2 have owners, no resource gap | Step 9: cross-repo hand-off issues assigned; `workstream-gap` clears. |

**Guardrails preserved:** never a hollow `Ok` (malformed still `Err`); recovery
strictly after strict parse; string-interior commas untouched; metrics
best-effort & test-silent.

---

## 6. Risks & dependencies

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | String-aware stripper corrupts a comma *inside* a JSON string that precedes `}`/`]` (e.g. `{"c":"a,}"}`) | High | Mirror `scan_balanced`'s `in_string`/`escaped` machine exactly; dedicated unit test `comma_inside_string_untouched`; property-style test over both. |
| R2 | Over-tolerance masks genuinely broken agent output (silent quality loss) | High | Scope strictly to trailing commas; genuinely malformed input still returns `Err` → retry-safe deferral. Test `genuinely_malformed_still_err`. |
| R3 | Recovery hides a *real* recurring agent bug behind auto-repair | Med | Brick D emits `Recovered` distinctly from `StrictOk`, so a high recovery rate is observable and can drive a prompt fix — not silently absorbed. |
| R4 | Zero-facts warn (Brick C) floods logs on legitimately empty batches | Low | `warn` only when the parse succeeded AND all facts were category-filtered (not for legitimately empty `{"facts":[]}` envelopes). |
| R5 | Branch sprawl (2495/2619/2622/2658 touch the same parse path) → merge conflict / duplicate fix | Med | Base is `origin/main` which already absorbed #2622/#2619; build the trailing-comma fix fresh (Step-2c A5: #2658 has no diff). Rebase before PR. |
| R6 | P1 goal/gym/workstream state is largely **out-of-repo or operational**; over-scoping into code creates hollow work | Med | Keep P1 code strictly to a verified gym regression; treat goal-expiry and kgpacks #16/#17 as telemetry/hand-off per Step-2c A1/A3. |
| R7 | Correlational coupling misread as causal — expecting P0 to clear blocked-goal signals | Med | Design keeps tracks independent (A2). C1/C2 gate on distill; C3–C5 gate on their own steps. |

**Dependencies:**
- `serde_json` (strict arbiter, unchanged), `self_metrics::record_metric`,
  recipe-runner-rs envelope shape (unchanged).
- `recipe_output::{balanced_objects, scan_balanced, strip_ansi}` — Brick A joins
  this module and reuses its string-aware convention.
- Overseer taxonomy — **present in base**, no dependency to land.
- Cross-repo: `rysweet/agent-kgpacks-rs` #16/#17 — hand-off only.

---

## 7. Out of scope (reaffirmed)
- Any code in `rysweet/agent-kgpacks-rs` (WS1/WS2 crate fixes) — hand-off only.
- Redesigning overseer anomaly detection (already in base; only verify self-heal).
- Re-opening closed #12 (tracked in #32).
- json5/comment/quote leniency beyond trailing-comma removal.
- Unrelated CVEs/embedding schemes outside #16/#17.

---

## 8. Ready-for-implementation checklist
- [x] Root cause located & confirmed (strict serde vs trailing comma at
      `scan_cleaned_for_facts` L1291/L1312).
- [x] Fix site chosen (shared `recipe_output::strip_json_trailing_commas` +
      recovery tier), zero coupling, string-aware.
- [x] Metric/observability plan (Brick D, Decision D-1).
- [x] Test plan mapped to acceptance criteria C1–C5.
- [x] Risks enumerated with mitigations.
- [x] Step-2c corrections documented (§0) so downstream steps don't try to land
      already-present overseer telemetry.
