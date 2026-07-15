# Tertiary (Architect) — Idempotency Durability & Remediation-Rung Design

**Role:** Tertiary investigator (architecture). **Investigation-only — no code changed.**
**Mandate:** (1) Assess whether persisting `last_delivered` would eliminate the
restart-driven double-record; (2) design (or reject) a remediation rung between
recurrence threshold `2` (Lane A) and escalation threshold `3` (Lane B); deliver a
**minimal landing-safe fix with cited lines** or a **justified no-change verdict**.
**HEAD:** `7293de99` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
Extends — does not restart — `tertiary_architecture_VALIDATION_HEAD.md`,
`tertiary_architecture_LANDING_HEAD_ad5e1060.md`, `RECONCILIATION_LEDGER.md`,
`FINAL_SYNTHESIS.md`.

---

## 0. HEAD re-grounding (every load-bearing line re-read this pass @ 7293de99)

| Claim | Source @ HEAD | Status |
|---|---|:--:|
| `observation_signature` = `sort_unstable`→`dedup`→`overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| Write-back = single 900 s `write_back_gate` peek→store→commit; slot consumed only after a successful store | `overseer/mod.rs:534-563` | ✅ exact |
| `WhisperGate.last_delivered` / `deliveries` are in-memory heap state, built empty in `new` | `overseer/guardrails.rs:291-333` (map `:294`, ctor `:301-308`) | ✅ per-process, no persistence |
| Lane A: `RecurringSignature` emits at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact |
| Lane B: `record_occurrence` → append-only `store_fact` (no upsert) | `overseer/mod.rs:1004,1034` | ✅ ratchet live |
| Lane B: escalate only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (3) | `overseer/mod.rs:1613`; `root_cause.rs:33` | ✅ exact |
| WHY reasoner double-gated (Gate A `completion_evidence`, Gate B `no_progress_investigation_enabled`), else → `Vec::new()` | `ooda_loop/cycle.rs:582-583,701` | ✅ starves accrual |

**Source drift since the prior tertiary docs is one TEST file only**
(`git diff --name-only 6e3113bc..HEAD -- '*.rs'` → `src/overseer/tests_root_cause.rs`).
The delta is **net-additive and corroborating**: it adds
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
(`tests_root_cause.rs:490`), a regression guard proving a LOUD Lane-A
`RecurringSignature` (occurrences far above both floors) with an EMPTY Lane-B recall
**leaves `recurrence` at 0** and self-heals via `UnblockGoal`. It **hardens**, not
contradicts, the two-decoupled-lanes verdict. No production line moved; every
citation above holds.

---

## 1. Mandate part 1 — Does persisting `last_delivered` fix the 2×? **NO. Justified no-change.**

### 1.1 Mechanism (confirmed)
The `write_back_gate` is a `WhisperGate` whose dedup memory is
`last_delivered: HashMap<String,i64>` (`guardrails.rs:294`), heap state on `Overseer`,
constructed empty (`guardrails.rs:301-308`; instantiated `mod.rs:299`). `peek`
suppresses only while `now - last < window_secs` (`guardrails.rs:312-317`). On daemon
restart the map is empty, so a still-true condition re-delivers immediately regardless
of the 900 s window — the most probable producer of *exactly* 2× (one pre-restart
episode + one post-restart episode). This is real.

### 1.2 Why persisting the gate is the WRONG lever (four independent reasons)

1. **It hides a true signal, not a false one.** The composite signature is a faithful
   fingerprint of an *open, unresolved* problem set (`observation_signature`,
   `mod.rs:1068-1073`; the set is still true post-restart). Suppressing the second
   episode by persisting `last_delivered` makes the operator-visible count stop
   climbing **while the backlog is still open** — converting honest recurrence
   telemetry into silence and manufacturing a false "converged" reading. The count
   trending to zero must mean *the loop closed*, not *the gate remembered*.

2. **Durability already has a home — Lane B — and it is by-design.** The system already
   owns exactly one durable, cross-restart recurrence ledger: Lane B occurrences
   (`store_fact`, `mod.rs:1034`), read as `recurrence` for escalation (`mod.rs:1613`).
   "This condition has persisted across restarts" is precisely what Lane B is *for*.
   Persisting the volatile whisper gate would stand up a **second competing durable
   counter** on the wrong lane (episodes), splitting the source of truth. The window
   gate's job is the orthogonal concern "did I already whisper this *within this live
   window*" — correct to forget on restart.

3. **It adds a real correctness surface for no product gain.** Persisting/rehydrating
   `last_delivered` (and the `deliveries` rolling-hour vector, `guardrails.rs:295`)
   introduces stale-slot pruning at boot, clock-skew handling across process
   boundaries, unbounded keyspace growth, and an ambiguous "is a 901 s-old delivery
   still suppressed after a 20-minute outage?" window-at-boot semantics. The
   `WhisperGate` primitive is small and provably correct *because* it is volatile
   (`tests_whisper.rs:437-475`); durability would be a materially larger change than
   it appears.

4. **The 2× is a symptom of missing closing edges, not of a leaky gate.** Even a
   perfectly durable gate would only mute the count; the underlying problem set
   (`goal:blocked:*` parked without a WHY class; `workstream-gap` notify-only) would
   remain open forever. Removing the *persistent condition* (D1/D2/D3, prior waves)
   removes restart re-emission **at the source** — the gate never gets the chance to
   re-fire because there is nothing left to observe.

### 1.3 The one narrow durable-dedup that would be defensible (conditional, NOT the fix)
IF (and only if) restart-flapping is *empirically* confirmed as the dominant 2× source
(not >900 s honest re-observation), the correct place to add durability is the **episode
store boundary**, not the whisper gate: give `record_observation`/`store_episode`
(`wiring.rs`, called from `mod.rs:554`) an idempotency key of
`(signature, floor(now/900))` — a count-in-content window bucket, mirroring the D2
count-in-content discipline. This dedups the *persisted artifact* without teaching the
volatile gate to lie, and it composes with a convergence gauge. **Still optional and
subordinate** to closing the loops; recorded as a follow-up, never as the minimal safe
fix. (This matches, and does not extend, the "cross-restart episode inflation" residual
already flagged in `tertiary_architecture_VALIDATION_HEAD.md §3`.)

**Verdict (part 1): NO-CHANGE to gate durability.** Leave `write_back_gate`,
`gap_gate`, `whisper_gate`, `blocked_goal_gate` volatile. Cited: `guardrails.rs:291-333`,
`mod.rs:534-563`, `mod.rs:1034`, `mod.rs:1613`.

---

## 2. Mandate part 2 — A remediation rung between `2` and `3`

### 2.1 The framing correction: it is not one axis, so the numbers are not the lever
`2` (Lane A, `RecurringSignature`, `signal.rs:362,463`) and `3` (Lane B, escalation,
`root_cause.rs:33` / `mod.rs:1613`) live on **decoupled storage lanes that share no
counter** — now a codified invariant (`tests_root_cause.rs:490`,
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`). Therefore:

