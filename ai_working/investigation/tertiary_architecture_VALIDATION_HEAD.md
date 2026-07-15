# Tertiary Investigation (re-run) — Systemic Fix Design & Remaining Gap at HEAD

**Role:** Tertiary investigator (architect). **Date:** 2026-07-15.
**HEAD:** `dea65df8` (the two latest commits are the investigation docs
themselves — **no fix has been merged**; every defect below is live at HEAD).
**Mandate:** Choose the correct systemic fix among *(a) dedup with
count/timestamp, (b) idempotent escalation, (c) blocked-transition handling*,
and identify any remaining gap at HEAD. Builds on `tertiary_architecture_design.md`
and `secondary_dedup_recurrence_VALIDATION_HEAD.md`; does not restate them.

---

## 0. HEAD verification ledger (every load-bearing claim re-checked in source)

| Claim | Source @ HEAD | Status |
|---|---|:--:|
| `observation_signature` = `sort_unstable` → `dedup` → `overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| Write-back records **all** problems (incl. recall-derived) through one 900 s gate | `overseer/mod.rs:534-563`; `wiring.rs:301` | ✅ |
| Recall-derived `RecurringSignature` → `ProcessHealth` problem, `dedup_key = sanitize_recalled(signature)` (an `overseer-obs:` string) | `overseer/mod.rs:1353-1363` | ✅ self-nesting live |
| `WorkstreamGap` → `WorkstreamCoverage`, fixed key `"workstream-gap"` | `overseer/mod.rs:1368-1373` | ✅ |
| `WorkstreamCoverage` Decide arm → **notify-only** `FlagWorkstreamGaps`, **no** `launch.rs` edge | `overseer/mod.rs:1534-1543` | ✅ flag-without-close |
| Act path peeks/commits `gap_gate` only; no `RecipeLauncher` | `overseer/mod.rs:884-948` | ✅ |
| `gap_gate` = intra-window `WhisperGate` only; forgets across windows | `overseer/mod.rs:901-933` | ✅ no cross-window ledger |
| `decide_blocked_goal` escalates only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (3) | `overseer/mod.rs:1613`; `root_cause.rs:33` | ✅ |
| `RecurringSignature` emits at `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (2) | `signal.rs:362,463` | ✅ |
| `record_occurrence` → **append-only** `store_fact` (no upsert) | `overseer/mod.rs:1034` | ✅ ratchet live |
| `recurrence = recall_occurrences(...).len()` (counts distinct facts ≤256) | `overseer/mod.rs:972-997` | ✅ |
| WHY-reasoner **double-gated**; both else-branches skip the self-resolving ladder | `ooda_loop/cycle.rs:582-702` | ✅ (Gate A else → `Vec::new()` @701; Gate B else → base bare-park ladder) |

**Nothing in the prior tertiary/secondary reports is stale.** The `×2` is a
faithful cross-window recurrence count (real loop, not a storage artifact), and
none of the proposed remediations exist in code.

---

## 1. The failure geometry, restated architecturally

The signature is produced by **three independent defects on three seams**. They
look like one problem because they co-occur in the same composite string, but
they need three different fixes and must not be conflated:

```
                 ┌─────────────────────────────────────────────────────────┐
 D1  Emission    │ write_back writes back the recall-derived RecurringSignature │
     hygiene     │ → its overseer-obs: key nests inside the NEXT signature      │  → the literal `overseer-obs:…|overseer-obs:…` shape
                 └─────────────────────────────────────────────────────────┘
                 ┌─────────────────────────────────────────────────────────┐
 D2  Escalation  │ Lane B counter: append-only store_fact + recurrence=len()   │  → BOTH a ratchet (over-escalate when ACT runs)
     counter     │ AND the ACT path is gated shut by the WHY double-gate        │     AND a dead-zone (never escalate when ACT is gated)
                 └─────────────────────────────────────────────────────────┘
                 ┌─────────────────────────────────────────────────────────┐
 D3  Closing     │ WorkstreamCoverage Decide arm has no edge into launch.rs;   │  → workstream-gap re-observed forever, never converges
     edge        │ gap_gate has no cross-window recurrence ledger              │     (the `workstream-gap|workstream-gap` tail)
                 └─────────────────────────────────────────────────────────┘
```

