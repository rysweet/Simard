# Hypotheses — Recurring `overseer-obs:…goal:blocked…|workstream-gap` Signature (seen 2×)

**Derived from:** [`CONSOLIDATED_FINDINGS.md`](./CONSOLIDATED_FINDINGS.md) (six re-validation waves,
HEAD `0289572e`; all investigation commits docs-only, defects live in source).
**Question restated:** *why does the composite observation signature recur (seen 2×) in
cognitive memory, and what mechanism keeps the underlying problem set from changing?*

Each hypothesis below is stated so it can be **confirmed or falsified** against source /
tests. Confidence reflects the weight of already-collected evidence; the leading
hypotheses (H1–H5) are treated as **validated** by the consolidated waves — retained here in
hypothesis form so the falsification tests remain the acceptance criteria for any fix.

---

## H0 (Null) — The `2×` is a dedup / storage / replay / collision artifact
**Statement:** the recurrence is a counting bug — the same episode double-read, a `dedup()`
collapse defect, a hash collision, or a cross-store (stewardship) duplication — not a real loop.

- **Mechanism if true:** one write-back read twice; `keys.dedup()` failing to collapse repeats;
  `sha256` prefix collision; or the `overseer-obs:` signature leaking into the GitHub-issue store.
- **Falsification test (all executed / traceable):**
  - `write_back_is_deduplicated_within_window` (`tests_memory_recall.rs:797-817`) proves the
    within-window gate suppresses same-window dupes → not a double-read.
  - `keys.dedup()` collapses only *adjacent equal* keys within **one** signature
    (`mod.rs:1071`); the repeated `workstream-gap|workstream-gap` are **distinct problems/episodes**
    concatenated in the recall stream → not a `dedup()` bug.
  - Store-boundary trace (§10.3): the composite lives **only** in cognitive memory
    (`overseer-obs:{join}` key), is **never** written to the stewardship store (keyed on
    `failure_signature = sha256(kind‖norm(err))[..8]`, `stewardship/dedup.rs:63-75`) → not
    cross-store duplication.
- **Verdict: REJECTED (high confidence).** The count is *honest* — two distinct episode nodes
  exist for two legitimate write-back passes.

---

## H1 (LEADING) — The `2×` is a REAL re-observation loop of a (near-)static problem set
**Statement:** the identical/near-identical set of open problems produced the identical composite
`observation_signature` across ≥2 distinct 15-min windows; the fingerprint repeats because the
**underlying problem set does not change between passes**.

- **Mechanism:** `observation_signature` (`mod.rs:1068-1073`) is deterministic (sort+dedup+join),
  so a static membership yields a stable string; `RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:362`) fires when recall sees ≥2 episodes sharing that `failure_signature`
  (`signal.rs:455-469`). Window-gating (`write_back_gate = WhisperGate::new(900,5)`) means the two
  counts are **two distinct 15-min windows**, not a within-tick duplicate.
- **Refinement (§11.2):** the two snapshots are *overlapping-but-different* — 8 kgpacks/core
  `goal:blocked` goals **persist**, five `simard-identity-*` goals **drop**, `resource:engineer_spawn`
  + an extra `workstream-gap` **appear**. Membership A ≠ B ⇒ `signature(A) ≠ signature(B)` by design;
  the recall counter then saw the recurring *family prefix* ≥2×. So "static set" → **"near-static set."**
- **Falsification test:** membership-delta diff of the two snapshots (would fail if the two episodes
  were byte-identical single-window writes); `signal.rs:455-469` tally is on episodes not facts (§10.2).
- **Verdict: SUPPORTED (high confidence).** This is the direct answer to "why 2×."

---

## H2 — Blocked goals persist because the no-progress WHY reasoner is double-gated off
**Statement (Root cause A):** self-resolvable stalls degrade to a **bare "needs human review"**
park because the WHY-classification ladder is opt-in and *fails open to bare-park*, so the goal
re-parks every window and stays in the `goal:blocked:*` population.

- **Mechanism:** breaker fires after `NO_PROGRESS_BREAKER_THRESHOLD = 3` idle cycles
  (`no_progress_breaker.rs:59,75`). The corrective `NoProgressClass` ladder
  (`no_progress_breaker.rs:384-417`) is behind two silent switches (`cycle.rs:582-702`):
  **Gate A** `completion_evidence.is_some()` (else the whole block → `Vec::new()`), **Gate B**
  `no_progress_investigation_enabled()` (`no_progress.rs:199-207`). No invariant guarantees a
  `Blocked` reason carries a `NoProgressClass`.
- **Canonical evidence:** seven `kgpacks-rs` goals parked "no progress" while work was **done**
  (issues CLOSED, PRs MERGED) — safeguard misread *done* as *stuck* (`no_progress_why.rs` header).
- **Falsification test (acceptance criterion):** **INV-WHY** — for any `Blocked(reason)`,
  `is_bare_no_progress_block(reason)` is `false` within one OODA cycle. If this can be violated in
  source today, H2 holds; if an invariant already forces a WHY token, H2 is false.
