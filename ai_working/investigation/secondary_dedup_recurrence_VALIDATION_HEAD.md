# Secondary Investigation (re-run) — Dead-Zone, Dedup-vs-Re-emit, Write-Back Loop

**Scope:** Validate/EXTEND the prior secondary findings against current HEAD
`dea65df8`. Focus: recurrence dead-zone below `RECURRENCE_ESCALATION_THRESHOLD=3`,
dedup vs. re-emit behavior, and the self-referential `overseer-obs:` write-back
loop.

**Bottom line:** Prior conclusions are **CONFIRMED at HEAD** — every cited line
number still matches. I add one sharper framing (the **two independent counter
lanes**) and one **critical caveat on the recommended fix** that must not be
applied blindly.

---

## 1. Validation — every prior citation still holds at HEAD dea65df8

| Claim | Location | Status |
|---|---|---|
| `RECURRING_SIGNATURE_THRESHOLD = 2` (emit) | `signal.rs:362`, used `:463` | ✅ holds |
| `RecurringSignature` emitted when `occurrences >= 2` | `signal.rs:462-468` | ✅ `>=` confirmed |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (escalate) | `root_cause.rs:33` | ✅ holds |
| Escalation gate `recurrence >= 3` | `mod.rs:1613` (`decide_blocked_goal`) | ✅ `>=` confirmed |
| `observation_signature` = sorted/deduped `dedup_key`s | `mod.rs:1068-1073` | ✅ unchanged |
| Write-back gated by `WhisperGate::new(900,5)` | `mod.rs:299`, `:548-556` | ✅ 15-min window, cap 5/h |
| Slot committed only after successful store | `mod.rs:556` | ✅ confirmed |
| `record_occurrence` uses **non-idempotent** `store_fact` | `mod.rs:1034` | ✅ **NOT** yet switched |
| `store_fact` appends unconditionally (no dedup) | `library_adapter.rs:657-683` | ✅ confirmed |
| Recall counts up to `OCCURRENCE_RECALL_LIMIT=256` | `mod.rs:977`, `:1137` | ✅ confirmed |
| `root_cause_signature` helper exists | `root_cause.rs:53-55` | ✅ exists |
| `store_fact_with_caller_key` (CallerKey dedup) exists | `library_adapter.rs:870-915` | ✅ exists |

**The recommended fix has NOT been applied.** `record_occurrence` still calls
`mem.store_fact(...)` at `mod.rs:1034`. Safe to propose — see §4 caveat first.

---

## 2. NEW sharper framing — TWO independent counter lanes (not one dead-zone axis)

Prior notes describe a single "emit at 2 / escalate at 3, dead-zone at 2" axis.
That is imprecise: the two thresholds live on **two different storage lanes with
different concepts, different increment cadences, and different dedup behavior**.
Confusing them hides the real failure geometry.

### Lane A — OBSERVATION EPISODES (drives the visible "seen 2×")
- Written by `write_back_observation` → `record_observation`
  (`wiring.rs:1076-1088`) → `store_episode` (unconditional, no dedup).
- Concept/key: the composite `overseer-obs:...` signature.
- Increment cadence: **once per 900 s window** (gate at `mod.rs:548`).
- Counted by `signals_from` → `RecurringSignature.occurrences`,
  threshold **2** (`signal.rs:463`).
- **This is the number in the investigation question ("seen 2×").** It counts
  *write-back episodes of an unchanged problem set*, one per window/restart.

### Lane B — ROOT-CAUSE OCCURRENCES (drives escalation)
- Written by `record_occurrence` (`mod.rs:1004-1043`) → `store_fact`
  (unconditional, no dedup, `library_adapter.rs:657`).
- Concept: `occurrence_concept(dedup_key)` = `overseerocc<8-byte sha256>`
  (`mod.rs:1147-1156`).
- Increment cadence: **once per ACT that touches this cause** (each cycle the
  blocked goal is acted on).
- Counted by `recall_occurrences` (`mod.rs:972-997`, LIMIT 256) →
  `RootCause.recurrence`, escalation threshold **3** (`mod.rs:1613`).

**Key consequence:** the two counters are **decoupled**. The operator-visible
`×2` (Lane A) tells you *nothing* about whether Lane B has reached escalation.
The "dead-zone at 2" is therefore a **cross-lane visibility gap**, not a single
counter sitting between two thresholds:

- If the ACT path for a blocked goal is **gated/unreached** (owned by the
  primary investigator: double-gated WHY-reasoner, flag-without-close), Lane B
  never increments → recurrence stays 0/1/2 → **never escalates**, while Lane A
  faithfully shows `×2` forever. This is the true dead-zone: **a visible,
  above-noise signal that is structurally incapable of reaching its own
  escalation rung.**
- If the ACT path **is** reached every cycle, Lane B ratchets monotonically
  (§3) and **over-escalates** — 2 distinct defects, opposite directions,
  depending only on whether one gate opens.

---

## 3. Dedup vs. re-emit — the definitive answer for BOTH lanes

**Neither lane deduplicates at the storage layer. Both re-emit.**

