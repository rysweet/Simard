# Tertiary (Architect) — Two-Lane Reconciliation, Defect Enumeration & Atomic Landing Order

**Role:** TERTIARY investigator (architecture). **Investigation-only — no code changed.**
**HEAD:** `a68296c6` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Mandate:** Reconcile the episode-lane vs. escalation-lane architecture, enumerate the
distinct defects behind the recurring `overseer-obs:…|goal:blocked:…|workstream-gap|
resource:engineer_spawn` signature, and give a risk-ranked landing order with atomicity
constraints. **Method:** VALIDATE prior artifacts against live source — do not re-derive.

Extends (does not restart): `RECONCILIATION_LEDGER.md`, `FINAL_SYNTHESIS.md`,
`tertiary_architecture_IDEMPOTENCY_DURABILITY_AND_REMEDIATION_RUNG_HEAD_7293de99.md`,
`secondary_blocked_park_and_gap_spawn_coupling_HEAD_9fd1ea0a.md`.

---

## 0. Provenance — the prior plan holds verbatim at HEAD

- `git diff --name-only 7293de99..HEAD -- '*.rs'` → **empty** (zero production or test
  drift since the prior tertiary wave). The prior tertiary landing plan is unchanged.
- `git diff --name-only dea65df8..HEAD -- '*.rs'` → **`src/overseer/tests_root_cause.rs`
  only** — net-additive test `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
  (`tests_root_cause.rs:477+`) that **codifies the two-lane decoupling as a regression
  invariant**. It hardens this wave's thesis; it contradicts nothing.

Every load-bearing citation below was independently re-read at `a68296c6` (I did not
trust the docs' own line numbers):

| Claim | Source @ HEAD | Status |
|---|---|:--:|
| `observation_signature` = `sort_unstable`→`dedup`→`overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| Write-back emits over oriented problems | `overseer/wiring.rs:301` (`write_back_observation(&cycle.problems)`) | ✅ exact |
| Recall-driven `RecurringSignature` → problem `dedup_key = sanitize_recalled(signature)` | `overseer/mod.rs:1353-1359` | ✅ exact |
| Lane A floor: `RECURRING_SIGNATURE_THRESHOLD = 2`, emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact |
| Lane B counter: append-only `store_fact` (no upsert) | `overseer/mod.rs:1034` | ✅ ratchet live |
| Lane B escalate at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3)` | `overseer/mod.rs:1613`; `root_cause.rs:33` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps` (no launch) | `overseer/mod.rs:1534-1543` | ✅ exact |
| Sibling `StepFailure` arm DOES launch (`LaunchRecipe`) — proves the hole is unique | `overseer/mod.rs:1549-1580` | ✅ exact |
| `act_flag_workstream_gaps` notify-only, gate keyed `workstream-gap:{g.signature}` | `overseer/mod.rs:884-948` (key `:901,932`) | ✅ exact |
| Composite token uses the BARE `"workstream-gap"` dedup_key (INV-GAP-KEY) | `overseer/mod.rs:1371` | ✅ exact |
| `goal:blocked:{goal_id}` carries NO WHY token | `overseer/mod.rs:1336` | ✅ exact |
| `resource:engineer_spawn` dedup_key (passive telemetry) | `overseer/mod.rs:1270` | ✅ exact |
| WHY reasoner double-gated (`completion_evidence` + `no_progress_investigation_enabled`) | `ooda_loop/cycle.rs:582-583` | ✅ exact |
| Whisper gates are per-process volatile heap state (`HashMap`, built empty) | `overseer/guardrails.rs:292-333` | ✅ exact |

**No stale citations.** The committed root-cause analysis is sound and live.

---

## 1. The two-lane architecture (reconciled)

The signature is produced by **two structurally decoupled lanes that share no counter**.
Confusing them is the central architectural hazard of any fix.