- **Verdict: SUPPORTED (high confidence).** Primary common root cause for the `goal:blocked` tokens.

---

## H3 — `workstream-gap` recurs forever because its Decide arm has no closing edge
**Statement (Root cause B / D3):** `WorkstreamCoverage` is the **only** High-priority arm with no
edge into `launch.rs` — it only *notifies*, files no issue, launches no workstream — and `gap_gate`
has no cross-window ledger, so the coverage gap is flagged and deduped forever without converging.

- **Mechanism:** Observe → consolidated `Signal::WorkstreamGap` → `WorkstreamCoverage` problem →
  `act_flag_workstream_gaps` (`mod.rs:884-948`) = notify-only. Siblings (`ProcessHealth`,
  `CrossCutting`, `StepFailure`) all reach `LaunchRecipe` (`mod.rs:1429,1436,1565`). Dual-path hole:
  observer routes to `Report` (`observer.rs:120`), acting overseer to notify-only
  `FlagWorkstreamGaps` (`mod.rs:1543`).
- **Key-identity caveat (INV-GAP-KEY):** any recurrence ledger / launch dedup / issue MUST key on
  `GapItem.signature` (`signal.rs:135-138`), **never** the constant `problem.dedup_key ==
  "workstream-gap"` (`mod.rs:1371`) which erases per-gap identity.
- **Falsification test:** add a gap that recurs across 2 windows; if no `LaunchRecipe`/issue/escalation
  is ever emitted (only a repeat notify), H3 holds. A closing rung that fires at 2× falsifies it.
- **Verdict: SUPPORTED (high confidence).** Explains the `workstream-gap|workstream-gap` tail.

---

## H4 — The nested `overseer-obs:` tokens are self-observation feedback (D1)
**Statement:** the overseer recalls its own prior `overseer-obs:…` write-back, admits it as a fresh
`RecurringSignature` problem, and writes it back — nesting its own bookkeeping inside future signatures.

- **Mechanism (verified path §0a):** recall of ≥2 episodes → `Signal::RecurringSignature`
  (`signal.rs:455-469`) → admitted as `ProcessHealth` problem keyed `sanitize_recalled(signature)`
  (`mod.rs:1353-1359`); differs from base keys so Orient's same-key merge (`mod.rs:1210-1221`) does
  **not** fold it → `push`ed (`mod.rs:1222`) → `write_back_observation(&cycle.problems)`
  (`wiring.rs:301`) re-emits it → next `observation_signature` embeds the prior `overseer-obs:` token.
- **Design smell:** `sanitize_recalled` at the admission boundary (`mod.rs:1359`) shows authors
  already treat recalled signatures as untrusted, yet still write them back.
- **Falsification test:** filter recall-derived `RecurringSignature` problems out of the write-back
  set (§6.5); if nested `overseer-obs:` tokens disappear from future signatures, H4 held.
- **Verdict: SUPPORTED (high confidence), bounded loop** (throttled by write-back gate, recall limit,
  merge, and the ×2 threshold).

---

## H5 — `2×` is a recurrence "dead zone" between two decoupled thresholds
**Statement:** detection and action are separated — the signal fires at
`RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A / episodes) but escalation only fires at
`RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B / root-cause occurrences), and coverage gaps have **no**
cross-window recurrence tracking at all. So 2× sits one below escalation and above one-off noise:
remediated never, escalated never.

- **Mechanism (§3c two lanes):** *Lane A* — observation episodes drive the visible `×2`
  (threshold 2, `signal.rs:463`); *Lane B* — root-cause occurrences drive escalation (threshold 3,
  `mod.rs:1613`). The lanes are **decoupled** — the operator-visible `×2` says nothing about whether
  Lane B reached 3.
- **Falsification test:** confirm the two counters read from different stores/keys (episodes vs.
  facts) and that no remediation rung exists at count 2 for either gaps or blocked goals. A unified
  "seen N× → remediate/escalate" policy falsifies the dead zone.
- **Verdict: SUPPORTED (high confidence).** Explains why the count is exactly, and only, "2×."

---

## H6 — Compounding, NOT causal: two non-idempotency defects inflate/decouple counts
**Statement:** the recurrence counters lack storage-layer idempotency, but this is a *compounding*
defect — it does **not** produce the visible `×2` (H1 does), and the naïve fix is a trap.

- **Mechanism:**
  - *Lane B ratchet (§3a):* `record_occurrence` uses non-deduping `store_fact` (`mod.rs:1034`;
    `library_adapter.rs:657`) → `recurrence` is a monotonic lifetime write-count; once ≥3 the goal
    **latches** on `EscalateBlockedGoal` forever.
  - *Lane A episodes (§3b):* `record_observation`→`store_episode` is unconditional
    (`wiring.rs:1076-1091`; `library_adapter.rs:609-628`); the `write_back_gate` `last_delivered` map
    is in-memory/per-process (`guardrails.rs:294`) → no cross-restart dedup.