- "Between 2 and 3" is **not** a single-threshold dead zone you can close by moving a
  number. Lane A's `×2` carries **no information** about whether Lane B reached `3`. A
  generic "escalate at 2" on Lane B would fire on honest transient re-observations
  (Lane A trips on any two recalled episodes sharing a signature) — a false-positive
  machine. **Do not move `RECURRING_SIGNATURE_THRESHOLD` or
  `RECURRENCE_ESCALATION_THRESHOLD`.** The thresholds are sound; the gap is *cross-lane
  visibility* plus *missing closing edges*, not the arithmetic.

### 2.2 Where a remediation rung legitimately belongs (design, not a threshold move)
A rung must sit on the lane that **observes the recurrence first** (Lane A / the
episode-forming lane, proven at `×2`) and route into the **existing closing edges**
— never introduce a new escalation number:

- **`workstream-gap` family (D3):** the rung is a per-gap recurrence partition keyed on
  `GapItem.signature` at the existing `gap_gate` commit site (`mod.rs:884-948`, gap key
  at `:901`): **1× → Notify** (unchanged), **≥2× → `LaunchRecipe`** through the existing
  `launch.rs` edge that every sibling High Decide arm already has (`WorkstreamCoverage`
  is the sole High arm lacking it, `mod.rs:1534-1543`). Threshold **2** is correct here
  *because a recurring coverage gap has no benign transient explanation* — this is the
  one place `2` (not `3`) is the right rung, and it is on Lane A, not a re-tuned Lane B.
  Must key on `GapItem.signature`, not the bare `"workstream-gap"` dedup_key
  (`mod.rs:1371`), or all gaps fold into one item (INV-GAP-KEY, `tertiary_gap_routing…`).

