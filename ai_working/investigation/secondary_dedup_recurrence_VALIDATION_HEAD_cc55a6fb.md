# Secondary Investigation (validation) — Within-Window Dedup + 2× Verdict @ HEAD cc55a6fb

**Role:** SECONDARY investigator. **Mandate:** validate within-window dedup
(`write_back_gate` / `WhisperGate`) and issue the **defect-vs-honest-re-observation**
verdict for the exactly-2× count, backed by *current* recurrence-test status.
**Method:** validate — do not re-derive. Prior secondary artifact was grounded at
`dea65df8`; this pass re-grounds every load-bearing citation at HEAD `cc55a6fb`
and runs the targeted tests empirically.

---

## VERDICT (headline)

The **×2 is an HONEST re-observation count, NOT a duplication / replay / collision
defect.** H1 (faithful count of a static unresolved problem set) is confirmed; H0
(dedup/replay/collision bug) is rejected. The within-window dedup guarantee is
present, correct, and test-enforced. The recurrence "problem" is a **missing
convergence rung**, not a broken counter — consistent with the DISCOVERIES
meta-pattern *"the recurrence count is honest; audit the closing action, not the
counter."*

---

## 1. Citation drift check — every load-bearing line still LIVE at HEAD cc55a6fb

| Claim | Location (HEAD cc55a6fb) | Status |
|---|---|---|
| `write_back_gate = WhisperGate::new(900, 5)` | `mod.rs:299` | ✅ 900 s window, cap 5/h |
| `observation_signature` = `sort_unstable → dedup → "overseer-obs:"+join("\|")` | `mod.rs:1068-1073` | ✅ unchanged |
| Write-back peeks then commits only after successful store | `mod.rs:546-556` | ✅ slot consumed post-store |
| `WhisperGate.last_delivered` is a per-process `HashMap<String,i64>` | `guardrails.rs:294`, `:305` | ✅ in-process only, no cross-restart dedup |
| `peek` suppresses when `now - last < window_secs` | `guardrails.rs:312-317` | ✅ time-boxed dedup |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A emit) | `signal.rs:362`, used `>=` at `:463` | ✅ holds |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B escalate) | `root_cause.rs:33` | ✅ holds |
| Escalation gate `recurrence >= 3` | `mod.rs:1613` | ✅ `>=` confirmed |
| `record_occurrence` still uses **non-idempotent** `store_fact` | `mod.rs:1004`, `:1034` | ✅ **NOT** switched (de-ratchet trap still unsprung — good) |

**Conclusion:** zero drift. Every prior secondary citation is live. The prior
recommended one-line `store_fact_with_caller_key` fix has **not** been applied,
so the de-ratchet trap (see §4) remains a live risk for the fix phase.

---

## 2. Within-window dedup — VALIDATED (empirical, not just static)

`cargo test --lib overseer::` → **361 passed / 0 failed** at HEAD cc55a6fb.
Load-bearing tests, all GREEN:

- `tests_memory_recall::write_back_is_deduplicated_within_window` ✅
  Two identical ticks inside the 900 s window ⇒ `t2.memory_writes == 0`,
  exactly one episode persisted. **This is the only dedup present**, and it is
  time-boxed + in-process (`last_delivered` HashMap).
- `tests_memory_recall::write_back_persists_again_for_a_distinct_signature` ✅
  Two *different* observations ⇒ two distinct signatures ⇒ both recorded. Proves
  the signature is discriminating, not a blanket suppressor.

So: **inside a window, identical signatures are correctly suppressed; across
windows/restarts they legitimately re-persist** (`store_episode`, no upsert). The
×2 is therefore a real count of two re-observation episodes — the honest,
above-noise-below-escalation number.

---

## 3. Two-lane framing is now ENCODED IN THE TEST SUITE (new since prior pass)

The prior secondary artifact *introduced* the "two independent counter lanes"
framing as prose. At HEAD it is now baked into live tests (`tests_root_cause.rs`),
all GREEN:

- `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` ✅
  Proves a loud Lane-A ×2 (observation episodes) does **not** increment Lane-B
  recurrence. The two counters are provably decoupled — the ×2 tells you nothing
  about escalation state.
- `lane_b_escalates_without_any_lane_a_signal` ✅
  Proves Lane B (root-cause occurrences → escalation at 3) fires independently of
  Lane A entirely.
- `occurrence_recall_accumulates_recurrence_across_ticks` ✅
  Confirms Lane B's `store_fact` ratchet accumulates per-act (monotonic).