- **Within a window**, Lane A is correctly suppressed by `write_back_gate`
  (test `write_back_is_deduplicated_within_window`, `tests_memory_recall.rs:797`).
  That is the *only* dedup present, and it is time-boxed + in-process
  (`WhisperGate.last_delivered` is a per-process HashMap — no cross-restart
  protection).
- **Across windows/restarts**, Lane A re-persists the same signature as a new
  episode (`store_episode`, no upsert). → the `×2` is a **true count of
  re-observation episodes**, not a duplicated node. **REAL loop, not artifact.**
- Lane B has **no gate at all** — `record_occurrence` writes a fresh fact under
  the same concept on every act. `recall_occurrences` counts them all (≤256). →
  `recurrence` is a **monotonic ratchet on act-count**, not a count of distinct
  re-occurrences of the problem. Once it crosses 3 it **latches escalation
  forever** (the escalation *action* is idempotent via `blocked_goal_gate`
  `WhisperGate(900,20)` at `mod.rs:292`, but the escalation *decision* never
  un-latches).

**Verdict:** the `×2` is a correct recurrence count (Lane A, real re-observation).
The escalation counter (Lane B) is separately corrupted by a non-idempotent
ratchet. Both are true; they are different defects on different lanes. Do not
conflate.

---

## 4. CRITICAL caveat on the prior "recommended fix" (new — must verify)

Prior notes recommend: *switch `record_occurrence` to
`store_fact_with_caller_key(root_cause_signature(...))`.*

**This fix, applied literally, would BREAK escalation.** `store_fact_with_caller_key`
uses `DedupMode::CallerKey` (`library_adapter.rs:885-889`): identical-content
reuse, changed-content **supersede** — **exactly one live fact survives per key**.
`recall_occurrences` reads only live facts via `search_facts` → it would return
**1**, permanently. Since `root_cause_signature(problem, primary)` is **stable**
for a repeating cause, every occurrence collapses to that single key, so
`recurrence` could **never reach `RECURRENCE_ESCALATION_THRESHOLD=3`** →
the recurring-re-park escalation rung (`mod.rs:1613`) would become **dead code**.

The two design goals are in direct tension:
1. *Stop the ratchet* (idempotency) — argues for caller-key dedup.
2. *Count genuine re-occurrences to cross 3* — argues against collapsing to 1.

**Correct fix must carry the count in-content**, not rely on node multiplicity:
a caller-key upsert whose `content` holds an incremented `occurrence_count`
(+ first/last-seen), with escalation reading that field instead of `recall.len()`.
Pure `store_fact_with_caller_key` alone is **not** a drop-in. Flag this for the
verification/design phase — the prior recommendation is a trap.

---

## 5. Self-referential write-back loop — confirmed, bounded

Confirmed at HEAD: `write_back_observation(&cycle.problems)` (`wiring.rs:301`)
writes back **all** cycle problems, including the recall-derived
`RecurringSignature` → `ProcessHealth` problem whose `dedup_key` is
`sanitize_recalled(signature)` (an `overseer-obs:...` string, `mod.rs:1359`).
Because that key differs from the base `goal:blocked:*` keys, `orient` does not
merge it away; it folds into the **next** `observation_signature`, producing the
`overseer-obs:` tokens **nested inside** the composite — exactly the shape in the
investigation question. The Overseer recalls and re-observes its own bookkeeping.

Bounded (900 s gate + recall LIMIT + orient same-key merge + threshold 2), so it
stabilizes into a small family of nested `×2` signatures rather than running away
— consistent with the observed data. Still a smell: the presence of
`sanitize_recalled` at the admission boundary shows the authors already treat
recalled signatures as untrusted, yet still write them back.

---

## 6. Concerns / questions for verification phase

1. **Two-lane decoupling is the root geometry.** Any remediation must state
   *which lane* it fixes. A "convergence rung at ×2" on Lane A does not touch
   Lane B's ratchet, and vice-versa.
2. **Do NOT apply `store_fact_with_caller_key(root_cause_signature(...))`
   verbatim** — it makes escalation unreachable (§4). Require the count-in-content
   variant + escalation reading the counter field.
3. **Lane A cross-restart gap:** confirm whether the two episodes came from two
   windows in one run or two restarts; the in-process gate (`guardrails.rs:294`)
   gives no cross-restart dedup, so a flapping daemon inflates Lane A too.
4. **Is `decide_blocked_goal`'s ACT path even reached** for these goals? (Primary
   investigator's area.) If gated shut, Lane B is the *dead-zone*; if open, Lane B
   is the *ratchet*. The fix set differs — both may be needed.
5. Consider **excluding recall-derived `ProcessHealth`/`overseer-obs:`-keyed
   problems from `write_back_observation`** to stop §5 self-observation nesting
   without touching genuine recurrence signalling.

**Reconciliation with CONSOLIDATED_FINDINGS.md:** no contradictions. I confirm
its "REAL re-observation loop, not a dedup artifact" verdict and its
"non-idempotent ratchet" finding, and I *extend* both by (a) separating them onto
two explicitly distinct storage lanes and (b) flagging that the proposed one-line
fix is unsafe as written.
