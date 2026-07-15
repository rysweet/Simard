# Consolidated Findings — Recurring `goal:blocked` + `workstream-gap` Signature

**Investigation:** the overseer signature seen 2× in cognitive memory:
`overseer-obs:goal:blocked:…|…|workstream-gap|workstream-gap`
**Branch / HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `f9cefec1`
**Date:** 2026-07-15  **Status:** Complete — re-validated against current source through **ten** waves
(latest HEAD `f9cefec1`; through the ninth wave every investigation commit was **docs-only**, and the
tenth wave added **only a test** — `git diff --name-only 6b2bf5e1..HEAD -- src/overseer src/stewardship`
returns solely `src/overseer/tests_root_cause.rs`, the working tree is source-clean, and **no production
`.rs` changed**, so every source citation still holds and **no fix is merged**). Fifth-wave net-new findings
(incl. the `resource:engineer_spawn` membership drift) are folded into §11; sixth-wave net-new findings
(the end-to-end pipeline trace and the **D2→D1→D3 dependency-safe landing order**, plus a reconciliation
of the two waves' D-numbering) are folded into §12; seventh-wave net-new findings (the falsifiable
**H0–H8 hypothesis set**, the executed **per-hypothesis verification matrix**, and the **minimal
contained signature-path fix** with a zero-drift re-grounding) are folded into §13; eighth-wave net-new
findings (the exact open-loop seam, the per-token emitter map, the config-conditional WHY-gating hole,
the **no-new-plumbing D1 fix** with the refined **D2→D3→D1→gauges** landing order, and the merge-base
`dcf909c5` zero-drift re-grounding across committed *and* working-tree state) are folded into §14;
ninth-wave net-new findings (the two **decoupled recurrence lanes**, the **structural
unreachability** of the raise-priority rung for the composite self-observation, the two concrete
**over-aggregation harms** — churn-brittle detection and composite non-actionability — and the
`workstream-gap ↔ engineer_spawn` **no-causal-edge** reconfirmation, all re-verified against live
`src/` at HEAD `440e024c` with zero drift) are folded into §15. Tenth-wave net-new findings at
HEAD `f9cefec1` (the load-bearing **D0 reconciliation-seam root cause** — the completion gate is a
*conjunction* and its reconciler is doubly-conditional, so an issue-closed-without-linked-merged-PR
anchor never leaves `Blocked`; the **revised L0→L1→L2→L3 whole-loop remediation order**; and an
explicit **reconciliation of the two newest deep dives' `engineer_spawn` disagreement** — one calls
it a benign "false lead," the other a real coupled-`8`s admission-cap; both are correct at different
seams) are folded into **§16**. Three further tenth-wave parallel dives at the same HEAD `f9cefec1`
(the explicit **intended-signal-vs-recording-defect adjudication** — the `×2` is signal, the only
genuine recording concern is Lane-B durability, not the count; an empirical **21/21 lane-isolation
re-run**; the **F2 doc/impl gap-rung mismatch** and the **F5 correction** that the WHY ladder is
default-**on** at its second gate so only the shared `completion_evidence` wiring gates both it and D0;
and a full **361/0 per-hypothesis re-verification**) are folded into **§17**. This tenth wave is the
first that is **not docs-only**: it added a single **test** (`src/overseer/tests_root_cause.rs`, commit
`f9cefec1`) proving Lane-A `RecurringSignature` does **not** feed Lane-B recurrence — i.e. the 2↔3
dead-zone (D2) is now green-by-test. **No production `.rs` changed; no remediation landed.** A
**twelfth-wave** set of **three parallel deep dives** at HEAD `bbddd23a`
(only `tests_root_cause.rs` differs from the `5a85317b` pin — all non-test source byte-identical) is
folded into **§19**. The **architect/tertiary** dive
([`tertiary_architecture_TWO_LOOPS_AND_DEADZONE_HEAD_bbddd23a.md`](./tertiary_architecture_TWO_LOOPS_AND_DEADZONE_HEAD_bbddd23a.md))
hardens the D2 dead-zone diagnosis from "ACT rarely records occurrences" to a
**total structural latch** — `ActOutcome::Reported ∉ outcome_records_occurrence` (`wiring.rs:612-627`),
so a sub-threshold blocked goal accrues **exactly zero** Lane-B occurrences by construction and can
**never** reach the escalation threshold of 3; names the three non-closing loops (L1/L2/L3) and the
missing **"recurrence-2 closing rung"**; and reconfirms `resource:engineer_spawn` as benign drift while
flagging that `Escalated` **is** in the occurrence set (`wiring.rs:619`) — an inconsistency vs. L1's
`Reported`. The **primary** dive
([`primary_signature_assembly_emitter_and_2x_deepdive.md`](./primary_signature_assembly_emitter_and_2x_deepdive.md))
pins the complete end-to-end **emitter map** (assembly `mod.rs:1068-1073`; human "N×" string
`mod.rs:1359-1362`; marker embed `wiring.rs:1084` / parse `wiring.rs:976-986`,`:1025`; 2× count
`signal.rs:455-470`, threshold `signal.rs:362`) and the **four in-memory `WhisperGate` configs**
(`mod.rs:286-304`) — establishing that the write-back dedup window is **ephemeral with no persisted gate
state**, so a daemon restart re-opens the 900 s window and re-persists the same `[sig:…]` episode, the
precise mechanism that makes `occurrences == 2`. The **secondary** dive
([`secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a.md`](./secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a.md))
adds the **decisive structural proof** that the literal `…|workstream-gap|workstream-gap|…` /
nested `overseer-obs:…|overseer-obs:…` doubling is a **positive fingerprint of D1 self-observation
nesting and is impossible from true per-token duplication** (because `orient` merges same-`dedup_key`
signals and `keys.dedup()` collapses adjacent equals ⇒ each family key appears **at most once per
snapshot**), and classifies every token load-bearing vs. benign drift. **No production `.rs` changed;
no remediation landed.** A **thirteenth-wave** set of **three parallel deep dives** at HEAD `1de21e71`
(only `src/overseer/tests_root_cause.rs` differs from the twelfth-wave pin — all non-test source
byte-identical; memory-recall suite re-run green **32/0** this wave) is folded into **§20**: the
**primary** dive traces the self-ingestion loop **link-by-link end-to-end** (the 8-edge
`write-back → recall → count → classify → orient → re-wrap → gate → Recur` cycle) and shows the recall
path has **no self-authorship exclusion**; the **secondary** dive proves the `orient` merge branch is
**dead for the composite** (so a `RecurringSignature` **always** becomes a *standalone* `ProcessHealth`
meta-problem that re-enters `write_back_observation(&cycle.problems)` with **no write-boundary filter**),
re-affirming the `×2` as an honest signal and the over-aggregated composite + unguarded write-boundary as
the defect; and the **tertiary/architect** dive inventories **all five dedup/idempotency gates** (only
G1 sits on the self-feed edge and is defeated by signature mutation; G2 is dead for the composite ⇒ the
loop has **no effective idempotency boundary**), confirms **Lane-A ⟂ Lane-A is UNTESTED** (test gap on
the exact defect edge), and specifies the **minimal landing-order-safe fix** — a single-function
write-boundary self-provenance filter in `write_back_observation` (`mod.rs:534-563`) that drops
`overseer-obs:`-keyed recall-derived problems before `observation_signature`, restoring G1's dedup power
with **no cross-file plumbing** and order-independent defence-in-depth with the recall-side
`source_label` filter. **No production `.rs` changed; no remediation landed.**

This consolidates all parallel deep dives:
[`investigation_report.md`](./investigation_report.md) (primary/secondary root cause),
[`tertiary_architecture_design.md`](./tertiary_architecture_design.md) (systemic-fix
architecture),
[`blocked_transition_and_escalation_idempotency.md`](./blocked_transition_and_escalation_idempotency.md)
(signature provenance + escalation-idempotency deep dive),
[`secondary_dedup_recurrence_findings.md`](./secondary_dedup_recurrence_findings.md)
(cognitive-memory dedup path: real-loop-vs-artifact verdict + self-referential
write-back feedback),
[`DISCOVERIES.md`](./DISCOVERIES.md), [`PATTERNS.md`](./PATTERNS.md),
plus a **verification pass** ([`verification_results.md`](./verification_results.md))
that traced the exact composite-signature construction the question references, and
**two re-validation passes at current HEAD `dea65df8`**:
[`secondary_dedup_recurrence_VALIDATION_HEAD.md`](./secondary_dedup_recurrence_VALIDATION_HEAD.md)
(the **two-counter-lane** framing + a critical caveat that the naïve one-line counter
fix is a trap) and
[`tertiary_architecture_VALIDATION_HEAD.md`](./tertiary_architecture_VALIDATION_HEAD.md)
(the **three-defect D1/D2/D3** geometry, the count-in-content fix, and dependency-correct
landing order),
[`tertiary_gap_routing_and_remediation_rung.md`](./tertiary_gap_routing_and_remediation_rung.md)
(the **dual-path gap quarantine** + `INV-GAP-KEY` + the already-built stewardship routing
seam), and
[`RECONCILIATION_LEDGER.md`](./RECONCILIATION_LEDGER.md) (independent knowledge-archaeology
pass: every load-bearing citation re-verified, one fix-recommendation contradiction
surfaced and resolved).

A **fourth re-validation wave at HEAD `85b9398a`** independently re-derived and extended the
above (all citations re-verified against live `src/`, no doc-to-doc trust):
[`SYNTHESIS.md`](./SYNTHESIS.md) (the five-output executive synthesis + JSON),
[`primary_signature_provenance_and_idempotency.md`](./primary_signature_provenance_and_idempotency.md)
and [`primary_signature_provenance_dedup_verdict.md`](./primary_signature_provenance_dedup_verdict.md)
(per-token provenance + the honest-count/vacuous-event verdict),
[`secondary_common_root_cause_HEAD_85b9398a.md`](./secondary_common_root_cause_HEAD_85b9398a.md)
(one-lever/four-symptom common root cause),
[`tertiary_pipeline_and_store_boundary.md`](./tertiary_pipeline_and_store_boundary.md) and
[`tertiary_pipeline_idempotency_RERUN_85b9398a.md`](./tertiary_pipeline_idempotency_RERUN_85b9398a.md)
(the **cognitive-memory vs stewardship-store boundary** + end-to-end pipeline with idempotency
marks), and two knowledge-archaeology re-checks
[`specialist_crosscheck_HEAD_85b9398a.md`](./specialist_crosscheck_HEAD_85b9398a.md) /
[`specialist_revalidation_HEAD_85b9398a.md`](./specialist_revalidation_HEAD_85b9398a.md)
(every load-bearing line re-verified exact; the one wrong *remedy* — §6.2b — confirmed already
corrected to a count-in-content upsert). Net-new items from this wave are folded into §10.

A **fifth re-validation wave at HEAD `388e6c29`** re-grounded every load-bearing citation to
live `src/` once more (`git diff --name-only 6e3113bc..HEAD -- '*.rs'` is **empty** — all five
investigation commits are docs-only, zero `.rs` changes) and analysed the one genuinely new
element, the `resource:engineer_spawn` token in the later snapshot:
[`secondary_token_provenance_membership_delta_HEAD_388e6c29.md`](./secondary_token_provenance_membership_delta_HEAD_388e6c29.md)
(per-token provenance + the two-snapshot membership-delta table),
[`specialist_revalidation_drift_HEAD_388e6c29.md`](./specialist_revalidation_drift_HEAD_388e6c29.md)
(drift assessment: prior findings re-validated, `engineer_spawn` is **benign membership drift,
not code drift**), and
[`tertiary_fix_landing_and_regression_safety_HEAD_388e6c29.md`](./tertiary_fix_landing_and_regression_safety_HEAD_388e6c29.md)
(minimal-fix landing location + a per-test no-regression argument). Net-new items from this
wave are folded into §11.

A **sixth re-validation wave at HEAD `0289572e`** (this consolidation) re-ground every prior verdict
to current source once more — `git diff --name-only 85b9398a..HEAD` touches **only** `ai_working/`,
`6e3113bc..HEAD -- '*.rs'` is **empty**, baseline tests green (13 passed) — across four parallel
deep dives:
[`specialist_regrounding_HEAD_0289572e.md`](./specialist_regrounding_HEAD_0289572e.md)
(knowledge-archaeology re-grounding: every load-bearing citation re-verified exact at HEAD,
`resource:engineer_spawn` re-confirmed benign membership drift),
[`tertiary_architecture_pipeline_and_landing_order_HEAD_0289572e.md`](./tertiary_architecture_pipeline_and_landing_order_HEAD_0289572e.md)
(the full OODA-tick pipeline trace showing the self-observation feedback runaway concretely, and a
**D2→D1→D3 dependency-safe landing order**),
[`primary_signature_provenance_REVALIDATION_HEAD_0289572e.md`](./primary_signature_provenance_REVALIDATION_HEAD_0289572e.md)
(the honest-2×-cross-window verdict + an **empirical fix-landing grep** proving D1/D2/D3 all unmerged,
and the Lane-A/Lane-B remediation-placement rule), and
[`secondary_token_classification_and_root_cause_HEAD_0289572e.md`](./secondary_token_classification_and_root_cause_HEAD_0289572e.md)
(the **per-goal stall-class map** — 3 of 4 blocks non-genuine, one common unwired-WHY-rung root cause
— plus the gap↔blocked oscillation and per-gap-identity-loss findings), corroborated by a second
architect drift-check
[`tertiary_architecture_DRIFT_AND_LANDING_HEAD_0289572e.md`](./tertiary_architecture_DRIFT_AND_LANDING_HEAD_0289572e.md)
(independent per-citation drift table + struct-level absence proofs). Net-new items — including a
reconciliation of the two waves' divergent D1/D2/D3 labels and one stale citation flagged — are
folded into §12.

A **seventh re-validation wave at HEAD `5a85317b`** re-cast the six-wave verdicts as an explicit,
falsifiable **hypothesis set** and executed a **per-hypothesis verification pass** — every citation
re-grounded to live `src/` once more (`git diff --name-only 6e3113bc..HEAD -- '*.rs'` still **empty**,
docs-only):
[`HYPOTHESES.md`](./HYPOTHESES.md) (the H0–H8 map: null-hypothesis H0 **rejected**, H1 the honest-`2×`
cause, H2/H3/H4 the three root defects, H5 the dead zone, H6 the compounding non-idempotency, H7/H8
the one-problem-in-N-views unifiers — each stated as a confirm/falsify test) and
[`verification_results_ALL_HYPOTHESES.md`](./verification_results_ALL_HYPOTHESES.md) (a practical test
executed for **every** H0–H8: full overseer suite **360 passed / 0 failed**, plus **17 targeted
discriminating tests** and an end-to-end no-bridge probe, all green), synthesized in
[`FINAL_SYNTHESIS.md`](./FINAL_SYNTHESIS.md) (the five-output executive synthesis + JSON at current
HEAD), and complemented by a fourth architect deep dive
[`tertiary_architecture_REGROUND_HEAD_5a85317b.md`](./tertiary_architecture_REGROUND_HEAD_5a85317b.md)
(the **minimal contained signature-path fix** — a ~4-line `dedup_key.starts_with("overseer-obs:")`
write-back filter — plus a zero-line-drift re-grounding of every prior citation at HEAD). Net-new
items from this wave — the falsifiable hypothesis framing, the complete per-hypothesis verdict matrix,
and the minimal contained D1 fix with its exactness proof — are folded into §13.

An **eighth re-validation wave at HEAD `b9f99879`** re-grounded the whole pipeline once more via three
parallel deep dives — every load-bearing citation independently re-read against live `src/` (no
doc-to-doc trust), and drift re-checked: `git diff 6b2bf5e1..b9f99879 -- src/overseer src/stewardship`
and `… -- src/ooda_loop` are **both empty** (every commit since the last code change `6b2bf5e1` is
`docs(investigation)` — zero source drift, so all citations are byte-identical):
[`primary_signature_provenance_HEAD_b9f99879.md`](./primary_signature_provenance_HEAD_b9f99879.md)
(full Observe→Orient→Act→Store→Recall provenance trace + the **2× write-back-gate verdict** — the 900 s
`WhisperGate` is a same-window de-dup, **not** a loop breaker; loop **open at HEAD** — pinpointing the
precise open seam at `recall_episodic` with **no source-label self-exclusion**, `wiring.rs:1013-1031`),
[`secondary_idempotency_and_gap_spawn_cycle_HEAD_b9f99879.md`](./secondary_idempotency_and_gap_spawn_cycle_HEAD_b9f99879.md)
(the **two-orthogonal-dedup-namespaces** table, the membership-fingerprint idempotency proof, and the
headline correction that **`workstream-gap → resource:engineer_spawn` is NOT a causal orchestration
cycle** — no code edge; co-occurrence = under-resourced state; split defect/steady-state verdict), and
[`tertiary_architecture_REGROUND_HEAD_b9f99879.md`](./tertiary_architecture_REGROUND_HEAD_b9f99879.md)
(the zero-drift per-citation ledger, the consolidated **Observe→Orient→Act pipeline diagram**, and the
architectural root cause of D1: provenance is **stamped on write** but **erased on read**). Net-new
items from this wave are folded into §14.

Every claim below is re-grounded to a current line in `src/overseer/` (re-verified at
HEAD `b9f99879`; all prior root-cause citations still hold exact — the one superseded item is
a *remedy*, §6.2b, not an analysis).

---

## 0. Direct answer — what the composite signature *is* and why it recurs

The signature in the question is **not** a raw memory key; it is the overseer's
own **observation write-back signature**, built by `observation_signature`
(`overseer/mod.rs:1068-1073`):

```rust
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}
```

- Each `goal:blocked:<slug>-<hash>` and each `workstream-gap` token is a single
  **problem `dedup_key`**. The overseer collects the current cycle's problems,
  **sorts + dedups** their keys, joins with `|`, and prefixes `overseer-obs:`.
- **"Seen 2×"** therefore means: *the identical set of open problems produced the
  identical composite observation across two write-back passes.* The write-back
  gate de-dups an unchanged observation (same signature ⇒ same episode identity),
  and the recurrence surfaces because **the underlying problem set never changed
  between passes** — the loops observed the same blocked goals and the same
  coverage gaps again.
- The repeated bare `workstream-gap|workstream-gap` fragments in the recall are
  multiple gap problems / multiple episodes concatenated in the recall stream,
  each a `WorkstreamCoverage` problem carrying the family `dedup_key` — not a
  `dedup()` bug (that only collapses *adjacent* equal keys within one signature).
- The repeated **`overseer-obs:` prefixes nested *inside* the composite** are
  the overseer observing **its own bookkeeping** (recall-derived
  `RecurringSignature` folded back into the next signature) — see §0a for the
  verified feedback path.

**Verdict (high confidence): the `×2` is a REAL re-observation loop, not a
dedup / storage / replay artifact.** The within-window write-back gate provably
suppresses duplicates (test `write_back_is_deduplicated_within_window`,
`tests_memory_recall.rs:797-817`); the two counted episodes are two *legitimate*
write-back passes of a *static* problem set across two 15-min windows.

### 0a. Verified — the *nested* `overseer-obs:` tokens are self-observation feedback

The question's signature contains `overseer-obs:` fragments **nested inside**
the composite (repeated between `goal:blocked:*` runs). That is not recall
noise — it is a real feedback path, verified in source:

1. Recall of ≥2 episodes sharing a `failure_signature` raises
   `Signal::RecurringSignature` (`signal.rs:455-469`), which Orient admits as a
   distinct `ProcessHealth` problem whose `dedup_key` is
   `sanitize_recalled(signature)` — i.e. the **prior** `overseer-obs:…` string
   (`mod.rs:1353-1359`).
2. Because that key differs from the base `goal:blocked:*` / `workstream-gap`
   keys, Orient's same-key merge (`mod.rs:1210-1221`) does **not** fold it away —
   it is `push`ed as its own problem (`mod.rs:1222`).
3. `write_back_observation(&cycle.problems)` (`wiring.rs:301`) then writes back
   **all** cycle problems — including that recall-derived one — so the **next**
   `observation_signature` embeds the prior `overseer-obs:…` token, producing the
   nested repetition seen in the question.

**The overseer is recalling and re-observing its own bookkeeping.** It is
**bounded** (throttled by the 15-min write-back gate, the recall limit, Orient's
same-key merge, and the `×2` threshold), so it stabilizes into a small family of
nested signatures each "seen ~2×" — consistent with the observed data — but it is
a real design smell: `sanitize_recalled` at this exact admission boundary
(`mod.rs:1359`) shows the authors already treat recalled signatures as untrusted,
yet still write them back into future signatures.

**So the recurring signature is a faithful fingerprint of a static problem set.**
The real question is not "why does the fingerprint repeat" but "why does the
problem set never change." Answer: **two observe-and-flag loops that never close**
(§1–§2), sitting in a **recurrence dead zone** (§3).

---

## 1. Root cause A — blocked goals that are parked but never resolved

- The **no-progress breaker** fires after `NO_PROGRESS_BREAKER_THRESHOLD = 3`
  consecutive no-action OODA cycles (`goal_curation/no_progress_breaker.rs:59`).
- Historically it parked with a **bare** reason `{PREFIX}{count}{SUFFIX}` —
  *"…consecutive no-action cycles; needs human review"*
  (`no_progress_breaker.rs:75`) — stating *what* but not *why*.
- The canonical incident: seven `kgpacks-rs` goals parked as "no progress" when
  the work was **already done** (issues CLOSED, PRs MERGED) — the safeguard
  **misread *done* as *stuck*** (`no_progress_why.rs` header).

The corrective vocabulary exists: `NoProgressClass` +
`resolution_for_why` map each stall cause to a self-resolving rung
(`no_progress_breaker.rs:384-417`):

| Class | Correct resolution |
|---|---|
| `AlreadyComplete` | auto-complete (`MarkDone`) |
| `Obsolete` | drop |
| `MissingPrecondition` | self-heal (clone) + retry (`Heal`) |
| `UpstreamDependency` | defer (`Paused`), auto-clears on landing |
| `UnclearCriteria` | guided engineer → human |
| `GenuinelyStuck` | guided engineer → human |

**Root cause:** only the last two classes should ever reach a human. When the
WHY reasoner is unwired or misclassifies, all six collapse to the bare park, the
ladder never runs, and the goal re-parks every window — the recurring
`goal:blocked` population.

### 1a. Verified refinement (tertiary) — the WHY reasoner is *double-gated*

Tracing `ooda_loop/cycle.rs:582-702` shows the ladder is **opt-in that fails open
to bare-park**, behind two silent switches:

- **Gate A** — `memories.completion_evidence.is_some()`. If `None` (no
  `EvidenceSource` wired, e.g. partial daemon boot or a test), the **entire**
  breaker block collapses to `Vec::new()` — no classification, no ladder, no
  re-investigation of existing bare parks.
- **Gate B** — `no_progress_investigation_enabled()` (`no_progress.rs:199-207`,
  default `true`; `SIMARD_NO_PROGRESS_INVESTIGATE=off` downgrades to the base
  ladder that authors the legacy bare block).

There is **no invariant** that a `Blocked` reason ever carries a `NoProgressClass`.

---

## 2. Root cause B — `workstream-gap` is a coverage gap that is only *flagged*

- **Meaning:** a **backlog-coverage gap**, *not* zero-workstream decomposition.
  `sensor.rs::detect_workstream_gaps` flags: a p1/p2 **active, non-blocked** goal
  with no assignee/PR/branch/session (`GoalUncovered`); a high-signal open issue
  with no PR (`IssueUncovered`); a live anomaly with no fix in flight
  (`AnomalyUnaddressed`). Blocked goals are **skipped** here (routed via
  `goal_health` instead — no double-notify).
