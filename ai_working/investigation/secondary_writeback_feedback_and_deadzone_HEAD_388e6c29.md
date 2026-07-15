# Secondary Investigation — Self-Observation Write-Back Feedback Loop & Recurrence Dead-Zone

**HEAD:** `388e6c29` (verified). Only commit since `85b9398a` is docs; every citation below
re-confirmed against live `src/` (no doc-to-doc trust).

**Focus:** (1) the RecurringSignature recall→write-back feedback loop that folds `overseer-obs:`
fragments back into the next `observation_signature` (D1); (2) the recurrence dead-zone /
non-convergence across `goal:blocked`, `workstream-gap`, `resource:engineer_spawn`.

---

## 1. D1 — Self-observation write-back feedback loop (CONFIRMED defect)

The composite blob in the investigation question — nested `overseer-obs:goal:blocked:…` fragments
joined by `|`, with the tell-tale `workstream-gap|workstream-gap` doubling, wrapped in
`recurring signature seen 2× in cognitive memory (…)` — is produced deterministically by this loop:

| Step | Location | Behavior |
|---|---|---|
| 1. Write | `mod.rs:534-563` (`write_back_observation`), call site `wiring.rs:301` | Writes **unfiltered** `&cycle.problems`. Signature = `observation_signature`. |
| 2. Signature | `mod.rs:1068-1073` | `"overseer-obs:" + sorted/deduped dedup_keys.join("|")`. No key is excluded. |
| 3. Store | `wiring.rs:1076-1091` (`record_observation`) | Persists episode content `"{content} [sig:{signature}]"`. |
| 4. Recall parse | `wiring.rs:976-986` (`parse_failure_signature`) + `:1013-1031` (`recall_episodic`) | Recovers the `[sig:overseer-obs:…]` marker back into `RecalledEpisode.failure_signature`. |
| 5. Count→signal | `signal.rs:455-469` | Counts episodes by `failure_signature`; at `>=2` (`RECURRING_SIGNATURE_THRESHOLD`, `signal.rs:362`) emits `Signal::RecurringSignature{ signature:"overseer-obs:…", occurrences }`. |
| 6. Classify | `mod.rs:1353-1363` | `Problem{ dedup_key: sanitize_recalled("overseer-obs:…"), summary:"recurring signature seen {n}× in cognitive memory ({sig})" }`. |
| 7. Re-fold | back to step 1 | That Problem is in `cycle.problems`, so its `overseer-obs:…` dedup_key enters the **next** `observation_signature` → nesting. |

**No guard exists.** `grep overseer-obs src/` returns exactly ONE hit — the construction site
(`mod.rs:1072`). There is no `starts_with("overseer-obs")` / meta-problem exclusion anywhere.
`sanitize_recalled` (`capabilities.rs:468-482`) only strips control chars + caps length; it does
**not** strip the `overseer-obs:` prefix, so the recalled composite survives verbatim and nests.

**Byte-level match to the observed artifact.** Because sort/dedup (`mod.rs:1070-1071`) run on the
*whole* dedup-key strings before the `|`-join, the nested `overseer-obs:…|…|workstream-gap` key is
one element that visually merges into the outer join. The standalone `workstream-gap` key AND the
`workstream-gap` embedded inside the nested composite both appear → the observed
`workstream-gap|workstream-gap` doubling. `×2` = `occurrences==2` = the emit threshold. This
reproduces the observed blob exactly.

**Second-order harm (beyond signature bloat).** The RecurringSignature meta-problem is
`ProblemKind::ProcessHealth` (`mod.rs:1357`). Its `dedup_key` (`overseer-obs:…`) matches no
in-cycle problem, so `orient` (`mod.rs:1211-1219`) does NOT merge it — it stands alone and reaches
`decide` → `ProblemKind::ProcessHealth => Intervention::LaunchRecipe` (`mod.rs:1429-1435`) with
`task_description = "recurring signature seen 2× in cognitive memory (overseer-obs:…)"`. This is
**cost-bearing** (`is_cost_bearing`, `mod.rs:1057-1062`): the system can spend LLM budget launching
a recipe about its own observation blob instead of remediating the real blocked goals.

**Fix boundary (investigation-only; not landed):** exclude recall-derived meta-problems at the
WRITE boundary. `observation_signature`/`write_back_observation` should skip problems whose
`dedup_key` starts with `overseer-obs:` (or, more generally, problems sourced from
`Signal::RecurringSignature`). This mirrors the existing READ-side posture where recalled text is
already treated as untrusted (`sanitize_recalled`). Excluding at the read boundary alone is
insufficient — the nesting happens on WRITE.

---

## 2. Two decoupled counter lanes (why `×2` is HONEST, not an artifact)

- **Lane A (episodic — this signature):** write-back episode → `parse_failure_signature` →
  `signal.rs:462-468`. Threshold **2**. Emits RecurringSignature. This is the `×2`.
- **Lane B (semantic — root cause):** `record_occurrence` `store_fact` (`mod.rs:1004-1043`) →
  `recall_occurrences` filter `signature == dedup_key` (`mod.rs:972-997`) → `analyze` counts
  `recurrence` by `cause_label` (`root_cause.rs:78-82`). Threshold **3**
  (`RECURRENCE_ESCALATION_THRESHOLD`, `root_cause.rs:33`).