```
                         Observe/Orient (signal_to_problem, mod.rs:1262-1373)
                                   │  mints dedup_keys:
                                   │  goal:blocked:<id> | workstream-gap | resource:engineer_spawn
                                   ▼
        ┌───────────── LANE A · EPISODE / VISIBLE-COUNT lane ──────────────┐
        │ wiring.rs:301  write_back_observation(&cycle.problems)           │
        │   → observation_signature()  overseer-obs:{keys.join("|")}       │  mod.rs:1068-1073
        │   → write_back_gate  WhisperGate(900s, cap 5)  [VOLATILE]        │  guardrails.rs:292
        │   → store_episode (cognitive memory)                            │
        │        ▲ recall next window → RecurringSignature @ occ>=2        │  signal.rs:362,463
        │        └── the operator-visible "seen 2× / |workstream-gap|…"    │
        └──────────────────────────────────────────────────────────────────┘
                                   │ (NO shared counter — codified by
                                   │  tests_root_cause.rs:loud_lane_a_…)
        ┌───────────── LANE B · ROOT-CAUSE / ESCALATION lane ──────────────┐
        │ record_occurrence → store_fact (append-only ratchet)            │  mod.rs:1034
        │   recurrence = recall_occurrences(dedup_key).len()              │
        │   escalate iff recurrence >= 3                                  │  mod.rs:1613; root_cause.rs:33
        └──────────────────────────────────────────────────────────────────┘
```

**Verdict on the `×2` (validated, cited):** the `×2` is Lane A telemetry
(`RECURRING_SIGNATURE_THRESHOLD = 2`), an **honest re-observation** of a still-open
problem set — most plausibly one pre-restart + one post-restart episode, because the
`write_back_gate`'s `last_delivered` map is volatile per-process (`guardrails.rs:294,301`)
and re-fires a still-true condition after a daemon restart regardless of the 900 s window.
It is **not** a dedup/replay bug on Lane B. The exact "2 vs. 3" is **not a single-axis
dead zone**: Lane A's `×2` carries zero information about whether Lane B reached `3`
(now a hard invariant, `tests_root_cause.rs`). **Moving `2` or `3` is rejected** — it
would escalate honest transients.

---

## 2. Defect enumeration (three seams, three independent fixes)

| ID | Seam (file:line) | Defect | Lane | Symptom token |
|---|---|---|---|---|
| **D1** | `wiring.rs:301` → `mod.rs:1068-1073` fed by `mod.rs:1353-1359` | A recall-driven `RecurringSignature` problem's `dedup_key` **already contains** `overseer-obs:…`; re-observing it re-wraps → **nested `overseer-obs:overseer-obs:…`**. Self-ingestion of the write-back into the next write-back. | A | the `overseer-obs:` prefix nesting |
| **D2** | `cycle.rs:582-583` (WHY double-gate) + `mod.rs:1034` (ratchet) / `mod.rs:1613` (gate) | Blocked goals are parked and the recurrence counter is **both** starved (WHY gated off when `completion_evidence` absent → no self-resolution) **and** an append-only ratchet. The gate + counter form a **latch**: either changed alone moves nothing observable. | B (+ drains A) | `goal:blocked:<slug>-<hash>` persistence |
| **D3** | `mod.rs:1534-1543` + `mod.rs:884-948` | `WorkstreamCoverage` is the **sole** High-priority Decide arm with **no launch/file-issue closing edge** — notify-only. The gap re-notifies every window forever; with gap-scan disabled it recurs in the signature with **zero** operator visibility. | A | `workstream-gap` (+ `resource:engineer_spawn` co-symptom) |

**`resource:engineer_spawn` is not a fourth defect.** It is benign passive telemetry
(`mod.rs:1270`, `ProblemKind::ResourcePressure`) with **no causal edge** to `workstream-gap`.
It co-occurs only because both predicates held in one window (backlog uncovered AND
engineers saturated). The three tokens are **three symptoms of one under-resourced,
non-converging state**, not an orchestration cycle. Do not build a spawn/gap coupling fix.

**Framing correction carried forward (do not regress):** the "no WHY classification"
premise **cannot be read from the signature** — `goal:blocked:<id>` omits the WHY token
(`mod.rs:1336`), so a correctly-classified terminal block (`UnclearCriteria`,
`GenuinelyStuck`, `UpstreamDependency`-defer) re-emits the identical token. The recurrence
is the expected fingerprint of *terminally-classified-but-unresolved* work; D2's real gap
is a **missing convergence rung on the observation lane**, not a missing subsystem (the
reasoner is default-wired, `cycle.rs:583`, `no_progress.rs:203-207`).

---

## 3. Atomicity constraints (what MUST ship together vs. independently)