- Decomposition producing `<2` sub-goals is a **separate, loud** path
  (`decompose.rs`, `MIN_SUBGOALS = 2`) that leaves the board untouched — it does
  **not** emit a `workstream-gap`.
- **Flow:** Observe → one consolidated `Signal::WorkstreamGap` → Orient to a
  `WorkstreamCoverage` problem → Act = `FlagWorkstreamGaps` →
  `act_flag_workstream_gaps` (`mod.rs:884-948`), which **only notifies the
  operator**. It files no issue and **launches no workstream**.

`WorkstreamCoverage` is the **only** High-priority Decide arm with **no edge into
`launch.rs`** — its three siblings (`ProcessHealth`, `CrossCutting`,
`StepFailure`) all reach `LaunchRecipe` (`mod.rs:1429,1436,1565`). The launcher,
`RecipeBrief`, per-cycle cap and dedup key already exist; the coverage arm simply
doesn't use them.

---

## 3. Why "2×" specifically — two distinct thresholds + a non-idempotent counter

The question's **"seen 2×"** is exact, not approximate: it is
`RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`). On a later tick, recall of
**≥2 write-back episodes** sharing a `failure_signature` fires
`Signal::RecurringSignature` (`signal.rs:455-469`). Because each `overseer-obs:`
write-back is itself window-gated (`write_back_gate = WhisperGate::new(900, 5)`,
≤1 write / 15-min window), **2× means the identical problem set was observed
across ≥2 distinct 15-min windows** — i.e. the goals are *genuinely, persistently*
blocked, not a within-tick duplicate.

But detecting recurrence and *acting* on it are separated by a second, higher bar:

- **Intra-window dedup:** `gap_gate = WhisperGate::new(900, 200)` (`mod.rs:304`) —
  a 15-min window that suppresses a repeat gap but **forgets across windows**, so
  cross-window recurrence never accrues for coverage gaps at all.
- **Cross-window escalation (blocked goals only):** root-cause occurrences are
  recalled from cognitive memory (`recall_occurrences`, `mod.rs:1147`) and the
  root cause escalates **only at** `RECURRENCE_ESCALATION_THRESHOLD = 3`
  (`root_cause.rs:33`; `mod.rs:1613`).

**So 2× fires the *recurring-signature signal* but sits one below the
*escalation* bar of 3** — the dead zone — and coverage gaps have *no* cross-window
recurrence tracking and *no* remediation rung at all. The signature is real,
deduped, and re-observed forever without being resolved or escalated.

### 3a. New defect — the recurrence counter is a non-idempotent ratchet
The `recurrence` count that drives escalation is **not** a streak; it is a
monotonic *lifetime write-count*. `record_occurrence` (`mod.rs:1034`) persists via
**plain `store_fact`** — an unconditional `CREATE` with **no dedup**
(`library_adapter.rs:657`; `mod.rs:340-353` confirms only
`store_fact_with_caller_key` dedups). One occurrence node is appended per
effective (non-suppressed) act per window and never pruned. Consequences:

1. `recurrence` only ever **grows** — it conflates "windows we ever touched this"
   with "genuine recurrence."
2. Once `recurrence ≥ 3`, the goal **latches** onto `EscalateBlockedGoal` on every
   future window forever, and can never fall back to self-heal (`UnblockGoal`)
   even after the underlying condition is fixed.
3. The observed `2×` is therefore **both** a real loop (window-gated write-back
   proves ≥2 distinct windows) **and** a counter whose *magnitude* is partly a
   write artifact.

**Fix (corrected — see §3c caveat):** the naïve
`store_fact_with_caller_key(root_cause_signature(problem, primary), …)` — first
proposed here and in §6.2b — is a **trap**: `DedupMode::CallerKey` keeps exactly
one live fact per key, so `recall_occurrences().len()` would stick at **1** and
escalation (`recurrence >= 3`) becomes **dead code**
(`secondary_dedup_recurrence_VALIDATION_HEAD.md §4`). The correct fix carries the
count **in the fact content**: a caller-key upsert whose `content` holds an
incremented `occurrence_count` + `first_seen`/`last_seen`, with escalation reading
that field instead of `recall.len()` (full design in §6.2b, corrected). (Note:
escalation is idempotent at the **action** level — a repeat within the 15-min
`blocked_goal_gate` window returns `GoalHealthSuppressed`, `mod.rs:810-878` — so
the ratchet is a *decision*-level defect only.)