- `recurring_reblock_escalates_root_cause_not_blind_unblock` ✅
- `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` ✅ (idempotent
  unblock on the health path).

**Implication:** the two-lane geometry is no longer a hypothesis — it is a
regression-guarded contract. Any remediation MUST declare which lane it touches;
a "convergence rung at ×2" on Lane A cannot fix Lane B's ratchet and vice-versa.

- **Lane A** (visible ×2): `write_back_observation → record_observation →
  store_episode`, +1 per 900 s window, threshold 2 (`signal.rs:463`). Dedup =
  in-process `WhisperGate` only.
- **Lane B** (escalation): `record_occurrence → store_fact` (`mod.rs:1034`),
  +1 per ACT, threshold 3 (`mod.rs:1613`). **No storage-layer dedup at all.**

---

## 4. Defect-vs-honest — the discriminating answer

| Question | Answer | Evidence |
|---|---|---|
| Is the ×2 a dedup/replay/collision defect? | **NO** | within-window dedup test GREEN; signature deterministic (sorted+deduped) `mod.rs:1070-1072` |
| Is the ×2 an honest re-observation count? | **YES** | cross-window re-persist by design (`store_episode`, no upsert); `last_delivered` is in-process so window/restart boundary legitimately yields a 2nd episode |
| What legitimately produces the 2nd episode? | window rollover (>900 s) **or** daemon restart resetting in-process `last_delivered` (`guardrails.rs:294`) | in-process HashMap = no cross-restart dedup |
| Where is the actual defect then? | the **closing action**, not the counter — Lane A has no convergence rung, Lane B ratchets non-idempotently | owned by primary/tertiary areas |

**Caveat carried forward (de-ratchet TRAP — still unsprung, keep it that way):**
the literal `store_fact_with_caller_key(root_cause_signature(...))` fix would make
`recall_occurrences` collapse to 1 forever (`DedupMode::CallerKey`,
`library_adapter.rs:885-889`), turning the `mod.rs:1613` escalation into dead
code. The correct de-ratchet is a **count-in-content upsert** with escalation
reading the counter field, not `recall.len()`. This remains a trap for the fix
phase; do not apply verbatim.

---

## 5. Reconciliation ledger (prior claims vs HEAD)

| Prior load-bearing claim | Re-grounded at cc55a6fb? |
|---|---|
| ×2 is honest re-observation, not dedup artifact | ✅ CONFIRMED (test-backed) |
| Only dedup is in-process time-boxed `WhisperGate` | ✅ CONFIRMED |
| Two decoupled counter lanes (A=episodes, B=occurrences) | ✅ CONFIRMED — now test-enforced |
| `record_occurrence` non-idempotent `store_fact` ratchet | ✅ CONFIRMED (mod.rs:1034 unchanged) |
| `store_fact_with_caller_key` verbatim = de-ratchet trap | ✅ STILL VALID (not yet applied) |
| Self-feed nesting `overseer-obs:\|overseer-obs:` = D1 write-back of recall-derived signature | ✅ CONSISTENT (owned by primary; signature assembly at mod.rs:1068-1073 unchanged) |

**No stale citations found.** Zero drift to correct.

---

## 6. Concerns / questions for verification phase

1. **Lane A cross-restart inflation:** confirm whether the observed two episodes
   came from two windows in one run or two daemon restarts. The in-process
   `last_delivered` gives no cross-restart dedup, so a flapping daemon can inflate
   Lane A beyond honest re-observation. Recommend a persisted last-seen if restart
   flapping is real (out of scope to fix here).
2. **Which lane does each proposed remediation touch?** Enforce this in review —
   the two lanes are now regression-guarded as independent.
3. **De-ratchet trap:** block any PR that swaps `store_fact` → plain
   `store_fact_with_caller_key` on the Lane-B path without count-in-content.
4. **Self-feed filter (D1):** excluding recall-derived `overseer-obs:`-keyed
   problems from `write_back_observation` stops the nested-prefix loop without
   touching genuine Lane-A recurrence signalling — verify it does not regress
   `write_back_persists_again_for_a_distinct_signature`.

**Bottom line:** within-window dedup is present, correct, and test-enforced; the
×2 is an **honest re-observation count**, not a defect. The work to close is the
missing convergence rung (Lane A) and the non-idempotent ratchet (Lane B), both
owned by the primary/tertiary areas — not the counter itself.