| Fix | Atomicity | Why |
|---|---|---|
| **D2** WHY-gate close **+** count-in-content upsert | **ATOMIC — must ship as one change** | The accrual gate (`cycle.rs:582`) and the counter (`mod.rs:1034/1613`) are a **latch**. Closing the gate without de-ratcheting leaves escalation on a broken counter; de-ratcheting without closing the gate leaves accrual starved. **Trap:** the de-ratchet must be a **count-in-content upsert**, NEVER the literal `store_fact_with_caller_key` one-liner — `DedupMode::CallerKey` keeps one live fact/key, collapsing `recall.len()` to 1 and making `>=3` **dead code** (`RECONCILIATION_LEDGER.md §2`). |
| **D3** per-gap `≥2× → LaunchRecipe` rung | **INDEPENDENT** (self-contained) | Adds a closing edge to one Decide arm via the existing `launch.rs` path. **Trap:** must key on `GapItem.signature` (per-gap), not the bare `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps fold into one issue (INV-GAP-KEY). |
| **D1** write-back emission filter | **INDEPENDENT**, pure/local | Exclude recall-derived `overseer-obs:*` problems before `join("|")` in `observation_signature` (`mod.rs:1068-1073`). No cross-module coupling; deletes the nested shape only. |
| Convergence gauges (residual) | **INDEPENDENT**, last | Counters beside `workstream_gaps_detected/_suppressed`: "gap signatures ≥2× with no launch" and "blocked reasons failing `is_bare_no_progress_block`". Proves closure; guards regression. Optional episode-lane `(signature, floor(now/900))` idempotency key belongs **here only**, and only if restart-flapping is empirically confirmed the dominant 2× source. |

**Rejected levers (re-endorsed no-change):** persisting `last_delivered` (masks an open
backlog as convergence; durability already lives on Lane B by design; loads a correctness
surface onto a primitive that is correct *because* volatile — `guardrails.rs:292-333`);
and moving `RECURRING_SIGNATURE_THRESHOLD`/`RECURRENCE_ESCALATION_THRESHOLD` (lanes are
decoupled; escalates honest transients).

---

## 4. Landing order (risk-ranked, dependency-correct)

1. **D2 (atomic latch):** close the WHY double-gate (`cycle.rs:582-701`) + count-in-content
   upsert (`mod.rs:1034` / read at `:1613`). **First** because it **drains the
   `goal:blocked:*` population at the source** — the largest token cluster — and unlatches
   escalation. Highest risk (touches accrual + counter); ship + verify alone.
2. **D3 (independent closing edge):** per-gap `≥2× → LaunchRecipe` keyed on
   `GapItem.signature` (`mod.rs:884-948`, `:1534-1543`) via existing `launch.rs`. Drains
   `workstream-gap|workstream-gap` + the persona cluster + relieves `resource:engineer_spawn`
   pressure indirectly. Medium risk (new launch edge).
3. **D1 (pure filter):** strip recall-derived `overseer-obs:*` from the write-back input
   (`mod.rs:1353-1363` before `mod.rs:1072`). Lowest risk; removes only the nested prefix
   shape. Land last so its diff is legible against an already-shrinking signature.
4. **Convergence gauges:** add after D1–D3 to *prove* the loops closed and lock regression.

**Rationale for the order:** D2 first maximizes signature-population reduction and is the
only latch; D3 and D1 are independent and could ship in either order, but D1 last keeps its
(cosmetic-looking but real) change reviewable once the volume has dropped. No fix depends on
another's *code*, but the *verification* of each is cleaner in this sequence.

---

## 5. Final verdict

- **The signature is a faithful fingerprint of a static, unresolved, under-resourced
  problem set** — a real re-observation loop, **not** a dedup/replay artifact. Validated
  against live source at `a68296c6`; zero production drift since the prior tertiary wave.
- **Three independent defects, three seams:** D1 (write-back self-ingestion / nesting),
  D2 (WHY-gate + ratchet latch on the escalation lane), D3 (missing gap closing edge).
  `resource:engineer_spawn` is a co-symptom, not a defect.
- **Atomicity:** D2 must ship as one change (gate+counter latch, count-in-content — never
  `CallerKey`); D1, D3, and gauges are independent. D3 must key on `GapItem.signature`.
- **Landing order:** D2 → D3 → D1 → gauges. No `2`/`3` threshold move; no durable whisper
  gate. This wave adds **no new code requirement** — it confirms the settled D1/D2/D3 plan
  remains correct and dependency-ordered at HEAD. Investigation-only.