- **Trap (falsifies the naïve fix):** replacing `store_fact` with
  `store_fact_with_caller_key(root_cause_signature,…)` collapses `recall_occurrences().len()` to 1
  permanently → `recurrence >= 3` becomes **dead code** (`secondary_dedup_recurrence_VALIDATION_HEAD.md §4`).
  Correct fix carries the count **in fact content** (§6.2b).
- **Falsification test:** a count-in-content upsert must (a) stop lifetime inflation *and* (b) still
  let `recurrence` cross 3 for a genuinely recurring cause *and* (c) fall back to self-heal once the
  cause clears. Any fix failing (b) or (c) confirms the trap.
- **Verdict: SUPPORTED as compounding (high confidence).** Not the cause of the visible `2×`.

---

## H7 — The two signatures (`goal:blocked` ↔ `workstream-gap`) are one problem in two views
**Statement:** an under-resourced important goal **oscillates** — `workstream-gap` (GoalUncovered)
while active with no workstream, then `goal:blocked` once the breaker parks it and it leaves the
gap-scan — so the same goals appear in both recurring families.

- **Evidence:** personas, the coverage audit, the coin harness, and kgpacks appear in **both**
  families and co-occur inside the same composite (§4). Blocked goals are skipped by the gap scan
  (routed via `goal_health`, no double-notify) — the transition *is* the oscillation.
- **Falsification test:** track a single goal across windows; if it never appears in both families,
  H7 is false. Convergence of one goal removes it from both → confirms one-problem framing.
- **Verdict: SUPPORTED (high confidence).**

---

## H8 — All three token families are one under-throughput problem in three views
**Statement:** `goal:blocked:*` (GoalHygiene), `workstream-gap` (WorkstreamCoverage), and
`resource:engineer_spawn` (ResourcePressure) are causally **one** under-resourcing/under-throughput
condition: the system *is* spawning engineers (`engineer_spawn` up) yet goals stay blocked and gaps
stay uncovered — all three are observe-and-flag with no closing action, all in the same 2× dead zone.

- **Evidence:** `resource:engineer_spawn` is **benign membership drift, not code drift** — the literal
  key predates the investigation (`add1708a`, #2419/#2533); its `{live}` count lands only in the
  summary, never in the signature (§11.1). All three deduped by the 15-min gate yet below escalation=3.
- **Falsification test:** if closing the WHY gate (H2) + gap rung (H3) does **not** also relieve
  `engineer_spawn` pressure, the three-in-one framing is too strong. Shared relief confirms it.
- **Verdict: SUPPORTED (medium-high confidence).** A generalization of H7; slightly weaker because
  `engineer_spawn` appeared in only the later snapshot.

---

## Consolidated hypothesis map

| ID | Hypothesis | Role | Defect | Confidence | Status |
|----|-----------|------|--------|-----------|--------|
| H0 | Dedup/storage/replay/collision artifact | Null | — | High | **REJECTED** |
| H1 | Real re-observation loop, near-static set | Cause of `2×` | — | High | Validated |
| H2 | WHY reasoner double-gated → bare parks | Root cause A | D2 (latch) | High | Validated |
| H3 | `WorkstreamCoverage` has no closing edge | Root cause B | D3 | High | Validated |
| H4 | Self-observation write-back feedback | Nesting cause | D1 | High | Validated |
| H5 | 2×↔3× dead zone, two decoupled lanes | Why "exactly 2×" | D2 | High | Validated |
| H6 | Non-idempotent counters (compounding) | Amplifier | D2 | High | Validated (non-causal) |
| H7 | blocked ↔ gap = one problem, two views | Unifier | — | High | Validated |
| H8 | Three token families = one under-throughput | Generalization | — | Med-High | Validated |

### Leading explanation (composite)
The `2×` is a **faithful, honest re-observation count of a genuinely re-observed near-static
problem set** (H1, H0 rejected). That set never changes because **two observe-and-flag loops never
close** — blocked goals bare-park with no WHY (H2), coverage gaps notify with no launch edge (H3) —
and the count parks in a **dead zone between thresholds 2 and 3** (H5), while the overseer
**re-observes its own bookkeeping** (H4) and the counters **lack idempotency** (H6). H7/H8 unify the
symptoms into **one under-throughput condition in three views**.

### Predictions that discriminate the fix (must all trend to zero once §6 lands)
1. Close the WHY double-gate + count-in-content counter atomically (D2 latch) ⇒ blocked goals route
   the self-resolving ladder; `goal:blocked:*` tokens converge (falsifies "stuck forever").
2. Add the recurrence-aware gap-closing rung at threshold 2 (D3) ⇒ `workstream-gap|workstream-gap`
   tail converges (falsifies "flag-forever").
3. Filter recall-derived `RecurringSignature` from write-back (D1) ⇒ nested `overseer-obs:` tokens
   vanish (falsifies self-observation nesting).
4. Persistent-unremediated gauge (§6.4): count of signatures with recurrence ≥2 and no
   launch/escalation, plus INV-WHY violations, must reach 0 and stay 0 — the leading regression
   indicator that a signature re-entered the dead zone.