### 3b. Verified — observation *episodes* also lack storage-layer idempotency
Distinct from §3a (root-cause occurrence *facts*), the **observation episodes**
that carry the `overseer-obs:…` signature are written by `record_observation`
(`wiring.rs:1076-1091`) via an **unconditional** `store_episode` — **no
query-before-store, no signature upsert** (`library_adapter.rs:609-628`). This is
asymmetric with the procedural path (#2298) which made `store_procedure` an
idempotent upsert keyed on `name`; observation episodes have no equivalent.
Consequence: the within-window `write_back_gate` correctly suppresses duplicates
*inside* a 15-min window (proven by `tests_memory_recall.rs:797-817`), but once
the window expires — **or the daemon restarts** (the gate's `last_delivered` map
is in-memory/per-process, `guardrails.rs:294`) — the same static problem set is
written **again as a new episode** with the **same** signature. So a long-lived
unresolved problem accumulates unbounded same-signature nodes, bounded only by the
recall `LIMIT`/consolidation. **Verdict (secondary deep dive):** the `×2` is a
*faithful* recurrence count of a genuinely re-observed static problem set — **not**
a dedup/storage/replay bug — but the storage layer offers no cross-restart
protection and no episode-level idempotency.

### 3c. Re-validated framing — TWO counter lanes, THREE defects, ONE latch
The two re-validation passes at HEAD `dea65df8` sharpen §3a/§3b into the load-bearing
geometry. The `×2` and the escalation counter live on **two decoupled storage lanes**,
and the whole symptom is **three independent defects** that merely co-occur in one string:

- **Lane A — observation episodes** (drives the visible `×2`): written by
  `record_observation` → `store_episode` (unconditional), keyed on the composite
  `overseer-obs:…` signature, incremented **once per 900 s window**, counted by
  `RecurringSignature.occurrences` (threshold **2**, `signal.rs:463`). *This is the
  number in the question.*
- **Lane B — root-cause occurrences** (drives escalation): written by
  `record_occurrence` → `store_fact` (unconditional, `mod.rs:1034`), keyed on
  `occurrence_concept(dedup_key)`, incremented **once per ACT that touches the cause**,
  counted by `RootCause.recurrence = recall.len()` (threshold **3**, `mod.rs:1613`).

**The lanes are decoupled** — the operator-visible `×2` (Lane A) says *nothing* about
whether Lane B reached 3. So the "dead zone at 2" is really a **cross-lane visibility
gap**, and Lane B has two opposite failure modes depending only on whether the blocked
goal's ACT path is reached: if the WHY double-gate keeps it **shut**, Lane B never
increments → never escalates (today's exact symptom); if it is **open**, Lane B ratchets
monotonically → **over-escalates and latches forever**.

Mapped to three seams (`tertiary_architecture_VALIDATION_HEAD.md §1`):

| Defect | Seam | Symptom in the signature |
|---|---|---|
| **D1** emission hygiene | `write_back` re-emits recall-derived `RecurringSignature` | the nested `overseer-obs:…\|overseer-obs:…` runs |
| **D2** escalation counter + gate | Lane B append-only ratchet **behind** the WHY double-gate | blocked goals never escalate *or* over-escalate |
| **D3** closing edge | `WorkstreamCoverage` has no `launch.rs` edge; `gap_gate` has no cross-window ledger | the `workstream-gap\|workstream-gap` tail, forever |

**The latch:** D2's counter and D2's accrual gate (the WHY double-gate) form a coupled
pair — fixing *either alone changes nothing observable*. The count-in-content counter
(§6.2b) and closing the WHY double-gate (§6.3) **must ship together**; D1 (§6.5) and D3
(§6.1) are independently shippable. Dependency-correct landing order in §6.6.

---

## 4. The two signatures are one problem in two views

An under-resourced important goal **oscillates**: `workstream-gap` (GoalUncovered)
while active with no workstream → once the breaker parks it, it leaves the
gap-scan and reappears as `goal:blocked`. That is why personas, the coverage
audit, the coin harness, and kgpacks appear in **both** recurring families
together — and why they co-occur inside the same `overseer-obs:…` composite.

---

## 5. Prioritized unblocking actions (per goal)

| P | Goal(s) | Root class | Closing action (seam) |
|:--:|---|---|---|
| **P0** | *WHY-reasoner wiring itself* | infra defect (§1a) | Close both silent gates + assert INV-WHY (§6.3). Every other blocked-goal row depends on the ladder actually running. |
| **P0** | kgpacks-rs → full parity; issues #12/#17/#18/#23/#25 | `AlreadyComplete` / `MissingPrecondition` | Run outcome-verify/done-gate against live artifacts (`goal_curation/outcome_verify.rs`, `completion_gate.rs`); deterministic reasoner (`no_progress.rs:931-1010`) certifies `AlreadyComplete` → `MarkDone`. Any not-complete → `MissingPrecondition` → `Heal` (clone via `repo_resolver.rs`) + retry. **Never reaches a human.** |
| **P1** | simard-identity personas (atelier, bursar, cartographer, concierge, gastronome) | `GoalUncovered` gap | Split the umbrella into 5 verifiable sub-goals (`decompose_goal`, 2..=6). At 2× recurrence auto-`LaunchRecipe` one bounded workstream per persona via `launch.rs`, gated by `max_launches_per_cycle` + board dedup. |
| **P1** | Build local coin benchmark harness | `MissingPrecondition` / `UpstreamDependency` | Establish harness precondition (data + runner, `src/coin_gym/`) → `Heal`+retry; if on an unlanded upstream → `Defer { blocking_ref }` (auto-clears). Do not human-park. |
| **P2** | Audit Simard test coverage → 70% | `UnclearCriteria` | Reformulate the done-criterion into a machine-checkable artifact (e.g. `cargo llvm-cov` line-coverage ≥70% committed to CI). Route once via `SpawnEngineer` to *set the metric*, not to do coverage work blindly. Escalate only after guided retry is exhausted. |

**Sequencing:** land P0 wiring **before** any bulk `unblock-all`, so bare parks
clear *with their real WHY attached* and route down the ladder — never blindly
re-unblocked (the operator's rejected antipattern, `mod.rs:1588`).

### 5a. Cluster classification (per success-criterion taxonomy)

Mapping each blocked-goal cluster to the requested taxonomy — **false-park (a)**,
**missing-perpetual-tag (b)**, **starvation (c)**, or **genuine dependency block**:

| Cluster | Class | Rationale |
|---|---|---|
| kgpacks-rs parity + #12/#17/#18/#23/#25 | **(a) false-park** | `AlreadyComplete`/`MissingPrecondition` — work done (issues CLOSED/PRs MERGED) or auto-healable, wrongly parked as "stuck." The canonical incident. |
| Audit Simard test coverage → 70% | **(b) missing-perpetual-tag** (via `UnclearCriteria`) | Ongoing/uncheckable done-criterion the done-gate can never certify → idles → re-parks every window. Needs a machine-checkable metric (and, if truly ongoing, a perpetual tag) so it stops being read as a one-shot stall. |
| simard-identity personas (atelier/bursar/cartographer/concierge/gastronome) | **(c) starvation** | `GoalUncovered` — p1/p2 with no assignee/workstream; under-resourced, oscillates gap↔blocked. |
| Build local coin benchmark harness | **genuine dependency block** (+ `MissingPrecondition`) | Blocked on an absent precondition / unlanded upstream (data + runner); correct resolution is `Heal`+retry or `Defer` that auto-clears — the one cluster where "blocked" is legitimately dependency-driven. |

**Cross-cutting:** all four funnel through **one shared mechanism** (bare
no-progress park with no WHY token) remediable at **one lever** (WHY-reasoner
wiring), with the non-idempotent recurrence counter (§3a) as an independent
compounding defect.

---

## 6. Systemic fix — make persistent signals converge

### 6.1 Close the workstream-gap loop (highest impact, D3)
`WorkstreamCoverage` is the **only** High-priority `ProblemKind` quarantined from
**both** closing seams the codebase already owns — a **dual-path** hole
(`tertiary_gap_routing_and_remediation_rung.md §1`): the read-only observer routes it
to `Intervention::Report` (`observer.rs:120`, never files) and the acting overseer to
notify-only `FlagWorkstreamGaps` (`mod.rs:1543`, never launches). Every *other* High
finding converges through `FileIssue` (M1) or `LaunchRecipe`/`VerifyAndMergePr` (M2).
The recurrence's routing origin is therefore a **Decide-table routing hole**, not a
detection bug — and both arms must be fixed (M1's `Report` fall-through will silently
swallow gaps the moment gap survey is added to the observer).

Give `WorkstreamCoverage` a **recurrence-aware three-rung ladder** (rewrite the Decide
arm at `mod.rs:1534-1543`): **1× → Notify** (unchanged); **≥2× → Remediate** =
`LaunchRecipe` via the existing `launch.rs` seam (or file via the already-built,
currently-dangling `stewardship::route_failure` seam, `routing.rs:39-52`, which
explicitly anticipates overseer gap briefs), honoring the launch cap + board dedup key;
**≥3× or launch-unsafe → Escalate once** with history. Classify the new remediation at
the same `RiskClass` `LaunchRecipe` carries (`guardrails.rs:60`) so the autonomy/budget
gate governs it.

> **INV-GAP-KEY** (`tertiary_gap_routing_and_remediation_rung.md §2`): the recurrence
> ledger, launch dedup key, and any per-gap issue MUST key on **`GapItem.signature`**
> (`goal:<id>`/`issue:<repo>#<n>`/`anomaly:<slug>`, `signal.rs:135-138`) — **never** the
> bare `problem.dedup_key == "workstream-gap"` (`mod.rs:1371`), which is a fixed constant
> that erases per-gap identity. The consolidated `WorkstreamGap` signal must be **fanned
> back out** to per-gap identities at the remediation seam (the inverse of the emission
> consolidation at `signal.rs:475-478`). Record each fresh gap as a `PriorOccurrence`
> keyed on `GapItem.signature` at the `commit` site (`mod.rs:931-934`) so gaps gain the
> cross-window memory they currently lack. **Trap:** naively wiring the observer's
> `FileIssue` arm folds *every* distinct gap into ONE issue (stewardship dedups on
> `failure_kind = dedup_key`, `observer.rs:133`) — under-reporting, not remediation.

### 6.2 Unify recurrence tracking; gap threshold = 2
Track gap signatures in cognitive memory like root-cause occurrences and apply
one "seen N× → remediate/escalate" policy. Use **2** for gaps (a coverage gap has
no benign transient explanation) vs **3** for blocked-goal causes.

### 6.2b De-ratchet the recurrence counter — count-in-content (idempotency defect)
**Do NOT** replace `store_fact` (`mod.rs:1034`) with a bare
`store_fact_with_caller_key(root_cause_signature(problem, primary), …)`. The
re-validation pass proved this is a **trap**
(`secondary_dedup_recurrence_VALIDATION_HEAD.md §4`): `DedupMode::CallerKey`
supersedes to **exactly one live fact per key**, and because
`root_cause_signature` is stable for a repeating cause, `recall_occurrences().len()`
would collapse to **1** permanently — making the `recurrence >= 3` escalation rung
(`mod.rs:1613`) **dead code**. The two goals (*stop the ratchet* **and** *still cross
3*) reconcile only by carrying the count **in the fact content**:
- **Write** (`record_occurrence`): caller-key upsert keyed on
  `root_cause_signature(entry.key, primary)`; on hit, deserialize,
  `occurrence_count += 1`, refresh `last_seen`, re-store (supersede). One live fact
  per cause, count inside. Add a `last_seen`/`distinct_windows` guard mirroring the
  900 s gate so a flapping daemon can't inflate the count within one window.
- **Read** (`RootCause.recurrence`): read `occurrence_count` from the single live
  fact instead of `recall.len()`.
This removes the lifetime write-count inflation *and* lets an already-escalated goal
fall back to self-heal once its cause clears — closing the "escalation latches on
forever" failure mode (§3a) without making escalation unreachable.

### 6.3 Guarantee the WHY reasoner (INV-WHY)
Make Gate A/B failures **loud, not open**: a daemon with
`completion_evidence = None` must escalate ("safeguard DISABLED"), not silently
`Vec::new()`; with the flag `off`, the base ladder must still stamp a
`GenuinelyStuck` WHY token so **no path authors a WHY-less block**. Run
`reinvestigate_bare_blocked_goals` on an **independent** cadence so the installed
base of bare parks gets a retroactive WHY. Pin the invariant with a CI assertion:

> **INV-WHY:** for any `Blocked(reason)`, `is_bare_no_progress_block(reason)` is
> `false` within one OODA cycle ⇒ `decide_blocked_goal` always gets
> `problem.why.is_some()` ⇒ `GoalHygiene` converges via `resolution_for_why`
> rather than the `Report` fall-through (`mod.rs:1630`).

### 6.4 Convergence observability (the single number)
Add a **persistent-unremediated** gauge (extend `activity.rs:66-68`, surfaced via
`wiring.rs`): count gap signatures with recalled recurrence ≥2 that produced no
launch/escalation this window, plus `is_bare_no_progress_block == true` (INV-WHY
violations). Both must trend to **zero** once §6.1–§6.3 land; either rising is the
leading indicator that a signature re-entered the dead zone.

### 6.5 Stop the overseer observing its own bookkeeping (§3b)
Exclude recall-derived `RecurringSignature` problems (kind `ProcessHealth`,
`dedup_key` prefixed `overseer-obs:`) from the `write_back_observation` set
(`wiring.rs:301`), so future observation signatures stop nesting prior
`overseer-obs:` tokens. This removes the self-referential signature pollution
while preserving the genuine recurrence *signal* (which still drives priority in
`orient`). Optionally, give observation episodes the same signature-keyed
idempotent upsert as procedures (#2298) — or a bounded retention/consolidation —
so a long-unresolved problem set does not accumulate unbounded same-signature
episode nodes across windows/restarts.

### 6.6 Dependency-correct landing order (the latch, §3c)
The fixes are **not** independent choices from a menu — they map 1:1 to the three
defects (D1/D2/D3) and one is a coupled pair:

1. **§6.2b counter + §6.3 WHY-gate, shipped together (D2 latch).** Fixing either
   alone changes nothing observable: counter-only leaves the gate shut (count stays 0,
   `×2` persists) or open (over-escalates); gate-only revives the append-only ratchet.
   *Highest priority, must be atomic* — it unblocks every `goal:blocked:*` row.
2. **§6.1 closing rung (D3).** Independently shippable; converges the `workstream-gap`
   family (simard-identity personas + the `workstream-gap|workstream-gap` tail).
3. **§6.5 write-back filter (D1).** Independently shippable one-liner; removes the
   nested `overseer-obs:` shape.
4. **§6.4 convergence gauges.** Proves the fix holds and guards regression; also cover
   the Lane-A cross-restart episode-inflation residual (dedup episodes on
   `(signature, window)` if restart-flapping is confirmed as a `×2` source).

---

## 7. Reusable anti-patterns (for the pattern library)

1. **Observe-and-flag without a closing action** — a loop detects, notifies,
   dedups re-notification, but never removes the condition. *Fix:* every
   persistent-signal loop needs a convergence rung.
2. **Recurrence dead zone** — a signal below the escalation threshold and above
   one-off noise gets neither remediation nor escalation. *Fix:* track recurrence
   uniformly; remediate/escalate at the first proven recurrence for signals with
   no benign explanation.
3. **Park instead of classify-then-route** — a stall that isn't first classified
   (`NoProgressClass`) degrades to a permanent bare park. *Fix:* classify → route
   the self-resolving ladder; only genuinely-unclear/stuck reach a human.
4. **Two signatures, one root problem** — under-resourced work oscillates between
   a coverage signature (active) and a blocked signature (idle); treat as one
   convergence problem, not two bugs.
5. **Self-observation feedback** — a monitoring loop writes its own recall-derived
   observations back into memory, then re-observes them, nesting its bookkeeping
   inside future signatures. *Fix:* never write back recall-derived meta-problems;
   treat recalled signatures as untrusted at the write boundary, not just the read
   boundary.
6. **Missing storage-layer idempotency** — a write-back is gated only by an
   in-memory, per-process window (no cross-window/cross-restart upsert), so a
   long-lived unresolved problem accumulates unbounded same-signature nodes. *Fix:*
   signature-keyed idempotent upsert (as #2298 did for procedures) or bounded
   retention.
7. **The count is honest — audit the closing action, not the counter** — when a
   signal recurs at a low, stable count, first prove the count is a faithful
   re-observation (deterministic, sorted/deduped signature; provable within-window
   dedup) before suspecting a storage/dedup bug. A *correct* count that never trends
   to zero points at a **missing convergence rung**, not a counting defect. Corollary:
   a "de-ratchet" fix that collapses a counter to one live node can silently make an
   escalation threshold unreachable (§6.2b trap) — carry counts *in content*, not in
   node multiplicity.

---

## 8. Consolidated evidence ledger

| Claim | Source |
|---|---|
| Composite signature = sorted/deduped problem `dedup_key`s joined `|`, prefixed `overseer-obs:` | `overseer/mod.rs:1068-1073` |
| Write-back content + de-dup identity on same signature | `overseer/mod.rs:1075-1089` |
| Occurrence recall token (SHA-256), recall limit | `overseer/mod.rs:1137,1147-1156` |
| Breaker fires after 3 idle cycles; bare "needs human review" park | `goal_curation/no_progress_breaker.rs:59,75` |
| `NoProgressClass` vocabulary; kgpacks "already done" incident | `goal_curation/no_progress_why.rs` header |
| Resolution ladder per class (MarkDone/Drop/Heal/Defer/Spawn/Escalate) | `goal_curation/no_progress_breaker.rs:384-417` |
| WHY reasoner double-gated, fails open to `Vec::new()`/bare-park | `ooda_loop/cycle.rs:582-702` |
| Investigation flag default-on; `=off` kill-switch | `ooda_loop/no_progress.rs:199-207` |
| Deterministic reasoner classifies from live artifacts | `ooda_loop/no_progress.rs:931-1010` |
| `workstream-gap` = coverage gap; p1/p2 + no workstream; blocked skipped | `overseer/sensor.rs::detect_workstream_gaps` |
| Zero/invalid decomposition handled loudly & separately | `goal_curation/decompose.rs` (`MIN_SUBGOALS=2`) |
| Consolidated `Signal::WorkstreamGap`; classifies to `WorkstreamCoverage` | `overseer/signal.rs`; `overseer/tests_gap_scan.rs` |
| Act path only notifies, never launches | `overseer/mod.rs:884-948` |
| `WorkstreamCoverage` is the only High arm with no launch edge; siblings launch | `overseer/mod.rs:1429,1436,1543,1565` |
| gap_gate = 15-min window (900s), no cross-window memory | `overseer/mod.rs:304,894-934` |
| Recurrence escalation threshold = 3 (blocked goals only) | `overseer/root_cause.rs:33`; `overseer/mod.rs:1613` |
| "seen 2×" = `RECURRING_SIGNATURE_THRESHOLD`; RecurringSignature@≥2 episodes | `overseer/signal.rs:362,455-469` |
| `record_occurrence` uses non-deduping `store_fact` (monotonic ratchet) | `overseer/mod.rs:1034`; `cognitive_memory/library_adapter.rs:657`; `mod.rs:340-353` |
| Escalate action idempotent per goal/15-min window (`GoalHealthSuppressed`) | `overseer/mod.rs:810-878,292` |
| Existing dedup-key helper for the counter fix | `overseer/root_cause.rs:53` |
| Recall-derived `RecurringSignature` admitted as distinct `ProcessHealth` problem, key = `sanitize_recalled(signature)` | `overseer/mod.rs:1353-1359`, `:1222` |
| Orient same-key merge does not fold the recall-derived problem away | `overseer/mod.rs:1210-1221` |
| All cycle problems (incl. recall-derived) written back → nested `overseer-obs:` tokens | `overseer/wiring.rs:301`; `overseer/mod.rs:534` |
| Observation episodes stored unconditionally (no upsert), unlike #2298 procedural upsert | `overseer/wiring.rs:1084-1090`; `cognitive_memory/library_adapter.rs:609-628` |
| Within-window write-back dedup proven; count = distinct episodes | `overseer/tests_memory_recall.rs:797-817,471-491` |
| WhisperGate `last_delivered` is in-memory/per-process (no cross-restart dedup) | `overseer/guardrails.rs:294` |
| Goal store = idempotent upsert-by-slug, flock-safe (no transition logic) | `goals/store.rs:291-300,233-253` |
| Launcher/`RecipeBrief` seam exists, bounded by launch cap | `overseer/launch.rs` |
| `tally_outcome`/counters seam for observability | `overseer/wiring.rs`; `overseer/activity.rs:66-68` |
| Guardrails risk classification seam | `overseer/guardrails.rs:60` |
| Two decoupled counter lanes (A=episodes/`×2`, B=occurrences/escalation) | `secondary_dedup_recurrence_VALIDATION_HEAD.md §2`; `wiring.rs:1076-1088`, `mod.rs:1004-1043` |
| Naïve `store_fact_with_caller_key(root_cause_signature)` collapses recall to 1 → escalation dead code (the trap) | `secondary_dedup_recurrence_VALIDATION_HEAD.md §4`; `library_adapter.rs:885-889` |
| Three-defect geometry D1/D2/D3; counter+WHY-gate form a latch (must ship together) | `tertiary_architecture_VALIDATION_HEAD.md §1,§3` |
| All prior citations re-verified at HEAD `dea65df8`; no fix merged | `tertiary_architecture_VALIDATION_HEAD.md §0`; `secondary_dedup_recurrence_VALIDATION_HEAD.md §1`; `RECONCILIATION_LEDGER.md §0` |
| `WorkstreamCoverage` quarantined from BOTH closing seams (M1 observer→Report, M2 acting→Notify) | `observer.rs:105,120`; `mod.rs:1543`; `tertiary_gap_routing_and_remediation_rung.md §1` |
| Per-gap identity is `GapItem.signature`; problem dedup_key is the constant `"workstream-gap"` (INV-GAP-KEY) | `signal.rs:135-138`; `mod.rs:1371,901`; `tertiary_gap_routing_and_remediation_rung.md §2` |
| Stewardship routing seam built + dangling, anticipates overseer gap briefs | `stewardship/routing.rs:11-15,39-52`; `tertiary_gap_routing_and_remediation_rung.md §3` |

---

## 9. Success-criteria coverage

1. **Common root cause of blocked goals** — ✅ §1/§1a: self-resolvable stalls
   degrade to bare "needs human review" parks when the WHY reasoner is
   double-gated off; the loop parks but never resolves.
2. **Meaning/trigger of `workstream-gap` + relation to blocked goals** — ✅ §2/§4:
   a notify-only backlog-coverage gap; the same goals oscillate between
   `workstream-gap` (active) and `goal:blocked` (parked).
3. **Why "seen 2×"** — ✅ §0/§0a/§3: the composite `observation_signature` is a
   faithful fingerprint of a static problem set (a **real** re-observation loop,
   **not** a dedup/storage bug); the *nested* `overseer-obs:` tokens are
   self-observation feedback (recall-derived problems written back); 2× sits in a
   recurrence dead zone (below the escalation bar of 3, no gap remediation rung at
   all), with two independent non-idempotency defects (§3a occurrence-fact ratchet,
   §3b episode storage) inflating counts / accumulating nodes. §3c re-frames these as
   **two decoupled counter lanes** (A drives the visible `×2`, B drives escalation).
4. **Prioritized per-goal unblocking actions** — ✅ §5.
5. **Systemic fix to stop the signature recurring** — ✅ §6: three-defect fix
   (§3c/§6.6) — a **count-in-content** occurrence record (§6.2b, *not* the naïve
   caller-key collapse, which is a proven trap) shipped **together with** guaranteed
   WHY-reasoner wiring (INV-WHY, §6.3) as the D2 latch; a recurrence-aware closing rung
   with gap threshold 2 (§6.1, D3); stopping the self-observation write-back that nests
   `overseer-obs:` tokens (§6.5, D1); and convergence observability (§6.4).

---

## 10. Fourth re-validation wave (HEAD `85b9398a`) — net-new, folded

This wave re-derived the analysis from source (no doc-to-doc trust) and confirmed **every**
load-bearing citation §0–§9 re-verifies *exactly* at HEAD `85b9398a`. No conclusion changed.
Four items are genuinely net-new and are folded in here:

- **10.1 — The investigation question is Simard-emitted (self-authored).** The literal string
  `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` is produced by
  the `Signal::RecurringSignature` arm of `signal_to_problem` (`mod.rs:1353-1362`). So the prompt
  we are answering is the overseer quoting **its own** write-back bookkeeping back to us — direct
  confirmation of the D1 self-observation loop, now traced end-to-end
  (`specialist_crosscheck_HEAD_85b9398a.md §2`).

- **10.2 — The `×2` counts recalled EPISODES, not occurrence facts (lane-A nailed).** The count
  comes from `signal.rs:455-469` tallying `snapshot.episodes[*].failure_signature`
  (`capabilities.rs:614`) — the overseer's own append-only `overseer-obs:` episodes — **not** the
  Lane-B root-cause occurrence facts. This pins the visible `×2` unambiguously to Lane A and
  explains the self-referential re-count at threshold 2
  (`specialist_revalidation_HEAD_85b9398a.md §1`, `primary_signature_provenance_dedup_verdict.md`).

- **10.3 — Store-boundary answer: cognitive memory ≠ stewardship store.** The `overseer-obs:…`
  signature lives **only** in cognitive memory and is **never** written to the stewardship
  (GitHub-issue) store. They are two physically distinct stores with different keys, idempotency
  contracts, and purposes: cognitive memory (`CognitiveClientMemoryStore`, dual-write Python
  client + `FileBackedMemoryStore`) is **append-only / not idempotent across windows**, keyed on
  `overseer-obs:{join}` (episodes) and `{dedup_key}::{label}` (facts); the stewardship store
  (GitHub Issues via `GhClient`) is **idempotent** (search-before-file → `FiledNew` once,
  `MatchedExisting` after), keyed on `failure_signature = sha256(kind‖norm(err))[..8]`
  (`stewardship/dedup.rs:63-75`). The two only share the word "signature" and the
  search-before-write idiom — the `×2` is a cross-window recurrence tally **in cognitive memory**,
  not a duplicated issue (`tertiary_pipeline_and_store_boundary.md §0`).

- **10.4 — Verdict re-affirmed with the honest-count / vacuous-event distinction.** Two things
  are simultaneously true: (a) **the count is honest** — two distinct episode nodes exist, the
  within-window gate provably suppresses same-window dupes, so it is not double-read / replay /
  collision / `dedup()` bug; and (b) **the event it certifies is vacuous** — the composite is an
  aggregate join of every open problem's `dedup_key`, so its recurrence means only "the same
  static problem set (partly the overseer's own bookkeeping) was observed twice." Audit the
  **missing closing action**, not the counter (`primary_signature_provenance_dedup_verdict.md`).

**Fix status (unchanged, re-confirmed):** all three investigation commits (`6e3113bc`,
`dea65df8`, `85b9398a`) are **documentation-only**; defects **D1/D2/D3 remain live in source**.
No remediation has been merged — §6's three-defect fix (D1 stop self-observation write-back,
D2 count-in-content occurrence record shipped atomically with guaranteed WHY-reasoner wiring,
D3 recurrence-aware gap-closing rung at threshold 2) is all **remaining scoped work**
(`specialist_revalidation_HEAD_85b9398a.md §0`).

---

## 11. Fifth re-validation wave (HEAD `388e6c29`) — net-new, folded

This wave re-grounded every load-bearing citation to live `src/` once more and reached the
same verdict with **zero source drift**: `git diff --name-only 6e3113bc..HEAD -- '*.rs'` is
**empty**, so all five investigation commits (`6e3113bc`, `dea65df8`, `85b9398a`, plus the two
consolidations to `388e6c29`) are **documentation-only** and every `src/overseer/*` +
`src/stewardship/dedup.rs` line citation in §0–§10 remains valid. Baseline idempotency tests
re-run green (`overseer::observer`, `tests_gap_scan`, `tests_root_cause`, `tests_memory_recall`;
incl. `dedup_signature_ignores_recipe_and_step_differences`, `write_back_is_deduplicated_within_window`,
`issue_filer_is_idempotent_across_cycles_no_network`). Six items are net-new and folded here (11.5
and 11.6 fold two fifth-wave deep dives — [`primary_signature_provenance_EMPIRICAL_HEAD_388e6c29.md`](./primary_signature_provenance_EMPIRICAL_HEAD_388e6c29.md)
and [`secondary_writeback_feedback_and_deadzone_HEAD_388e6c29.md`](./secondary_writeback_feedback_and_deadzone_HEAD_388e6c29.md)):

- **11.1 — `resource:engineer_spawn` is benign membership drift, not code drift.** The later of
  the two snapshots carries a NEW `resource:engineer_spawn` token (absent from the `6e3113bc`
  and `85b9398a` docs). It is **not new code** — the `"resource:engineer_spawn"` literal key has
  existed since `add1708a` (#2419/#2533); its appearance means an `EngineerSpawnRate{live}` signal
  crossed threshold **at observe time**. Structurally it is identical to `goal:blocked` and
  `workstream-gap`: the volatile `{live}` count lands only in the summary
  (`mod.rs:1267-1272` → `observation_content`), the `dedup_key` is a fixed literal, so **no
  volatile component leaks into the signature**. The prior "deterministic membership fingerprint"
  verdict absorbs it cleanly (`specialist_revalidation_drift_HEAD_388e6c29.md §3c`;
  `secondary_token_provenance_membership_delta_HEAD_388e6c29.md §1`).

- **11.2 — The two occurrences are overlapping-but-DIFFERENT snapshots (membership-delta table).**
  A per-token diff of the two snapshots shows: the 8 kgpacks/core `goal:blocked` goals **PERSIST**
  (unremediated across both passes); the five `simard-identity-*` goals **DROP** (unblocked between
  passes); `resource:engineer_spawn` + an extra nested `workstream-gap` **APPEAR**. Because
  membership A ≠ B, `observation_signature(A) ≠ observation_signature(B)` **by design** → both were
  legitimately stored, and the recall counter later saw the recurring *family* prefix ≥2× and
  emitted `RecurringSignature`. This confirms the `2×` is a faithful re-observation **loop, not an
  artifact** — and sharpens §10.4 from "static set" to "*near*-static set"
  (`secondary_token_provenance_membership_delta_HEAD_388e6c29.md §4`).

- **11.3 — One under-throughput problem in three views.** The three persistent token families —
  `goal:blocked:*` (GoalHygiene), `workstream-gap` (WorkstreamCoverage), and now
  `resource:engineer_spawn` (ResourcePressure) — are causally **one under-resourcing/under-throughput
  problem**: the system *is* spawning engineers (`engineer_spawn` up) yet goals stay blocked and gaps
  stay uncovered. All three are observe-and-flag problems with no closing action, sitting in the same
  **"2× dead zone"** — deduped by the 15-min `write_back_gate` (`mod.rs:548`) yet below
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) — flagged forever, escalated never
  (`secondary_token_provenance_membership_delta_HEAD_388e6c29.md §5`).

- **11.4 — Fix landing + per-test no-regression argument (tertiary deliverable).** The recurring
  `overseer-obs:…` signature is produced **only** in `overseer/mod.rs`; `stewardship/dedup.rs` is a
  confirmed **red herring** (it governs the already-idempotent issue-filing lane —
  `failure_signature`/`find_existing` — which never carries the composite). Landing map, re-affirmed
  at HEAD: **D2 core** count-in-content upsert at `record_occurrence` (`mod.rs:1034`) + read at
  `recall_occurrences` (`mod.rs:972-997`), **coupled** with closing the WHY double-gate in
  `ooda_loop/cycle.rs` (the pair is a latch — must ship atomically); **D1** one-line write-back
  filter (`mod.rs:534-563`, drop recall-derived `RecurringSignature` problems before
  `observation_signature`); **D3** recurrence-aware gap-closing rung (`mod.rs:901-934` + `launch.rs`).
  **Never `dedup.rs`.** Every named idempotency test is on a separate seam the fix does not touch, so
  each stays green; the fix set moves the system **toward** stronger idempotency (upsert replaces
  append; content-count replaces node-multiplicity; self-observation stops re-entering the graph)
  and weakens no existing dedup guarantee
  (`tertiary_fix_landing_and_regression_safety_HEAD_388e6c29.md §1,§4,§5`).

- **11.5 — Empirical (test-executed) proof the `2×` is real-and-vacuous, not an artifact (primary).**
  Beyond citation, the fifth-wave primary **ran the two gating suites** — `overseer::tests_memory_recall`
  (**32 passed**) and `stewardship::tests` (**23 passed**), 55 tests total — turning the verdict from
  code-read to test-anchored. Load-bearing cases: `write_back_is_deduplicated_within_window` (two
  identical-signature ticks in one 900 s window ⇒ exactly **1** episode — rules out double-write);
  `write_back_persists_again_for_a_distinct_signature` (`tests_memory_recall.rs:820`; distinct obs ⇒
  **2** episodes — the count is honest); `recurring_signature_emitted_when_two_episodes_share_signature`
  (`:471`, the `≥2` floor) bracketed by `recurring_signature_not_emitted_for_single_occurrence`. The
  stewardship suite proves the `failure_signature = sha256(kind‖msg)` hash lane is a **separate,
  isolated subsystem that never touches the `overseer-obs:` composite** — closing the "ruled-out
  artifact" column with executable evidence. Every artifact hypothesis (double-write, replay,
  off-by-one, `dedup()` collapse, hash collision, `stewardship` mis-key, `notify.rs` duplication) is
  now test- or structure-anchored (`primary_signature_provenance_EMPIRICAL_HEAD_388e6c29.md §A,§B,§ruled-out`).

- **11.6 — Second-order harm + non-durable gate sharpen the D1/D2 case (secondary).** Two net-new
  refinements: (a) **The self-observation meta-problem is *cost-bearing*.** The recall-derived
  `RecurringSignature` problem is `ProblemKind::ProcessHealth` → `Intervention::LaunchRecipe`
  (`mod.rs:1429`), and `is_cost_bearing` (`mod.rs:1057-1062`) classes `LaunchRecipe`/`RunAudit` as
  budget-spending — so the loop can spend LLM budget launching a recipe **about the overseer's own
  observation blob** instead of remediating real blocked goals. It stands alone (its `overseer-obs:*`
  key merges with no in-cycle problem, `mod.rs:1211`), so it is the *only* constituent with a
  `LaunchRecipe` edge — the one convergent action aimed at the wrong target. (b) **The write-back
  dedup gate is in-memory only.** `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`) is a
  `HashMap`+`Vec` with **no persistence** (`guardrails.rs:291-297`), so a daemon restart resets it and
  the same `observation_signature` is re-recorded → recall climbs to 2. Hence the missing lever is
  **storage-layer idempotency (signature-keyed upsert / bounded retention), not the counter** — and
  the D1 fix must land at the **WRITE boundary** (exclude `overseer-obs:`-prefixed / `RecurringSignature`
  problems before `observation_signature`), because read-side `sanitize_recalled` scopes "untrusted"
  to injection/length, not to the system's own recycled signatures. A regression test (feed a recalled
  `overseer-obs:`-prefixed episode, assert it does NOT re-enter the next signature) does not yet exist
  (`secondary_writeback_feedback_and_deadzone_HEAD_388e6c29.md §1,§2,§5,§6`).

**Fix status (unchanged, re-confirmed at HEAD `388e6c29`):** all investigation commits remain
**documentation-only**; defects **D1/D2/D3 stay live in source**. No remediation merged — §6's
three-defect fix is all remaining scoped work. **INVESTIGATION-ONLY** — this wave specifies and
re-validates; it does not implement.

---

## 12. Sixth re-validation wave (HEAD `0289572e`) — net-new, folded

This wave (the current consolidation) re-ground every prior verdict against live `src/` a sixth
time across four parallel deep dives (specialist re-grounding, two architect pipeline/landing
deliverables, a primary re-validation, and a secondary token-classification pass). Drift is docs-only
once more:
`git diff --name-only 85b9398a..HEAD` touches **only** `ai_working/investigation/*.md`;
`git diff --name-only 6e3113bc..HEAD -- '*.rs'` is **empty**. Baseline re-run green: `cargo test -p
simard --lib -- overseer::observer:: dedup_signature brief_to_summary write_back_is_deduplicated`
→ **13 passed; 0 failed** (incl. `write_back_is_deduplicated_within_window`,
`dedup_signature_ignores_recipe_and_step_differences`,
`issue_filer_is_idempotent_across_cycles_no_network`,
`same_process_problem_dedups_to_one_issue_across_cycles`,
`brief_to_summary_synthesises_stable_run_id_from_signature`). Every load-bearing citation was
independently re-verified exact at HEAD (`mod.rs:1068-1073` signature builder; `mod.rs:1353-1363`
`RecurringSignature`→`ProcessHealth`/`High`/`sanitize_recalled` — whose summary string is literally
`"recurring signature seen {occurrences}× in cognitive memory ({signature})"`, i.e. the exact text
of the investigation question; `mod.rs:1211,1217-1218` same-key merge raising priority via `.min`;
`mod.rs:1615-1633` `decide_blocked_goal` fall-through to `Intervention::Report`; `signal.rs:362`
`RECURRING_SIGNATURE_THRESHOLD = 2`; `root_cause.rs:33` `RECURRENCE_ESCALATION_THRESHOLD = 3`;
`wiring.rs:301` write-back passes **all** `cycle.problems`; `mod.rs:1267-1272` `EngineerSpawnRate`
→ fixed literal `resource:engineer_spawn`). The net-new items folded here (12.1–12.10):

- **12.1 — End-to-end OODA-tick pipeline trace makes the feedback runaway concrete.** The tertiary
  deep dive traces one `run_cycle()` tick token-by-token: (A) OBSERVE snapshot → (B) pre-recall keys
  (`RecallKeys::from_signals`) → (C) RECALL recovers prior `overseer-obs:…` from the `[sig:…]` marker
  → (D) `signals_from` counts recalled episodes by `failure_signature`, emitting
  `Signal::RecurringSignature` at ≥2 (`signal.rs:463`) → (E) ORIENT sets the recurring problem's
  `dedup_key = sanitize_recalled(signature)` — i.e. the `overseer-obs:…` string **itself** →
  (G) WRITE-BACK folds that key into the **next** `observation_signature`. Shown concretely, tick
  *n*'s whole keyset re-enters tick *n+1* as one giant nested key, reproducing the observed
  `overseer-obs:overseer-obs:…` shape. This is the sharpest single-tick proof yet that the nesting is
  a genuine self-observation control-flow loop, not display noise
  (`tertiary_architecture_pipeline_and_landing_order_HEAD_0289572e.md §2`).

- **12.2 — Dependency-safe landing order `D2→D1→D3`, with a hard ordering constraint.** The tertiary
  argues a *true* dependency chain (not preference): land the **loop-breaker first** — exclude
  recall-derived meta-problems (evidence solely `Signal::RecurringSignature`, or
  `dedup_key.starts_with("overseer-obs:")`) from write-back, co-landing a classify/decide meta-guard
  so an `overseer-obs:*` `RecurringSignature` never becomes an actionable `ProcessHealth`/`LaunchRecipe`.
  **Rationale — the load-bearing new insight:** making the store idempotent *before* breaking the loop
  is useless, because **each nesting level is a *different* signature, so a signature-keyed upsert
  cannot collapse a moving target.** Only after the signature set is stabilized (meta-free) does
  "one node per signature" describe a fixed target and does the recall `occurrences` count reflect
  distinct observation *windows* rather than write cadence; only then can a first-recurrence rung
  escalate on a *meaningful* count over a *meta-free* signature. Hence the strict chain: stabilize
  (loop-breaker) → make counting meaningful (idempotent upsert) → act on the count (recurrence rung)
  (`tertiary_architecture_pipeline_and_landing_order_HEAD_0289572e.md §4`).

- **12.3 — RECONCILIATION: the two waves relabel D1/D2/D3; the underlying three defects are
  identical.** The §3c/§6 numbering (waves 1–5) and the sixth-wave tertiary use **conflicting** D
  labels for the **same** three seams. Readers must map them explicitly:

  | Underlying defect (one per seam) | §3c/§6 label | §12 (wave-6) label | §6 landing slot |
  |---|---|---|---|
  | Self-observation write-back re-enters recall-derived `overseer-obs:` tokens (`wiring.rs:301`, filter before `observation_signature`) | **D1** (§6.5) | **D2** (loop-breaker) | write-back filter |
  | Recurrence counter is a non-idempotent ratchet / cadence artifact → signature-keyed count-in-content upsert (`mod.rs:1034`, read `mod.rs:972-997`) | **D2** (§6.2b + WHY-gate latch) | **D1** (idempotent upsert) | count-in-content + WHY-gate latch |
  | Recurrence dead-zone + notify-only gap routing → first-recurrence closing rung (`mod.rs:1615-1633`, `901-934`) | **D3** (§6.1) | **D3** (recurrence rung) | gap-closing rung |

  The three fixes and their seams are **unchanged**; only the labels differ. To avoid future
  confusion, treat §6's descriptive names (not the bare D-numbers) as canonical; the sixth-wave
  D-numbers are local to `tertiary_architecture_pipeline_and_landing_order_HEAD_0289572e.md`.

- **12.4 — Landing-order tension surfaced (loop-breaker-first vs. counter-latch-first).** §6.6's
  order (wave-1–5 labels) was **D2 counter+WHY-gate (atomic latch) → D3 rung → D1 write-back filter**;
  the sixth-wave order (its own labels) is **D2 loop-breaker → D1 upsert → D3 rung**, which in §6's
  canonical names is **write-back filter → counter-upsert → rung**. These **disagree on whether the
  write-back filter or the counter latch lands first.** The sixth-wave argument is the stronger one
  and should be adopted: the write-back filter (loop-breaker) **must precede** the idempotent counter,
  because an upsert keyed on a signature that keeps growing new nesting levels collapses nothing —
  the target moves every tick until the loop is cut. This **refines §6.6**: keep §6.2b's
  count-in-content + WHY-gate as an atomic latch, but land the §6.5 write-back filter (loop-breaker)
  **before** that latch, then the §6.1 gap/recurrence rung last. No source citation changes; this is
  a sequencing correction to the fix *plan*, not to the root-cause analysis.

- **12.5 — One stale citation flagged (does not affect any verdict).** The sixth-wave tertiary cites
  `tests_signature_verification.rs:164` as "a test pins the [2 vs 3] ordering." **That file does not
  exist** at HEAD `0289572e` (`ls src/overseer/tests_signature_verification.rs` → not found; no test
  file references `RECURRING_SIGNATURE_THRESHOLD` besides `signal.rs` itself). The two thresholds are
  nonetheless **real and verified** as constants (`signal.rs:362 = 2`, `root_cause.rs:33 = 3`), so the
  **dead-zone verdict stands**; only the "a test pins it" sub-claim is unsupported. Per the
  no-doc-to-doc-trust discipline (see `RECONCILIATION_LEDGER.md`), this citation should be dropped or
  repointed to the live constants rather than the nonexistent test.

- **12.6 — Empirical fix-landing proof: D1/D2/D3 are ALL unmerged at HEAD (primary).** Beyond the
  `*.rs`-diff-empty argument, a direct grep confirms **no fix has quietly landed**: `grep -rn
  "count_in_content\|upsert_fact\|occurrence_count" src/overseer/` is **empty** (D2 idempotent
  count-in-content upsert absent — `store_fact` at `mod.rs:1034` remains append-only); the only
  `overseer-obs:` sites are the construction (`mod.rs:1072`) and a comment (`mod.rs:440`) — **no
  write-boundary exclusion** (D1/loop-breaker absent); and there is no `quarantine` nor any
  `workstream-gap`→`LaunchRecipe` edge (D3 gap-closing absent, still notify-only). This upgrades the
  fix-status claim from "docs-only diff" to "**verified absent by symbol search**"
  (`primary_signature_provenance_REVALIDATION_HEAD_0289572e.md §6`). A second architect drift-check
  independently corroborates this at the **struct level**: `StoredOccurrence` (`mod.rs:1180-1185`)
  has exactly four fields — `signature`, `cause_label`, `action`, `outcome` — **no `count` /
  `first_seen` / `last_seen`**, so the D2 count-in-content field literally does not exist yet; and
  `grep 'starts_with("overseer-obs'` in `mod.rs` returns **0 matches** (loop-breaker guard absent).
  That deep dive also re-derives the full per-citation drift table (all ✅ exact, docs-only) and
  endorses §12.2/§12.4's loop-breaker→idempotent-counter→closing-rung order and the count-in-content
  (not bare caller-key) remedy (`tertiary_architecture_DRIFT_AND_LANDING_HEAD_0289572e.md §2,§4`).

- **12.7 — Remediation must land on the EPISODE lane (Lane A), not the occurrence lane (Lane B).**
  The primary sharpens §3c's two-lane model into an actionable placement rule: **Lane A** =
  episode-count recall (`signal.rs:455-470`, gate `RECURRING_SIGNATURE_THRESHOLD = 2`) drives the
  **visible `2×`** and priority promotion; **Lane B** = root-cause occurrence facts
  (`record_occurrence`→append-only `store_fact`, `mod.rs:1004-1043`; gate
  `RECURRENCE_ESCALATION_THRESHOLD = 3` at `mod.rs:1613`) drives escalation. Because the `2×` is only
  ever provable on Lane A, the first-recurrence closing rung (§6.1/§12.2-D3) **must read the episode
  lane**, not the occurrence lane — escalating on Lane B alone can never fire at 2×. The
  15-min `write_back_gate` is re-confirmed as a `WhisperGate` **900 s** window (`mod.rs:191-192,286`),
  so `occurrences == 2` ⇒ two Observe passes ≥15 min apart over an unchanged state
  (`primary_signature_provenance_REVALIDATION_HEAD_0289572e.md §3,§4,§6`).

- **12.8 — Per-goal stall-class map: 3 of 4 blocked clusters are NOT genuine, one common root cause
  (secondary).** The secondary classifies each aggregate member goal and finds the recurring
  population is **one unwired classification rung, not many goal bugs**:

  | Cluster | Stall class | Genuine block? |
  |---|---|---|
  | kgpacks-rs parity + issues #12/#17/#18/#23/#25 | false-park `AlreadyComplete`/`MissingPrecondition` (work already closed/merged, misread as stuck) | **NO** |
  | audit Simard coverage → 70% | uncheckable done-gate `UnclearCriteria` (idles, re-parks) | **NO** |
  | simard-identity personas (atelier/bursar/cartographer/concierge/gastronome) | starvation `GoalUncovered` (p1/p2, no assignee/workstream) | **NO** (resourcing) |
  | coin benchmark harness | genuine `MissingPrecondition`/`UpstreamDependency` | **YES** (1 of 4) |

  All three non-genuine classes funnel through the same **bare no-progress park with no `WHY` token**:
  the corrective vocabulary exists (`NoProgressClass` + `resolution_for_why`) but the WHY reasoner in
  `ooda_loop/cycle.rs` is **double-gated (completion-evidence gate + feature-flag gate) and fails
  open to bare-park**, with no invariant tying a `Blocked` reason to a `NoProgressClass`. So the
  blocks are **causally linked (one common root cause)**, merely co-aggregated — with the coin-harness
  the lone independently-genuine dependency block mixed in
  (`secondary_token_classification_and_root_cause_HEAD_0289572e.md §4`).

- **12.9 — `workstream-gap` and `goal:blocked` are one entity oscillating between two views
  (secondary).** Re-verified at `sensor.rs:300-302`: `detect_workstream_gaps` **explicitly skips
  `GoalProgress::Blocked` goals** ("Blocked goals flow through goal_health; never re-flag them here"),
  so a single under-resourced entity is `workstream-gap` while active-uncovered and `goal:blocked`
  while parked — never both at once, but both across windows. Corollary trap: the bare family key
  `"workstream-gap"` (`mod.rs:1371`) **destroys per-gap identity** at the write boundary (every persona
  gap is indistinguishable), whereas the per-gap gate already uses a distinct key
  `workstream-gap:{signature}` (`mod.rs:901`) — so any gap-closing rung (D3) must key on
  `GapItem.signature`, echoing the `INV-GAP-KEY` trap in §10/§3-ledger
  (`secondary_token_classification_and_root_cause_HEAD_0289572e.md §5,§6,§7`).

- **12.10 — Open verification questions carried forward (secondary).** Four checks would harden the
  membership-drift and root-cause claims but require a live daemon / goal board (out of scope for this
  static investigation): (1) confirm the `simard-identity-*` goals genuinely transitioned to unblocked
  between snapshots (DROP A→B) rather than merely dropping out of recall ranking; (2) confirm
  `resource:engineer_spawn` fired from real elevated live-spawn telemetry at snapshot B (convergence
  class) vs. a one-off spike; (3) confirm `completion_evidence` (WHY Gate A) is actually `None` in the
  live daemon (determines whether the WHY ladder ever ran for the kgpacks cluster); (4) confirm the
  escalation decision latches at `recurrence ≥ 3` and never un-latches
  (`secondary_token_classification_and_root_cause_HEAD_0289572e.md §Questions`).

**Fix status (unchanged, re-confirmed at HEAD `0289572e`):** all six investigation commits remain
**documentation-only**; defects **D1/D2/D3 (either labeling) stay live in source**. No remediation
merged — §6's three-defect fix, landed in the §12.4-refined order, is all remaining scoped work.
**INVESTIGATION-ONLY** — this wave specifies, re-validates, and reconciles; it does not implement.

---

## 13. Seventh re-validation wave (HEAD `5a85317b`) — hypothesis formalization + full per-hypothesis verification, folded

This wave re-cast the accumulated six-wave verdicts as an explicit **falsifiable hypothesis set**
([`HYPOTHESES.md`](./HYPOTHESES.md)) and then executed a **practical verification test for every
hypothesis** ([`verification_results_ALL_HYPOTHESES.md`](./verification_results_ALL_HYPOTHESES.md)),
synthesized in [`FINAL_SYNTHESIS.md`](./FINAL_SYNTHESIS.md). All source citations were re-grounded to
live `src/` at HEAD `5a85317b`; `git diff --name-only 6e3113bc..HEAD -- '*.rs'` is **empty**
(docs-only), so every prior line citation still holds. Independently re-confirmed this wave:
`observation_signature` (`mod.rs:1068-1072`), the `"workstream-gap"` constant key (`mod.rs:1371`),
`RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`), `RECURRENCE_ESCALATION_THRESHOLD = 3`
(`root_cause.rs:33` — the escalation constant now cited at its definition, superseding the earlier
`mod.rs:1613` use-site citation), and the bare-block pair `is_bare_no_progress_block`
(`no_progress_breaker.rs:108`) / `no_progress_blocked_reason` (`:123`).

- **13.1 — The six-wave analysis is now a confirm/falsify hypothesis map (H0–H8).** Each prior finding
  is restated so a single source/test probe can confirm or falsify it, and the falsification tests
  become the **acceptance criteria for any eventual fix**:

  | ID | Hypothesis | Role | Defect | Verdict |
  |----|-----------|------|--------|---------|
  | H0 | Dedup / storage / replay / collision artifact | Null | — | **REJECTED (high)** |
  | H1 | Real re-observation loop of a near-static set | Cause of `2×` | — | **CONFIRMED (high)** |
  | H2 | WHY reasoner double-gated → bare parks | Root cause A | D2 (latch) | **SUPPORTED (high)** |
  | H3 | `WorkstreamCoverage` has no closing edge | Root cause B | D3 | **SUPPORTED (high)** |
  | H4 | Self-observation write-back feedback | Nesting cause | D1 | **SUPPORTED (bounded)** |
  | H5 | 2×↔3× dead zone, two decoupled lanes | Why "exactly 2×" | D2 | **SUPPORTED (high)** |
  | H6 | Non-idempotent counters (compounding) | Amplifier | D2 | **SUPPORTED (non-causal)** |
  | H7 | blocked ↔ gap = one problem, two views | Unifier | — | **SUPPORTED (high)** |
  | H8 | Three token families = one under-throughput | Generalization | — | **SUPPORTED (med-high)** |

  This map **reconciles cleanly with §0–§12** — no verdict reversed. H1 restates §0's honest-`2×`
  verdict; H0's rejection restates §0/§3's not-a-dedup-bug proof; H2↔§1/§1a (double-gated WHY),
  H3↔§2/§6.1 (notify-only gap arm / D3), H4↔§0a/§6.5 (self-observation, D1), H5↔§3/§3c (dead zone,
  two lanes), H6↔§3a/§3b/§6.2b (non-idempotent ratchet, count-in-content remedy, non-causal),
  H7↔§4/§12.9 (oscillation), H8↔§11 (`resource:engineer_spawn` benign membership drift). The one
  refinement carried in: H1 is sharpened from "static set" to **"near-static set"** to match §11.2's
  overlapping-but-different two-snapshot membership delta.

- **13.2 — Null hypothesis empirically rejected; the `2×` is an honest count.** H0 (the `2×` is a
  counting/dedup/replay/collision artifact) was actively falsified, not merely assumed away:
  `write_back_is_deduplicated_within_window` proves intra-window suppression (not a double-read);
  `write_back_persists_again_for_a_distinct_signature` proves distinct observations legitimately
  persist (the count is honest); `keys.dedup()` collapses only *adjacent-equal* keys within one
  signature (`mod.rs:1071`), so `workstream-gap|workstream-gap` are distinct concatenated
  problems/episodes, not a dedup failure; and the store-boundary trace confirms the composite lives
  **only** under the cognitive `overseer-obs:` key, never in the stewardship store (keyed on
  `sha256(kind‖norm(err))[..8]`) — no cross-store duplication.

- **13.3 — Every hypothesis has an executed practical test; suite is green at current HEAD.** The
  verification pass reproduced at HEAD `5a85317b`: the full overseer suite (`cargo test --lib
  overseer::`) **360 passed / 0 failed**; **17 targeted `--exact` discriminating tests** (7 + 10, two
  batches) all passed; and an **end-to-end no-bridge probe** — a `RecurringSignature{occurrences:2}`
  fed through `signals_from → orient → analyze → decide` yields a `ProcessHealth` `LaunchRecipe` with
  root-cause `recurrence == 0`, **never** `EscalateBlockedGoal` — executed and passed. This last probe
  is the empirical proof of the §3c/§12.7 **two-lane decoupling**: Lane A's visible `×2` (episodes)
  cannot advance Lane B's `≥3` escalation rung (occurrence facts) because no code path converts one
  into the other. (The absolute suite count drifts by a few tests across waves as the suite grows —
  earlier `verification_results.md` recorded 359; the invariant is **0 failures** and all 17
  discriminating tests green, which reproduces here.)

- **13.4 — Smoking-gun test for INV-WHY (H2).** `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
  demonstrates that a bare-block reason (no `NoProgressClass` / WHY token) can persist **indefinitely**
  — the goal re-parks every window and stays in the `goal:blocked:*` population. Combined with the
  reachability of `no_progress_blocked_reason(consecutive)` (renders `{PREFIX}{count}{SUFFIX}` with no
  WHY, `no_progress_breaker.rs:123-125`) and `is_bare_no_progress_block` returning `true` for it
  (`:108-113`), this proves **INV-WHY is violable in source today** — the acceptance-criterion
  falsification test for H2 holds, confirming the primary common root cause behind the `goal:blocked`
  tokens.

- **13.5 — Discriminating predictions locked as the fix acceptance criteria (unchanged from §6.4).**
  The four falsification tests that must flip once §6 remediation lands are re-confirmed verbatim as
  the regression contract: (1) close the WHY double-gate + count-in-content counter atomically ⇒
  `goal:blocked:*` tokens converge (falsifies H2 "stuck forever"); (2) add a recurrence-aware
  gap-closing rung at threshold 2 ⇒ the `workstream-gap|workstream-gap` tail converges (falsifies H3
  "flag-forever"); (3) filter recall-derived `RecurringSignature` from write-back ⇒ nested
  `overseer-obs:` tokens vanish (falsifies H4); (4) the persistent-unremediated gauge (§6.4: count of
  signatures with recurrence ≥2 and no launch/escalation, plus INV-WHY violations) must reach and stay
  0 — the leading indicator that a signature re-entered the dead zone.

- **13.6 — Minimal contained D1 signature-path fix, with exactness proof (tertiary architect).** The
  seventh-wave architect specified the *cheapest, orthogonal* fix for the self-referential
  `overseer-obs:…|overseer-obs:…` nesting: a ~4-line filter inside `write_back_observation`
  (`mod.rs:534-563`) that drops any problem whose `dedup_key.starts_with("overseer-obs:")` before
  signature/content assembly (empty-survivor set ⇒ clean tick, write nothing). Two net-new proofs make
  it exact and safe: **(a) single-producer** — the `overseer-obs:` prefix has exactly **one** literal
  emitter in the tree, `observation_signature` (`mod.rs:1072`), re-confirmed by
  `grep '"overseer-obs:' src/overseer/*.rs` returning only that line; the prefix reaches a
  `dedup_key` only via the RecurringSignature recall arm (`mod.rs:1359`), and `sanitize_recalled`
  (`capabilities.rs:468`) preserves the position-0 prefix (only blanks control chars / end-truncates),
  so the filter is robust even against deeply nested composites. **(b) prefix-filter beats an
  evidence-based filter** — a RecurringSignature whose recalled signature is a *domain* key (e.g.
  `goal:blocked:X`) keeps that domain `dedup_key` and **merges** into the genuine fresh blocked-goal
  problem in `orient` (`mod.rs:1211-1219`), boosting its priority; filtering on "evidence contains
  `RecurringSignature`" would wrongly drop that real, recall-*boosted* observation, whereas the
  `starts_with("overseer-obs:")` filter excludes **only** the truly self-referential standalone echoes.
  Scope is deliberately **write-back only**: the RecurringSignature problem stays in `cycle.problems`
  so Decide/Act still fire its `LaunchRecipe` (`mod.rs:1429`); the fix removes the nested *shape* but
  does **not** by itself converge the static problem set (that is D2+D3). This confirms and sharpens
  §6.5 and the recommended **D2 (atomic gate+counter) → D3 (closing rung) → D1 (this filter)** landing
  order (§12.4/§6.6), and re-grounds every prior citation at HEAD `5a85317b` with **zero line drift**
  (`git diff --stat dea65df8..HEAD -- src/` empty). Two residuals are explicitly logged: the
  window-vs-restart origin of the `×2` is undecidable from static source, and whether the overseer
  should *act* on (not just avoid re-recording) its own recalled echo — the `LaunchRecipe` from a
  recall-derived ProcessHealth problem — is a broader emission-hygiene question left to the D-set owner.

**Bottom line (seven-wave consolidated verdict):** the `×2` is a **faithful cross-window recurrence
count of a genuinely re-observed near-static problem set** (H1 confirmed; H0 empirically rejected).
It never changes because two observe-and-flag loops never close — blocked goals can bare-park with no
WHY (H2/D2), coverage gaps notify with no launch edge (H3/D3) — and the count parks in the **dead zone
between thresholds 2 and 3** (H5), while the overseer **re-observes its own bookkeeping** (H4/D1,
bounded) and the counters **lack idempotency** (H6, compounding not causal). H7/H8 unify the symptoms
into **one under-throughput condition in three views**. Every defect is design-level; **none is a
dedup/storage bug.**

**Fix status (unchanged, re-confirmed at HEAD `b9f99879`):** all eight investigation commits remain
**documentation-only**; defects **D1/D2/D3 stay live in source**. No remediation merged — §6's
three-defect fix, landed in the §12.4/§13.6-refined **D2→D3→D1 dependency-safe order**, is all
remaining scoped work. **INVESTIGATION-ONLY** — this wave formalizes and verifies; it does not
implement.

---

## 14. Eighth-wave net-new (HEAD `b9f99879`) — provenance re-trace, open-loop seam, and the "spawn cycle" correction

The eighth wave changed **no verdict**; it re-grounded the entire pipeline at a fresh HEAD with **zero
code drift** and sharpened three things: the *exact* open-loop seam, the correct classification of the
`workstream-gap ↔ engineer_spawn` co-occurrence, and the architectural naming of the D1 cause.

- **14.1 — Zero code drift, independently re-verified (all three deep dives).** The last pipeline code
  change is `6b2bf5e1` (2026-07-14, `fix(stewardship): stop recursive issue flood safely (#4063)`);
  `src/ooda_loop` last changed at `ad8a2b81` (2026-07-08). `git diff 6b2bf5e1..b9f99879 -- src/overseer
  src/stewardship` and `… -- src/ooda_loop` are **both empty** — every commit in between is
  `docs(investigation)`. Consequence: every line citation across all prior waves is **byte-identical**
  at HEAD; the D1 fix (`dedup_key.starts_with("overseer-obs:")` write-back filter) is still **not
  present** at `mod.rs:534-563`; the §6.2b remedy trap remains correctly superseded. The per-citation
  re-read table (tertiary §1) marks every load-bearing claim ✅ exact.

- **14.2 — The precise open-loop seam: provenance stamped on write, erased on read (D1 root cause,
  architect naming).** `record_observation` correctly stamps `source_label = "overseer"`
  (`wiring.rs:1084-1088`), but `recall_episodic` (`wiring.rs:1013-1031`) recovers
  `failure_signature = parse_failure_signature(&e.content)` from **every** episode and **never reads
  `e.source_label`** — the self-vs-external distinction *exists in storage but is discarded on read*
  (`wiring.rs:1024-1029`). That single seam is where the loop closes: the Overseer's own
  `overseer-obs:…` write-back re-enters recall as a `failure_signature`, two such episodes fire
  `RecurringSignature{occurrences:2}` (`signal.rs:455-470`, threshold 2 at `signal.rs:362`), Orient
  admits it as a **standalone** `overseer-obs:…` problem that never merges (`mod.rs:1353-1363`,
  `1211`), and the next `observation_signature` nests it one level deeper. The cheapest correct fix is
  a ~4-line drop of own-source episodes **at recall** *or* of `overseer-obs:*` keys **at write-back**;
  both are orthogonal to D2/D3.

- **14.3 — Write-back-gate verdict: correct as designed, NOT a loop breaker; loop OPEN at HEAD
  (primary).** `WhisperGate::new(900, 5)` (`mod.rs:298-299`) suppresses **same-window** duplicates
  only. Two legitimate write-back passes ≥15 min apart produce two episodes with the identical
  composite; recall counts 2 and fires at `occurrences >= 2`. The gate cannot — and was never meant to
  — prevent this. Worse, **while the signature is nesting it is a *new* string every cycle**, so even
  in-window the gate always `Deliver`s until the composite saturates `RECALLED_TEXT_MAX_LEN = 8192`
  (`capabilities.rs:455`); after saturation the truncated prefix stabilizes and cross-window
  re-delivery sustains the `×2`. Either regime keeps the loop alive. This resolves the earlier
  window-vs-restart residual: a daemon restart is *sufficient* but **not necessary** for exactly 2×.

- **14.4 — `workstream-gap → resource:engineer_spawn` is NOT a causal orchestration cycle (secondary
  headline correction; confirmed by tertiary §4).** There is **no code edge** between the two.
  `workstream-gap` (`detect_workstream_gaps`, `sensor.rs:288`) drives only
  `act_flag_workstream_gaps` (`mod.rs:884`) → **operator notification only** (email/Signal), keyed
  `workstream-gap:{g.signature}`; it launches no workstream, files no issue, **spawns no engineer**.
  `resource:engineer_spawn` is a **passive** telemetry read of `live_engineers >=
  ENGINEER_SPAWN_THRESHOLD (8)` (`signal.rs:351,393-396`). Real engineer spawning lives in a
  **different subsystem** — OODA `dispatch_spawn_engineer` (`cycle.rs:665`) / `no_progress.rs`
  `SpawnEngineer` (`:712-713`), **bounded to one guided retry** (`mark_guided_retry`,
  `no_progress.rs:716`). The two tokens co-occur only because both conditions were true in the same
  window (engineers maxed **and** coverage incomplete) — a real **under-resourced STATE**, not a loop.

- **14.5 — Split defect/steady-state verdict along the co-occurrence (secondary S4).** The
  `resource:engineer_spawn` side is **benign steady-state** — passive telemetry, count in summary only,
  real spawn path bounded — no unfulfilled-spawn defect at the overseer boundary. The `workstream-gap`
  side is a **real defect**, but the *observe-and-flag-without-closing* defect (D3, the missing
  convergence rung), **not** an orchestration cycle. `WorkstreamCoverage` remains the only High-priority
  Decide arm with no `launch.rs` close edge (`mod.rs:1534-1543`). Both share the same root: one
  resourcing/convergence problem surfaced through multiple lenses (confirms H7/H8, §7).

- **14.6 — Two dedup namespaces, orthogonal, re-confirmed (secondary S1 / tertiary §2).** The
  investigation signature is composed **entirely** from Overseer Problem `dedup_key`s (the single
  `overseer-obs:` composite namespace, minted by `observation_signature`, `mod.rs:1068-1073`). It
  **never** touches the stewardship `sha256(kind + norm(text))[..8]` `failure_signature` namespace
  (`stewardship/dedup.rs:63`), which is GitHub-**issue** dedup only. The two systems are correctly
  orthogonal; `stewardship/routing.rs` is ruled out as the token origin. Each family key is idempotent
  (Orient merges same-key problems, `keys.dedup()` collapses adjacent equals), so the signature is a
  **deterministic membership fingerprint of the open-problem SET**, not an inflating join — a *fixed*
  stuck set yields a *stable* signature; the `×2` is cross-window re-observation, not per-goal
  inflation.

- **14.7 — Landing order re-affirmed and residual verification questions logged.** The dependency-safe
  order stands: **D2 (atomic gate+counter) → D3 (per-`GapItem.signature` closing rung, honouring the
  INV-GAP-KEY trap) → D1 (recall/write-back self-exclusion filter)**. D1 alone stops the *nesting
  shape* but not the `×2` (the static set is D2+D3). Two questions are handed to the verification
  phase: **(Q1)** confirm the `ResourcePressure → Escalate` path for the Normal-priority
  `engineer_spawn` problem (`mod.rs:1444`) is priority/dedup gated so an elevated-but-normal spawn rate
  cannot escalate spuriously; **(Q2)** confirm the OODA guided-retry bound (`no_progress.rs:716`)
  cannot be re-armed every cycle under sustained gaps (no unbounded-spawn path). Prior waves say both
  are benign; these are targeted closes, not open risks.

- **14.8 — Definitive per-token emitter map + the "2×" is a recall count, not a gate count (primary
  emitter dive).** Every token in the composite is minted in exactly one place. The `overseer-obs:`
  prefix and `|`-join come from `observation_signature` — `format!("overseer-obs:{}", keys.join("|"))`
  over **sorted+deduped** `Problem.dedup_key`s (`mod.rs:1068-1073`). Each member key is minted in the
  `classify_signal` signal→problem map (`mod.rs:1237-1373`): `goal:blocked:{goal_id}` (`:1336`),
  `workstream-gap` literal (`:1371`), `resource:engineer_spawn` literal (`:1270`), and the
  recall-driven `sanitize_recalled(signature)` self-key (`:1359`). `observer.rs` only *labels* variants
  for telemetry (`:216`) and `notify.rs` is a *sink* for operator subjects (`:98,204`) — **neither
  emits** the composite. Crucially, the **"2×" is `Signal::RecurringSignature.occurrences`**
  (`signal.rs:455-470`), a tally of recalled episodes sharing one `failure_signature` — **not** a
  `WhisperGate` counter. It reaches 2 because the write-back gate is an in-process, process-local
  `HashMap<String,i64>` with a 900 s window (`guardrails.rs:291-333`, gate built `WhisperGate::new(900,5)`
  at `mod.rs:299`): an identical observation is re-persisted whenever the daemon **restarted**, the
  condition is re-observed **>900 s later**, or a **different instance** wrote it. This pins §14.3's
  mechanism to the exact gate storage line.

- **14.9 — The blocked-WHY ladder is well-formed; its closure hole is *conditional*, and elevated
  `engineer_spawn` is a read-back of the ladder's own spawns (secondary two-loops dive).** The
  remediation ladder (`resolution_for_why`, `no_progress_breaker.rs:384-417`) correctly routes most WHY
  classes to non-blocking outcomes; only `UnclearCriteria/GenuinelyStuck` terminate at
  **Escalate→Blocked** awaiting a human (`:402-410`) — the arm with **no auto-clearing convergence rung**
  (unlike `UpstreamDependency`'s self-clearing `Defer`), which is the static set feeding the `2×`. The
  ladder is **double-gated** (`cycle.rs:582-583`): Gate A `completion_evidence.is_some()` + Gate B
  `no_progress_investigation_enabled()`. In the **default daemon Gate A is satisfied** (wired to
  `GhCliEvidenceSource`, `daemon/mod.rs:455-471`), so the classifier runs — the closure hole is
  **latent, active only** when `SIMARD_COMPLETION_EVIDENCE=off` or a non-daemon path leaves
  `completion_evidence=None` (`client_factory.rs:109`, `daemon/mod.rs:1982`), which skips the whole block
  incl. `reinvestigate_bare_blocked_goals` (`cycle.rs:700-702`). This *refines* the D2 defect: the bare-
  block dead zone is real, but the WHY-gating hole is config-conditional, not unconditional. It also
  fixes the causal direction of the `engineer_spawn` token: the ladder's own `dispatch_spawn_engineer`
  (`cycle.rs:648-681`) **raises `live_engineers`**, so an elevated `EngineerSpawnRate` signal
  (`signal.rs:393-396`) is a **read-back of the very stall the ladder is retrying** — a benign symptom of
  Loop (a), consistent with §14.4's "not a spawn loop."

- **14.10 — The D1 fix needs *no new plumbing*: the provenance it must consult is already in scope at
  the drop site (tertiary minimal-fix dive — sharpens the estimate).** `recall_episodes_ranked` returns
  `Vec<CognitiveEpisode>` (`cognitive_memory/mod.rs:542`); `CognitiveEpisode` carries a **public
  `source_label: String`** (`memory_cognitive.rs:47-53`) populated end-to-end (`library_adapter.rs:559`)
  and set to `"overseer"` on the Overseer's own write-backs (`wiring.rs:952,1088`). At the open seam
  `recall_episodic` (`wiring.rs:1024-1029`) the loop **binds `e` but maps only `e.content`+`e.node_id`,
  discarding `e.source_label`**. So closing D1 requires **no schema change, no new field, no new query** —
  only a single-predicate `skip if e.source_label == OVERSEER_SOURCE_LABEL`, tightening the prior
  "~4 lines" estimate. Landing order is confirmed and extended to **D2 (atomic gate+counter) → D3
  (per-`GapItem.signature` closing rung, honouring INV-GAP-KEY) → D1 (recall-side self-source filter) →
  Step 4 convergence gauges** (telemetry that `goal:blocked:*`/`workstream-gap` counts trend to zero — the
  acceptance test, not a code defect). D1 alone stops the nesting **shape** but not the `2×`
  **recurrence** (the set stays static until D2+D3 converge), so it is correctly ordered last despite
  being the "true self-loop."

- **14.11 — Drift baseline reset to the merge-base, and the *working tree* is also zero-drift (specialist
  re-grounding dive).** The correct "last consolidated code" baseline is `dcf909c5` — the last real code
  commit and `git merge-base HEAD main`; `6e3113bc..HEAD` is seven `docs(investigation)` commits. Beyond
  the committed check, the **working tree** is clean of source: `git diff HEAD -- '*.rs'` is **0 lines**
  (some `src/overseer/*.rs` carry fresh mtimes from checkout/build, but content is byte-identical — mtime
  is not drift), and all staged/untracked changes are **exclusively `ai_working/investigation/*.md`**.
  Whole-branch delta `dcf909c5..HEAD` = **34 files, +6476, 100% docs**. The overseer/stewardship pipeline
  is byte-identical to `6b2bf5e1` (PR #4063), so the D1 self-exclusion filter is provably **unimplemented**
  — committed *and* uncommitted. Confidence: **high** (reproducible git evidence + line-anchored re-read).

**Bottom line (nine-wave consolidated verdict):** unchanged and now re-grounded at HEAD `440e024c`
with **zero code drift** (`git diff dcf909c5..HEAD -- '*.rs'` empty; `git diff HEAD -- '*.rs'` = 0
lines). The `×2` is a **faithful cross-window recurrence count of a genuinely
re-observed near-static problem set** (H1 confirmed; H0 rejected). It persists because two
observe-and-flag loops never close (D2 bare-blocked WHY double-gate; D3 notify-only `WorkstreamCoverage`),
parks in the **dead zone between thresholds 2 and 3** (D2), and the Overseer **re-observes its own
bookkeeping** through a single open seam — provenance stamped on write but erased on read at
`recall_episodic` (D1, `wiring.rs:1024-1029`). The `workstream-gap ↔ engineer_spawn` pairing is a
**co-occurring under-resourced state, not a spawn loop**. Every defect is design-level; **none is a
dedup/storage bug**; **no remediation is merged** — all nine investigation commits are docs-only.

---

## §15 — Ninth-wave net-new findings (HEAD `440e024c`, zero drift)

This wave consolidates three new parallel deep dives, each re-grounded line-by-line against live
`src/` (no doc-to-doc trust):
[`primary_signature_recurrence_VERDICT_HEAD_b9f99879.md`](./primary_signature_recurrence_VERDICT_HEAD_b9f99879.md)
(signature computation/recurrence + `goal:blocked` lifecycle, with oracle suites re-run),
[`secondary_deadzone_and_overaggregation_HEAD_440e024c.md`](./secondary_deadzone_and_overaggregation_HEAD_440e024c.md)
(the `2×→3×` dead-zone geometry + over-aggregation harms), and
[`secondary_gap_and_spawn_HEAD_440e024c.md`](./secondary_gap_and_spawn_HEAD_440e024c.md)
(`workstream-gap` vs `resource:engineer_spawn` detection sources). All three **confirm-not-contradict**
the eight-wave verdict; the net-new content sharpens the D2 dead-zone and the over-aggregation
mechanism.

- **15.1 — The two thresholds are two decoupled lanes, named precisely (sharpens D2).** The visible
  `2×` and the escalation bar of `3` live on **different counters that never meet**:
  - **Lane A — episodic recall:** `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`), fired at
    `signal.rs:462-468`, counts **write-back episodes** whose `failure_signature` string is
    byte-identical. This is the `Signal::RecurringSignature.occurrences` the question string reports.
  - **Lane B — semantic root cause:** `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`),
    gated at `mod.rs:1613` inside `decide_blocked_goal`, counts recalled `PriorOccurrence`s sharing a
    `cause_label`.
  The `2×` is **Lane A**; the escalation gate reads **Lane B**; **they never share a counter.** This is
  the *cross-lane visibility gap* — not a single mis-set threshold, and it is why bumping either
  constant in isolation cannot close the loop.

- **15.2 — NET-NEW: for the composite self-observation the raise-priority rung is *structurally
  unreachable* (deepens the dead-zone verdict).** The intended "raise priority before escalate"
  mechanism is Orient's same-key merge (`mod.rs:1210-1219`, **verified this wave**):
  `problems.iter_mut().find(|p| p.dedup_key == key)` then, for a `RecurringSignature` co-signal,
  `existing.priority = existing.priority.min(priority)`. But the `RecurringSignature` problem's key is
  `sanitize_recalled(signature)` (`mod.rs:1359`, **verified**) — the **whole-cycle composite**
  `overseer-obs:…` — which is **not equal to any per-goal `goal:blocked:{goal_id}` key**
  (`mod.rs:1336`). So the merge predicate `p.dedup_key == key` can **never** match, and the `2×`
  **never raises the priority of any individual blocked goal it is composed of.** The meta-problem
  instead stands alone → `ProblemKind::ProcessHealth` (`mod.rs:1356`) → `LaunchRecipe`, i.e. the one
  cost-bearing convergent edge in the whole flow is aimed at the **meaningless composite blob**, not at
  any real goal. The dead zone is therefore **worse than "no rung between raise and escalate"**: for the
  composite the raise rung is unreachable **by construction** — *priority never raised AND never
  escalated* — so the `2×` exerts **zero** remediation pressure on the actual goals.

- **15.3 — Over-aggregation is *expected* co-occurrence aggregation (no identity loss) but carries two
  real harms (corroborates D1).** `observation_signature` (`mod.rs:1068-1073`) folds the **entire
  cycle's** problem set into one composite; recall counts **episodes by exact composite string**
  (`signal.rs:456-460` builds a `BTreeMap<&str,u32>` keyed on the whole `failure_signature`). Per-goal
  identity is **not** lost (each goal ID survives as its own `|`-token), so this is **not** a
  token-duplication bug — but the whole-cycle granularity produces two harms:
  - **Harm A — detection brittleness / false negatives under churn.** The recall key is a logical
    **AND** of the *entire* membership set. `RecurringSignature` fires only when **two cycles share a
    byte-identical composite** — the whole blocked/gap set unchanged. Any churn (one goal resolves, one
    new goal blocks, a gap opens/closes) mutates the composite → recall resets to 1 → **no**
    `RecurringSignature`, **even for a goal re-blocking every single cycle**. Recurrence is tracked at
    whole-cycle granularity when it should be **per-`dedup_key`**; a chronically-stuck goal in a
    churning environment can evade Lane-A detection indefinitely.
  - **Harm B — composite non-actionability.** When the composite *does* recur to `LaunchRecipe`, the
    `task_description` is the whole blob ("fix goal A AND B AND … AND a coverage gap AND an
    engineer-spawn note") — a *diagnostic aggregate*, not a well-formed recipe brief. The sole
    convergent edge is pointed at something no engineer can execute against.
  Both harms trace to the **same** whole-cycle `observation_signature`, the very mechanism behind D1's
  nesting loop — **same root mechanism, three symptoms** (nesting, brittle detection, non-actionable
  remediation unit).

- **15.4 — `workstream-gap` and `resource:engineer_spawn` are two *independent* detection sources with
  no causal code edge; their pairing is an under-resourced *state* (reconfirms H7/H8).**
  - `workstream-gap` (`detect_workstream_gaps`, `sensor.rs:288-372`) emits a `GapItem` per **uncovered**
    high-priority backlog item (`goal:{id}` / `issue:{repo}#{n}` / `anomaly:{slug}`), **explicitly
    skipping Blocked goals** (`sensor.rs:300-302`, delegated to `goal_health`) — which is exactly the
    active⇄idle oscillation (§14/H7) that puts the *same* entities in both families. Its Decide arm
    `act_flag_workstream_gaps` (`mod.rs:884-946`) is **operator-notify-only** (email + Signal, deduped
    per-gap by `WhisperGate::new(900,200)`) — the **only** High-family Decide arm with **no `launch.rs`
    edge**; a persistently uncovered item re-notifies every window forever. **Root cause = a missing
    convergence rung, not a counting bug** (D3).
  - `resource:engineer_spawn` is **benign passive telemetry**: `Signal::EngineerSpawnRate{live}` when
    `state.live_engineers >= 8` (`signal.rs:351,393-396`) → `ResourcePressure`, `Priority::Normal`
    (`mod.rs:1267-1272`) → `Escalate{reason}` (`mod.rs:1444-1446`); its `{live}` count lives only in
    the summary. Actual spawning lives in the OODA loop (`no_progress.rs` `SpawnEngineer`, bounded to
    one guided retry) — **no unfulfilled-spawn defect at the overseer boundary.**
  - **No code path** connects the two. They co-occur only because both predicates held in one window:
    engineers saturated (≥8) **AND** backlog coverage incomplete. This **unifies the whole signature**:
    `goal:blocked` (idle stuck goals) + `workstream-gap` (active uncovered goals) +
    `resource:engineer_spawn` (no spare executors) are **three symptoms of one resourcing/convergence
    deficit**, not three independent bugs.

- **15.5 — Refined remediation guidance and re-affirmed coupling trap (investigation-only, nothing
  landed).** (a) **Key recurrence per `dedup_key`, not per composite** so a single re-blocking goal
  trips `2×` regardless of cycle-mate churn (fixes Harm A; larger change — one write-back episode per
  problem, or per-key markers). (b) **Add a 2× remediation rung gated on the WHY class, not the raw
  count** — the final `Report` arm is *correct* for a deliberate operator/upstream block
  (`mod.rs:1597-1598`), so a bare count bump would over-escalate benign blocks; route down a resolution
  action only for WHY classes carrying **no** benign explanation, reserving human `EscalateBlockedGoal`
  for `UnclearCriteria`/`GenuinelyStuck`. This rung and the WHY-ungating (D2, config-conditional per
  §14.9) are a **coupled pair**. (c) **Do not point `LaunchRecipe` at the composite** — target a single
  decomposed member or route to per-goal resolution (Harm B). (d) **Coupling trap re-affirmed**
  (`RECONCILIATION_LEDGER`): any Lane-B threshold or accrual change must ship **atomically** with its
  counter or `recurrence >= 3` (`mod.rs:1613`) becomes dead code / latches.

- **15.6 — Zero source drift re-confirmed at HEAD `440e024c`; oracle suites green.** `git diff
  dcf909c5..HEAD -- '*.rs'` is empty and `git diff HEAD -- '*.rs'` = **0 lines** (committed *and*
  working-tree source byte-identical). The two new load-bearing citations were **independently
  re-verified this wave** against live source: the Orient merge predicate + `RecurringSignature`
  priority-raise (`mod.rs:1210-1219`) and the composite `sanitize_recalled(signature)` key
  (`mod.rs:1359`) — confirming §15.2's structural-unreachability by construction. Primary re-ran the
  behavioral oracles at HEAD: `overseer::tests_memory_recall` **32 passed**, `overseer::tests_gap_scan`
  **21 passed**, no-progress/goal-health **77 passed** (incl.
  `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` — parks, does not converge). D1/D2/D3
  remain **unimplemented**, committed and uncommitted.

- **15.7 — Open questions carried to the verification phase.** (Q1) Add a unit test asserting the Orient
  merge at `mod.rs:1211` **never** matches an `overseer-obs:` composite key against a `goal:blocked:`
  key (static reading says never; §15.2). (Q2) Confirm whether the composite `RecurringSignature`'s
  `LaunchRecipe` is actually **admitted by `gate()`** under default autonomy/budget — determines whether
  Harm B is **live vs latent** (still open from prior waves). (Q3) Add a regression test: two cycles with
  a **one-goal-different** blocked set must **NOT** emit `RecurringSignature` (demonstrates Harm A), and
  a per-`dedup_key`-keyed variant **SHOULD** (demonstrates the fix direction). (Q4) Confirm the `Report`
  default arm is reached only for genuinely-benign blocks once a WHY class is wired, so the proposed 2×
  rung would not swallow deliberate operator blocks.

**Ninth-wave delta:** no verdict change. The wave **deepens D2** (the raise-priority rung is
structurally unreachable for the composite, so the `2×` is not merely under-escalated but exerts *zero*
pressure on real goals) and **decomposes the over-aggregation** into two concrete harms (churn-brittle
detection, non-actionable remediation unit) — both tracing to the same whole-cycle `observation_signature`
that drives D1. `workstream-gap ↔ engineer_spawn` is re-confirmed as a **co-occurring under-resourced
state with no causal edge**. Zero code drift; investigation-only; nothing landed.

---

## §16 — Tenth-wave net-new findings (HEAD `f9cefec1`) — the D0 reconciliation-seam root cause, revised whole-loop remediation order, and the `engineer_spawn` reconciliation

Sources folded this wave:
[`primary_signature_recurrence_VERDICT_HEAD_b9f99879.md`](./primary_signature_recurrence_VERDICT_HEAD_b9f99879.md)
(independent line-by-line re-read + oracle re-run; D1/D2/D3 re-affirmed),
[`tertiary_orchestration_synthesis_and_remediation_HEAD_f9cefec1.md`](./tertiary_orchestration_synthesis_and_remediation_HEAD_f9cefec1.md)
(**the new D0 upstream root cause** + whole-loop L0–L3 ordering), and
[`secondary_starvation_coupling_HEAD.md`](./secondary_starvation_coupling_HEAD.md)
(the executor/capacity-layer **coupled-`8`s** refinement of the `engineer_spawn` framing).
All load-bearing citations re-verified against live `src/` at HEAD `f9cefec1`.

**No verdict reversal.** The `×2` remains a *faithful, honest cross-window re-observation of a
near-static blocked/gap set* (H1 CONFIRMED, H0 REJECTED). This wave does two things the prior nine
did not: it names the **upstream reason the anchor is `Blocked` at all** (D0 — previously only
classified as "a real near-static set" without saying *why* it stays static), and it **resolves the
one genuine cross-deep-dive disagreement** about `resource:engineer_spawn`.

- **16.1 — D0 (NEW, load-bearing): the reconciliation seam cannot clear an
  issue-closed-without-linked-merged-PR goal out of `Blocked`.** Prior waves located three defects
  *inside* the Overseer (D1 write-back self-nesting, D2 escalation dead-zone, D3 coverage
  notify-only). All three explain why the signature **persists, is mis-counted, and never
  converges** — none explains why the dominant anchor `goal:blocked:fix-agent-kgpacks-rs-issue-17-…`
  is on the board as `Blocked` **in the first place while its GitHub issue is closed**. The cause is
  upstream, at the only seam that moves a goal out of `Blocked` on completion evidence:
  - **The completion gate is a *conjunction*, not a disjunction.** `CompletionEvidenceGate::evaluate`
    (`goal_curation/completion_gate.rs:394`, missing-evidence variants `PrNotMerged` / `IssueOpen` /
    `NotDeployed` at `:43-47`) marks a goal `Complete` **iff** `pr_merged ∧ issue_closed
    [∧ deployed]`. A **closed issue alone** yields `missing = [PrNotMerged]` → `Blocked`, *not*
    `Complete`. If issue #17 was closed out-of-band (manually / duplicate / wontfix, or by a PR the
    evidence source cannot tie to the goal's `wip_refs`), the goal is pinned to `Blocked` **forever**.
  - **The reconciler is doubly conditional and silent when off.** `sweep_done_goals` runs only inside
    `if let Some(evidence) = &memories.completion_evidence` (`operator_commands_ooda/daemon/mod.rs:1322-1323`),
    and `completion_evidence` is itself `None` unless `completion_evidence_enabled()` (`:455-457`). On
    any deployment where the feature flag is off or the source is absent, the **only** board-draining
    reconciler is a **silent no-op** and *every* blocked goal is permanent — with **no fail-loud**.
  - **Fail-closed on a flaky source compounds it.** Any error from the evidence source returns
    `Blocked{CouldNotVerify}`, so a rate-limited / unauthenticated `gh` re-blocks the goal every cycle
    — a live, near-static blocked set is exactly what the `×2` recall needs.
  - **Verdict:** **D0 is the root of the anchor; D1/D2/D3 are why the anchor's signature recurs, is
    mis-counted, and never converges. Complementary, not competing.** This is the missing upstream
    link the nine prior waves under-weighted.

- **16.2 — Reconciliation of the `resource:engineer_spawn` disagreement between this wave's two
  deep dives (the one genuine cross-report tension).** The tenth-wave **tertiary** calls
  `engineer_spawn` a **benign "false lead"** with **no spawn semaphore** gating goal work (grep of
  `agent_supervisor`/`engineer_loop`/`signal.rs` finds only the `ENGINEER_SPAWN_THRESHOLD = 8`
  *observe* threshold); the tenth-wave **secondary** calls it a **real resource-starvation coupling**.
  Both are correct at different seams — they do **not** contradict, they resolve as follows:
  - **Agreed by both (and by §15.4):** there is **no data-flow *code* edge** from `engineer_spawn`
    (or from a `workstream-gap`) into any spawn/launch path. The composite pairing is a *state*
    co-occurrence, not a producer→consumer edge.
  - **Secondary's refinement is a *state* coupling on a shared constant, not a code edge.** The
    telemetry threshold `ENGINEER_SPAWN_THRESHOLD = 8` (`signal.rs:351,394`) is **the same number** as
    the hard admission cap `max_concurrent_engineers = 8` (`typed_ooda/types.rs:680`), enforced by the
    capability ledger `admit()` which rejects `SpawnEngineer` on
    `concurrent_engineers >= max_concurrent_engineers` with `"engineer concurrency limit reached"`
    (`typed_ooda/ledger.rs:1793-1796`). So at exactly 8 live claims the *same* condition that **mints**
    `resource:engineer_spawn` also **rejects** the only spawn attempt an idle goal makes (the
    no-progress `SpawnEngineer` guided retry). The cap is a **saturation ceiling** (PID-liveness
    counted, self-clears when engineers exit) — **not a deadlock** — but engineer subprocesses have
    **no wall-clock timeout by design**, so a few wedged agents can extend the saturation window.
  - **Tertiary's point stands for *active* gap goals:** `detect_workstream_gaps` never attempts a
    spawn, so for uncovered-but-active goals the cap is irrelevant — the block is the **missing
    convergence rung** (D3), not the cap. The cap only bites *idle* goals whose no-progress breaker
    tries (and is rejected).
  - **Resolved framing (supersedes §15.4's "benign passive telemetry" for precision):**
    `resource:engineer_spawn` is **not a driver of the signature and remains lowest-priority for
    remediation** (do **not** build spawn-capacity controls to chase this stall, and do **not** treat
    it as an escalation trigger on its own — tertiary is right that it is a *passenger* in the
    composite). **But** it is **not pure noise either**: it is an honest **early-warning that the
    concurrency admission cap is being hit**, which is a genuine *secondary* contributor to
    `goal:blocked` persistence **under saturation** (secondary is right that the cap is real and
    cap-rejects idle-goal spawns). Net: **peripheral to the fix, not fictitious.**

- **16.3 — Revised whole-loop remediation order (L0→L1→L2→L3), superseding the prior D2→D3→D1
  landing order now that D0 is known.** Prior waves ordered fixes by *dependency safety within the
  Overseer* (D2 counter → D3 rung → D1 hygiene). With D0 identified as the upstream anchor cause, the
  **leverage-ranked** order is:
  - **L0 (fixes D0; highest leverage).** Reconcile issue-closed goals out of `Blocked`: either treat
    `issue_closed && !self_affecting` as `Complete` at `completion_gate.rs:394-438` (issue closure is
    the definition of done for a "fix issue N" goal), **or** add a `sweep_stale_blocked` pass that
    tombstones a goal blocked > N cycles whose issue is closed. **And fail loud** when
    `completion_evidence` is `None`/disabled (`daemon/mod.rs:1322`) instead of silently skipping
    reconciliation. **L0 alone likely collapses the observed `×2` to nothing for the kgpacks anchors.**
  - **L1 (fixes D3).** Give `WorkstreamCoverage` a real closing edge (`LaunchRecipe`/`FileIssue`) in
    Decide (`mod.rs:1534-1543`), gated behind a **cross-window recurrence ledger** and keyed on a
    **per-gap identity** replacing the opaque constant `"workstream-gap"` dedup_key (`mod.rs:1371`;
    `INV-GAP-KEY`) so the ledger can tell *which* gap recurs.
  - **L2 (fixes D2).** Close the 2↔3 escalation dead-zone with a recurrence-aware rung *between*
    Lane-A (`RecurringSignature` @ 2) and Lane-B (escalation @ 3) — the two lanes share no counter
    (**now proven by the new `tests_root_cause.rs` test at `f9cefec1`**). Route the rung down a
    resolution action **only for WHY classes carrying no benign explanation** (per §15.5), so a bare
    count bump does not over-escalate deliberate operator blocks. Ship any Lane-B accrual change
    **atomically with its counter** or `recurrence >= 3` (`mod.rs:1613`) latches / becomes dead code
    (the re-affirmed coupling trap, `RECONCILIATION_LEDGER`).
  - **L3 (fixes D1; hygiene, lowest urgency).** Exclude recall-derived `RecurringSignature`
    (`ProblemKind::ProcessHealth`) problems from the write-back set (`mod.rs:534-563`) so the
    `overseer-obs:…|overseer-obs:…` self-nesting stops. Cosmetic-plus-bounded; does **not** by itself
    stop the loop (D0/D3 do).
  - **Executor-side (from secondary, orthogonal to L0–L3, lower priority):** decouple
    `ENGINEER_SPAWN_THRESHOLD` from the hard cap or make `engineer_spawn` escalate **only when it
    co-occurs with unmet `workstream-gap`s**; consider prioritized admission/preemption for p1/p2
    gap-goals under saturation; bound engineer slot-hold time via progress-signal liveness (respecting
    the PR#1988/#1989 no-SIGKILL constraint).

- **16.4 — Source-drift & empirical state at HEAD `f9cefec1`.** Unlike waves 1–9 (docs-only), this
  wave added **one test file**: `src/overseer/tests_root_cause.rs` (commit `f9cefec1`,
  `git diff 440e024c..f9cefec1 -- '*.rs'` = that file only; working tree source-clean). **No
  production `.rs` changed** and `git diff 6b2bf5e1..HEAD -- src/overseer src/stewardship` shows only
  that test — so every prior source citation still holds and **no remediation has landed** (D0/D1/D2/D3
  all live). The added test **confirms empirically** that Lane-A `RecurringSignature` does not feed
  Lane-B recurrence — i.e. D2's 2↔3 dead-zone is a *verified* property, not an inference. Oracle
  suites remain green as re-run by primary: `overseer::tests_memory_recall` **32 passed**,
  `overseer::tests_gap_scan` **21 passed**, no-progress/goal-health **77 passed** (incl.
  `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` — parks, does not converge).

- **16.5 — Open questions carried forward (from the tenth-wave secondary; verification-phase).**
  (Q1) Confirm `count_live_engineer_claims` (`ooda_brain/context.rs`) and the ledger snapshot's
  `concurrent_engineers` are computed from the **same** claim source, so the emit-threshold and the
  reject-threshold genuinely fire on the same count of 8 (E1 hinges on this). (Q2) Confirm no
  production config raises `max_concurrent_engineers` above `ENGINEER_SPAWN_THRESHOLD`, which would
  decouple the two `8`s. (Q3) Confirm a cap-rejected no-progress `SpawnEngineer` retry is **retryable
  next window**, not counted as an exhausted retry that pushes the goal to permanent bare-block.
  (Q4) Confirm `memories.completion_evidence` is actually `Some`/enabled in the running daemon build
  — if it is `None`/disabled, **D0 is not just the anchor's cause but a total reconciliation outage**,
  and L0's fail-loud is the single most urgent change.

**Tenth-wave delta:** verdict unchanged (honest `×2`, real re-observation). The wave **adds the
upstream root cause D0** (issue-closed-without-merged-PR pins the anchor `Blocked`; reconciler is a
silent, doubly-conditional no-op when disabled), **re-orders remediation to L0→L1→L2→L3** by leverage,
and **reconciles the `engineer_spawn` framing** to "peripheral early-warning of a real admission cap,
not a driver and not fictitious." First non-docs-only wave — a single test locks in the D2 dead-zone
property. Nothing else landed; D0–D3 remain live.

---

## §17 — Tenth-wave (continued): the signal-vs-recording-defect adjudication, convergence-rung refinements, and full per-hypothesis re-verification at HEAD `f9cefec1`

Three additional parallel deep dives from the same tenth wave (all HEAD `f9cefec1`), folded here:
[`tertiary_lane_isolation_signal_vs_defect_VERDICT_HEAD_f9cefec1.md`](./tertiary_lane_isolation_signal_vs_defect_VERDICT_HEAD_f9cefec1.md)
(the explicit **"intended signal vs recording defect"** adjudication + lane-isolation re-run),
[`secondary_reemission_and_convergence_HEAD_f9cefec1.md`](./secondary_reemission_and_convergence_HEAD_f9cefec1.md)
(convergence-rung asymmetry + **two net-new refinements**: a doc/impl mismatch on the gap rung and a
**correction to the "double-gated WHY ladder" claim**), and
[`verification_results_HEAD_f9cefec1.md`](./verification_results_HEAD_f9cefec1.md)
(re-execution of a practical test for **every** hypothesis H0–H8 on the latest tree). All citations
re-verified against live `src/` at HEAD `f9cefec1`. **No verdict reversal**; this cohort *sharpens the
adjudication, corrects one imprecise prior claim, and re-pins the empirical baseline.*

- **17.1 — The `×2` is *intended signal*, not a *recording defect* (NEW explicit adjudication).** Prior
  waves classified the `×2` as an "honest re-observation" but never squarely answered the user's implicit
  question — *is this a bug in how recurrence is recorded?* The tenth-wave tertiary adjudicates it
  directly and answers **no**:
  - The full observe→signature→count loop is deterministic and honest end-to-end: OBSERVE is a **pure
    projection** of `GoalProgress::Blocked` (`sensor.rs:204-221`, no fabrication); the composite
    `observation_signature` is a stable **set hash** (`sort→dedup→"overseer-obs:{join('|')}"`,
    `mod.rs:1068-1073`); Lane-A `occurrences` is a faithful count of recalled episodes carrying the same
    `failure_signature`, and recall reads **live facts only** (`include_superseded:false`,
    `library_adapter.rs:763,773,830`) so there is **no storage amplification / replay**. A truthful
    sensor reporting a genuinely-static blocked world **is signal by definition** — silencing the count
    would itself be the defect.
  - **Where a recording concern genuinely exists (and its boundary):** only in **Lane-B durability**, not
    in the `×2` the user saw. `record_occurrence` writes via non-idempotent `store_fact` (`mod.rs:1034`),
    an append-only ratchet; the correct hardening is a **caller-key upsert carrying an `occurrence_count`
    in the fact content** (escalation reading that field), **not** the naïve
    `store_fact_with_caller_key` swap floated in §6.2b — which would collapse recall to **1 forever** and
    make the `≥3` escalation rung **dead code** (re-confirms the `RECONCILIATION_LEDGER` trap). This
    cohort **re-affirms** that trap rather than re-deriving it.
  - **The 2↔3 dead-zone is a *design consequence of correct lane isolation*, not a bug in it.** Because
    Lane-A (`RecurringSignature @ 2`) and Lane-B (`recurrence @ 3`) are isolated by construction, a
    signature stably re-observed at Lane-A `×2` **raises priority in `orient` but never reaches Lane-B's
    `≥3`** unless Lane-B independently accumulates ≥3 cause-matched `PriorOccurrence`s. The remedy is a
    rung that *acts on Lane-A `×2`* (L1/L2), **not** merging the lanes or touching the counter.
  - **Verdict (this cohort):** do **not** touch the counter or the lane isolation — both are correct and
    now guarded. This *sharpens* §16.1–§16.3: the honest-`×2` framing now carries an explicit
    signal-vs-defect ruling and a precise recording-concern boundary (Lane-B durability only).

- **17.2 — Lane isolation empirically re-run and reconfirmed (not just inferred).** The tenth-wave
  tertiary re-executed `cargo test --lib overseer::tests_root_cause` → **21 passed, 0 failed** at HEAD,
  including both directional guards: `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
  (`tests_root_cause.rs:490-529` — a deliberately loud Lane-A `occurrences: 10` yields
  `problem.why.recurrence == 0` and the goal **self-heals** via `UnblockGoal`) and the converse
  `lane_b_escalates_without_any_lane_a_signal` (`:535-573`). `git show --stat f9cefec1` confirms this is a
  **net-new verification test (+99 lines in `tests_root_cause.rs`), not a behavior change** — the
  isolation was always true; `f9cefec1` pins it against regression. This is the empirical anchor under
  §16.4's "D2 dead-zone is now a *verified* property."

- **17.3 — Convergence-rung asymmetry re-grounded, plus two net-new refinements (F2, F5).** The
  tenth-wave secondary re-confirms the core defect — Simard has exactly **one** durable convergence
  mechanism (`stewardship::process_orchestrator_run` → `route_failure` → search-or-file a **deduplicated**
  GitHub issue, `stewardship/mod.rs:70-115`, `routing.rs:39`), reached **only** for orchestrator run
  failures; `workstream-gap` (→ `FlagWorkstreamGaps`, notify-only) and `resource:engineer_spawn`
  (→ `Escalate`/`Report`, notify-only) are **routed away** from it and therefore re-emit every dedup
  window forever. Two refinements are net-new to the consolidated record:
  - **F2 (NEW, doc/impl mismatch):** `observer.rs:117-119` asserts gaps are *"acted on … (notify +
    **deduped file**)"* and `routing.rs:11-15` provisions a default-repo fallback for the *"Overseer's
    **workstream-gap briefs**"* — **but neither is wired.** No code constructs an
    `OrchestratorRunSummary`/`Brief` from a `GapItem`; `act_flag_workstream_gaps` only notifies. The
    comments describe a **file rung that does not exist** — concrete evidence the convergence rung was
    *intended and dropped*, not deliberately omitted. This upgrades D3 from "missing by omission" to
    "missing against a documented-but-unwired intent," strengthening the case for L1.
  - **F5 (CORRECTION to a prior claim):** earlier waves called the WHY resolution ladder *"double-gated
    off"* (`cycle.rs:582-702`). **Precision correction, verified at HEAD:** Gate 2
    (`no_progress_investigation_enabled()`, `cycle.rs:583`) is **default-TRUE**
    (`no_progress.rs:203-207`, `unwrap_or(true)`; only `SIMARD_NO_PROGRESS_INVESTIGATE=off` disables it).
    The **operative** gate is Gate 1 — `if let Some(source) = &memories.completion_evidence`
    (`cycle.rs:582`), tied to `SIMARD_PROGRESS_EVIDENCE`. So the ladder is **not** "off by two default-off
    flags"; when Gate 1's evidence source is wired, the ladder runs and resolves via `resolution_for_why`
    (`no_progress_breaker.rs:384-414`), proven by the 77 investigation/reinvestigation tests. **This
    reframes the production risk as a *mis-provisioned evidence source*, not a hard-off reasoner** — and
    it is the *same* `completion_evidence` wiring that gates D0's reconciler (§16.1), so **Gate-1 wiring
    is a single shared leverage point for both the WHY ladder and D0 reconciliation** (raising the
    priority of §16.5-Q4 / the L0 fail-loud).

- **17.4 — Full per-hypothesis re-verification on the latest tree (empirical baseline re-pinned).** The
  tenth-wave verification dive re-executed a practical test for **every** hypothesis H0–H8 at HEAD
  `f9cefec1`: the **full overseer suite = 361 passed, 0 failed**, plus **22 targeted discriminating
  tests green** (2 whisper-gate + 15 named + 5 Lane-A/B decoupling). All seven source invariants the
  hypotheses depend on are **unchanged** at HEAD (`RECURRING_SIGNATURE_THRESHOLD=2` `signal.rs:362`;
  `RECURRENCE_ESCALATION_THRESHOLD=3` `root_cause.rs:33`; non-deduping `store_fact` `mod.rs:1034`;
  bare-block renderer split `no_progress_breaker.rs:123/141`; `WhisperGate::new(900,5)` `mod.rs:299`;
  completion done-gate conjunction `completion_gate.rs:394-438`). Verdict matrix reproduced unchanged:
  **H0 REJECTED** (dedup/storage/replay artifact excluded), **H1 CONFIRMED** (real cross-window
  re-observation), **H2–H8 SUPPORTED** (bare-park no-WHY, coverage notify-only, bounded write-back
  feedback, 2↔3 dead-zone, non-idempotent-but-non-causal counters, blocked↔gap one-problem-two-views,
  three-families-one-under-throughput). The absolute overseer count drifts a few tests across waves
  (359→360→**361**) as the suite grows; the invariant — **0 failures, every discriminating test green** —
  holds. This is the empirical floor under §16's source-drift claim: no production `.rs` changed, every
  citation still resolves, no remediation landed.

**§17 delta:** verdict still unchanged. This cohort **(a)** issues the explicit *signal-vs-recording-
defect* ruling — the `×2` is intended signal; the only genuine recording concern is Lane-B durability,
not the count; **(b)** re-runs and reconfirms lane isolation empirically (21/21); **(c)** adds F2 (the
gap file-rung is documented-but-unwired) and the F5 **correction** (the WHY ladder is default-on at Gate
2; only Gate-1 evidence wiring gates it — the same wiring that gates D0, making it a shared leverage
point); and **(d)** re-pins the empirical baseline at 361/0 with all invariants intact. No new defects;
D0–D3 remain live and unremediated; remediation order L0→L1→L2→L3 (§16.3) stands, with L0's fail-loud
now doubly-motivated by the shared Gate-1 wiring.

---

## §18 — Eleventh-wave net-new findings (HEAD `f1db90f4`, zero source drift) — the two-and-only-two re-persistence conditions, the type-grounded WhisperGate real-vs-bug ruling, the membership-drift precision defect, and the gap↔spawn non-coupling verdict

Five parallel deep dives at HEAD `f1db90f4`, folded here. Two PRIMARY dives adjudicate the
signature-assembly + idempotency gate from complementary angles
([`primary_emitters_and_idempotency_gate_HEAD_f1db90f4.md`](./primary_emitters_and_idempotency_gate_HEAD_f1db90f4.md),
[`primary_signature_assembly_and_2x_verdict_HEAD_f1db90f4.md`](./primary_signature_assembly_and_2x_verdict_HEAD_f1db90f4.md));
one VERIFICATION dive re-executes a practical test for every hypothesis on the latest tree
([`verification_results_HEAD_f1db90f4.md`](./verification_results_HEAD_f1db90f4.md)); and two TERTIARY
(architect) dives settle the `workstream-gap`↔`resource:engineer_spawn` coupling question and
characterise signature idempotency under membership drift
([`tertiary_gap_spawn_coupling_HEAD_f1db90f4.md`](./tertiary_gap_spawn_coupling_HEAD_f1db90f4.md),
[`tertiary_lane_isolation_and_membership_drift_HEAD_f1db90f4.md`](./tertiary_lane_isolation_and_membership_drift_HEAD_f1db90f4.md)).
`f1db90f4` is a docs-only commit over `f9cefec1` (`git diff --stat f9cefec1 HEAD -- src/` → **empty**);
all prior source citations hold verbatim. **No verdict reversal.** This cohort *pins the exact
re-persistence conditions, grounds the real-vs-bug ruling in the gate's own type, adds one net-new
precision defect (membership-drift signature forking), and closes out the gap↔spawn coupling question.*

- **18.1 — The two — and only two — conditions that re-persist an identical signature (NEW precise
  enumeration).** The first PRIMARY dive nails down exactly when a second identical `overseer-obs:…`
  episode appears, reducing the space to two mutually-exclusive, both-honest causes:
  1. **`> 900 s` window expiry (same process).** The near-static problem set is re-observed on a tick
     ≥ 900 s after the last store; `now - last ≥ window` ⇒ `peek` falls through to `Deliver` ⇒ a second
     store with the identical key (`guardrails.rs:313-317`). Honest cross-window re-observation.
  2. **Daemon restart.** `WhisperGate.last_delivered` is a process-local in-memory `HashMap` with **no
     persistence** (`guardrails.rs:294`); `Overseer::new` reconstructs a fresh `WhisperGate::new(900,5)`
     (`mod.rs:299`), wiping all dedup state, so the next tick observing the same set sees an empty map ⇒
     `Deliver` ⇒ re-store even if `< 900 s` of wall-clock elapsed across the restart boundary.
  Both yield exactly-2× (and N× over N windows/restarts). **From the signature alone the two causes are
  indistinguishable** — no restart/window timestamps are stamped on the episode. This is the sharpest
  statement yet of §16/§17's "honest re-observation": the idempotency guarantee is *intentionally scoped
  to one process's 900 s window*, and these are the two — and only two — ways past it.

- **18.2 — Type-grounded real-vs-bug ruling: the `2×` is real signal, not a WhisperGate bug (NEW
  adjudication angle).** The second PRIMARY dive adjudicates the same `2×` **from the gate's own type**
  rather than from the observe-loop: `last_delivered: HashMap<String,i64>` (`guardrails.rs:294`) yields
  three structural properties directly from the type — **process-local** (reset empty on every
  `WhisperGate::new`), **windowed-not-permanent** (`now - last < window` lapses by design), and
  **commit-after-success only** (`mod.rs:548-557`, a failed write never suppresses a later one). The
  gate's contract, per its own doc-comment (`mod.rs:520-523`), is *"record a persistent condition at
  most once per window"* — a **per-tick anti-flood, NOT a global once-only ledger**. A real-vs-bug
  decision matrix enumerates every producer of the second episode (restart / `>900 s` lapse / distinct
  process / storage replay) and finds **every** path that yields `2×` is a genuine second observation of
  a still-true condition; the only path the gate must stop — *within-window per-tick* duplication — it
  stops correctly. **Verdict: no gate defect behind the `2×`.** This independently reproduces §17.1's
  signal-vs-recording-defect ruling from the WhisperGate/`last_delivered` angle, with no contradiction.

- **18.3 — NET-NEW precision defect: `observation_signature` is a membership-sensitive set-hash, and
  `resource:engineer_spawn` drift forks Lane-A recurrence identity.** The membership-drift TERTIARY dive
  characterises signature idempotency *precisely* — a genuinely new refinement:
  - `observation_signature` (`mod.rs:1068-1074`) is **idempotent under ordering** (`sort_unstable`) and
    **under duplication** (`dedup`), but **NOT idempotent under membership drift**: it set-hashes the
    *entire tick's problem set*, so adding/removing any one incidental member changes the string identity.
  - `resource:engineer_spawn` (`dedup_key="resource:engineer_spawn"`, fired at `ENGINEER_SPAWN_THRESHOLD=8`,
    `signal.rs:351`) is benign transient telemetry uncorrelated with the blocked cluster; as spawn load
    crosses 8 it toggles in/out of the set, forking the composite hash `S` ↔ `S'`. Two consequences on the
    self-fed **Lane-A** loop: **(a) write-back dedup defeated** — `S` and `S'` are distinct `write_back_gate`
    keys, so the 900 s window suppresses neither and **both persist** as near-duplicate episodes; **(b)
    Lane-A bucket fragmentation / undercount** — `signals_from` counts by exact `failure_signature`, so
    `S`- and `S'`-episodes land in different buckets and each must independently reach `≥2`, *splitting*
    one static cluster's recurrence and holding the visible count at a low `×2`. This membership-drift
    heterogeneity (variants with/without `resource:engineer_spawn` and `workstream-gap`, single-member up to
    the full ~14-goal cluster) is **exactly the fingerprint of the observed signature blob** and explains
    why the count sits at `×2` despite a persistently-blocked world.
  - **Quarantine holds:** Lane-B keys recurrence on the *per-problem* `dedup_key`
    (`recall_occurrences(&problem.dedup_key)`, filter `o.signature == dedup_key`, `mod.rs:456,972-997`), so
    it is **structurally immune** to composite membership drift — `resource:engineer_spawn` toggling can
    never reset, inflate, or deflate a blocked goal's Lane-B `recurrence`. **Adjudication: a benign-but-
    latent Lane-A *precision* defect, not a correctness defect in escalation.** It exposes the root design
    smell that Lane-A recurrence identity is simultaneously **over-aggregated** (many goals share one hash;
    the coverage `dedup_key` is the constant `"workstream-gap"` — cannot tell 2 gaps from 20) **and
    identity-fragile** (incidental co-members fork it) — the two faces of keying recurrence on the *whole
    tick-set* instead of the *condition*. Closing hardening (diagnosis only): key advisory recurrence on
    each *individual* `dedup_key` recalled ≥2 (matching Lane-B's granularity, curing both faces), **or**
    exclude volatile `resource:*` telemetry from `observation_signature`. Do **not** merge lanes, silence
    the `×2`, or add spawn controls.

- **18.4 — `workstream-gap` ↔ `resource:engineer_spawn` are INDEPENDENT aggregation artifacts, not a
  causal chain (NEW explicit coupling verdict).** The gap↔spawn TERTIARY dive settles the coupling
  question that the interleaved blob (`…workstream-gap|…|resource:engineer_spawn|workstream-gap…`) invites.
  The two share **no** producer input, `ProblemKind`, `dedup_key`, root cause, intervention, or dedup gate
  (full trace table): spawn is `live_engineers >= 8` → `EngineerSpawnRate` → `ResourcePressure`/Normal →
  `engineer-spawn-storm` → `Escalate` (notify-only); gap is `!workstream_gaps.is_empty()` →
  `WorkstreamGap` → `WorkstreamCoverage`/High → `important-work-with-no-active-workstream` →
  `FlagWorkstreamGaps` (notify-only). **No path** lets the engineer count feed gap detection or vice-versa;
  the interleaving is a **flattened recall concatenation of distinct persisted `dedup_key`s across ticks**,
  not a mechanized loop. `goal:blocked:…issue-17-ws2…-7f5afcca` is a **third, deliberately isolated lane**
  (`GoalBlocked`→`GoalHygiene`), and blocked goals are explicitly excluded from the gap scan
  (`detect_workstream_gaps` `continue`s on `GoalProgress::Blocked(_)`, `sensor.rs:300-302`, pinned by
  `delegates_blocked_goals_to_goal_health_and_never_reflags_them` `tests_gap_scan.rs:413`); the `-7f5afcca`
  suffix is a stable content-hash of one persisted record recalled, not genuine re-duplication. The **only**
  real coupling is *semantic, at the operator level*: the `engineer-spawn-storm` cause text itself reads
  *"a fan-out storm OR stuck workstreams"*, so ≥8 live engineers **and** blocked/uncovered high-value goals
  is a plausible operational story surfaced as **two separate escalations by design**, not a code loop.
  **Remediation: NONE trivial-and-safe** — coupling the lanes or suppressing recall would invent a fix for
  non-broken behavior and risk regressing the pinned isolation guarantees; an advisory correlation note in
  the escalation text is a product change, out of scope.

- **18.5 — Full per-hypothesis re-verification re-pinned at HEAD `f1db90f4` (empirical baseline).** The
  verification dive re-executed a practical test for **every** hypothesis H0–H8 on the current tree: the
  **full overseer suite = 361 passed, 0 failed** (7960 filtered), plus **22 targeted discriminating tests
  green** (4 H0 whisper/write-back probes + 3 H1 recall + 2 H2 reinvestigation + 4 H3 gap-scan + 2 H5 lane
  + 3 H6 root-cause + 4 H7 gap/goal-health). Both PRIMARY dives independently reran the gate/recall suites
  (`overseer::tests_whisper::whisper_gate` → **2 passed**; `overseer::tests_memory_recall` → **32 passed,
  0 failed**, incl. `write_back_is_deduplicated_within_window`,
  `whisper_gate_suppresses_an_identical_whisper_within_the_window` @0/300/899 Suppress → **Deliver@901**,
  and `recurring_signature_emitted_when_two_episodes_share_signature`). All six source invariants unchanged
  from `f9cefec1` (`RECURRING_SIGNATURE_THRESHOLD=2` `signal.rs:362`; `RECURRENCE_ESCALATION_THRESHOLD=3`
  `root_cause.rs:33`; non-deduping `store_fact` `mod.rs:1034`; `WhisperGate::new(900,5)` `mod.rs:299`;
  blocked-goal gate `WhisperGate::new(900,20)` `mod.rs:292`; gap gate `WhisperGate::new(900,200)`
  `mod.rs:304`). Verdict matrix reproduced unchanged: **H0 REJECTED** (dedup/storage/replay artifact
  excluded), **H1 CONFIRMED** (real cross-window re-observation), **H2–H8 SUPPORTED**.

**§18 delta:** verdict still unchanged. This cohort **(a)** enumerates the *two-and-only-two*
re-persistence conditions (`>900 s` window expiry vs. daemon restart) and pins them as indistinguishable
from the signature alone; **(b)** grounds the real-vs-bug ruling in the WhisperGate's own type
(`process-local` + `windowed` + `commit-after-success`), reproducing §17.1 from the gate angle;
**(c)** adds the **net-new membership-drift precision defect** — `observation_signature` is a set-hash
that `resource:engineer_spawn` drift forks, defeating write-back dedup and fragmenting/undercounting the
Lane-A `×2`, while Lane-B stays per-`dedup_key`-immune; **(d)** issues the explicit **gap↔spawn
non-coupling** verdict (independent aggregation artifacts, no trivial fix warranted); and **(e)** re-pins
the empirical baseline at **361/0** with all invariants intact and no production `.rs` changed. Both
PRIMARY suggested hardenings converge with prior waves and add one new observability idea — **stamp
episode metadata with a window/epoch or restart-id** so a future reader can attribute a `2×` to window
vs. restart (removing the §18.1 "indistinguishable" gap) — and **filter self-provenance on recall** (skip
episodes whose `failure_signature` starts with `overseer-obs:`) so the Overseer never counts its own
write-back. D0–D3 remain live and unremediated; remediation order L0→L1→L2→L3 (§16.3) stands.

---

## §19 — Twelfth-wave net-new findings (HEAD `bbddd23a`, zero non-test source drift) — three parallel dives: the D2 dead-zone is a *total* structural latch (tertiary), the complete emitter map + four `WhisperGate` configs (primary), and the decisive "doubling is D1 nesting, impossible from duplication" proof (secondary)

**Three parallel deep dives** at HEAD `bbddd23a`, folded here — architect/tertiary
([`tertiary_architecture_TWO_LOOPS_AND_DEADZONE_HEAD_bbddd23a.md`](./tertiary_architecture_TWO_LOOPS_AND_DEADZONE_HEAD_bbddd23a.md)),
primary ([`primary_signature_assembly_emitter_and_2x_deepdive.md`](./primary_signature_assembly_emitter_and_2x_deepdive.md)),
and secondary ([`secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a.md`](./secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a.md)).
Drift re-check: since the `5a85317b` tertiary pin only `src/overseer/tests_root_cause.rs` (+99 test
lines) changed — **all non-test source is byte-identical**, so every load-bearing line number below is
exact at HEAD (independently re-read per dive; every citation verifies). **No verdict reversal; no
production `.rs` changed; no remediation landed.** This wave *upgrades* the D2 diagnosis from
probabilistic to total (§19.1), maps all three non-closing loops in one table (§19.2), adds the
two-closing-seams boundary insight (§19.3), names the missing middle remediation tier (§19.4),
reconfirms `engineer_spawn` benign while flagging the Lane-B counter inconsistency (§19.5), pins the
complete signature-assembly **emitter map** and the **four in-memory gate configs** (§19.6), and proves
the observed token-doubling is a **positive fingerprint of D1 nesting** — impossible from true
duplication — with a full load-bearing-vs-benign **token taxonomy** (§19.7–§19.8).

- **19.1 — NET-NEW hardening: the D2 dead-zone latch is a HARD exclusion, not a probabilistic slowdown.**
  Prior waves framed D2 as "ACT is gated shut, so `record_occurrence` rarely runs." This dive grounds the
  stronger, exact claim at the `Decide × outcome` intersection: a plain blocked goal (not
  perpetual+no-progress, not needs_review, recurrence < 3) falls to `Intervention::Report`
  (`mod.rs:1630`) → `ActOutcome::Reported` (`mod.rs:658`), and **`Reported ∉ outcome_records_occurrence`**
  (`wiring.rs:612-627` — the set is exactly `Launched | Merged | Deployed | IssueFiled | Escalated |
  Whispered | GoalUnblocked | GoalEscalated | ConflictResolved | GoalTransferred | Audited`;
  **verified this wave**). The Act loop only calls `record_occurrence` when
  `outcome_records_occurrence(&outcome)` is true (`wiring.rs:276-280`), so a sub-threshold blocked goal
  records **exactly zero** Lane-B occurrences → `recall_occurrences(dedup_key)` stays empty →
  `recurrence` stays **pinned at 0** (`root_cause.rs:79-82`) → `decide_blocked_goal`'s `>= 3` escalation
  rung (`mod.rs:1613`) is **unreachable by construction**, forever. The circular dependency is the latch:
  the counter that unlocks escalation can only advance through an outcome (`Escalated`/`GoalEscalated`/
  `GoalUnblocked`) **itself already gated behind having accrued that counter**. **The dead zone is total,
  not probabilistic** — Lane B is not "slow to reach 3," it is pinned at 0 for precisely the blocked-goal
  class the escalation rung was designed to catch.

- **19.2 — The three non-closing observe-and-flag loops (unified L1/L2/L3 map).** The composite persists
  because three Decide arms terminate without a closing edge:

  | # | Loop | Decide arm | Act outcome | Closing edge? |
  |---|---|---|---|---|
  | **L1** | Blocked-goal WHY gate | `decide_blocked_goal` → `Report` (sub-threshold, `mod.rs:1630`) | `Reported` | **None** — re-emits `goal:blocked:<id>` every cycle; Lane B pinned at 0 (§19.1) |
  | **L2** | Workstream-gap launch gap | `WorkstreamCoverage` → `FlagWorkstreamGaps` (`mod.rs:1534-1543`) | `WorkstreamGapsFlagged` (notify-only, `mod.rs:884-948`) | **None** — no `launch.rs` edge, no issue filed |
  | **L3** | Engineer-spawn pressure | `ResourcePressure` → `Escalate` (`mod.rs:1444-1446`) | `Escalated` (bare no-op, `mod.rs:663`) | **None**, but **benign** — transient resource state self-resolves |

  This is the concise structural restatement of root causes A (§1) and B (§2): L1 and L2 are the two
  genuine non-closing loops; L3 is technically a third but structurally benign (nothing to "close").

- **19.3 — Two "closing seams," neither reached by L1/L2 (component-boundary insight).** Only two seams
  can actually close a problem: the `RecipeLauncher` (`launch.rs` — `smart_orchestrator_args` +
  `SmartOrchestratorLauncher`) and the `IssueFiler`. Only `LaunchRecipe`-routed problems
  (`ProcessHealth`/`CrossCutting`/`StepFailure`, `mod.rs:1429-1435`, `1565-1579`) cross a closing seam
  **and** feed Lane B (`Launched ∈ outcome_records_occurrence`). `WorkstreamCoverage` is the **only**
  High-priority coverage problem with **no edge into either seam**; L1 (sub-threshold) dead-ends at
  `Reported`. Auditing which Decide arms reach these two seams exposes every non-closing loop in one pass.

- **19.4 — The named missing tier: the "recurrence-2 closing rung."** The two decoupled recurrence lanes
  create a middle gap with no remediation rung: at **count 1** nothing fires (correct — noise); at
  **count 2** Lane A raises a **High-priority** `RecurringSignature` (`signal.rs:463`, admitted
  `mod.rs:1353-1363`) but the priority bump is **inert** — Decide routes `GoalHygiene`/`WorkstreamCoverage`
  purely by `ProblemKind`, never reading `problem.priority` (`mod.rs:1447`, `1534`), so L1 still `Report`s
  and L2 still notify-only flags; at **count 3** Lane B escalation *would* be eligible but is unreachable
  for L1 (§19.1/§1.2). So a problem sits "recognized as recurring, still uncovered" indefinitely — this is
  the count-2 signal the user observed ("seen 2×"). **Missing architectural element (named, not built):** a
  middle tier that, on Lane-A recognition (count ≥ 2), converts a non-closing flag into a closing action —
  e.g. a `WorkstreamCoverage → LaunchRecipe`/`FileIssue` edge, and/or a Lane-A-driven bump that lets a
  blocked goal escalate without waiting on the starved Lane-B counter (equivalently: add `Reported` to
  `outcome_records_occurrence` so Lane B can accrue). This is the **"recurrence-2 closing rung."**

- **19.5 — `resource:engineer_spawn` reconfirmed benign, plus a NET-NEW counter-inconsistency flag.**
  `EngineerSpawnRate { live }` → `ResourcePressure`/Normal → fixed key `"resource:engineer_spawn"`
  (`mod.rs:1267-1272`) → `Escalate` → `Escalated` no-op. Its appearance/disappearance across the corpus is
  **membership drift** of the per-cycle problem set (enters only on ticks where live engineers crossed the
  threshold), **not a new defect** — consistent with §11/§15/§18. **New honest asymmetry flagged (not a
  bug in scope):** `Escalated` **is** in `outcome_records_occurrence` (`wiring.rs:619`), so
  `resource:engineer_spawn` *does* accrue Lane-B occurrences — the exact opposite of the L1 `Reported`
  starvation (§19.1). The two paths treat the recurrence counter **inconsistently**: the benign transient
  telemetry accrues occurrences it will never need, while the genuinely-stuck blocked goal accrues none it
  desperately needs.

- **19.6 — NET-NEW: the complete emitter map + the four in-memory `WhisperGate` configs (primary dive).**
  The primary deep dive pins the full end-to-end signature-assembly emitter chain to exact loci at HEAD,
  several of which prior waves cited only piecemeal:

  | Step | Locus @ HEAD | Emits |
  |---|---|---|
  | Signature assembly (`overseer-obs:` emitter) | `mod.rs:1068-1073` | `format!("overseer-obs:{}", sorted_deduped_keys.join("\|"))` |
  | Constituent `goal:blocked:<id>` | `mod.rs:1336` | `Signal::GoalBlocked` dedup_key |
  | Constituent `workstream-gap` | `mod.rs:1371` | `Signal::WorkstreamGap` fixed literal |
  | Constituent `resource:engineer_spawn` | `capabilities.rs:562` (recall keyword) / `mod.rs:1270` (key) | `Signal::EngineerSpawnRate` |
  | Human `recurring signature seen N×…` string | `mod.rs:1359-1362` | `sanitize_recalled(format!("recurring signature seen {occurrences}× in cognitive memory ({signature})"))` — **verbatim** the investigation-question string |
  | Marker embed on write | `wiring.rs:1084` | `format!("{} [sig:{}]", episode.content, episode.signature)` |
  | Marker parse on recall | `wiring.rs:976-986` (+ call `:1025`) | `parse_failure_signature("[sig:…]")` → `failure_signature` |
  | 2× recurrence count | `signal.rs:455-470`, threshold `signal.rs:362` (`= 2`) | `Signal::RecurringSignature{signature,occurrences}` |

  **NET-NEW precision — the four gates (`mod.rs:286-304`):** `whisper_gate` (900 s / 5-per-hr),
  `blocked_goal_gate` (900 s / 20), **`write_back_gate` (900 s / 5)**, and **`gap_gate` (900 s / 200)** are
  all `WhisperGate` instances (`guardrails.rs:291-343`) whose `last_delivered`/`deliveries` maps are
  initialized **empty** by `WhisperGate::new` (`guardrails.rs:301`) with **no load/save anywhere**. This
  makes the exact re-persistence mechanism explicit: the write-back dedup window is **ephemeral**, so a
  daemon **restart** resets the 900 s window and the same `overseer-obs:…` write-back is stored **again**
  into the *persistent* memory graph — two restarts within a recall horizon ⇒ two identical `[sig:…]`
  episodes ⇒ `occurrences == 2` (`signal.rs:463`). This is the concrete **second** path to `×2` alongside
  two ordinary 900 s windows (the residual-uncertainty item tertiary §7 leaves un-adjudicated from static
  source — both paths yield *real* re-observation, so the verdict is unchanged either way). Minimal-fix
  pointers (verdict unchanged, not landed): store-side idempotency on the `signature` metadata in
  `record_observation` (`wiring.rs:1076-1091`) kills the count inflation; the D1 write-boundary filter
  (§19.7) stops the nesting; optionally persist `write_back_gate.last_delivered` across restarts.

- **19.7 — NET-NEW decisive proof: the literal doubling is D1 nesting, *impossible* from true duplication
  (secondary dive).** The secondary deep dive supplies the structural argument that settles the
  nesting-vs-duplication question the investigation string raises. Two invariants hold at HEAD:
  (a) `orient` merges any two same-`dedup_key` signals into a single `Problem` (`mod.rs:1200-1221`, merge
  at `:1211`), and (b) `observation_signature`'s `keys.dedup()` collapses adjacent equal keys
  (`mod.rs:1071`). Together they guarantee **each family key can appear at most once per snapshot**.
  Therefore a literal `workstream-gap|workstream-gap` (or repeated `overseer-obs:`) inside one composite
  is **impossible from true per-token duplication** — it can *only* arise from **nested recalled
  `overseer-obs:…` fragments**, each a distinct string (it embeds its own `workstream-gap`) that survives
  `dedup()`. The observed doubling is thus a **positive fingerprint of the D1 self-observation feedback**,
  not a counting bug and not per-token duplication. Corollary: the `×2` is an **honest** Lane-A occurrence
  tally — audit the closing action, not the counter (consistent with §15/§16/§17).

- **19.8 — Token taxonomy: load-bearing vs. benign membership drift, and the Lane-A-only precision defect
  (secondary dive).** Re-grounded at HEAD, every constituent token classifies cleanly:

  | Token | dedup_key @ HEAD | Volatile field | Class |
  |---|---|---|---|
  | `goal:blocked:<goal_id>` | `format!("goal:blocked:{goal_id}")` (`mod.rs:1336`) | `consecutive_no_action`/`needs_review` (summary/priority only) | **Load-bearing** — the persistent membership set that *is* the problem |
  | `overseer-obs:…` (nested) | `sanitize_recalled(signature)` (`mod.rs:1359`) | `occurrences` (summary only) | **Load-bearing / signature-inflating** — the D1 artifact that manufactures the doubling |
  | `workstream-gap` | fixed literal (`mod.rs:1371`) | `gaps.len()` (summary only) | **Benign membership drift** |
  | `resource:engineer_spawn` | fixed literal (`mod.rs:1270`) | `{live}` (summary only) | **Benign membership drift / telemetry** |

  **Precision nuance (Lane-A only, *not* a correctness defect):** because `observation_signature` is a
  **set-hash over the whole tick's membership**, `workstream-gap`/`engineer_spawn` are benign as *tokens*
  yet, as *co-members*, they **fork the composite Lane-A identity** under drift. That fork is confined to
  the self-fed **advisory Lane-A**; **Lane-B escalation keys on the per-problem `dedup_key`** and is
  immune. So membership drift is a benign-but-latent **precision** defect in Lane-A, never a correctness
  defect in escalation. **D1 fix seam (named, not built):** filter recall-derived
  (`overseer-obs:` / `RecurringSignature`) dedup_keys out of the set fed to `observation_signature` at
  `mod.rs:546` — a symptom-seam fix, orthogonal to the counter (agrees with the §13 minimal-contained
  D1 fix).

**§19 delta:** verdict unchanged across all twelve waves. This wave **(a)** upgrades the D2 dead-zone
from a probabilistic "ACT rarely records" to a **total structural latch** grounded at the newly-verified
`wiring.rs:612-627` (`Reported ∉ outcome_records_occurrence` ⇒ Lane B pinned at 0, escalation unreachable
by construction); **(b)** unifies root causes A/B/benign into the **L1/L2/L3 non-closing-loop table**;
**(c)** adds the **two-closing-seams** boundary insight (neither reached by L1/L2); **(d)** names the
missing **"recurrence-2 closing rung"** and pins *why* the count-2 recognition is inert (Decide routes by
`ProblemKind`, ignores the priority bump); and **(e)** reconfirms `engineer_spawn` benign while flagging
the net-new **Lane-B counter inconsistency** (`Escalated` accrues, `Reported` does not); **(f)** pins the
complete **emitter map** and the **four in-memory `WhisperGate` configs** (`mod.rs:286-304`, empty on
init, no persistence), making the **restart-driven re-persistence** path to `×2` explicit (primary); and
**(g)** proves the observed token-doubling is a **positive fingerprint of D1 nesting, impossible from
true duplication** (`orient` merge + `keys.dedup()` ⇒ each key at most once per snapshot), with a full
load-bearing-vs-benign **token taxonomy** and the Lane-A-only precision-defect nuance (secondary). D0–D3
remain live and unremediated; remediation order L0→L1→L2→L3 (§16.3) stands; no production `.rs` changed.

## §20 — Thirteenth-wave net-new findings (HEAD `1de21e71`, zero non-test source drift) — three parallel dives: the end-to-end self-ingestion loop traced link-by-link (primary), the "orient-merge-is-dead ⇒ standalone self-feeding meta-problem" mechanism + signal-vs-defect verdict (secondary), and the complete gate inventory + the minimal landing-order-safe write-boundary fix with the A→A test gap (tertiary)

**Three parallel deep dives** at HEAD `1de21e71`, folded here — primary
([`primary_self_ingestion_loop_pipeline_trace_HEAD_1de21e71.md`](./primary_self_ingestion_loop_pipeline_trace_HEAD_1de21e71.md)),
secondary ([`secondary_composite_overaggregation_and_selffeed_HEAD_1de21e71.md`](./secondary_composite_overaggregation_and_selffeed_HEAD_1de21e71.md)),
and tertiary/architect ([`tertiary_two_loop_architecture_and_landing_order_safe_fix_HEAD_1de21e71.md`](./tertiary_two_loop_architecture_and_landing_order_safe_fix_HEAD_1de21e71.md)).
Drift re-check: since the twelfth-wave (`bbddd23a`) pin only `src/overseer/tests_root_cause.rs` changed —
**all non-test source is byte-identical**, so every load-bearing line number below is exact at HEAD
(independently re-read per dive; every citation verifies). **Empirical re-grounding this wave:**
`cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed** at HEAD `1de21e71` (re-run and
confirmed during consolidation), including `write_back_is_deduplicated_within_window` and
`write_back_persists_again_for_a_distinct_signature`. **No verdict reversal; no production `.rs` changed;
no remediation landed.** This wave converts the accumulated diagnosis into (a) a single continuous
**link-by-link loop trace** with an explicit `Recur` edge (§20.1), (b) the sharpened **standalone-meta-problem
self-feed** mechanism naming the exact unguarded call site (§20.2), (c) a unified **five-gate idempotency
inventory** proving the self-feed edge has *no* effective boundary (§20.3), (d) the **A→A test gap** on the
precise defect edge (§20.4), and (e) the concrete **minimal, landing-order-safe write-boundary fix** with a
buildable diff, out-of-scope carve-outs, and the required regression test (§20.5) — plus a re-affirmed
**signal-vs-defect verdict** with fresh green empirics (§20.6).

- **20.1 — NET-NEW consolidation: the self-ingestion loop, traced link-by-link end-to-end (primary dive).**
  Prior waves cited the loop's edges piecemeal (§14 pipeline trace, §19.6 emitter map, §19.7 nesting proof).
  This dive assembles them into **one continuous eight-edge cycle**, each edge a direct HEAD citation:
  1. **Seed / write-back embeds a recoverable self-marker** — `record_observation` stores
     `content = "{content} [sig:{signature}]"` with fixed `source_label = OVERSEER_SOURCE_LABEL`
     (`wiring.rs:1076-1091`, label `:952`).
  2. **Recall lifts the Overseer's own marker — NO provenance filter** — `recall_episodic`
     (`wiring.rs:1013-1031`) maps *every* recalled episode to `RecalledEpisode { failure_signature:
     parse_failure_signature(&e.content), … }`; `parse_failure_signature` (`wiring.rs:976-986`) extracts the
     `[sig:…]` payload regardless of author, and **does not exclude `source_label == "overseer"`**.
  3. **Count fires at 2 → the reported "seen 2×"** — `signals_from` (`signal.rs:455-470`) buckets by
     `failure_signature`; any bucket `>= RECURRING_SIGNATURE_THRESHOLD` (`= 2`, `signal.rs:362`) emits
     `Signal::RecurringSignature { signature: "overseer-obs:…", occurrences }`. The threshold value **is** the
     reported `2×`.
  4. **Classify keeps the self-prefix in the dedup_key** — `classify_signal` (`mod.rs:1353-1363`) sets
     `dedup_key = sanitize_recalled(signature)`; `sanitize_recalled` (`capabilities.rs:468-482`) strips only
     control chars and caps length — it **does not strip/reject the `overseer-obs:` self-prefix**.
  5. **Orient folds it in as a first-class problem** — `orient` (`mod.rs:1200-1235`) pushes it as a
     `Problem` whose `dedup_key` is the `overseer-obs:…` string, sitting alongside the tick's *fresh* bare keys
     (`goal:blocked:…`, `workstream-gap`, `resource:engineer_spawn`).
  6. **`observation_signature` RE-WRAPS → the nested prefix** — `mod.rs:1068-1073`,
     `format!("overseer-obs:{}", keys.join("|"))` over the sorted, deduped keys; because one key is already
     `overseer-obs:goal:blocked:…`, the emitted signature nests: an outer `overseer-obs:` wrapping a mix of
     `overseer-obs:goal:blocked:…` fragments and fresh bare keys — **the exact structure of the reported blob**.
  7. **The write-back gate does NOT break the loop** — `write_back_gate` (`WhisperGate::new(900,5)`,
     `mod.rs:299`; internals `guardrails.rs:291-343`; used `mod.rs:546-556`) suppresses only the
     **byte-identical** signature within its 900 s window, but **every generation mutates** the signature
     (the nested prefix grows and the aggregated fresh-key set churns), so each generation is a *new* gate key
     → `Deliver` → persisted → recallable again.
  8. **Recur** — the freshly persisted, deeper-nested episode is recalled next pass (edge 2), re-counted,
     re-raised; the recurrence counter inflates and the signature accretes prefixes without bound.
  **Single-sentence root cause (primary):** the episodic recall path has **no self-authorship exclusion**, and
  neither `sanitize_recalled` nor `observation_signature` treats an already-`overseer-obs:`-prefixed key
  specially — so the Overseer recalls, re-classifies, and re-wraps its own write-backs into an ever-nesting
  recurring signature. (Confidence: **High** — every edge is a HEAD citation; the nested-prefix structure is
  reproducible *only* by edges 2→4→6, and `2× == RECURRING_SIGNATURE_THRESHOLD` exactly.)

- **20.2 — NET-NEW mechanism sharpening: the `orient` merge branch is DEAD for the composite ⇒ the
  `RecurringSignature` ALWAYS becomes a *standalone* self-feeding meta-problem (secondary dive F3).** §19.7
  established that nesting is a fingerprint of D1; this dive names the exact *vehicle* and the exact *unguarded
  call site*. Two facts, both re-grounded at HEAD:
  1. **The only episodic `[sig:…]` writer is the Overseer's own `record_observation`** (`wiring.rs:1076-1091`);
     `record_occurrence` (Lane B) writes via `store_fact`, **not** episodic — so **no per-problem episodic
     writer exists**. Therefore the `orient` "merge into a same-`dedup_key` in-cycle problem" branch
     (`mod.rs:1211-1221`) is **effectively dead for this pipeline**: the composite key
     `overseer-obs:g1|g2|…` never equals a single problem's `goal:blocked:g1`. The `RecurringSignature`
     **always spawns a STANDALONE `ProcessHealth` meta-problem** whose `dedup_key = sanitize_recalled(composite)`.
  2. **That standalone meta-problem re-enters the next write-back with NO write-boundary filter.**
     `wiring.rs:301` calls `write_back_observation(&cycle.problems)` over **all** problems — including the
     recall-derived meta-problem — so next cycle `observation_signature` folds the prior composite back in →
     `overseer-obs:overseer-obs:g1|…`. This is the precise, cited origin of the nested tokens in the
     investigation string. Net: **Lane-A is isolated from Lane-B (tested) but NOT isolated from itself
     (untested, §20.4).** The base composite parks at a stable `2×` while a growing nested tail accumulates
     episodes and consumes memory (bounded only by the 8192-byte cap in `sanitize_recalled`,
     `capabilities.rs:455,472`).

- **20.3 — NET-NEW unified inventory: all five dedup/idempotency gates, and why the self-feed edge has NONE
  effective (tertiary dive B).** Every gate on (or near) the loop, located and adjudicated at HEAD:

  | # | Gate | Location | Keyed on | Durability | Stops | Why it fails on the self-loop |
  |---|------|----------|----------|------------|-------|-------------------------------|
  | **G1** | `write_back_gate` (`WhisperGate::new(900,5)`) | `mod.rs:299`; internals `guardrails.rs:291-333`; used `mod.rs:546-556` | full `observation_signature` string | **in-memory only** (resets on restart, `guardrails.rs:294-295`) | re-persisting a **byte-identical** signature within 900 s | signature **mutates every generation** ⇒ each gen is a new key ⇒ `Deliver` every time |
  | **G2** | `orient` in-cycle merge | `mod.rs:1211-1221` | `dedup_key` equality | per-tick | duplicate problems in one tick | **dead for composite**: `overseer-obs:g1\|g2` never equals a bare `goal:blocked:g1` |
  | **G3** | `orient` in-flight dedup | `mod.rs:1207-1209` | key vs engineers' refs | per-tick | fighting an engineer already on it | irrelevant — no engineer owns a meta-key |
  | **G4** | Lane-B `recall_occurrences` exact-match | `mod.rs:983` | per-problem `dedup_key` | store-durable (semantic facts) | cross-problem key bleed | works correctly; keeps Lane-B immune to churn |
  | **G5** | `blocked_goal_gate` / `gap_gate` / `whisper_gate` | `mod.rs:286,292,304` | respective signatures | in-memory | flooding those act-paths | not on the write-back path; out of scope |

  **Key architectural finding:** the *only* idempotency gate on the self-feed edge is **G1**, and G1 is
  structurally defeated by signature mutation; **G2 (which could collapse a recalled signature into an existing
  problem) is dead for the composite**. So **the loop has no effective idempotency boundary** — the exact
  structural reason the 15-minute window never suppresses the recurrence.

- **20.4 — NET-NEW test gap on the exact defect edge: Lane-A ⟂ Lane-A is UNTESTED (tertiary C, secondary
  "test gaps").** Isolation is asymmetrically covered:
  - **A↔B isolated — TESTED, PASS:** `tests_root_cause.rs:490`
    (`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`) and its converse `:536`. Different
    stores (`store_episode` `wiring.rs:1088` vs `store_fact` `mod.rs:1034`), different counters (floor 2 vs 3).
  - **A→A NOT isolated — UNTESTED (the leak):** the two isolation tests cover only A↔B; the write-back tests
    (`tests_memory_recall.rs:797,820`) feed **per-problem** signatures (`process:distill_fail`,
    `goal:blocked:research`), never a composite `overseer-obs:g1|g2|…` back through recall. The
    standalone-meta-problem outcome (§20.2) and its re-nesting are therefore **unasserted**. This is a test gap
    on precisely the defect edge, and it must ship with the fix (§20.5).

- **20.5 — NET-NEW remediation spec: the minimal, landing-order-safe write-boundary fix (tertiary dive E).**
  The architect specifies a fix constrained to be **single-function, no cross-file plumbing, idempotent/additive,
  and counter-untouching** — so it cannot half-land and is order-independent with the recall-side fix. In
  `write_back_observation` (`mod.rs:534-563`), drop recall-derived meta-problems **before** computing the
  signature:

  ```rust
  // mod.rs, inside write_back_observation, replacing the `let signature = …` line:
  let own: Vec<Problem> = problems
      .iter()
      .filter(|p| !p.dedup_key.starts_with("overseer-obs:")) // never fold our own recall back in
      .cloned()
      .collect();
  if own.is_empty() {
      return Ok(None); // nothing but our own echoes this tick — write nothing
  }
  let signature = observation_signature(&own);
  // … build observation_content from `own` too (mod.rs:551); gate/peek/commit unchanged …
  ```

  **Why it is the correct cut:** (i) it **kills the nesting at the source** — the composite can never again
  contain an `overseer-obs:` key, so `observation_signature` stops growing and the signature becomes **stable**
  across ticks for a stable board, which **restores G1's dedup power** (the 900 s window now actually suppresses
  the repeat) — one fix repairs both "duplicate persistence" (via re-enabled G1) and "over-aggregation
  self-feed"; (ii) it **keeps the honest signal** — Lane-A still fires `RecurringSignature` for *genuine*
  recurring board states, it just stops recording its **own** meta-problem as new evidence; (iii) it is
  **order-independent / defence-in-depth** with the primary's recall-side fix (exclude
  `source_label == OVERSEER_SOURCE_LABEL` in `recall_episodic`) — if both land, redundant guards at two seams;
  if only one, the loop is still cut; (iv) **no plumbing** — it reads `dedup_key`, already present on `Problem`,
  vs. the recall-side fix which needs `source_label` returned through `recall_episodes_ranked`.
  **Required regression test (ship with the fix):** an **A→A isolation test** (none exists, §20.4) — drive a
  cycle that write-backs a composite, recalls it, fires `RecurringSignature`, and asserts the meta-problem's
  `overseer-obs:` key is **excluded** from the next `observation_signature` (signature does not nest; G1
  suppresses the within-window repeat). Mirror `tests_memory_recall.rs:820` but assert **no** distinct nested
  signature is produced.
  **Deliberately out of scope (flag, don't bundle):** (a) the **restart-reset duplicate** — G1's in-memory
  state clears on daemon restart (`guardrails.rs:294-295`), so one identical write-back can re-persist per
  restart even for a *stable* signature; bounded (≤1 per stable signature per restart), a distinct
  lower-severity issue needing store-schema work — land separately; (b) **demote the composite
  `ProcessHealth → LaunchRecipe` to advisory/telemetry** (`mod.rs:1429-1435`) — a *routing* policy change,
  complementary but separate so the loop cut can land first and alone.

- **20.6 — Signal-vs-defect verdict, re-affirmed with fresh green empirics (secondary dive F4, tertiary D).**
  The `2×` is an **honest** re-observation: `observation_signature` is deterministic (sort+dedup), and because
  each constituent `dedup_key` is `goal:blocked:{id}` (reason-independent, `mod.rs:1336`), an identical composite
  means "the same *set* of things is still stuck" — a genuine *board-did-not-advance* signal, not a counting bug
  (within-window dedup proven green: `write_back_is_deduplicated_within_window`; distinct-signature
  re-persistence green: `write_back_persists_again_for_a_distinct_signature`; **32/0** this wave). **The DEFECT
  is the response, on two counts:** (1) `WorkstreamCoverage → FlagWorkstreamGaps` notifies only
  (`mod.rs:884-948`) — no closing rung (endorsed from §16/§19 L2); (2) `ProcessHealth` (composite Lane-A) *has*
  a closing rung but the **wrong one** — it `LaunchRecipe`s (cost-bearing, gated by `max_launches_per_cycle=2`,
  `mod.rs:283,607-611`) on a self-referential meta-string, and the meta-problem it acts on **re-amplifies its
  own signature** (§20.2). **Do NOT touch the counter** (`signal.rs:455-470`) — the documented trap
  (`PATTERNS.md`). The composite adds little that the churn-immune per-problem Lane-B (floor 3 →
  `EscalateBlockedGoal`) does not cover more precisely, which is why the standing recommendation is to
  **exclude recall-derived meta-problems at the write boundary (§20.5) and/or demote the composite to pure
  telemetry**, not to adjust the honest count.

**§20 delta:** verdict unchanged across all thirteen waves — the `×2` is an honest signal of a non-advancing
board; the over-aggregated composite + the **unguarded write-boundary** (self-ingestion with no
self-provenance filter) is the defect. This wave **(a)** assembles the scattered edges into one
**link-by-link end-to-end loop trace** with the explicit `Recur` edge (primary, §20.1); **(b)** sharpens the
mechanism to the **dead `orient` merge branch ⇒ standalone `ProcessHealth` meta-problem** that re-enters the
**unfiltered** `write_back_observation(&cycle.problems)` at `wiring.rs:301` (secondary, §20.2); **(c)** proves
via a **unified five-gate inventory** that the self-feed edge has **no effective idempotency boundary** (G1
defeated by mutation, G2 dead for the composite; tertiary, §20.3); **(d)** identifies the **A→A isolation test
gap** on the exact defect edge (§20.4); **(e)** specifies the **minimal, landing-order-safe write-boundary
fix** — a single-function `overseer-obs:`-prefix filter in `write_back_observation` (`mod.rs:534-563`) that
restores G1's dedup power with no plumbing and is defence-in-depth with the recall-side `source_label` filter,
plus its required regression test and explicit out-of-scope carve-outs (§20.5); and **(f)** re-affirms the
**signal-vs-defect verdict** with a fresh **32/0** memory-recall re-run at HEAD `1de21e71` (§20.6). D0–D3
remain live and unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands; **no production
`.rs` changed; no remediation landed.**

## §21 — Fourteenth-wave net-new findings (HEAD `f455c06d`, zero non-test source drift) — three parallel dives: an independent end-to-end signature-assembly re-trace landing on a *recall-side* count-exclusion fix seam (primary), the refreshed two-loops citation table + the "escalation ≥3 rung is itself non-closing" sharpening + the INV-GAP-KEY trap (secondary), and the decoupling-constrained landing order + a per-rung regression-safety matrix (tertiary)

**Three parallel deep dives**, folded here — primary at HEAD `ad5e1060`
([`primary_signature_assembly_pipeline_trace_HEAD_ad5e1060.md`](./primary_signature_assembly_pipeline_trace_HEAD_ad5e1060.md)),
secondary at HEAD `f455c06d`
([`secondary_two_loops_and_drift_HEAD_f455c06d.md`](./secondary_two_loops_and_drift_HEAD_f455c06d.md)),
and tertiary/architect at HEAD `f455c06d`
([`tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_f455c06d.md`](./tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_f455c06d.md)).
Drift re-check: `git diff --stat 1de21e71..HEAD -- src/` (the §20 pin → this HEAD) touches **only**
`src/overseer/tests_root_cause.rs` (the two lane-decoupling pins added at `f9cefec1`, already folded at §17/§20.4);
`git diff --stat ad5e1060 f455c06d -- src/overseer/ src/ooda_loop/` is **empty** (`f455c06d` is a docs-only
verification re-run of `ad5e1060`). **All non-test production source is byte-identical to the §20 grounding**, so
every load-bearing line number below re-verifies exact (each dive independently re-opened its citations).
**Empirical re-grounding this wave (re-run during consolidation):** `cargo test --lib overseer::tests_root_cause`
→ **21 passed, 0 failed** at HEAD `f455c06d`, including both decoupling pins
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and `lane_b_escalates_without_any_lane_a_signal`;
the dives additionally report `no_progress` **77**, `tests_whisper` **28**, `tests_memory_recall` **32** green at
the same HEAD. **No verdict reversal across fourteen waves; D1/D2/D3 remain live and unmerged; no remediation
landed.** This wave is a **re-grounding + remediation-sharpening** pass: the *self-referential* verdict, the
*two-non-closing-loops* structure, the *dead-zone*, and the *benign `engineer_spawn`* reading all re-confirm at a
new HEAD, and the three dives contribute four net refinements — (a) a **recall-side** count-exclusion fix seam that
is defence-in-depth with §20.5's write-boundary filter (§21.1), (b) the sharpening that even the **≥3
`EscalateBlockedGoal` rung is itself non-closing** so crossing the escalation floor still removes no block (§21.2),
(c) the **INV-GAP-KEY** ledger-key trap for the gap closing rung (§21.3), and (d) the **decoupling-constrained
landing order** with a per-rung regression-safety matrix naming exact tests to keep-green vs deliberately-update
(§21.4).

- **21.1 — NET-NEW fix-seam refinement: the *recall-side* count-exclusion cut (primary §3, §7), defence-in-depth
  with §20.5's write-boundary filter.** §20.5 specified the loop-breaker as a filter inside
  `write_back_observation` (`mod.rs:546`) that drops `overseer-obs:`-prefixed problems **before** computing the
  next signature (the *write* seam). This wave's independent primary re-trace lands on the **complementary read
  seam**: exclude self-authored episodes from the recurrence **count** in `signals_from`
  (`signal.rs:455-470`) — skip any recalled episode whose recovered `failure_signature` carries the
  `overseer-obs:` prefix (or thread `source_label` through `RecalledEpisode` and skip `OVERSEER_SOURCE_LABEL`).
  Re-confirmed root facts making this the *highest-leverage* recall-side cut: (i) recall has **no source filter** —
  `recall_episodes_ranked` (`cognitive_memory/mod.rs:542-550`) is a pure keyword search that never excludes
  `OVERSEER_SOURCE_LABEL`, and `recall_episodic` (`wiring.rs:1013`) passes no provenance (**D1** re-confirmed);
  (ii) `sanitize_recalled` (`capabilities.rs:468-482`) only replaces control chars + caps length — it does **not**
  strip the `overseer-obs:` prefix, so a recalled composite re-enters as a `dedup_key` and nests (**nesting**
  re-confirmed). **Architectural framing (net-new):** the two seams are **defence-in-depth on the same self-feed
  edge** — the read-side cut (§21.1) stops the Overseer *counting* its own bookkeeping as an incident (kills the
  spurious `RecurringSignature` emission at source); the write-side cut (§20.5) stops a recall-derived meta-problem
  *re-entering* the next signature (kills nesting at the write boundary). If both land, redundant guards at two
  seams; if only one, the loop is still cut. Primary's ordered candidate set: **[1]** count-exclusion at
  `signal.rs:455-470` (smallest, highest leverage) → **[2]** refuse a self-signature as a problem key in the
  `RecurringSignature` arm (`mod.rs:1353-1363`) → **[3]** strip a leading `overseer-obs:` in `observation_signature`
  (`mod.rs:1068`, caps growth). **Do NOT** touch the threshold or de-ratchet the escalation counter — the `2×` is
  honest (the documented `store_fact_with_caller_key` collapse-to-1 trap still applies).

- **21.2 — NET-NEW sharpening: the ≥3 `EscalateBlockedGoal` rung is *itself* non-closing (secondary §2).** Prior
  waves established the `2 ≤ rec < 3` dead-zone falls to `Report` (rung 4). This wave adds the load-bearing
  observation that the ladder's **top** rung does not close either: `decide_blocked_goal`
  (`mod.rs:1603-1631`) rung 1 escalates only at `recurrence >= 3` to `EscalateBlockedGoal`, which is a
  **notification** (`mod.rs:814-834`), *not* a block-removing action; the **only** closing rung is rung 2
  `UnblockGoal`, which fires *solely* for a `perpetual && is_no_progress_marker` false-park. Consequence: for the
  non-perpetual, non-review blocked goals that dominate the signature (kgpacks #12/#17/#18/#23/#25,
  simard-identity personas, coverage-to-70, coin harness), **every** ladder outcome — `Report` at 1–2× *and*
  `EscalateBlockedGoal` at ≥3× — leaves the block in place. So closing Loop A requires **adding a block-removing
  action** in the `2 ≤ rec` band (D2's closing rung), not merely lowering the escalation floor; a "fix" that only
  makes goals escalate sooner still resolves nothing. Confirms Loop A is unclosed at *both* ends of its ladder.

- **21.3 — NET-NEW remediation trap: INV-GAP-KEY — the gap closing-edge ledger must key on `GapItem.signature`,
  not the bare `workstream-gap` dedup_key (secondary §7, tertiary §4[3b]).** The `WorkstreamGap` arm mints a
  single **bare constant** key `"workstream-gap"` (`mod.rs:1371`) with per-gap identity erased; `dedup()` in
  `observation_signature` (`mod.rs:1071`) collapses only *adjacent* equal keys, which is why distinct gaps surface
  as the `workstream-gap|workstream-gap` tail. When D3 adds a `LaunchRecipe`/`FileIssue` closing edge to
  `WorkstreamCoverage` (`mod.rs:1534-1543`), keying its cross-window idempotency ledger on that bare dedup_key
  would **fold all distinct gaps into one launch/issue** (an issue-storm-in-reverse: under-filing). The closing
  edge must therefore key on the per-gap `GapItem.signature` (the identity `act_flag_workstream_gaps` already
  peeks against `gap_gate`, `mod.rs:900-908`), and must fire only for **proven-recurring** gaps so **first-sight**
  gaps stay on the existing notify path. This is the gap-arm analogue of D2's Lane-B honesty requirement:
  *close on stable identity, not on the aggregate family key.*

- **21.4 — NET-NEW: the decoupling-constrained landing order + a per-rung regression-safety matrix (tertiary
  §3–§5).** The two lanes being decoupled at `decide` (Lane A only raises priority in `orient`; Lane B alone drives
  `decide` via `why.recurrence`, `mod.rs:972-997`) is already established (§15.1/§17/§20.4) and re-pinned green
  this wave. The tertiary's net contribution is to make that fact a **hard constraint on the fix**: because Lane A
  is **inert for closure**, (i) any remediation that "makes the 2× stop" by touching Lane A's `×N` is **cosmetic**;
  (ii) the closing rung *and* the idempotency fix must **both** target **Lane B**; and (iii) the loop-breaker must
  **NOT** bridge Lane A → Lane B — bridging would break the two now-regression-pin tests *and* re-introduce the
  self-feed the loop-breaker exists to sever. The re-justified strict dependency chain:
  **[1] write-back self-observation guard** (loop-breaker; `mod.rs:546`, drop `overseer-obs:`-prefixed problems) →
  **[2] Lane-B count-in-content + WHY-gate** (ATOMIC latch; `record_occurrence`/`StoredOccurrence`/`recall_occurrences`
  at `mod.rs:1004-1043`,`:1180-1185`,`:972-997` — a signature-keyed upsert whose payload carries
  `occurrence_count`/`first_seen`/`last_seen`, escalation reading the field, **not** `recall.len()`; ship count +
  gate together or nothing changes) → **[3] closing rungs** (3a `decide_blocked_goal` dead-zone rung, `mod.rs:1603-1631`;
  3b `WorkstreamCoverage` launch/file edge with the INV-GAP-KEY cross-window ledger, §21.3). Order is a **true
  dependency chain**: [1] freezes the signature *set* so [2]'s idempotent upsert has a fixed target (an upsert
  cannot collapse a moving nested key); [2] makes the count mean "distinct windows" not "write cadence" before
  [3] consumes it. **Per-rung regression matrix (exact tests):**
  - **[1]** keep GREEN `tests_memory_recall` (32) + `tests_whisper` (28); add an A→A isolation test proving a
    slice of only `overseer-obs:*`/`RecurringSignature` problems yields `Ok(None)` (the §20.4 test gap).
  - **[2]** deliberately UPDATE any test pinning `StoredOccurrence`'s 4-field shape (it gains the count fields);
    keep GREEN the two decoupling pins and `recurring_reblock_never_files_an_issue`; add an idempotency test
    (N write-backs → `occurrence_count == N` via one logical record, recurrence tracks distinct windows).
  - **[3a]** keep GREEN `tests_no_progress`/`_investigation`/`_reinvestigation` (the perpetual self-heal branch
    must still win its band) + `loud_lane_a_…`; add a pin for the previously-`Report` band's new outcome.
  - **[3b]** deliberately UPDATE the notify-only pins `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`
    (`tests_gap_scan.rs:579`) and `flagged_gap_never_constructs_an_issue_brief` (`:663`), scoped to *cross-window
    recurrence* only; keep GREEN the opt-out/identity-safety invariants `delegates_blocked_goals_to_goal_health_and_never_reflags_them`
    (`:413`), `disabled_gap_scan_holds_the_whole_action` (`:688`), `gap_scan_fails_closed_without_a_distinct_identity`
    (`:719`); add a cross-window ledger test keyed on `GapItem.signature`.

- **21.5 — `resource:engineer_spawn` re-affirmed benign at HEAD (secondary §5).** Re-verified an ordinary leaf
  `dedup_key` minted by `Signal::EngineerSpawnRate` (`mod.rs:1267-1272`, `Priority::Normal`, sig-mapped
  `engineer_spawn` at `capabilities.rs:562`), routing to a **notify-only global** `Escalate` (`mod.rs:1444-1446`)
  that fires only at/above **8 live** engineers (`tests_m1.rs:133-149`, green). It is **membership drift** into the
  same composite (its `{live}` count lands only in the summary, never the key), so it does **not** reset recurrence
  counting; the grep shows **no causal edge** coupling `EngineerSpawnRate` to `WorkstreamGap` (independent
  signal→token→escalate chain). The gap↔spawn overlap is a legitimate resource-allocation tension at *different
  seams* (per-goal coverage vs global admission cap) — **not** a defect, and **not** to be coupled by any resourcing
  rung (verification-phase guard: confirm no spawn-based remediation accidentally edges back into `WorkstreamGap`).

**§21 delta:** verdict unchanged across all fourteen waves — the `×2` is an **honest** re-observation of a
non-advancing board (self-referential write-back recall, `mod.rs:1360-1362` verbatim = the task string); the
**defect is the response**: two observe-and-flag loops (blocked-goal ladder + notify-only `WorkstreamCoverage`)
that never close, an unguarded self-feed edge with no effective idempotency boundary, and a `2↔3` recurrence
dead-zone. This wave re-grounds every load-bearing citation at HEAD `f455c06d` (zero non-test source drift; fresh
**21/0** `tests_root_cause` re-run incl. both decoupling pins) and sharpens the remediation on four axes:
**(a)** a **recall-side count-exclusion** fix seam (`signal.rs:455-470`) that is **defence-in-depth** with §20.5's
write-boundary filter (§21.1); **(b)** the finding that even the **≥3 `EscalateBlockedGoal` rung is itself
non-closing** — closing Loop A needs a block-removing action, not a lower floor (§21.2); **(c)** the **INV-GAP-KEY**
trap — the gap closing-edge ledger must key on `GapItem.signature`, never the bare `workstream-gap` (§21.3); and
**(d)** the **decoupling-constrained landing order** `[1] loop-breaker → [2] Lane-B count-in-content (atomic) →
[3] closing rungs`, with a per-rung keep-green/deliberately-update regression matrix (§21.4). D1/D2/D3 remain live
and unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands; **no production `.rs` changed; no
remediation landed.**