The lanes never share a counter. `×2` is a faithful Lane-A re-observation over a static blocked
set, **not** a hash/counter/replay artifact. The write-back dedup gate is **in-memory only**
(`WhisperGate`, `guardrails.rs:290-296`: `HashMap`+`Vec`, no persistence;
`write_back_gate = WhisperGate::new(900,5)`, `mod.rs:299`). A daemon restart resets it, so the same
`observation_signature` is re-recorded → recall climbs to 2. The `2×` is real; the missing lever is
**storage-layer idempotency** (signature-keyed upsert or bounded retention), not the counter.

---

## 3. Non-convergence / recurrence dead-zone (CONFIRMED)

Each constituent of the composite has **no convergent closing action** at first/second recurrence:

| Signal | Problem kind | Decide arm | Closing action? |
|---|---|---|---|
| `goal:blocked:*` (`mod.rs:1336`) | `GoalHygiene` | `decide_blocked_goal` (`mod.rs:1603-1631`) | Only at `recurrence>=3` → `EscalateBlockedGoal`. `perpetual`+no-progress → blind `UnblockGoal` (re-blocks next cycle). Else → **`Report`** (no-op). |
| `workstream-gap` (`mod.rs:1371`) | `WorkstreamCoverage` | `mod.rs:1534-1543` → `FlagWorkstreamGaps` | `act_flag_workstream_gaps` (`mod.rs:884-948`) only **notifies** + commits a dedup gate. No launch, no issue, no transfer. Observe-and-flag. |
| `resource:engineer_spawn` (`mod.rs:1270`) | `ResourcePressure` | `mod.rs:1444-1446` → `Escalate{reason}` | Notify-level only; adds no capacity. |

**Dead-zone geometry:** `recurrence < 3` (Lane B) gets neither remediation nor escalation — it
either blindly re-`UnblockGoal`s (which re-blocks) or `Report`s. The `2×` sits above noise but below
the escalation threshold. The only arm with a `LaunchRecipe` edge is the RecurringSignature
meta-problem (§1), and it targets the meaningless self-signature — so the one convergent action is
aimed at the wrong thing. This is the "observe-and-flag without a closing action" + "recurrence dead
zone" pair from `PATTERNS.md`, re-confirmed against source.

**Missing rung:** a first-proven-recurrence (2×) remediation/escalation rung, and a
`WorkstreamCoverage` launch edge, are absent. Any remediation ledger MUST key on
`GapItem.signature` (as `act_flag_workstream_gaps` already does at `mod.rs:901`), not the bare
`workstream-gap` literal, or all gaps collapse into one issue (INV-GAP-KEY).

---

## 4. Integration points / connections

- Write→store→recall→signal→classify→write is a closed cycle within `overseer/`; the composite
  **never** flows through `stewardship/dedup.rs` (that is the separate GitHub-issue dedup path) —
  confirmed dead end, not chased.
- Lane B's `store_fact` is append-per-action (`mod.rs:1034`); the count lives in accrued facts.
  TRAP re-confirmed (RECONCILIATION_LEDGER): switching to
  `store_fact_with_caller_key(root_cause_signature)` pins `recall.len()==1` → `recurrence>=3`
  becomes dead code. The count must stay in fact content. D2 (Lane-B counter + its accrual gate) is
  a coupled pair — must ship atomically or it latches/over-escalates.

---

## 5. Design rationale observed

- Write-back is intentionally minimal/idempotent-within-window (`#2628` comments, `mod.rs:1064-1067`)
  but idempotency is **per-process**, so it is not durable across restarts — an intended-but-leaky
  guarantee.
- Recall text is treated as untrusted at the READ admission boundary (`sanitize_recalled` at
  `mod.rs:1359`, `:1082`). The gap is that "untrusted" was scoped to injection/length, not to the
  system's own recycled signatures — so the meta-problem is sanitized but not excluded.

---

## 6. Questions for verification phase

1. Confirm no runtime path filters `overseer-obs:`-prefixed problems before `write_back_observation`
   (static grep says none; verify no dynamic filter in `run_cycle`).
2. Confirm the RecurringSignature meta-problem's `LaunchRecipe` is actually admitted (not always
   held) by `gate()` under default autonomy/budget — determines whether §1's second-order harm is
   live or latent.
3. Add a regression test: feed a recalled episode whose `failure_signature` starts with
   `overseer-obs:` and assert it does NOT re-enter the next `observation_signature` (no such test
   exists in `tests_memory_recall.rs`).
4. Confirm D2's counter and accrual gate ship together (atomicity) before any escalation-threshold
   change.

**Verdict:** D1 (write-back feedback nesting) is a real, un-guarded defect at HEAD reproducing the
observed artifact byte-for-byte in structure. `×2` is an honest Lane-A re-observation, not an
artifact. The dead-zone/non-convergence is confirmed: the missing first-recurrence remediation rung
and the missing `WorkstreamCoverage` launch edge. Investigation-only — no change landed.