- **`goal:blocked:*` family (D2):** the rung is **not a new number** — it is *closing
  the WHY double-gate* (`cycle.rs:582-583,701`) so the six stall classes self-resolve
  (`AlreadyComplete→MarkDone`, `Obsolete→Drop`, `MissingPrecondition→Heal`,
  `UpstreamDependency→Defer`) and only the two genuinely-stuck classes reach a human.
  This **drains the blocked population before Lane B ever needs to escalate**, so the
  "2→3 gap" for this family evaporates rather than needing a rung. Any counter change
  here must be **count-in-content upsert**, never the `store_fact_with_caller_key`
  one-liner (which collapses `recall.len()` to 1 and makes `>=3` dead code —
  `RECONCILIATION_LEDGER.md §2`, `secondary_dedup_recurrence_VALIDATION_HEAD.md §4`).

**Verdict (part 2): NO new threshold; NO change to `2` or `3`.** The only legitimate
"rung between 2 and 3" is the **D3 per-gap `≥2× → LaunchRecipe` partition on Lane A**,
plus draining the blocked family by closing the WHY gate — both of which are the
already-designed D2/D3 closing edges, not a new escalation tier.

---

## 3. Architectural disposition & landing order (defers to the settled D1/D2/D3 plan)

For the two levers this mandate owns, the verdict is **no code change**:

| Lever (this mandate) | Verdict | Justification (cited) |
|---|---|---|
| Persist `last_delivered` / durable whisper gate | **REJECT (no-change)** | §1.2 — wrong lane, masks a true signal, adds correctness surface; durability belongs to Lane B (`mod.rs:1034`), gate is correct to be volatile (`guardrails.rs:291-333`) |
| Move `RECURRING_SIGNATURE_THRESHOLD`(2) / `RECURRENCE_ESCALATION_THRESHOLD`(3) | **REJECT (no-change)** | §2.1 — lanes are decoupled (`tests_root_cause.rs:490`); moving numbers escalates honest transients; thresholds sound (`signal.rs:362`, `root_cause.rs:33`) |
| Remediation rung `2→3` | **DESIGN, not a number** | §2.2 — D3 per-gap `≥2×→LaunchRecipe` on Lane A (`mod.rs:884-948`, `:1534-1543`) + close WHY gate for blocked family (`cycle.rs:582-701`) |

The actual code fixes are **D1/D2/D3**, already specified by prior tertiary/secondary
waves; this investigation confirms they remain the correct, dependency-ordered plan and
adds nothing that requires re-implementation. **Landing order (unchanged, re-endorsed):**

1. **D2 — WHY-gate close + count-in-content upsert, atomically** (`cycle.rs:582-701` +
   `mod.rs:1034/1613`). The counter and its accrual gate are a latch; either alone
   changes nothing observable. Drains `goal:blocked:*`.
2. **D3 — per-gap recurrence-aware closing rung** (`mod.rs:884-948`, `:1534-1543`),
   keyed on `GapItem.signature`, `≥2× → LaunchRecipe` via existing `launch.rs`. Drains
   `workstream-gap|workstream-gap` + the persona cluster.
3. **D1 — write-back emission filter** (`wiring.rs:301` / `observation_signature`
   input, `mod.rs:1068-1073`): exclude recall-derived `overseer-obs:*` problems
   (`mod.rs:1353-1363`) before `join("|")`. Pure/local; removes the nested shape.
4. **Convergence gauges** (residual): count "gap signatures ≥2× with no launch" and
   "blocked reasons failing `is_bare_no_progress_block`" beside existing
   `workstream_gaps_detected/_suppressed`. Proves closure; guards regression. Only here
   does the optional episode-lane `(signature, window)` idempotency key (§1.3) belong,
   and only if restart-flapping is confirmed the dominant 2× source.

---

## 4. Final verdict (defensible against success criteria)

- **Persisting `last_delivered` would technically suppress the post-restart second
  episode, but is REJECTED** as the fix: it masks a still-open backlog as convergence,
  duplicates durability that already lives (by design) on Lane B (`mod.rs:1034`), and
  loads a correctness surface onto a primitive that is correct precisely because it is
  volatile (`guardrails.rs:291-333`). Leave all four window gates volatile.
- **No remediation rung is added by moving `2` or `3`.** The `2↔3` "dead zone" is a
  **cross-lane visibility gap** (codified by `tests_root_cause.rs:490`), not a
  single-axis threshold gap. The legitimate rung is the **D3 per-gap `≥2× →
  LaunchRecipe` partition on Lane A** plus draining the blocked family by closing the
  WHY double-gate — both already in the D2/D3 plan.
- **Net minimal landing-safe change owned by *this* mandate: NONE (justified
  no-change).** The convergence-producing work is D1/D2/D3, which remain correctly
  specified and dependency-ordered at HEAD `7293de99`. This is investigation-only; no
  production line was modified.