The two secondary "lanes" map cleanly: **Lane A = D1's episode counter** (drives
the visible `×2`); **Lane B = D2's occurrence counter** (drives escalation).
They are decoupled — the operator-visible `×2` says nothing about whether Lane B
reached 3 — which is why the symptom persists at a *low, stable* count instead
of either escalating or vanishing.

---

## 2. Systemic fix — which of the three options, and why *all three seams*

The mandate offers three fix shapes. The evidence shows they are **not
alternatives** — each maps to exactly one defect. The correct systemic fix
applies the right shape at each seam:

| Fix shape (mandate) | Target defect | Verdict |
|---|---|---|
| **Dedup with count/timestamp** | **D2 counter** | ✅ **Primary fix.** Convert Lane B from node-multiplicity counting to a **caller-key upsert carrying an in-content `occurrence_count` + `first_seen`/`last_seen`**. |
| **Idempotent escalation** | **D2 decision** | ✅ **Required companion** to the above — escalation must read the counter field, and the escalate *decision* must latch/clear on the counter, not on `recall.len()`. |
| **Blocked-transition handling** | **D2 accrual gate (WHY double-gate)** | ✅ **Prerequisite.** Without closing the double-gate, the counter never accrues and any counter fix is dead code. |
| *(architectural, not in the 3-list)* **Recurrence-aware closing rung** | **D3** | ✅ **Independent, still required** — otherwise `workstream-gap` never converges regardless of the counter fixes. |
| *(emission hygiene)* **Exclude recall-derived problems from write-back** | **D1** | ✅ **Cheapest, orthogonal** — removes the literal nested shape without touching any counter. |

### 2.1 D2 — the count-in-content occurrence record (the core systemic fix)

**Do NOT** apply `store_fact_with_caller_key(root_cause_signature(...))`
verbatim — the secondary proved (`…VALIDATION_HEAD.md §4`) it collapses to a
single live fact so `recall.len()` sticks at 1 and escalation becomes
unreachable. The two goals — *stop the ratchet* and *still cross 3* — are only
reconcilable by moving the count **into the fact content**:

- **Write** (`record_occurrence`, mod.rs:1034): replace append-only `store_fact`
  with a caller-key upsert keyed on `root_cause_signature(entry.key, primary)`.
  On hit, deserialize, `occurrence_count += 1`, refresh `last_seen`, re-store
  the same key (supersede). One live fact per cause, count carried inside.
- **Read** (`recall_occurrences`/`RootCause.recurrence`): read
  `occurrence_count` from the single live fact instead of `recall.len()`.
- **Result:** the ratchet is gone (idempotent per cause) *and* the counter still
  advances monotonically to cross `RECURRENCE_ESCALATION_THRESHOLD` — satisfying
  both mandate shapes (a) and (b) with one record contract. Add a
  `distinct_windows`/`last_seen` guard so a flapping daemon can't inflate the
  count within one window (mirror the 900 s gate semantics in-content).

### 2.2 D2 — close the WHY double-gate so the counter can accrue (blocked-transition)

The counter fix is inert while `cycle.rs:582-702` fails open. Make WHY presence
an **invariant** (INV-WHY, prior §3.3): every `GoalProgress::Blocked` reason
carries a `NoProgressClass` within one OODA cycle. Concretely at HEAD:

- **Gate A else (`Vec::new()`, cycle.rs:701):** in a *daemon* context a `None`
  evidence source is a mis-boot — fail **loud** (startup `error!` + one operator
  escalation "safeguard DISABLED"), not silent skip. Tests/non-daemon keep an
  explicit `BreakerMode::Disabled` so `None` stops meaning two things.
- **Gate B else (base ladder, cycle.rs:685):** keep the kill-switch but have the
  base ladder stamp a WHY token (`no_progress_blocked_reason_with_why`) instead
  of the legacy bare `{PREFIX}{count}{SUFFIX}` — no path authors a WHY-less block.
- Run `reinvestigate_bare_blocked_goals` on a cadence needing **only** an
  `EvidenceSource`, so the installed base of bare parks converges retroactively.

### 2.3 D3 — recurrence-aware closing rung for workstream-gaps

Give gaps the same "seen N× → act" policy blocked goals have. Record each fresh
gap as a `PriorOccurrence` keyed `workstream-gap:{signature}` at the existing
commit site (mod.rs:931-934); on each Act, recall the count and partition:
**1× → Notify** (unchanged), **≥2× → LaunchRecipe** through the *existing*
`launch.rs` seam (bounded by `max_launches_per_cycle` + board dedup),
**≥3× / launch-unsafe → single operator escalation with history.** Threshold 2
(not 3): a recurring coverage gap has no benign transient explanation. Classify
the new remediation intervention at `LaunchRecipe`'s risk tier in
`guardrails.rs` (not `Routine`) so the autonomy/budget gate governs it.

### 2.4 D1 — stop the self-observation nesting (emission hygiene)

In `write_back_observation` (mod.rs:534-563), **filter out recall-derived
problems** (those whose `dedup_key` starts with `overseer-obs:`, i.e. the
`RecurringSignature` re-emission) before computing `observation_signature`. The
Overseer should not persist recollections of its own bookkeeping as fresh
observations. This alone removes the literal `overseer-obs:…|overseer-obs:…`
nesting from the composite without perturbing genuine `goal:blocked` /
`workstream-gap` recurrence signalling.

---

## 3. Remaining gap at HEAD (the answer to the mandate's second half)

**All four fixes are still PROPOSED; the entire defect surface is live at
`dea65df8`.** The single **most load-bearing remaining gap** is the coupling
between D2's two halves:

> The recurrence counter (Lane B) and the accrual gate (WHY double-gate) form a
> **latch**: fixing either alone changes nothing observable.
> - Fix the counter (idempotent count-in-content) but leave the gate open →
>   still over-escalates the moment ACT runs.
> - Fix the counter but leave the gate **shut** → count stays 0, escalation stays
>   dead, `×2` persists forever (today's exact symptom).
> - Close the gate but leave the append-only ratchet → escalation fires, then
>   **re-fires every cycle** (new defect substituted for old).

Therefore **§2.1 (count-in-content) and §2.2 (close the double-gate) must ship
together**; neither is independently correct. §2.3 (D3 closing rung) and §2.4
(D1 hygiene) are independently shippable and independently valuable.

Secondary residual gaps not yet addressed anywhere in code:
- **Cross-restart episode inflation (Lane A):** `write_back_gate` is an
  in-process `HashMap` (`guardrails.rs:294`) — a flapping daemon re-persists the
  same signature per restart, inflating `occurrences`. The count-in-content
  discipline should extend to the episode lane (dedup on `(signature, window)`)
  if restart-flapping is confirmed as a source of the `×2`.
- **No convergence gauge:** nothing counts "gap signatures ≥2× that produced no
  launch" or "blocked reasons failing `is_bare_no_progress_block`". Without it,
  a re-regression is invisible. Add both counters beside the existing
  `workstream_gaps_detected/_suppressed` (`activity.rs`, `wiring.rs`).

---

## 4. Recommended landing order (dependency-correct)

1. **D2 gate + counter together** (§2.2 then §2.1) — the latch; unblocks every
   `goal:blocked:*` row. *Highest priority, must be atomic.*
2. **D3 closing rung** (§2.3) — converges the `workstream-gap` family (the
   simard-identity personas + the `workstream-gap|workstream-gap` tail).
3. **D1 write-back filter** (§2.4) — removes the nested `overseer-obs:` shape.
4. **Convergence gauges** (§3 residual) — proof the fix holds, guards regression.

**Verdict:** the correct systemic fix is **not** a single choice from the
three-option menu — it is *dedup-with-count/timestamp* **and** *idempotent
escalation* (one count-in-content record satisfies both) gated behind
*blocked-transition handling* (closing the WHY double-gate), plus an independent
recurrence-aware closing rung for workstream-gaps and a one-line write-back
hygiene filter. None of it exists at HEAD `dea65df8`.
