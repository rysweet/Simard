# Consolidated Findings — Recurring `goal:blocked` + `workstream-gap` Signature

**Investigation:** the overseer signature seen 2× in cognitive memory:
`overseer-obs:goal:blocked:…|…|workstream-gap|workstream-gap`
**Branch / HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `856f854b`
**Date:** 2026-07-16  **Status:** Complete (fixpoint) — re-validated against current source through **twenty** waves (latest folded into **§28**, HEAD `a0c5ed4c`/`856f854b`, zero non-test source drift; only `src/overseer/tests_root_cause.rs` differs and the working tree is source-clean). Earlier status note below is retained verbatim from the **ten**-wave milestone
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

## §22 — Fifteenth-wave net-new findings (HEAD `7293de99`/`3fac68a5`, zero non-test source drift) — six parallel dives: the **two-counter-system** framing + exact half-open `[0,900)`s window boundary (primary), the full named-emitter `①→⑧` pipeline with the **`signal.rs:644-647` literal-title display emitter** (secondary/primary), the **disjoint-detector `blocked`-skip proof** that gap and blocked goals can *never* co-derive + the **`routing.rs` dead-end** read (secondary), the **dedup-gate-defeated-by-mutating-signature** mechanism as the ~20 KB-blob cause (tertiary), the architect **REJECT-persist-`last_delivered`** verdict + **no-threshold-move** ruling with the conditional episode-store idempotency key (tertiary), and the specialist **source-frozen-since-`dea65df8`** reconciliation (144-test re-pin + the `ooda_loop/cycle.rs` path fix)

**Six parallel deep dives**, folded here — two primaries at HEAD `3fac68a5`
([`primary_signature_emitter_token_assembly_and_2x_semantics_HEAD_3fac68a5.md`](./primary_signature_emitter_token_assembly_and_2x_semantics_HEAD_3fac68a5.md),
[`primary_signature_assembly_emission_pipeline_and_idempotency_gate_HEAD_3fac68a5.md`](./primary_signature_assembly_emission_pipeline_and_idempotency_gate_HEAD_3fac68a5.md)),
two secondaries at HEAD `7293de99`/`3fac68a5`
([`secondary_emission_pipeline_and_two_loops_HEAD_7293de99.md`](./secondary_emission_pipeline_and_two_loops_HEAD_7293de99.md),
[`secondary_nesting_vs_dup_and_gap_spawn_routing_HEAD_3fac68a5.md`](./secondary_nesting_vs_dup_and_gap_spawn_routing_HEAD_3fac68a5.md)),
two tertiary/architect at HEAD `3fac68a5`/`7293de99`
([`tertiary_architecture_SPAWN_GAP_COUPLING_AND_SELFFEED_HEAD_3fac68a5.md`](./tertiary_architecture_SPAWN_GAP_COUPLING_AND_SELFFEED_HEAD_3fac68a5.md),
[`tertiary_architecture_IDEMPOTENCY_DURABILITY_AND_REMEDIATION_RUNG_HEAD_7293de99.md`](./tertiary_architecture_IDEMPOTENCY_DURABILITY_AND_REMEDIATION_RUNG_HEAD_7293de99.md)),
and a knowledge-archaeologist specialist at HEAD `3fac68a5`
([`specialist_RECONCILIATION_VALIDATE_DONT_REDERIVE_HEAD_3fac68a5.md`](./specialist_RECONCILIATION_VALIDATE_DONT_REDERIVE_HEAD_3fac68a5.md)).
Drift re-check: `git diff --stat f455c06d..HEAD -- src/` (the §21 pin → this HEAD) is **empty** — the two intervening
commits (`d6ba8b25`, `3fac68a5`) plus HEAD `7293de99` are all `docs(investigation)/*.md`-only. The specialist's wider
audit `git diff --stat dea65df8..HEAD -- src/` = **1 file, +99** (only `src/overseer/tests_root_cause.rs`, the
net-additive decoupling pins added at `f9cefec1`, already folded at §17/§20.4). **All non-test production source is
byte-identical to the §20/§21 grounding**, so every load-bearing line number below re-verifies exact (each dive
independently re-opened its citations).
**Empirical re-grounding this wave (re-run during consolidation at HEAD `7293de99`):**
`overseer::tests_root_cause` → **21/0** (incl. `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`),
`overseer::tests_memory_recall` → **32/0**, `overseer::tests_gap_scan` → **21/0**, `overseer::tests_whisper` → **28/0**,
`no_progress` → **77/0** = **179 passed, 0 failed**. **No verdict reversal across fifteen waves; D1/D2/D3 remain live and
unmerged; no remediation landed.** This wave is a **re-grounding + emitter-precision + fix-lever-adjudication** pass:
the *self-referential* `2×` verdict, the *two-non-closing-loops* structure, the *dead-zone*, the *benign
`engineer_spawn`*, and the *correlational-not-causal spawn↔gap* readings all re-confirm at the current HEAD, and the six
dives contribute seven net refinements (§22.1–§22.7).

- **22.1 — NET-NEW framing: the `2×` comes from *two disjoint counter systems*, and only one surfaces (primary §3).**
  Prior waves established the `2×` is `RecurringSignature.occurrences` and not a gate artifact; this wave makes the
  distinction load-bearing and names both systems. **System A (the recall fold)** — `signals_from` builds
  `counts: BTreeMap<&str,u32>`, +1 per recalled episode whose parsed `failure_signature` equals the signature, and
  emits `RecurringSignature{signature,occurrences}` at `occurrences >= RECURRING_SIGNATURE_THRESHOLD (=2)`
  (`signal.rs:455-470`, threshold `:362`); this **is** the rendered `2×`. **System B (the WhisperGate internals)** —
  `last_delivered: HashMap<String,i64>` + `deliveries: Vec<i64>` (`guardrails.rs:294-295`) drive **suppression
  decisions only**, are never read by the fold, never rendered. Consequence: a plausible misfix — "reset/persist the
  dedup counter" — targets **system B and would only throttle honest re-observations feeding system A**, hiding a true
  signal. **Exact window boundary (net-new):** `peek` suppresses **iff** in `last_delivered` **and**
  `now - last < window_secs` with **strict `<`** (`guardrails.rs:314`), so with `window_secs=900` (`mod.rs:299`) the
  suppression window is **half-open `[0,900)` s** — gap `<900` Suppress, gap `==900` Deliver (`900 < 900` false), gap
  `>900` Deliver; the virtual-clock test pins it exactly (`Deliver@0`, `Suppress@899`, `Deliver@901`,
  `whisper_gate_suppresses_an_identical_whisper_within_the_window`). `commit` runs only after a successful store
  (`mod.rs:555-556`, fail-open-on-error); `admit == peek + commit-on-Deliver` (`guardrails.rs:336-342`) so the whisper
  suite faithfully models the production path.

- **22.2 — NET-NEW emitter precision: the *literal investigation-question title* is emitted at `signal.rs:644-647`,
  distinct from the `mod.rs:1353-1363` classify arm (secondary §1, primary §1).** The full `①→⑧` named pipeline
  re-verified verbatim: ① token synthesis (`classify_signal` arms — `goal:blocked:{id}` `mod.rs:1336`, bare
  `workstream-gap` `mod.rs:1371`, `resource:engineer_spawn` `mod.rs:1270`) → ② composite emitter
  `observation_signature = "overseer-obs:"+sort∘dedup∘join("\|")` (`mod.rs:1068-1073`) → ③ human body
  `observation_content` (`mod.rs:1079-1089`) → ④ caller+gate `write_back_observation` (`mod.rs:534-563`) → ⑤ fixed-
  provenance persist `record_observation`/`store_episode` (`wiring.rs:1076-1091`, `OVERSEER_SOURCE_LABEL` `:952`) → ⑥
  recall parse-back `parse_failure_signature`/`recall_episodic` (`wiring.rs:976-986,1013-1030`) → ⑦ recall fold →
  `RecurringSignature{occurrences}` (`signal.rs:455-470`) → ⑧ classify re-admission (`mod.rs:1353-1363`). **The refined
  detail:** the human-visible string *"recurring failure signature '{sig}' seen {N} time(s)"* — which matches the
  investigation-question wrapper verbatim — is produced by the **`signal_to_problem` display arm at `signal.rs:644-647`**,
  a separate emit site from the `classify_signal` `RecurringSignature` arm (`mod.rs:1359-1363`) that mints the recall
  `dedup_key`. Two emitters, one string family: `signal.rs:644-647` renders the *summary the operator sees*;
  `mod.rs:1359` mints the *key that re-enters the composite*. **`record_observation` stores exactly one episode per
  admitted `Deliver`** (`wiring.rs:1084-1089`, single `store_episode` node_id), so `occurrences=2` **cannot** arise from
  a non-idempotent episodic write — it necessarily means two genuinely-persisted episodes (primary §2 decision matrix,
  re-confirmed).

- **22.3 — NET-NEW causal-independence proof: `goal:blocked:*` and `workstream-gap` are minted by *disjoint detectors
  over non-overlapping goal partitions* — `detect_workstream_gaps` explicitly *skips blocked goals* (secondary §2).**
  The strongest form yet of the "spawn↔gap and blocked↔gap are not causally coupled" verdict. `detect_workstream_gaps`
  (`sensor.rs:288-320`) contains `if matches!(g.status, GoalProgress::Blocked(_)) { continue; }` with the comment
  *"Blocked goals flow through goal_health; never re-flag them here."* Therefore the `goal:blocked:<slug>-<hash>` tokens
  and the `workstream-gap` token derive from **two separate detectors over provably non-overlapping goal sets**, and
  **neither reads `live_engineers`**. Combined with the leaf-signal table — `EngineerSpawnRate{live}` reads only
  `state.live_engineers` (`sensor.rs:123`) and `WorkstreamGap{gaps}` reads only `state.workstream_gaps` (injected at
  `wiring.rs:772`; the read-only snapshot leaves it empty, `sensor.rs:153`), with both volatile counts (`{live}`,
  `{gaps.len()}`) landing in **summaries only, never a `dedup_key`** — this closes the causal question at the *detector*
  level, not just the signal level: there is **no code edge** binding the three tokens; their only relationship is
  set-hash co-membership in one write-back composite (**over-aggregation**) plus a *latent, code-invisible* common cause
  (an under-resourced Simard). **Do not special-case the pair; any resource-aware gap launch must NOT read the spawn
  signal** or it would *manufacture* the coupling the code correctly lacks.

- **22.4 — NET-NEW dead-end read: `stewardship/routing.rs` is a dormant `source_module→TargetRepo` router that never
  reads `live_engineers` or gap counts and is off the notify-only path entirely (secondary §2 `routing.rs`).** A direct
  whole-file read (53 lines) settles the "does routing couple gap↔spawn?" question: `route_failure` (`routing.rs:39-52`)
  matches only a source-module *string* against `AMPLIHACK_KEYWORDS`/`SIMARD_KEYWORDS` and falls back to `rysweet/Simard`;
  it **never reads either signal field**, and it only *mentions* the Overseer's gap briefs in a comment
  (`routing.rs:12-14`). Because `WorkstreamCoverage` is **notify-only** (`mod.rs:1534-1543`, no `FileIssue`/`LaunchRecipe`
  edge), gap briefs never reach the filing path that would even invoke `route_failure`. **`routing.rs` contributes
  nothing to the signature** — confirming the strategy's own "potential dead end" and removing it from the suspect set.

- **22.5 — NET-NEW mechanism: the exact-string dedup gate is *defeated by the mutating (nesting) signature* — this is
  what inflates one record to the ~20 KB blob (tertiary §3, secondary §1).** The 5-edge self-feed cycle
  `[E1] observation_signature (mod.rs:1068-1073) → [E2] record_observation embeds [sig:…] (wiring.rs:1084) →
  [E3] recall parse-back (wiring.rs:1025) → [E4] signals_from ≥2 → RecurringSignature (signal.rs:455-470) →
  [E5] classify dedup_key = sanitize_recalled(signature) = "overseer-obs:…" (mod.rs:1359) → back to [E1]` re-read
  verbatim. The sharpened architectural point: `write_back_observation` gates on `write_back_gate.peek(&signature)`
  (`mod.rs:548`), an **exact-string** WhisperGate — but edge `[E5]→[E1]` **mutates** the signature every cycle (each
  nesting level `overseer-obs:overseer-obs:…` is a *different* string), so every write-back looks novel → always
  `Deliver` → re-persisted. **The dedup primitive is defeated by the very feedback it is meant to suppress**; each
  nested fragment carries its own full `goal:blocked` block, so concatenating nested snapshots is exactly what grows one
  record into the ~20 KB pipe-delimited blob. This also *proves* the `workstream-gap|workstream-gap` doubling is D1
  nesting, **not** two distinct gap keys — the `WorkstreamCoverage` Problem carries the single bare dedup_key
  `"workstream-gap"` (`mod.rs:1371`), and `observation_signature`'s `sort_unstable();dedup()` (`mod.rs:1069-1072`)
  collapses only *adjacent equals within one snapshot*, so a literal doubling can arise **only** from a nested recalled
  fragment sorting beside a freshly-emitted bare key — a positive fingerprint of the self-feed.

- **22.6 — NET-NEW architect adjudication of the two fix-levers this mandate owned: (a) persisting `last_delivered` is
  REJECTED; (b) no threshold (`2`/`3`) moves — the only legitimate "rung between 2 and 3" is the D3 per-gap
  `≥2×→LaunchRecipe` partition *on Lane A* (idempotency tertiary §1–§2).** Two candidate "quick fixes" are definitively
  ruled out with cited justification. **(a) Persist the whisper gate → REJECT (four reasons):** it (i) *hides a true
  signal* — the composite is a faithful fingerprint of a still-open set, so muting the post-restart second episode
  manufactures a false "converged" reading (the count must fall because *the loop closed*, not because *the gate
  remembered*); (ii) *duplicates durability that already lives, by design, on Lane B* — the cross-restart recurrence
  ledger is `store_fact` occurrences read as `recurrence` (`mod.rs:1034`,`:1613`); persisting the gate stands up a
  second competing durable counter on the wrong (episode) lane; (iii) *adds a real correctness surface for no product
  gain* (stale-slot pruning at boot, clock-skew, unbounded keyspace, "is a 901 s-old delivery still suppressed after a
  20-min outage?"), whereas the primitive is provably correct *because* it is volatile (`guardrails.rs:291-333`,
  `tests_whisper.rs:437-475`); (iv) the `2×` is a *symptom of missing closing edges*, so even a perfect durable gate
  leaves the open backlog forever — removing the persistent condition (D1/D2/D3) removes restart re-emission at the
  source. **(b) Move `2` or `3` → REJECT:** the lanes are **decoupled and share no counter** (now codified by
  `tests_root_cause.rs:490` `loud_lane_a_…`), so "between 2 and 3" is **not** a single-axis dead zone a number can
  close — Lane A's `×2` carries *no information* about Lane B reaching `3`, and a generic "escalate at 2" on Lane B
  would fire on honest transients (a false-positive machine). **The one legitimate rung** is the **D3 per-gap
  recurrence partition keyed on `GapItem.signature` at the `gap_gate` commit site (`mod.rs:884-948`, key `:901`):
  1× → Notify, ≥2× → `LaunchRecipe`** via the existing `launch.rs` edge every sibling High arm already has
  (`WorkstreamCoverage` is the sole High arm lacking it, `mod.rs:1534-1543`) — this is the one place `2` (not `3`) is
  correct, *because a recurring coverage gap has no benign transient explanation*, and it sits on Lane A, not a re-tuned
  Lane B. **Conditional durable-dedup (subordinate, not the fix):** *iff* restart-flapping is *empirically* confirmed as
  the dominant `2×` source, add an idempotency key `(signature, floor(now/900))` at the **episode-store boundary**
  (`record_observation`/`store_episode`, `mod.rs:554`) — a count-in-content window bucket mirroring D2 discipline — which
  dedups the *persisted artifact* without teaching the volatile gate to lie; recorded as a **convergence-gauge-phase
  follow-up**, never the minimal safe fix.

- **22.7 — NET-NEW reconciliation baseline: source is *frozen since `dea65df8`* (only `tests_root_cause.rs` +99 added),
  all load-bearing citations re-pin exact, 144 referenced tests green, and the `overseer/cycle.rs → ooda_loop/cycle.rs`
  path is corrected (specialist §1–§5).** The knowledge-archaeologist audit over the 69-artifact / 15-HEAD corpus
  establishes that the *code under investigation has been frozen since before `dea65df8`* — the waves are re-validation
  of a **stable** target, not tracking a moving one, which is why re-derivation has near-zero expected value. Every
  ledger citation re-pins identically at HEAD (`observation_signature` `mod.rs:1068-1073`; Lane-B ratchet
  `store_fact` `mod.rs:1034`; notify-only both modes `mod.rs:1534-1543` + `observer.rs:120`; thresholds `signal.rs:362`
  =2, `root_cause.rs:33` =3; `store_fact_with_caller_key` = `DedupMode::CallerKey` "exactly one live fact survives per
  key" `library_adapter.rs:885-890`; recall reads live-only `library_adapter.rs:763,773,830`). **One path correction
  (cosmetic, not logic drift):** the WHY double-gate lives at **`src/ooda_loop/cycle.rs:583`**
  (`no_progress_investigation_enabled()`), **not** `src/overseer/cycle.rs` (which does not exist); any doc citing a bare
  `cycle.rs:582-702` should read `ooda_loop/cycle.rs`. **Two forward-carried remedy traps re-affirmed as implementation
  guardrails:** (i) D2 must use a **count-in-content caller-key upsert** (increment `occurrence_count`, `first_seen`/
  `last_seen`, escalation reading the field not `recall.len()`) — the literal `store_fact_with_caller_key` one-liner
  collapses recall to 1 and makes `mod.rs:1613` **dead code**; (ii) D3 must key the coverage ledger on
  `GapItem.signature`, **not** the bare `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps fold into one issue
  (INV-GAP-KEY). Specialist recommendation: **evidence base is saturated — proceed to implementation, not further
  investigation.**

**Open ordering question surfaced this wave (honest divergence, not a reversal).** Two landing orders coexist across
the six dives and must be reconciled at the implementation phase: **(A) D1-first** — `[1] write-back self-feed cut →
[2] Lane-B count-in-content (atomic) → [3] closing rungs` (secondary §8, tertiary SPAWN_GAP §5, matching §20.5/§21.4),
justified because freezing the signature *set* first gives the signature-keyed upsert a fixed target (*an upsert cannot
collapse a moving nested key*); versus **(B) D2-first** — `D2 (WHY-gate close + count-in-content, atomic) → D3 (per-gap
rung) → D1 (write-back filter) → convergence gauges` (specialist §6, idempotency tertiary §3), justified because D2/D3
drain the blocked/gap populations while D1 is a pure/local *shape* fix. **Reconciliation:** the two do **not** conflict
on the atomic-latch requirement (D2 gate+counter ship together) nor on the endpoints (convergence gauges last); they
differ only on whether the write-back filter (D1) is a **prerequisite** for D2's idempotent upsert or a **cosmetic last
step**. The §21.4 dependency argument (an upsert cannot collapse a *moving* nested key, so the set must be frozen first)
favors **order (A)**; the implementation phase should either land D1 before D2 or prove D2's upsert key is stable under
nesting before deferring D1.

**§22 delta:** verdict unchanged across all fifteen waves — the `×2` is an **honest** two-counter-system reading
(recall-fold `occurrences` = system A, `signal.rs:455-470`; **not** the WhisperGate internals = system B,
`guardrails.rs:294-295`) of a non-advancing board, its half-open `[0,900)` s window boundary now pinned exactly; the
**defect is the response** — two observe-and-flag loops that never close, an unguarded self-feed whose *mutating* nested
signature **defeats the exact-string dedup gate** (the ~20 KB-blob cause), and a `2↔3` **cross-lane visibility** gap
(codified by `tests_root_cause.rs:490`), not a threshold-arithmetic gap. This wave re-grounds every load-bearing
citation at HEAD `7293de99`/`3fac68a5` (zero non-test source drift; fresh **179/0** across five suites) and adds seven
refinements: **(1)** the two-counter-system framing + exact `[0,900)` boundary that pre-empts a "reset/persist the dedup
counter" misfix (§22.1); **(2)** the `signal.rs:644-647` literal-title *display* emitter distinguished from the
`mod.rs:1359` *key* emitter (§22.2); **(3)** the disjoint-detector `blocked`-skip proof (`sensor.rs:288-320`) that gap
and blocked goals can never co-derive (§22.3); **(4)** the `routing.rs` dead-end read removing it from the suspect set
(§22.4); **(5)** the dedup-gate-defeated-by-mutating-signature mechanism as the blob cause (§22.5); **(6)** the architect
REJECT-persist-`last_delivered` (four reasons) + no-threshold-move ruling, with the only legit rung being D3 per-gap
`≥2×→LaunchRecipe` on Lane A and a conditional episode-store `(signature, floor(now/900))` idempotency key (§22.6); and
**(7)** the source-frozen-since-`dea65df8` reconciliation with the `ooda_loop/cycle.rs` path fix and two forward-carried
remedy traps (§22.7). It also surfaces one **open landing-order question** (D1-first vs D2-first) for the implementation
phase. D1/D2/D3 remain live and unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands; **no
production `.rs` changed; no remediation landed.**

## §23 — Sixteenth-wave net-new findings (HEAD `9fd1ea0a`/`a68296c6`, zero non-test source drift) — two parallel dives: the **"unwired WHY reasoner" lever is STALE at HEAD** — the breaker *and* the issue-#17 bare-park reinvestigation sweep are both default-wired, so the real bare-park condition is the `completion_evidence` gate + env kill-switch, not a missing subsystem (secondary); the **signature-invariant recurrence** proof that `goal:blocked:<id>` omits the WHY token so a correctly-classified terminal block is indistinguishable from a bare park (secondary); the **emission/notification decoupling** — the `workstream-gap` token can recur in the composite with *zero* operator notifications when gap-scan is disabled, a strictly weaker "observe-into-signature-without-closing" precondition (secondary); and the architect **two-lane reconciliation + 14-claim exact-citation re-verification** ("no stale citations" at `a68296c6`) that comes down on **D2-first (order B)** for §22's open landing-order question, arguing no fix depends on another's *code* (tertiary)

**Two parallel deep dives**, folded here — a patterns/secondary dive at HEAD `9fd1ea0a`
([`secondary_blocked_park_and_gap_spawn_coupling_HEAD_9fd1ea0a.md`](./secondary_blocked_park_and_gap_spawn_coupling_HEAD_9fd1ea0a.md))
and an architect/tertiary dive at HEAD `a68296c6`
([`tertiary_architecture_TWO_LANE_RECONCILIATION_AND_LANDING_HEAD_a68296c6.md`](./tertiary_architecture_TWO_LANE_RECONCILIATION_AND_LANDING_HEAD_a68296c6.md)).
Drift re-check: `git diff --name-only 7293de99..HEAD -- '*.rs'` (the §22 pin → this HEAD) is **empty** — the two
intervening commits (`9fd1ea0a`, `a68296c6`) are `docs(investigation)/*.md`-only. The wider audit
`git diff --name-only dea65df8..HEAD -- '*.rs'` = **`src/overseer/tests_root_cause.rs` only** (the net-additive
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` decoupling pin, already folded at §17/§20.4/§22.7).
**All non-test production source is byte-identical to the §20/§21/§22 grounding**, so every load-bearing line number below
re-verifies exact (both dives independently re-opened their citations; the architect published a 14-row re-verification
table and I spot-re-confirmed `cycle.rs:582-583`, `no_progress.rs:203-207`, `sensor.rs:299-301`, `mod.rs:596-597,1603-1613`,
`cycle.rs:628` at HEAD).
**Empirical re-grounding this wave (re-run during consolidation at HEAD `a68296c6`):**
`overseer::tests_root_cause` → **21/0** (incl. `loud_lane_a_…`), `overseer::tests_memory_recall` → **32/0**,
`overseer::tests_gap_scan` → **21/0**, `overseer::tests_whisper` → **28/0**, `no_progress` → **77/0** = **179 passed,
0 failed** — identical to the §22 re-grounding. **No verdict reversal across sixteen waves; D1/D2/D3 remain live and
unmerged; no remediation landed.** This wave is a **framing-correction + reconciliation** pass: it *corrects a stale
mechanism claim* carried from early artifacts, *sharpens* the observe-without-closing thesis on both lanes, and *takes a
position* on the §22 open landing-order question. The five net refinements are §23.1–§23.5.

- **23.1 — NET-NEW correction: the "unwired/degraded WHY reasoner" lever is STALE at HEAD — the breaker AND the issue-#17
  bare-park reinvestigation sweep are BOTH default-wired; the real "no WHY classification" condition is the
  `completion_evidence` gate + env kill-switch, a degraded *configuration*, not a missing subsystem (secondary A2).**
  Prior artifacts (`blocked_transition_and_escalation_idempotency.md §3`; `DISCOVERIES.md #4`) named an *unwired/degraded
  WHY reasoner* as the "single lever." At live HEAD this is **no longer accurate**: (i) `cycle.rs:583` gates on
  `no_progress_investigation_enabled()` — **default ON** (`no_progress.rs:203-207`; `SIMARD_NO_PROGRESS_INVESTIGATE=off`
  is an opt-out kill-switch, `unwrap_or(true)`); (ii) `cycle.rs:599-608` calls `apply_no_progress_breaker_investigated`
  with a real production `DeterministicNoProgressReasoner::new(source_ref)` (`cycle.rs:593-594`), a `CloneRepoHealer`, and
  a `QueueingEngineerDispatcher` — so a stall is classified and routed down `resolution_for_why`, not parked bare; (iii)
  `cycle.rs:627-636` **additionally** calls **`reinvestigate_bare_blocked_goals`** (`cycle.rs:628`, verified) — the
  issue-#17 sweep that scans the board for goals still in a *bare* `[OODA-SAFEGUARD] … needs human review` block and
  re-runs the same reasoner + ladder, un-blocking on a non-terminal rung or authoring a WHY-bearing reason otherwise. So
  bare parks are **actively upgraded to WHY-bearing every cycle**. The gate that actually *preserves* bare, unexplained
  parks is narrower: the **whole breaker + reinvestigation block is gated on `memories.completion_evidence == Some`**
  (`cycle.rs:582`, `if let Some(source) = &memories.completion_evidence`). Absent that memory pair (non-daemon callers, or
  a daemon config without the completion-evidence source), **neither parking nor reinvestigation runs**, so a pre-existing
  bare park persists untouched and no WHY is ever produced — and the env kill-switch path (`cycle.rs:684-698`) falls back
  to `apply_no_progress_breaker`, whose ladder authors the *bare* `no_progress_blocked_reason` ("…consecutive no-action
  cycles; needs human review", `no_progress_breaker.rs:75,123`). **Net correction:** the classification machinery exists
  and is default-wired; bare parks are a **degraded-configuration artifact** (evidence source absent or kill-switch
  engaged), not a missing subsystem. The root-cause *shape* (bare park → no self-resolution) is unchanged **where the gate
  is off**; the *mechanism description* in the early artifacts is corrected. **Verification-phase question:** does the
  production daemon actually supply `memories.completion_evidence`? If yes, sustained bare parks should self-heal and the
  reviewer should audit the reasoner's *classification accuracy* (e.g. kgpacks recurring despite an `AlreadyComplete`
  classification implies the done-gate/verify path is misfiring), not the breaker's existence; if no, that absence is the
  concrete "no WHY" root cause.

- **23.2 — NET-NEW pattern "signature-invariant recurrence": `goal:blocked:<id>` is dedup-keyed on `goal_id` ONLY and
  carries NO WHY token, so a *correctly-classified terminal block* re-emits the identical signature as a bare park —
  recurrence is NOT evidence of a missing classification (secondary A1/A3).** `Signal::GoalBlocked` orients to a `Problem`
  with `dedup_key = format!("goal:blocked:{goal_id}")` (`mod.rs:1336`). The WHY class (`AlreadyComplete`,
  `UnclearCriteria`, `GenuinelyStuck`, `UpstreamDependency`, …) lives only in the goal's block-reason *text* and in
  `problem.why`; it is **never** part of the `dedup_key`, and the observation signature is built purely from sorted/deduped
  dedup_keys (`observation_signature`, `mod.rs:1068-1073`). **Consequence:** the recurring `goal:blocked:<slug>` token is
  *invariant* to whether the park is bare or WHY-bearing — a goal correctly classified `UnclearCriteria` (→ human),
  `GenuinelyStuck` (→ human), or `UpstreamDependency` (→ defer until upstream lands) **legitimately stays blocked and
  re-emits the identical signature every window**. So the focus premise "bare no-progress, no WHY classification"
  **cannot be confirmed from the signature**; the recurrence looks the same either way. The persistent `goal:blocked`
  recurrence is the expected fingerprint of *terminally-classified-but-unresolved* work. Even with the reasoner wired
  (§23.1), the cluster recurs because the terminal WHY classes *intentionally* keep the goal blocked, and **no rung
  converges the recurring observation signal** — `decide_blocked_goal` (`mod.rs:1603-1631`) only *escalates once per
  window* (gated, at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` `mod.rs:1613`) or *reports*; it **never marks the
  observation lane resolved**. Per-class ladder (secondary A3 table): kgpacks #12/17/18/23/25 + parity →
  `AlreadyComplete`/`MissingPrecondition` (should clear — if they recur, classification or the done-gate is failing);
  coverage-audit-to-70% → `UnclearCriteria` (uncheckable done-gate → **stays blocked, correctly**); coin-benchmark harness
  → `MissingPrecondition`/`UpstreamDependency`-defer (**stays blocked** until upstream lands); simard-identity personas →
  `GoalUncovered`(active)/`UnclearCriteria`(blocked) (**stays blocked**). **Same root shape as the gap loop (§23.3):
  observe-without-closing on the observation lane.** New named anti-pattern for the library: *don't infer "unclassified"
  from recurrence.*

- **23.3 — NET-NEW nuance: signature emission is DECOUPLED from notification — the `workstream-gap` token can recur in
  the composite with ZERO operator notifications when gap-scan is disabled, a strictly WEAKER precondition than
  "notify-without-closing" (secondary B2).** `act_flag_workstream_gaps` (`mod.rs:884-946`) is **notify-only**: it peeks
  `gap_gate` (`WhisperGate::new(900, 200)`) keyed `workstream-gap:{g.signature}` (`:901`), sends one consolidated operator
  notification (email + Signal, `:929-930`), commits the gate (`:932-933`) — **no `launch`, no `file_issue`, no engineer
  spawn** (the Decide arm `mod.rs:1534-1543` merely carries `WorkstreamGap` evidence into `Intervention::FlagWorkstreamGaps`,
  vs. the sibling `StepFailure` arm `:1549-1580` which produces a real `Intervention::LaunchRecipe` — `WorkstreamCoverage`
  is the sole High-family arm with no launch edge, reconfirming D3). **The net-new nuance:** the `workstream-gap` token
  enters the composite at **Orient** — `signal_to_problem` mints `dedup_key = "workstream-gap"` (`mod.rs:1371`) and
  `write_back_observation(&cycle.problems)` (`wiring.rs:301`) writes `observation_signature` over the oriented problems,
  gated only by the `write_back_gate` — **independent of the Act phase**, which can be *held entirely* when
  `!self.gap_scan_enabled` (`mod.rs:596-597`: `if matches!(iv, Intervention::FlagWorkstreamGaps { .. }) &&
  !self.gap_scan_enabled { return held_plan(iv, "held: gap-scan disabled (SIMARD_OVERSEER_GAP_SCAN)") }`; default `false`
  at `:300`). **Consequence:** the token can recur in the signature **even when zero notifications fire** (gap-scan
  disabled) ⇒ the loop is "observe-into-signature *without any* closing action" — a strictly weaker precondition than the
  prior "notify-without-closing" framing, and a **silent-degradation surface**: gaps recur in the operator-visible
  signature with *zero* operator alerting. Worth a convergence gauge on `gap_scan_enabled` state. Any remediation must key
  on `GapItem.signature` (per-gap), **not** the bare `"workstream-gap"` dedup_key (**INV-GAP-KEY trap**, `mod.rs:1371`),
  else all gaps fold into one issue.

- **23.4 — NET-NEW reconciliation: the architect re-verified all 14 load-bearing citations exact at `a68296c6` ("no stale
  citations") and confirmed the `tests_root_cause.rs` `loud_lane_a_…` test codifies the two-lane decoupling as a
  regression invariant (tertiary §0–§1).** Rather than trust the docs' own line numbers, the tertiary dive independently
  re-opened every cited seam at HEAD and published a 14-row status table, all ✅ exact: `observation_signature`
  (`mod.rs:1068-1073`), write-back over oriented problems (`wiring.rs:301`), recall-driven `RecurringSignature` →
  `dedup_key = sanitize_recalled(signature)` (`mod.rs:1353-1359`), Lane-A floor `RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:362,463`), Lane-B append-only `store_fact` ratchet (`mod.rs:1034`), Lane-B escalate at
  `RECURRENCE_ESCALATION_THRESHOLD (3)` (`mod.rs:1613`; `root_cause.rs:33`), notify-only `WorkstreamCoverage`
  (`mod.rs:1534-1543`) vs. launching `StepFailure` (`:1549-1580`), bare `"workstream-gap"` dedup_key (`mod.rs:1371`),
  `goal:blocked:{goal_id}` carrying no WHY (`mod.rs:1336`), `resource:engineer_spawn` passive telemetry (`mod.rs:1270`),
  the WHY double-gate (`cycle.rs:582-583`), and volatile per-process whisper gates (`guardrails.rs:292-333`). The wider
  `dea65df8..HEAD` audit shows the *only* `.rs` change is the net-additive test
  `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` (`tests_root_cause.rs:477+`), which **hardens this
  wave's two-lane thesis as a regression invariant and contradicts nothing** — the two lanes (Lane A episode/visible-count
  `RecurringSignature.occurrences`; Lane B root-cause/escalation `store_fact` ratchet) **share no counter**, so the `×2`
  carries zero information about whether Lane B reached `3`. The committed root-cause analysis is **sound and live**.

- **23.5 — NET-NEW position on §22's open landing-order question: the architect comes down on D2-FIRST (order B),
  arguing no fix depends on another's *code* (only verification is cleaner in sequence); the atomicity table + rejected
  levers are re-endorsed (tertiary §2–§5).** §22 surfaced an honest divergence between **(A) D1-first** (freeze the
  signature *set* first so the signature-keyed upsert has a fixed target — an upsert cannot collapse a *moving* nested key)
  and **(B) D2-first**. This wave's architect **endorses order (B): D2 → D3 → D1 → convergence gauges**, with the rationale
  that D2 "drains the `goal:blocked:*` population at the source — the largest token cluster — and unlatches escalation,"
  and crucially that **"no fix depends on another's *code*, but the *verification* of each is cleaner in this sequence"**
  (D1 last keeps its cosmetic-looking-but-real diff reviewable once volume has dropped). **Honest reconciliation (not a
  reversal):** this *narrows* the §22 question but does **not** fully rebut the §21.4/§22 dependency argument — it sidesteps
  the "upsert cannot collapse a moving nested key" concern by asserting D1↔D2 is not a hard *code* dependency; the
  implementation phase should still either land D1 before D2 **or** prove D2's upsert key is stable under nesting before
  deferring D1. The rest of the architect ruling re-confirms §22 verbatim: **atomicity** — D2 (WHY-gate close +
  count-in-content upsert) is **ATOMIC** (the accrual gate `cycle.rs:582` and the counter `mod.rs:1034/1613` form a
  *latch*; the de-ratchet must be a count-in-content upsert, **NEVER** the literal `store_fact_with_caller_key` one-liner,
  or `DedupMode::CallerKey` collapses `recall.len()` to 1 and makes `>=3` dead code); D1 (write-back emission filter), D3
  (per-gap `≥2×→LaunchRecipe` keyed on `GapItem.signature`), and gauges are **INDEPENDENT**; **rejected levers re-endorsed**
  — persisting `last_delivered` (masks an open backlog as convergence; duplicates Lane-B durability; loads a correctness
  surface onto a primitive correct *because* volatile) and moving `2`/`3` (lanes decoupled; escalates honest transients);
  and **`resource:engineer_spawn` is NOT a fourth defect** — benign passive telemetry (`mod.rs:1270`) with no causal edge
  to `workstream-gap`, co-occurring only because both predicates held in one window (backlog uncovered AND engineers
  saturated). The three tokens are **three symptoms of one under-resourced, non-converging state** — `goal:blocked` (idle
  stuck) + `workstream-gap` (active uncovered) + `resource:engineer_spawn` (no spare executors) — not an orchestration
  cycle; actual spawning lives in the OODA loop (`no_progress.rs` `SpawnEngineer` rung, bounded to one guided retry via
  `mark_guided_retry`), with no unfulfilled-spawn defect at the overseer boundary.

**§23 delta:** verdict unchanged across all sixteen waves — the `×2` is an **honest** re-observation of a static,
under-resourced, non-advancing problem set (Lane A `RecurringSignature.occurrences`, `signal.rs:455-470`), **not** a
dedup/replay artifact; the **defect is the response** — two observe-and-flag loops that never close (`goal:blocked`
observation lane never resolved by `decide_blocked_goal`, `workstream-gap` notify-only with no launch edge), a self-feed
whose mutating nested signature defeats the exact-string dedup gate (D1), and a cross-lane `2↔3` visibility gap (D2). This
wave re-grounds every load-bearing citation at HEAD `9fd1ea0a`/`a68296c6` (zero non-test source drift; fresh **179/0**
across five suites) and adds five refinements: **(1)** the *stale "unwired WHY reasoner"* correction — the breaker and the
issue-#17 `reinvestigate_bare_blocked_goals` sweep are both default-wired, so bare parks are a `completion_evidence`-gate +
kill-switch *configuration* artifact, not a missing subsystem (§23.1); **(2)** the *signature-invariant recurrence* pattern
— `goal:blocked:<id>` omits the WHY token, so recurrence cannot be read as a missing classification (§23.2); **(3)** the
*emission/notification decoupling* — the `workstream-gap` token can recur with **zero** operator notifications when
gap-scan is disabled, a silent-degradation surface and a strictly weaker observe-without-closing precondition (§23.3);
**(4)** the architect *14-claim exact-citation re-verification* ("no stale citations" at `a68296c6`) with the
`loud_lane_a_…` two-lane-decoupling regression invariant (§23.4); and **(5)** the architect *D2-first (order B)* position
on §22's open landing-order question — narrowing it by arguing no hard *code* dependency, while leaving the "prove-the-
upsert-key-is-stable-under-nesting-or-land-D1-first" caveat open — plus the re-endorsed atomicity table, rejected levers,
and `engineer_spawn`-is-not-a-fourth-defect ruling (§23.5). D1/D2/D3 remain live and unremediated; the L0→L1→L2→L3
whole-loop remediation order (§16.3) stands; **no production `.rs` changed; no remediation landed.**

## §24 — Seventeenth-wave net-new findings (HEAD `b47b6413`→`641f9c37`→`d187e414`, zero non-test source drift) — seven parallel dives across three HEADs: the **byte-for-byte empirical reproduction** of the self-ingestion growth loop with the decisive proof that the write-back dedup gate is *fueled* (not merely bypassed) by the growth it causes (`gate_dedup_hit=False` at every generation) (primary); the **second self-feed** — a recalled `RecurringSignature` is `ProblemKind::ProcessHealth` and `decide()`-routes to `LaunchRecipe` with the recurring-signature text as its task, so a self-observation can spawn a recipe to investigate itself (secondary); the **`×2`-is-honest verdict upgraded from reasoned to test-locked** by the `+99` two-lane decoupling tests, plus an independent VALIDATION verdict (full overseer **361/0**, discriminating probes **5/0**, verdict VALID) (primary + validator); the **measured 53/0 regression baseline** yielding the sharpest new landing-safety constraint — **D3 must be additive, not a Decide-arm swap**, because `tests_gap_scan.rs:852` hard-asserts `FlagWorkstreamGaps` and panics otherwise (tertiary); and the **"dead zone is a two-lane visibility/coverage gap, not a single-axis counter dead zone"** sharpening that names the missing rung as **Rung 4 (`else → Report`)** of `decide_blocked_goal` plus the workstream-gap ladder's absent second rung (tertiary)

**Seven parallel deep dives across three consecutive HEADs**, folded here:
a drift-recheck primary at `b47b6413` ([`primary_signature_emission_2x_verdict_DRIFT_RECHECK_HEAD_b47b6413.md`](./primary_signature_emission_2x_verdict_DRIFT_RECHECK_HEAD_b47b6413.md));
a two-loops/dead-zone/token-class secondary and a landing-safe-remediation tertiary at `641f9c37`
([`secondary_two_loops_deadzone_token_class_HEAD_641f9c37.md`](./secondary_two_loops_deadzone_token_class_HEAD_641f9c37.md),
[`tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_641f9c37.md`](./tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_641f9c37.md));
and a signature-construction/write-back primary, an escalation-ladder/missing-rung tertiary, an all-hypotheses verification re-run, and an independent validation verdict at `d187e414`
([`primary_signature_construction_writeback_and_duplicated_prefix_loop_HEAD_d187e414.md`](./primary_signature_construction_writeback_and_duplicated_prefix_loop_HEAD_d187e414.md),
[`tertiary_architecture_escalation_ladder_and_missing_rung_HEAD_d187e414.md`](./tertiary_architecture_escalation_ladder_and_missing_rung_HEAD_d187e414.md),
[`verification_results_ALL_HYPOTHESES.md`](./verification_results_ALL_HYPOTHESES.md),
[`VALIDATION_VERDICT_HEAD_d187e414.md`](./VALIDATION_VERDICT_HEAD_d187e414.md)).
Drift re-check: `git diff --stat b47b6413..HEAD -- src/` is **empty**; the two intervening commits (`641f9c37`, `d187e414`)
are `docs(investigation)`-only. The wider audit `git diff --stat dea65df8..HEAD -- src/` = **`src/overseer/tests_root_cause.rs` only**
(the net-additive `+99` two-lane decoupling tests, already folded at §17/§20.4/§22.7/§23.4). **All non-test production source is
byte-identical to the §22/§23 grounding**, so every load-bearing line number below re-verifies exact (all seven dives independently
re-opened their citations; the validator published an exact-citation table and re-confirmed `mod.rs:1068-1073`, `signal.rs:362/463`,
`root_cause.rs:33`, `mod.rs:1361` verbatim at `d187e414`). **Empirical re-grounding this wave:** full overseer suite **361/0**;
`overseer::tests_root_cause` **21/0** (incl. both decoupling pins); the three-suite regression floor
`tests_gap_scan`+`tests_goal_health`+`tests_root_cause` **53/0**; discriminating H0/H1/H2 probes **5/0**. **No verdict reversal
across seventeen waves; D1/D2/D3 remain live and unmerged; no remediation landed.** This wave's contribution is *empirical* and
*landing-safety*: it reproduces the D1 loop byte-for-byte, upgrades the honest-`×2` claim to test-locked, and pins the exact
regression assertion that constrains the D3 fix shape. The five net refinements are §24.1–§24.5.

- **24.1 — NET-NEW empirical: the duplicated prefix is a byte-for-byte-reproduced *growing* self-ingestion loop, and the write-back
  dedup gate is FUELED (not merely bypassed) by the growth it causes (primary `d187e414` §3–§4).** A faithful mirror of the two exact
  functions — `observation_signature` (`mod.rs:1068-1073`, the sole `|`/`overseer-obs:` producer at `1072`), the classify dedup_key
  rule `sanitize_recalled(signature)` (`mod.rs:1359`), the 8192-byte cap (`capabilities.rs:468-482`), the `>=2` recall count
  (`signal.rs:455-469`), and the full-signature write-back gate key (`mod.rs:548`) — seeded with the persistent set
  `{goal:blocked:…-7f5afcca, workstream-gap}` reproduces the investigation blob's shape **exactly**: a run of
  `overseer-obs:goal:blocked:…-7f5afcca` repeats followed by a run of `workstream-gap`, `+98 bytes/generation`, with **`overseer-obs:`
  repeat count == generation count** (the real blob simply carries the full multi-goal payload under the same growth law). The decisive
  new fact: **`gate_dedup_hit=False` at *every* generation.** Because step-5 write-back (`wiring.rs:301` → `observation_signature`) nests
  the entire prior composite one level deeper each cycle, generation *G+1* is strictly longer than *G* → never byte-identical →
  `write_back_gate.peek` (`mod.rs:548`) always returns `Deliver` and `commit` (`mod.rs:556`) records a fresh episode. **The idempotency
  mechanism is structurally unable to converge this loop** — the very growth the loop causes defeats the dedup the gate promises; the
  `mod.rs:1064-1067` "identical observations ⇒ identical signature" invariant holds only for a *fixed* input, which self-ingestion
  guarantees never occurs. Growth is bounded only by the 8192-byte `sanitize_recalled` cap on the classify key (D1b), after which the
  **corruption/false-merge regime** begins (the untruncated stored `[sig:…]` at `wiring.rs:1084` and the truncated classify key diverge:
  distinct large composites collapse to the same 8192-byte prefix on the classify side → false merges, while the gate on the untruncated
  sig still stores them). This **upgrades D1/D1b from reasoned to empirically-reproduced** and confirms the observed ≈5–7 `overseer-obs:`
  repeats ⇒ ≈5–7 self-ingestion generations before capture. (Reproduction script was a `/tmp` scratch artifact, not committed.)

- **24.2 — NET-NEW corroboration: a recalled `RecurringSignature` is `ProblemKind::ProcessHealth` and `decide()`-routes to
  `LaunchRecipe` with the recurring-signature *text* as its task — a second self-feed that can spawn a recipe to investigate itself
  (secondary `641f9c37` §4).** Beyond the write-back nesting (D1), the recall-derived `Signal::RecurringSignature` orients to a Problem
  of kind `ProcessHealth` (`mod.rs:1357`); `decide()` routes `ProcessHealth` to **`Intervention::LaunchRecipe`** (`mod.rs:1429-1435`)
  with `task_description = problem.summary` — i.e. the literal `"recurring signature seen 2× in cognitive memory (…)"` string
  (`mod.rs:1361`). So a recalled self-observation can **spawn a recipe to investigate itself** — plausibly the very origin of this
  investigation branch — throttled (not eliminated) by `gate()` + `max_launches_per_cycle`. This is a *second* self-referential edge
  distinct from the signature-nesting one, and it sharpens the D1 fix direction: the write-back set must **exclude recall-derived
  `ProblemKind::ProcessHealth`/`RecurringSignature` meta-problems** (both to stop the prefix nesting *and* to stop the self-spawn), not
  merely filter `overseer-obs:` tokens from the join. Both self-feeds share the same non-filtered seam: `wiring.rs:301` passes
  `&cycle.problems` whole, with no stripping of recall-derived problems.

- **24.3 — NET-NEW: the "`×2` is honest" verdict is upgraded from *reasoned* to *test-locked*, and independently re-validated at HEAD
  (primary `b47b6413` §4; validator `d187e414`).** The `+99` lines added to `tests_root_cause.rs` are two two-lane decoupling tests that
  convert "the count is honest; audit the loop, not the counter" from argument to assertion:
  `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` (`:490` — a `RecurringSignature{occurrences=10}`, loud above *both*
  floors, with an empty Lane-B recall leaves `why.recurrence==0` and `decide` returns `UnblockGoal`) and
  `lane_b_escalates_without_any_lane_a_signal` (`:536` — Lane B escalates on its own `>=3` recall with Lane A silent). Re-run at HEAD:
  `overseer::tests_root_cause` → **21/0**, both green. This closes the bug-vs-honest question: **honest count, unhealthy loop** — the `×2`
  is a real re-observation on Lane A (episodic recall, `signal.rs:455-469`) while escalation lives on the decoupled Lane B (root-cause
  occurrences, floor `3`), so a stuck "`×2` forever" indicts a **missing convergence rung (D2/D3)**, never the counter. An **independent
  validation pass** (`VALIDATION_VERDICT_HEAD_d187e414.md`) re-grounded all citations (zero drift), re-ran the full overseer suite
  (**361/0**) and five discriminating H0/H1/H2 probes (**5/0**: `write_back_is_deduplicated_within_window`,
  `whisper_gate_suppresses_an_identical_whisper_within_the_window`, `recurring_signature_emitted_when_two_episodes_share_signature`,
  `recurring_signature_not_emitted_for_single_occurrence`, `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`), and returned
  **VERDICT: VALID** — H0 (dedup-artifact null) correctly **REJECTED**, H1 (real re-observation) **SUPPORTED**. Caveat preserved: the
  "exactly 2× after daemon restart" mechanism (H6, in-process `WhisperGate.last_delivered` at `guardrails.rs:294`, cleared on restart) is
  a **plausible, source-consistent amplifier labelled "SUPPORTED (non-causal amplifier)"** — not a directly test-reproduced fact; the
  dominant source of the specific `2` remains empirically unmeasured.

- **24.4 — NET-NEW landing-safety constraint (measured, not inferred): the 53/0 regression baseline pins that D3 must be ADDITIVE, not a
  Decide-arm swap (tertiary `641f9c37` §4).** The three-suite floor was *actually executed* at HEAD —
  `cargo test -p simard --lib -- overseer::tests_gap_scan overseer::tests_goal_health overseer::tests_root_cause` → **53 passed; 0
  failed**. Mapping each proposed change onto that surface yields the sharpest new constraint of the wave:
  `tests_gap_scan.rs:852 decide_routes_workstream_coverage_to_flag_gaps` asserts **verbatim** that `decide()` on a `WorkstreamCoverage`
  problem returns `Intervention::FlagWorkstreamGaps` and **`panic!`s on anything else**, and `:872
  flag_workstream_gaps_is_routine_and_admitted_by_default_gate` pins `classify(FlagWorkstreamGaps)==RiskClass::Routine`. **Therefore a
  Decide-arm *swap* to `LaunchRecipe` is rejected** (it breaks that assertion *and* launches on every first-seen gap → thrash). The
  landing-safe D3 shape is: **keep** `decide(WorkstreamCoverage)==FlagWorkstreamGaps` for first-observation/below-threshold (both existing
  tests stay unchanged and green) and **add** a *second* rung firing only when a **per-gap** signature has recurred `≥2×`, mirroring the
  `decide_blocked_goal` recurrence pattern (`mod.rs:1610-1616`) and routed through the **existing** `launch.rs` edge already proven by the
  sibling `StepFailure` arm (`mod.rs:1549-1581`) — keyed on `GapItem.signature` (INV-GAP-KEY: the Act gate already keys per-gap at
  `mod.rs:901,932`), **never** the bare `"workstream-gap"` constant (`mod.rs:1371`), or all gaps fold into one launch. New behavior ⇒
  **new** tests (`workstream_gap_recurring_2x_launches_keyed_on_gap_signature`, `first_observation_still_only_flags`), not edits to the
  two existing assertions. D2 is likewise additive (existing terminal-shape assertions
  `recurring_reblock_escalates_root_cause_not_blind_unblock`, `escalate_blocked_goal_notification_carries_the_why`,
  `recurring_reblock_never_files_an_issue`, plus the two decoupling invariants must all stay green; add
  `recurrence_counts_in_fact_content_not_node_multiplicity`, `why_gate_closed_classifies_instead_of_bare_park`); D1 breaks nothing (no
  test asserts nesting) and adds `recall_derived_overseer_obs_excluded_from_next_signature` + a large-blob idempotency test for D1b. The
  wave also **closes the strategy's "drift HAS landed" warning as a FALSE ALARM**: `mod.rs`/`observer.rs`/`signal.rs`/`wiring.rs`/
  `guardrails.rs` are byte-identical to `6e3113bc` (only `tests_root_cause.rs` +99 differs); the newer filesystem mtimes are a
  checkout/rebase artifact, not content drift.

- **24.5 — NET-NEW sharpening: the "2↔3 dead zone" is a two-lane VISIBILITY/COVERAGE gap, not a single-axis counter dead zone, and the
  missing remediation rung is precisely Rung 4 (`else → Report`) of `decide_blocked_goal` plus the workstream-gap ladder's absent second
  rung (tertiary `d187e414` §1–§2).** The blocked-goal ladder `decide_blocked_goal` (`mod.rs:1603-1631`) has four ordered arms
  (`recurrence>=3 → EscalateBlockedGoal`; `perpetual && is_no_progress_marker → UnblockGoal`; `needs_review → EscalateBlockedGoal`;
  **`else → Report`**, surface-only). Lane A (`RecurringSignature`, floor `2`, `signal.rs:362/463`) and Lane B (`RootCause.recurrence`,
  floor `3`, `root_cause.rs:33`) are **decoupled counters on different storage lanes** — `decide_blocked_goal` reads `recurrence` **only**
  from Lane B and never from Lane A (now pinned by the two decoupling tests). So the operator-visible `×2` says **nothing** about whether
  Lane B reached `3`; a goal recurring on Lane A can sit at `×2` **indefinitely** while Lane B stays `0`, because Lane-B accrual is starved
  upstream by the WHY double-gate (`cycle.rs:582-583`). The "missing rung" is therefore **Rung 4** — a goal that is Lane-A-recurring yet
  carries neither `perpetual`+no-progress nor `needs_review` is **visible, recurring, and terminal: observed forever, remediated never** —
  and, on the gap side, the **absent second step** of the `WorkstreamCoverage` ladder (`mod.rs:1534-1543`, notify-only, no `launch.rs`
  edge where the sibling `StepFailure` arm has one). **This is not fixed by moving a threshold:** lowering `3→2` would escalate honest
  Lane-B transients and still would not help the double-gate-starved goals (whose Lane-B count is `0`, not `2`). The gap is structural (a
  missing rung + a starved accrual gate), so **threshold moves stay rejected**, and the landing order re-endorses **D2 → D3 → D1 →
  convergence gauges** (D2 the atomic WHY-gate-close + count-in-content upsert — never the `store_fact_with_caller_key` one-liner that
  makes `>=3` dead code; D1/D3/gauges independent), consistent with §23.5 — with the §23.5 caveat still open that the implementation phase
  must either land D1 before D2 **or** prove D2's upsert key is stable under nesting before deferring D1.

**§24 delta:** verdict unchanged across all seventeen waves — the `×2` is an **honest, now test-locked** re-observation of a static,
under-resourced, non-advancing problem set (Lane A `RecurringSignature.occurrences`, `signal.rs:455-469`), **not** a dedup/replay/
collision artifact; the **defect is the response** — two observe-and-flag loops that never close (D2/D3) plus a self-feed whose *mutating*
nested signature defeats the exact-string write-back gate (D1). This wave's net contribution is *empirical + landing-safety*: **(1)** the
D1 loop is now reproduced **byte-for-byte** with the decisive `gate_dedup_hit=False`-every-generation proof that the write-back dedup gate
is *fueled* by the growth it causes and can never converge, bounded only by the 8192-byte cap into a corruption/false-merge regime (D1b)
(§24.1); **(2)** a **second self-feed** — recalled `RecurringSignature` (`ProblemKind::ProcessHealth`) routes to `LaunchRecipe` with its
own summary as the task, letting a self-observation spawn a recipe to investigate itself (§24.2); **(3)** the honest-`×2` verdict upgraded
from reasoned to **test-locked** by the `+99` two-lane decoupling tests (`21/0`) and independently re-validated (full overseer `361/0`,
probes `5/0`, VERDICT VALID; H0 REJECTED, H1 SUPPORTED, H6 restart-amplifier labelled non-causal) (§24.3); **(4)** the **measured 53/0**
regression baseline pinning that **D3 must be additive, not a Decide-arm swap** — `tests_gap_scan.rs:852` hard-asserts `FlagWorkstreamGaps`
and panics otherwise, so the recurrence→launch rung is *added* (per-gap `≥2×`, keyed on `GapItem.signature`) over the preserved base arm —
and closing the "drift has landed" warning as a false alarm (§24.4); and **(5)** the **two-lane-coverage-gap** sharpening naming the
missing rung as **Rung 4 (`else → Report`)** of `decide_blocked_goal` plus the gap ladder's absent second rung, re-rejecting threshold
moves and re-endorsing the **D2 → D3 → D1 → gauges** order with the D1↔D2 sequencing caveat still open (§24.5). D1/D2/D3 remain live and
unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands; **no production `.rs` changed; no remediation landed.**

## §25 — Eighteenth-wave net-new findings (HEAD `cc55a6fb`, zero non-test source drift) — four parallel dives (two primaries, one secondary-validation, one tertiary-architect) plus a full re-executed H0–H8 verification matrix. The load-bearing contribution is a **drift correction that relocates the Lane-B starvation mechanism**: prior waves (§22–§24) blamed a store-layer caller-key upsert collapsing `recall.len()→1`; at HEAD the tertiary proves that is **obsolete** — `record_occurrence` already *appends* via plain `store_fact` into a durable `open_persistent` store (the de-ratchet is effectively already in place). The **live** self-seal is one rung earlier, at the **ACT→record boundary**: `ActOutcome::Reported` is excluded from `outcome_records_occurrence` (`wiring.rs:612-627`), so the terminal Rung-4 `Report` sink never records, Lane-B `recurrence` stays `0` forever, and Rung 1 (`>=3`) is unreachable for exactly the dead-zone goals. This relocates the minimal D2 fix from "de-ratchet the store counter" to "**record the acknowledged blocked-goal park.**"

All four dives re-grounded every load-bearing citation **live at `cc55a6fb`** (4 commits newer than the §24 grounding). `git diff --stat d187e414..HEAD -- src/` and `b47b6413..HEAD -- src/` are both **empty**; the intervening commits are `docs(investigation)/*.md`-only. The net refinements are §25.1–§25.4.

- **25.1 — RE-GROUNDED at HEAD (two independent primaries, 4 commits newer, zero drift): the signature is the Overseer's own write-back and the `×2` is honest — both re-confirmed line-for-line.** `observation_signature` (`mod.rs:1068-1073`) collects each cycle problem's `dedup_key`, `sort_unstable` + `dedup`, joins with `|`, and prepends `overseer-obs:`; **line 1072 is the sole producer** of both the prefix and the join. The investigation-question string is minted **verbatim** at `mod.rs:1360-1362` — the `RecurringSignature` arm builds `sanitize_recalled("recurring signature seen {occurrences}× in cognitive memory ({signature})")`, where the `2×` is the `occurrences` field floored at `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, emit `>=` at `:462-468`). The self-ingestion loop (D1) is re-confirmed: the entire recalled composite is admitted as **one** new problem `dedup_key` (`mod.rs:1359`, `sanitize_recalled(signature)`) then re-wrapped in a fresh `overseer-obs:` prefix (`wiring.rs:301 → mod.rs:1072`), adding one nested prefix layer + one more copy of every frozen inner token per generation, byte-shape reproduced. The write-back dedup gate **fuels** the loop: it keys on the full `observation_signature` (`mod.rs:546-548`), which grows every generation, so consecutive generations are never byte-identical → `peek` always returns `Deliver` → a fresh episode persists each window. `sanitize_recalled` (`capabilities.rs:459-482`) only replaces control chars and caps at `RECALLED_TEXT_MAX_LEN = 8192`, preserving `:`, `|`, and the prefix, so nesting re-forms intact until the 8 KB cap. Nothing here is new vs §24; the value is the **zero-drift re-anchor at a HEAD 4 commits newer**.

- **25.2 — NET-NEW (load-bearing drift correction, tertiary `cc55a6fb` §0, §3, §6): the Lane-B starvation is at the ACT→record boundary, NOT a store-layer counter collapse — the prior "de-ratchet the store counter" fix is OBSOLETE.** Prior waves (§22.x/§23.x/§24.5) located Lane-B starvation in `record_occurrence` using `store_fact_with_caller_key(root_cause_signature(...))`, collapsing `recall.len()→1`, and prescribed "de-ratchet the counter." At HEAD this is **STALE / PARTIALLY OBSOLETE**: `record_occurrence` (`mod.rs:1004-1043`) uses **append** `store_fact` (`mod.rs:1034`) into a **durable** `open_persistent` store (`library_adapter.rs:188-190`), and `recall_occurrences` uses `search_facts` (`mod.rs:972-996`) — the store-boundary collapse is **not** the live mechanism; the de-ratchet is **effectively already in place** (secondary §1 independently pins `record_occurrence` at `mod.rs:1004,1034` still on non-idempotent `store_fact`, confirming the de-ratchet trap is **unsprung**, which is good). The **live self-seal** is one rung earlier: the blocked-goal ladder `decide_blocked_goal` (`mod.rs:1603-1631`) is four ordered arms — `recurrence>=3 → EscalateBlockedGoal`; `perpetual && is_no_progress_marker → UnblockGoal`; `needs_review → EscalateBlockedGoal`; **`else → Report → Reported`** — and only that terminal Rung 4 is **non-recording** because `ActOutcome::Reported` is **excluded** from `outcome_records_occurrence` (`wiring.rs:612-627`). A genuinely-stuck goal that merely fails the no-progress marker (Rung 2) and the `needs_review` flag (Rung 3) is misfiled as a "deliberate" block, acknowledged, and can **never** accrue toward Rung 1: **park → don't record → never escalate → re-observe → park**, self-sealing. Rung 4 itself is intentional and correct for a real operator/dependency wait (pinned green: `tests_root_cause.rs:648-680 deliberate_operator_block_is_acknowledged_not_symptom`); the defect is that it is a *terminal, non-recording sink*. Two prior blame-targets are also retired at HEAD: the WHY double-gate (`cycle.rs:582-701`) is **OUT-OF-SCOPE for overseer Lane B** — Lane B (`record_occurrence`/`recall_occurrences`) is self-contained and does not route through `cycle.rs` (that gate governs the engineer-loop's `MarkGoalBlocked` reason classification feeding Rungs 2/3, a different accrual); and the "ladder lives in `stewardship/routing.rs`" prompt claim is **DRIFT** (`routing.rs` is a 52-line repo-keyword router; the ladder is in `overseer/mod.rs`).

- **25.3 — NET-NEW (tertiary `cc55a6fb` §4, verified against a re-run 53/0 + 60/0 floor): the minimal landing-safe fix moves to "record the acknowledged blocked-goal park," a one-line accrual change that reuses the existing idempotent escalation primitive.** Threshold moves stay **rejected** (lowering `RECURRENCE_ESCALATION_THRESHOLD` `3→2` escalates honest transients and *still* does nothing while Lane B sits at `0`). **Step 1 (the load-bearing one-line change):** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`) so a Rung-4 park records its occurrence and the `goal:blocked:<id>` signature accrues `1,2,3,…` in the durable store, reaching the **existing** Rung 1 → `EscalateBlockedGoal` at 3 (idempotent via `blocked_goal_gate` `escalate:{goal_id}`, `mod.rs:823-838`). Landing-safe because **no green test pins `Reported` as excluded** from recording (verified), each Report source carries a distinct `dedup_key` (no cross-source collisions), and first observation (`recurrence 0`) still Reports → `deliberate_operator_block_is_acknowledged_not_symptom` stays green. A narrower-blast-radius variant scopes recording to `problem.kind == GoalHygiene` + `GoalBlocked` evidence, sparing the `DeliveryReady`/`QualityRegression` Report paths. **Step 2 (optional, fills the literal 2→3 band, depends on Step 1):** insert `if recurrence >= 2 && recurrence < RECURRENCE_ESCALATION_THRESHOLD && !needs_review && !(perpetual && marker) → EscalateBlockedGoal` *before* the terminal Report — one notify-once operator surface at the exact recurrence=2 point the signature already flags, idempotent, never firing for a first-sighting deliberate wait. **§4.2 workstream-gap seam is unchanged from §24.4:** keep `decide(WorkstreamCoverage)==FlagWorkstreamGaps` for first/below-threshold and **add** a per-gap `≥2×` rung keyed on `GapItem.signature` (`mod.rs:901,932` — never the bare `"workstream-gap"` constant `mod.rs:1371`) routed through the existing `launch.rs` edge and classified at `LaunchRecipe`'s (non-`Routine`) risk tier. **Landing order:** §4.1 Step 1 → §4.1 Step 2 → §4.2; the `overseer-obs:` de-nesting (D1, primary) is orthogonal and independent. Floors re-run green at HEAD: `tests_root_cause + tests_goal_health + tests_gap_scan` = **53/0**; `tests_memory_recall + tests_whisper` = **60/0**.

- **25.4 — RE-VERIFIED: the full H0–H8 hypothesis matrix re-executed at HEAD `cc55a6fb` returns the same verdict — H0 REJECTED, H1–H8 SUPPORTED — with byte-identical production source and no remediation landed.** `cargo test -p simard --lib overseer::` → **361 passed, 0 failed** (7960 filtered), test binary clean (`simard v0.32.1`). Every hypothesis's named discriminating probe re-run green: **H0** (null: dedup/replay/collision) → REJECTED via `write_back_is_deduplicated_within_window`, `write_back_persists_again_for_a_distinct_signature`, `whisper_gate_suppresses_an_identical_whisper_within_the_window`, `whisper_gate_caps_whispers_per_rolling_hour` (**7/0** co-run with H1); **H1** (real re-observation loop) → SUPPORTED; **H2** (WHY double-gate → bare park) → SUPPORTED, smoking gun `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` re-run `--exact` (**1/0**), broad filter **21/0**; **H3** (`WorkstreamCoverage` notify-only, no closing edge) → SUPPORTED (**4/0**); **H4** (self-observation write-back feedback) → SUPPORTED (bounded); **H5** (2×↔3× dead zone) → SUPPORTED (`RECURRING_SIGNATURE_THRESHOLD=2` `signal.rs:362` vs `RECURRENCE_ESCALATION_THRESHOLD=3` `root_cause.rs:33`, decoupled); **H6** (non-idempotent counters) → SUPPORTED (non-causal amplifier); **H7** (blocked↔gap = one problem two views) → SUPPORTED (**4/0**); **H8** (three token families = one under-throughput) → SUPPORTED (med-high). `git diff --stat b47b6413..HEAD -- src/` and `87206fbb..HEAD -- src/` are **empty**; `git status --porcelain -- src/` clean.

**§25 delta:** verdict unchanged across all eighteen waves — the `×2` is an **honest, test-locked** re-observation (Lane A `RecurringSignature.occurrences`, `signal.rs:455-469`) of a static, under-resourced, non-advancing problem set, **not** a dedup/replay/collision artifact (H0 REJECTED, H1 SUPPORTED at a HEAD 4 commits newer). This wave's net contribution is a **load-bearing drift correction**: **(1)** two independent primaries re-anchor the emission pipeline at `cc55a6fb` with zero source drift, re-confirming `mod.rs:1072` as the sole signature producer, `mod.rs:1360-1362` as the verbatim mint of the investigation-question string, and the growth-fueled write-back dedup gate (§25.1); **(2)** the tertiary **retires the store-layer counter-collapse blame as obsolete** (the de-ratchet is already in place — `record_occurrence` appends via `store_fact` into a durable store) and **relocates the live Lane-B self-seal to the ACT→record boundary**, where `ActOutcome::Reported` is excluded from `outcome_records_occurrence` (`wiring.rs:612-627`) so the terminal Rung-4 sink never records and Rung 1 (`>=3`) is unreachable (§25.2); **(3)** the minimal D2 fix therefore **moves from "de-ratchet the store counter" to "record the acknowledged blocked-goal park"** — a one-line addition of `Reported` to `outcome_records_occurrence`, landing-safe (no test pins the exclusion; first-observation still Reports), plus the optional recurrence≥2 earlier-surface rung and the unchanged additive workstream-gap launch rung (§25.3); and **(4)** the full H0–H8 matrix re-runs green at HEAD (overseer **361/0**) with byte-identical source (§25.4). D1/D2/D3 remain live and unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands, with the D2 remediation target now sharpened to the ACT→record seam; **no production `.rs` changed; no remediation landed.**

## §26 — Nineteenth-wave net-new findings (HEAD `d00e4c3f`, zero non-test source drift) — two parallel VALIDATE-not-rederive dives (one secondary, one tertiary-architect). The load-bearing contribution is an **architectural reframe + a root-cause reconciliation that promotes §25's single one-liner into a dependency-ordered R1→R2→R3→R4 plan**: the tertiary reframes the recurring signature as the string fingerprint of **three coupled OODA loops that never close** and proves the 2↔3 dead zone (`Lane-A ≥ 2 ∧ Lane-B < 3`) is an **absorbing region, not a transient band**; and it **reconciles §25.2's "`cycle.rs` OUT-OF-SCOPE for Lane B" ruling** into a precise two-part root cause — `cycle.rs:582-583`'s D0 completion-gate conjunction is out-of-scope for the Lane-B *accrual* seam (that starvation is at the ACT→record boundary, §25.2) **but IS the upstream root cause of the "deliberate" *misclassification*** that routes honest-stuck goals into the non-recording Rung-4 sink. Both must be fixed: R1 un-starves accrual (record the park), R2 stops the misclassification (author a WHY class). The secondary independently re-confirms **D1-is-nesting-not-duplication** with the decisive two-invariant structural proof and re-warns that the **`store_fact_with_caller_key` one-liner (§6.2b) remains a trap**.

Both dives re-grounded every load-bearing citation **live at `d00e4c3f`** (1 commit newer than the §25 grounding at `cc55a6fb`; the single intervening commit, `2191fcd2`, is the §25 consolidation, `docs(investigation)`-only). Drift re-check: `git diff --name-only cc55a6fb..d00e4c3f -- '*.rs'` is **empty**, `git diff --name-only d00e4c3f..HEAD -- '*.rs'` is **empty**, and the wider audit `git diff --name-only dea65df8..HEAD -- '*.rs'` = **`src/overseer/tests_root_cause.rs` only** (the net-additive `+99` two-lane decoupling tests, folded since §17). **All non-test production source is byte-identical to the §22–§25 grounding**; both dives independently re-opened their citations and confirmed no stale line numbers (`engineer_spawn` `1268→1270`, `RecurringSignature` arm `1353→1353`, otherwise identical). Verdict unchanged across all nineteen waves; D1/D2/D3 remain live and unmerged; no remediation landed. The net refinements are §26.1–§26.5.

- **26.1 — RE-CONFIRMED (secondary VALIDATE, HEAD 1 commit newer): the doubling is D1 self-observation nesting, and per-token duplication within a single composite is STRUCTURALLY IMPOSSIBLE — proven by two HEAD-live invariants.** The decisive proof re-verified at `d00e4c3f`: **Invariant A — full dedup in the signature.** `observation_signature` runs `keys.sort_unstable(); keys.dedup();` (`mod.rs:1070-1071`) before the `|`-join, so sorting makes equal keys adjacent and `dedup()` removes **all** duplicates ⇒ each unique `dedup_key` appears **at most once** per composite. **Invariant B — merge before signature.** `orient` folds any two same-`dedup_key` signals into a single `Problem` (`.find(|p| p.dedup_key == key)`, `mod.rs:1211-1213`), so the `problems` slice handed to `observation_signature` never carries two equal-`dedup_key` problems in the first place. **Consequence:** a single-level `overseer-obs:` signature can never contain `X|X`; therefore the observed `overseer-obs:…|overseer-obs:…` and literal `|workstream-gap|workstream-gap|` adjacency in the raw recall dump can **only** arise from **distinct nested `overseer-obs:…` strings** (each a different full string embedding its own `workstream-gap`, each unequal and thus surviving `dedup()`) — the **D1 self-observation-nesting fingerprint, not a duplication bug**. The self-feed loop is intact and unguarded at HEAD (`observation_signature → write_back_observation` gated only by in-memory `WhisperGate(900,5)` → later recall → `signal.rs:463` emits `RecurringSignature` at `≥2` → `classify_signal` mints a Problem whose `dedup_key` **is** that `overseer-obs:…` string, `mod.rs:1359` → `wiring.rs:301` passes **all** `cycle.problems` back into `write_back_observation` with **no exclusion filter** → the key nests one level deeper). D1 fix seam unchanged and still valid: exclude recall-derived (`overseer-obs:` / `RecurringSignature`) dedup_keys from the set fed to `observation_signature` at `mod.rs:546`.

- **26.2 — RE-CONFIRMED (secondary): nothing landed, and the `store_fact_with_caller_key(root_cause_signature(...))` one-liner (§6.2b) REMAINS A TRAP — do not adopt.** No production `.rs` remediation has merged since `dea65df8`; the only `.rs` change to HEAD is `src/overseer/tests_root_cause.rs` (a **test**, `+99`). Every load-bearing defect is **live at HEAD**: `record_occurrence` still calls **non-idempotent `store_fact`** (`mod.rs:1034`, ratchet unfixed — which, per §25.2, is *correct*: the append is the durable accrual, so this "unfixed ratchet" is actually the de-ratchet trap staying **unsprung**, and that is good); the write boundary at `mod.rs:534-546` still has **no recall exclusion** (D1 unfixed); the escalation gate `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (`mod.rs:1613`, `=3` at `root_cause.rs:33`) unchanged. The §6.2b committed one-liner — swap `store_fact` → `store_fact_with_caller_key(root_cause_signature(...))` — is re-adjudicated a **trap**: `DedupMode::CallerKey` keeps exactly one live fact per key, `recall_occurrences` reads only live facts, and `root_cause_signature` is stable for a repeating cause ⇒ recall collapses to **1 forever** ⇒ `recurrence` can never reach `3` ⇒ the `mod.rs:1613` escalation rung becomes **dead code**. Correct remedy (if a caller-key path is ever taken) = **count-in-content caller-key upsert** (`occurrence_count`/`first_seen`/`last_seen`, escalation reads the field not `recall.len()`) — but §25.3/§26.5 R1 supersede this by recording at the ACT→record boundary instead. The `×2` is **honest**: `RECURRING_SIGNATURE_THRESHOLD=2` (Lane A, episodes) vs `RECURRENCE_ESCALATION_THRESHOLD=3` (Lane B, root-cause occurrences) ⇒ the visible `×2` lands in the `[2,3)` cross-lane dead zone. Token classification re-confirmed benign-drift: `goal:blocked:<id>` load-bearing (the persistent membership set that IS the problem); `overseer-obs:…` load-bearing/signature-inflating (the D1 artifact); `workstream-gap` and `resource:engineer_spawn` **benign membership drift** (fixed-literal dedup_keys, volatile fields in summary only) — never an escalation-correctness defect (Lane B keys on per-problem dedup_key, immune to Lane-A set-hash membership drift).

- **26.3 — NET-NEW architectural reframe (tertiary architect): the recurring signature is the string fingerprint of THREE coupled OODA loops that never close, and the 2↔3 dead zone is an ABSORBING region, not a transient band.** The `×2` is decoupled by construction from escalation; the dead zone `Lane-A ≥ 2 ∧ Lane-B < 3` is absorbing **because the genuinely-stuck goal lands at a terminal, non-recording sink**. The three loops, re-grounded live at HEAD: **Loop 1 — blocked-goal ladder terminating in a non-recording `Report`.** `decide_blocked_goal` (`mod.rs:1603-1631`), first-match-wins: Rung 1 `recurrence>=3 → EscalateBlockedGoal` (recorded); Rung 2 `perpetual && is_no_progress_marker → UnblockGoal` (recorded); Rung 3 `needs_review → EscalateBlockedGoal` (recorded); **Rung 4 `else → Report → ActOutcome::Reported` (NOT recorded** — `Reported` is absent from `outcome_records_occurrence`, `wiring.rs:612-627`, re-read line-for-line at HEAD; the `matches!` arm lists `Launched|Merged|Deployed|IssueFiled|Escalated|Whispered|GoalUnblocked|GoalEscalated|ConflictResolved|GoalTransferred|Audited`, **no `Reported`**). A genuinely-stuck goal that merely misses Rung 2's no-progress marker and Rung 3's `needs_review` flag is misfiled "deliberate," acknowledged, and **can never accrue toward Rung 1**: park → don't record → never escalate → re-observe → park. **Loop 2 — workstream-gap ladder: notify-without-launch.** `WorkstreamCoverage` Decide arm (`mod.rs:1534-1543`) → `FlagWorkstreamGaps` → `act_flag_workstream_gaps` (`mod.rs:884-948`) **notifies operator only** (email+Signal, deduped by `gap_gate` on `workstream-gap:{signature}`); there is **no second rung, no `launch.rs` edge, no `FileIssue`**, and its outcome `WorkstreamGapsFlagged` is **also not** in `outcome_records_occurrence`. The sibling `StepFailure` arm (`mod.rs:1549-1580`) *does* return `LaunchRecipe`, proving the launch edge exists and is simply unwired to the gap arm. Blocked goals are additionally gap-scan-skipped (green: `delegates_blocked_goals_to_goal_health_and_never_reflags_them`), so an under-resourced goal **oscillates** between `workstream-gap` (uncovered) and `goal:blocked` (parked) with **no terminal state on either side**, feeding both recurring families. **Loop 3 — Lane-A ProcessHealth recipe that never touches the specific goal.** `Signal::RecurringSignature` fires at `≥2` (`signal.rs:362,463`) → classifies to a **separate** `ProblemKind::ProcessHealth` (`mod.rs:1353-1363`) → `Intervention::LaunchRecipe` with the **signature text** as its task — a generic recipe whose dedup key (sanitized signature) differs from `goal:blocked:<id>`, so it never merges into or advances the blocked-goal ladder. This is the operator-visible `×2`; it spins without closing the goal.

- **26.4 — NET-NEW root-cause reconciliation of §25.2: the D0 completion-gate conjunction (`cycle.rs:582-583`) is out-of-scope for Lane-B *accrual* but IS the upstream root cause of the "deliberate" *misclassification* — a genuinely TWO-part root cause.** §25.2 correctly ruled `cycle.rs` OUT-OF-SCOPE for the Lane-B accrual seam (Lane B is self-contained via `record_occurrence`/`recall_occurrences`, does not route through `cycle.rs`). §26 sharpens this without contradicting it: the latch is manufactured **upstream of** `decide_blocked_goal` at the D0 decide/observe seam by the conjunction `if let Some(source) = &memories.completion_evidence` (Gate A, `cycle.rs:582`) `&& if no_progress_investigation_enabled()` (Gate B, `cycle.rs:583` → `no_progress.rs:203-207`). **Gate A (reconciliation seam):** when `completion_evidence == None` — exactly the issue-closed-**without**-linked-merged-PR reconciliation case, or any absent/non-daemon evidence path — the investigated-breaker block is skipped and the goal is parked with a **bare marker and no WHY class**. **Gate B:** if `SIMARD_NO_PROGRESS_INVESTIGATE=off`, the legacy verify-once fallback also authors no WHY class. Either gate failing ⇒ **no WHY classification** ⇒ the goal misses Rung 2 (`perpetual && marker`) and Rung 3 (`needs_review`) ⇒ falls through to the Rung-4 `Report` sink ⇒ Lane B pinned at `0`. So the full root cause is a **conjunction across two seams**: `cycle.rs:582-583` produces the *misclassification* (governing the engineer-loop's WHY/`needs_review`/`reason` inputs to Rungs 2/3), while the **ACT→record boundary** (`wiring.rs:612-627`) independently *starves the accrual* of whatever does reach Rung 4. Both are load-bearing and both must be addressed — this is the precise reconciliation of §25.2 (accrual seam) with the D0 completion-gate framing of §20/§21 (misclassification seam).

- **26.5 — NET-NEW: §25.3's single one-liner is promoted to a dependency-ordered, landing-safe R1→R2→R3→R4 remediation plan that preserves every green test.** Design constraints: preserve every green assertion (esp. `deliberate_operator_block_is_acknowledged_not_symptom`, `decide_routes_workstream_coverage_to_flag_gaps`, `flagged_gap_never_constructs_an_issue_brief`, the two two-lane-decoupling tests); **never** turn a goal-board observation into a per-tick operator page or a new GitHub issue. **R1 — Un-starve Lane-B accrual (atomic, load-bearing, smallest diff; no deps).** Add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`) so a Rung-4 park records; a genuinely-recurring "deliberate" block then accrues `1,2,3,…` and at 3 reaches the **existing** Rung 1 → `EscalateBlockedGoal` (idempotent via `blocked_goal_gate` `escalate:{goal_id}`). Landing-safe: no green test pins `Reported` as excluded; each Report source carries a distinct `dedup_key` (no cross-source collision); first sighting (`recurrence 0`) still Reports, so the deliberate-block test stays green. *Narrower blast-radius variant:* record only when `problem.kind == GoalHygiene` and evidence is `GoalBlocked`, sparing `DeliveryReady`/`QualityRegression` fall-through Reports. **R2 — Close the D0 completion-gate WHY gap (root cause of the misclassification; complements R1).** At the Gate-A/`completion_evidence == None` and Gate-B-disabled paths (`cycle.rs:582-583`), author a **conservative WHY class** (a `DEPENDENCY`/`UNCLEAR-CRITERIA`-style class for issue-closed-without-linked-merged-PR reconciliation stalls) instead of a bare marker, routing the goal to Rung 2/3 on its own merits rather than to the sink. R1 makes recurrence *observable*; R2 stops the misclassification that sends honest-stuck goals to the sink at all — land R1 first (strictly smaller, unblocks the existing ladder), then R2 (reduces how often the sink is reached). **R3 — Optional earlier-surface rung (fills the literal 2→3 band; dep: R1).** Before the terminal `Report`, insert `if recurrence >= 2 && recurrence < RECURRENCE_ESCALATION_THRESHOLD && !needs_review && !(perpetual && marker) → EscalateBlockedGoal` — reuses the idempotent gate ⇒ one notification at the recurrence=2 point the signature already flags. **Do NOT lower `RECURRENCE_ESCALATION_THRESHOLD`** — that escalates honest transients and still does nothing while Lane B sits at `0`. **R4 — Add the missing workstream-gap closing edge (no deps; largest, land last).** Mirror the proven `StepFailure → LaunchRecipe` pattern: **keep** `WorkstreamCoverage → FlagWorkstreamGaps` for first/below-threshold (preserves gap-scan tests + `Routine` risk class) and **add** a rung firing only when a **per-gap** signature (`GapItem.signature`, the key `gap_gate` already uses at `mod.rs:901,932` — **never** the bare `"workstream-gap"` constant at `mod.rs:1371`) has recurred `≥2×`, routed through the existing `launch.rs` edge and classified at `LaunchRecipe`'s non-`Routine` risk tier so autonomy/budget and `max_launches_per_cycle` govern it. **Landing order: R1 → R2 → R3 → R4** (with the `overseer-obs:` de-nesting for D1 orthogonal and independent). This supersedes §25.3 (which named only Step 1 + optional Step 2) by adding the R2 root-cause-misclassification fix and the explicit four-step dependency ordering.

**§26 delta:** verdict unchanged across all nineteen waves — the `×2` is an **honest, test-locked** re-observation (Lane A `RecurringSignature.occurrences`, `signal.rs:455-469`) of a static, under-resourced, non-advancing problem set, **not** a dedup/replay/collision artifact; the **defect is the response**, not the counter. This wave (two VALIDATE dives, HEAD `d00e4c3f`, zero non-test source drift) contributes: **(1)** an independent structural re-proof at a HEAD 1 commit newer that the doubling is **D1 nesting, impossible from per-token duplication**, via the two HEAD-live invariants `keys.dedup()` (`mod.rs:1070-1071`) + `orient` merge-before-signature (`mod.rs:1211-1213`) (§26.1); **(2)** a re-confirmation that **nothing landed** and the `store_fact_with_caller_key` §6.2b one-liner **remains a trap** (would make `>=3` dead code) — with the "unfixed ratchet" re-read as the de-ratchet trap staying correctly *unsprung* per §25.2 (§26.2); **(3)** the **three-non-closing-loops** architectural reframe proving the 2↔3 dead zone is an **absorbing region** anchored on the non-recording Rung-4 `Report` sink (`wiring.rs:612-627`, `Reported` absent — re-verified line-for-line), the notify-without-launch gap ladder, and the generic Lane-A ProcessHealth recipe (§26.3); **(4)** the **root-cause reconciliation** that resolves §25.2's "`cycle.rs` out-of-scope" ruling into a precise **two-seam conjunction** — `cycle.rs:582-583` is the *misclassification* root cause (WHY-gate ⇒ bare park ⇒ Rung-4) while the ACT→record boundary is the *accrual* starvation, both load-bearing (§26.4); and **(5)** the promotion of §25.3's one-liner into the dependency-ordered **R1 (record the park) → R2 (author the WHY class) → R3 (earlier-surface rung) → R4 (gap launch edge)** landing-safe plan, threshold moves re-rejected (§26.5). D1/D2/D3 remain live and unremediated; the L0→L1→L2→L3 whole-loop remediation order (§16.3) stands, with the D2 target now split into the R1 accrual seam + the R2 upstream WHY-gate seam; **no production `.rs` changed; no remediation landed.**

---

## §27 — Nineteenth-wave (continued): closing convergence verdict + fixpoint (HEAD `2191fcd2`, base dive HEAD `d00e4c3f`) — validation-only, folds the tip minimal-remediation dive

This wave folds the remaining parallel deep dives — two at base HEAD `d00e4c3f`
([`secondary_nesting_vs_duplication_VALIDATE_HEAD_d00e4c3f.md`](./secondary_nesting_vs_duplication_VALIDATE_HEAD_d00e4c3f.md),
[`tertiary_architecture_NONCLOSING_LOOPS_DEADZONE_D0_HEAD_d00e4c3f.md`](./tertiary_architecture_NONCLOSING_LOOPS_DEADZONE_D0_HEAD_d00e4c3f.md))
and one at the current branch tip
([`tertiary_architecture_MINIMAL_REMEDIATION_HEAD_2191fcd2.md`](./tertiary_architecture_MINIMAL_REMEDIATION_HEAD_2191fcd2.md)) —
and closes the investigation at a **fixpoint**. All three were **validate-don't-re-derive** mandates; none
landed a fix nor found a new mechanism. The tip dive independently re-confirms the D1/D2/D3 minimal-remediation
geometry (no over-engineering) and reclassifies `resource:engineer_spawn` as **benign membership drift**, not a
contradicting signal. Ground-truth re-checked live at the current branch tip `2191fcd2`.

### 27.1 — Ground-truth re-verification (live at HEAD `2191fcd2`)
- `git diff --name-only cc55a6fb..HEAD -- '*.rs'` → **empty**; `git diff --name-only d00e4c3f..HEAD -- '*.rs'` → **empty**;
  `git status --porcelain -- src/` → **clean**. No production `.rs` changed anywhere in the corpus; the only non-doc
  `.rs` delta across the whole investigation remains the test `src/overseer/tests_root_cause.rs`.
- The two load-bearing citations were re-read byte-for-byte at HEAD and **hold exactly**:
  - `observation_signature` (`src/overseer/mod.rs:1068-1073`) — `sort_unstable()` → `dedup()` → `format!("overseer-obs:{}", keys.join("|"))`. **Sole producer** of the `overseer-obs:` prefix and `|`-join.
  - `outcome_records_occurrence` (`src/overseer/wiring.rs:612-627`) — arm lists `Launched|Merged|Deployed|IssueFiled|Escalated|Whispered|GoalUnblocked|GoalEscalated|ConflictResolved|GoalTransferred|Audited`; **`ActOutcome::Reported` is absent.** The Rung-4 terminal sink still never records.

### 27.2 — Net-new from the d00e4c3f dives (nil mechanism; one framing sharpening)
- **Secondary (VALIDATE):** re-confirms **"D1, not duplication"** at HEAD — the doubled `overseer-obs:…|overseer-obs:…`
  and literal `|workstream-gap|workstream-gap|` are the **positive fingerprint of D1 self-observation nesting** and are
  **structurally impossible** from per-token duplication (Orient merges same-`dedup_key` signals; `keys.dedup()` collapses
  adjacent equals ⇒ each family key appears at most once per snapshot). Verdict: the `×2` is an **honest re-observation tally**, not a dedup/replay/collision bug. **Re-confirmed, zero drift.**
- **Tertiary (Architect):** consolidates the root cause as a **D0 conjunction** — a goal parked with **no WHY-class**
  is routed to Rung 4 `Report` → `ActOutcome::Reported`, and `Reported` is excluded from occurrence recording, so
  **Lane-B `recurrence` can never leave 0**, Rung 1 (`>=3`) is unreachable, and the goal re-observes/re-parks forever
  (self-sealing). This is the same mechanism as §25.2, stated as a single load-bearing conjunction: *bare-park (D0 WHY-gate)
  ∧ terminal-non-recording-sink (Rung 4 `Reported ∉ outcome_records_occurrence`)* ⇒ the absorbing dead zone Lane-A ≥ 2 ∧ Lane-B < 3.

### 27.3 — Settled answer (unchanged across nineteen waves)
1. **What the signature is:** the overseer's own **observation write-back signature** (`observation_signature`, `mod.rs:1068-1073`) — a sorted/deduped `|`-join of the current cycle's problem `dedup_key`s, prefixed `overseer-obs:`. It is a *faithful fingerprint of a static, unresolved problem set*, not a raw memory key.
2. **Why "2×":** two decoupled counters. Lane A (`RECURRING_SIGNATURE_THRESHOLD = 2`, `signal.rs:362`) makes recurrence **visible** at 2 but never remediates; Lane B (`RECURRENCE_ESCALATION_THRESHOLD = 3`, `root_cause.rs:33`) is the only escalation gate and keys on a quantity the composite never increments. The signature parks in the absorbing **`[2,3)` dead zone**.
3. **Why it recurs:** three coupled OODA loops never close — (L1) blocked-goal ladder Rung 4 `Report` is a terminal non-recording sink; (L2) `WorkstreamCoverage` is notify-only (`FlagWorkstreamGaps`, `mod.rs:1543`) with no closing edge; (L3) the overseer recalls and re-observes its own `overseer-obs:` bookkeeping (bounded self-feed). The problem set never changes, so the fingerprint repeats.
4. **The `×2` is HONEST** (test-locked): not a dedup/storage/replay artifact (H0 REJECTED; H1–H8 SUPPORTED; overseer suite 361/0).

### 27.4 — Minimal remediation (settled; none landed)
- **D2 (make the dead zone reachable):** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`) so acknowledged parks accrue toward Rung 1 — landing-safe (no test pins the exclusion; first observation still Reports).
- **D3 (close the gap loop):** give `WorkstreamCoverage` a recurrence-aware ladder (1× Notify / ≥2× `LaunchRecipe` / ≥3× Escalate), keyed on **`GapItem.signature`** (INV-GAP-KEY), as an **additive** Decide arm (never swap `FlagWorkstreamGaps` — `tests_gap_scan.rs:852` hard-asserts it).
- **D1 (stop self-ingestion):** a single-function write-boundary self-provenance filter in `write_back_observation` (`mod.rs:534-563`) dropping `overseer-obs:`-keyed recall-derived problems before `observation_signature`.
- **L0 (prerequisite):** close the WHY-reasoner wiring so bare parks carry their real WHY down the ladder. Whole-loop order: **L0 → D2/D3 → D1**. **Do not** blind `unblock-all` (operator-rejected antipattern, `mod.rs:1588`).

### 27.5 — Convergence / closure verdict
The investigation has reached a **fixpoint**: the last several waves are docs-only re-groundings that reproduce the
identical verdict against byte-identical production source. **Continuing to spawn parallel re-observation waves that
never land a fix is itself an instance of the very pathology under study** — an over-aggregated composite observed `N×`
and re-emitted while the underlying problem set (here: the unremediated D1/D2/D3 defects) never changes. The correct
next action is **not another investigation wave** but to **land the D2 one-line fix** (lowest-risk, unblocks Lane-B
recording) behind the L0 WHY-wiring prerequisite, then D3 and D1. **Investigation: COMPLETE. Remediation: NOT STARTED — this is the sole open item.**

---

## §28 — Twentieth-wave net-new findings (HEAD `a0c5ed4c`/`856f854b`, zero non-test source drift) — three parallel VALIDATE-not-rederive dives (primary emission-pipeline trace, secondary dedup/recurrence REVALIDATION, tertiary OODA-loop-map + missing-unblock-rung + `routing.rs` self-ingestion). The load-bearing contributions are: **(1)** a drift correction that **partly supersedes the "resolution ladder is double-gated OFF" framing for the goal-blocked lane** — at HEAD the no-progress root-cause investigation is **ON by default** and the issue-#17 already-blocked re-investigation path exists, so the *goal-blocked* loop is narrowed and the **`workstream-gap` notify-only quarantine (L3) is now the single load-bearing non-closing loop**; **(2)** the sharpening that the `Blocked → active` `UnblockGoal` rung is **present-but-unreachable** for the issue-17 cluster (it fires only on `perpetual && is_no_progress_marker`, false for a real AlreadyComplete/MissingPrecondition block); and **(3)** the **`routing.rs` "receiver built, caller never wired" discovery** — `stewardship::route_failure` was explicitly built to accept the Overseer's `"overseer"` gap briefs but is reachable only from `process_orchestrator_run`, never from the Overseer gap Act.

All three dives re-grounded every load-bearing citation **live at HEAD** (`a0c5ed4c` for the two `HEAD_a0c5ed4c` dives, `856f854b` for the tertiary). Drift re-check: `git diff --name-only 6e3113bc..HEAD -- '*.rs'` returns **only** `src/overseer/tests_root_cause.rs` (a test); `git status --porcelain -- src/` is **clean**. All non-test production source is byte-identical to the §22–§27 grounding. Verdict unchanged across all twenty waves; D1/D2/D3 remain live and unmerged; no remediation landed. The net refinements are §28.1–§28.4.

- **28.1 — RE-GROUNDED at HEAD `a0c5ed4c` (primary, 11/11 citations ✅ exact) with a load-bearing DELTA: the "resolution ladder double-gated OFF" framing is PARTLY SUPERSEDED for the goal-blocked lane, and `workstream-gap` is now named the single load-bearing non-closing loop.** The full emission pipeline was re-traced link-by-link and every citation re-read at HEAD: `observation_signature` sort→dedup→`overseer-obs:{join("|")}` (`mod.rs:1068-1073`, fmt @1072, **sole producer**); single write-back call site (`wiring.rs:301 → mod.rs:534`); write-back gate `WhisperGate::new(900,5)`, commit-after-store (`mod.rs:299`, `:548-557`); gate `last_delivered` in-process `HashMap` (`guardrails.rs:291-329`); `record_observation` embeds `[sig:…]` via `store_episode` (`wiring.rs:1076-1091`); recall parses `[sig:…]` back to `failure_signature` (`wiring.rs:976-986`, `:1013-1030`); `RecurringSignature` emitted at `occurrences>=2`, threshold `2` (`signal.rs:455-469`, `:362`); admitted with `dedup_key=sanitize_recalled(sig)` + verbatim message `mod.rs:1361`; `goal:blocked:{goal_id}` (`mod.rs:1336`); `workstream-gap` literal, count→summary (`mod.rs:1371`); `WorkstreamCoverage` Decide = notify-only `FlagWorkstreamGaps` (`mod.rs:1534-1543`, `:884-948`). **The DELTA:** the no-progress breaker's root-cause investigation is **ON by default** — `no_progress_investigation_enabled()` (`no_progress.rs:200-203`, `SIMARD_NO_PROGRESS_INVESTIGATE` defaults on) — and an **already-blocked re-investigation path** exists (issue #17, `reinvestigate_bare_blocked_goals`, wired in `ooda_loop/cycle.rs`). This **narrows** the goal-blocked loop for goals parked with a bare `[OODA-SAFEGUARD]` marker, so the FINAL_SYNTHESIS "double-gated off" framing is superseded for that lane. The **`workstream-gap` notify-only quarantine is unchanged** and is now the **single load-bearing non-closing loop** at HEAD; the `goal:blocked:*` recall self-feed remains the source of the nested `overseer-obs:…` fragments. Consistent with §23/§26.4: the default-on kill-switch (Gate B) means the live bare-park condition is the **`completion_evidence` gate (Gate A)**, not a missing/disabled subsystem.

- **28.2 — RE-VALIDATED at HEAD `a0c5ed4c` (secondary, zero citation drift) with a store-discipline sharpening and a present-but-unreachable-rung NET-NEW.** The store exposes **exactly three write disciplines**, and the recurrence lanes deliberately use the weakest: (1) **`store_fact`** (`library_adapter.rs:657-683`) — append, no dedup — **used by `record_occurrence`** (`mod.rs:1034`), an append-only ratchet; (2) **`store_fact_with_caller_key`** (`library_adapter.rs:870-915`) — CallerKey supersede, **exactly one live fact per key** (comment `:885-889`) — **used on neither recurrence lane**; (3) the in-process **`WhisperGate`** — the only dedup actually applied to recurrence, a per-process `HashMap` with a `900 s` window that **expires every 900 s and dies on restart**. Consequence: idempotency is *available* (CallerKey) but the recurrence paths bypass it; the dedup that *is* present (WhisperGate) is non-persistent. So Lane A's missing idempotency is a **cross-restart expiry gap** (in-process gate only) and Lane B's is an **append ratchet** — **audit the closing action, not the counter.** The §6.2b `store_fact_with_caller_key(root_cause_signature(...))` remedy is re-confirmed a **live trap** (`mod.rs:1034` still `store_fact`; CallerKey would collapse `recall.len()→1` forever ⇒ the `mod.rs:1613` `>=3` escalation rung becomes **dead code**); correct remedy = **count-in-content upsert** (`occurrence_count`/`first_seen`/`last_seen`, escalation reading the field). **NET-NEW (§5.1):** the `UnblockGoal` rung **DOES exist but is present-but-unreachable** for this cluster — `decide_blocked_goal`'s `UnblockGoal` arm (`mod.rs:1620-1621`) fires only when `perpetual && is_no_progress_marker(reason)`; for issue-17 (a real AlreadyComplete/MissingPrecondition block) that predicate is **false**, so the goal is never auto-unblocked → it re-parks and re-emits forever. Two open verification questions handed forward: (a) is Lane B's ACT path actually reached for these goals (starved dead-zone vs over-ratcheting)? (b) should recall-derived `overseer-obs:`-keyed `ProcessHealth` problems be excluded from `write_back_observation` (`wiring.rs:301`) to stop self-nesting without touching genuine recurrence signalling?

- **28.3 — NET-NEW architectural map (tertiary architect, HEAD `856f854b`): model the defect as THREE INDEPENDENT MISSING GRAPH EDGES, not one bug — and the load-bearing L3 discovery that the `route_failure` receiver was built for Overseer gap briefs but its caller edge was never wired.** The recurring signature is the faithful fingerprint of an OODA loop that **Observes and Decides but does not Close**, via three structural seams: **L1 — self-ingestion (Memory→Observe re-entry), the only *growing* loop:** the Overseer writes its own episode with a recoverable `[sig:overseer-obs:…]` marker (`wiring.rs:1076-1091`, `source_label "overseer"` `:952`) and recalls it with **no self-provenance filter** (`recall_episodic` `wiring.rs:1013-1031`; `parse_failure_signature` `:976-986`), so its output re-enters Observe, `sanitize_recalled` keeps the prefix (`capabilities.rs:468-482`), and `observation_signature` **re-wraps** an already-`overseer-obs:`-prefixed key (`mod.rs:1068-1073`) ⇒ nesting; the 900 s gate cannot brake it because each generation mutates the signature. Two absent cut-vertices: a recall provenance filter (exclude `source_label=="overseer"`) **and/or** an idempotent re-wrap (do not re-prefix). **L2 — missing unblock rung (Decide→Act):** the rung exists (`Intervention::UnblockGoal` `mod.rs:1621`; `GoalCurator::unblock` `capabilities.rs:419-427`) but is **starved on two axes** — Axis A, the WHY reasoner is double-gated (`cycle.rs:582-702`: Gate A `completion_evidence.is_some()` + Gate B `no_progress_investigation_enabled()`) and fails **open to a bare park** when Gate A is off, with the re-investigation pass **inside the same double-gate**; Axis B, the escalation `recurrence` counter is a **decoupled, starved, non-idempotent ratchet** on a different storage lane than the operator-visible `×2` (latches to `EscalateBlockedGoal` at `>=3` and never falls back to `UnblockGoal`). **L3 — dangling routing edge (Act→`routing.rs` never connected), the load-bearing discovery:** `WorkstreamCoverage` is the **only** High-priority Decide arm whose Act is **notify-only** (`FlagWorkstreamGaps` `mod.rs:1534-1543`; `act_flag_workstream_gaps` `mod.rs:884-948` — no `FileIssue`, no `LaunchRecipe`, no `route_failure`), yet `stewardship::route_failure` (`routing.rs:39`) was **explicitly built to accept the Overseer's `"overseer"` gap briefs** (its `DEFAULT_TARGET_REPO` docstring `routing.rs:11-15`) — but it is reachable **only** via `process_orchestrator_run` (`stewardship/mod.rs:75`), **never from the Overseer gap path**. **The receiver exists; the caller edge was never wired**, so the gap is doubly quarantined and re-emits the **bare family key** `"workstream-gap"` (`mod.rs:1371`; the per-gap `signature` reaches only the summary/gate) every window. **Fixing any one edge shrinks the composite but does not stop recurrence:** L1 stops the growth/nesting, L2 stops the `goal:blocked` re-parks, L3 stops the `workstream-gap` re-emits — three independent, additive fixes (map 1:1 onto D1/D2/D3), each with the `store_fact_with_caller_key` trap (ledger §2) and the INV-GAP-KEY caveat (`GapItem.signature`, never the bare constant) re-affirmed.

- **28.4 — RE-CONFIRMED cluster co-occurrence: one shared STRUCTURAL cause, not one shared upstream dependency.** An under-resourced important goal **oscillates** between two seams — while active-but-uncovered it emits `workstream-gap` (L3, `sensor.rs:288-320`, blocked goals skipped at `:300-302`); once the no-progress breaker parks it, it leaves gap-scan and reappears as `goal:blocked` (L2). This is why the `kgpacks-rs` issues (12/17/18/23/25 + parity), the `simard-identity` personas, the coverage audit, and the coin harness all appear in **both** recurring families and land in the same composite episode: they share **one structural cause — two non-closing rungs (L2, L3) feeding one non-braked observation loop (L1)** — not a common upstream blocker. The issue-17 (ws2 int8/PQ embed) block is best classified as a **real, persistent block the loop cannot resolve, amplified by an over-counting recurrence lane** — *real block, artifact-inflated magnitude*, not a pure observation artifact.

**§28 delta:** verdict unchanged across all twenty waves — the `×2` is an **honest, test-locked** re-observation (Lane A `RecurringSignature.occurrences`, `signal.rs:455-469`) of a static, under-resourced, non-advancing problem set, **not** a dedup/replay/collision artifact; the **defect is the response**, not the counter. This wave (three VALIDATE dives, HEAD `a0c5ed4c`/`856f854b`, zero non-test source drift) contributes: **(1)** a drift correction that **partly supersedes the "resolution ladder double-gated OFF" framing for the goal-blocked lane** — no-progress investigation is default-on and the issue-#17 re-investigation path exists — leaving `workstream-gap` L3 as the **single load-bearing non-closing loop** (§28.1); **(2)** the store-discipline sharpening (three write disciplines; Lane A = cross-restart expiry gap, Lane B = append ratchet) and the **present-but-unreachable `UnblockGoal` rung** for the issue-17 cluster (fires only on `perpetual && marker`), with the `store_fact_with_caller_key` trap re-confirmed live (§28.2); **(3)** the **three-missing-graph-edges** architectural framing (L1/L2/L3 = D1/D2/D3, independent and additive) anchored on the load-bearing **`routing.rs` "receiver built, caller never wired"** discovery — `route_failure` accepts `"overseer"` briefs but is never called from the Overseer gap Act (§28.3); and **(4)** the re-confirmation that the cluster co-occurs from **one shared structural cause**, not a common upstream dependency (§28.4). D1/D2/D3 remain live and unremediated; the §27.5 fixpoint stands: **Investigation: COMPLETE. Remediation: NOT STARTED.** The correct next action is to **land** the L1/L2/L3 (D1/D2/D3) fixes — spawning another parallel re-observation wave is itself an instance of the pathology under study. **No production `.rs` changed; no remediation landed.**

## §29 — Twenty-first-wave net-new findings (HEAD `973c294b`, zero non-test source drift) — three parallel VALIDATE-not-rederive dives (secondary two-loops re-verification, secondary gap-coupling + distinct-goal roster, tertiary-architect root-cause synthesis + split verdict) plus a re-run regression baseline. The load-bearing contributions are: **(1)** a **scope correction** that reconciles §28.3 — `stewardship/routing.rs` is **not** the workstream-gap loop; the real notify-only gap loop lives *entirely* in `overseer/mod.rs`, and `routing.rs` is a total source→repo router that is a *candidate remediation seam*, not the current live loop; **(2)** the investigation is declared at a **21-wave fixpoint** with a crystallized **split verdict** — NO FIX to the signature/counter (honest nesting), MINIMAL FIX to the response — reducing the entire remediation to **one load-bearing open item: land D2 (a one-line, test-safe change)**; and **(3)** a re-run regression baseline at HEAD confirming **zero `.rs` drift** and green suites, re-pinning that every defect is design-level and contract-locked (intended-as-built), so remediation is a *design decision*, not a bugfix.

- **29.1 — Two non-closing loops RE-VALIDATED at HEAD `973c294b` (secondary, zero `.rs` drift since `f455c06d`) with a load-bearing SCOPE CORRECTION on `routing.rs`.** `git diff --name-only f455c06d..HEAD -- 'src/**/*.rs'` is **EMPTY** (production and test byte-identical); the only drift in the whole prior window is a +test-only commit (`f9cefec1`, `tests_root_cause.rs`) that merely *encodes* the lane-isolation finding as the regression test `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`. Twelve load-bearing citations re-read exactly at HEAD (no stale citations). **Scope correction (reconciles §28.3):** the strategy named `stewardship/routing.rs` for the gap loop, but read in full (52 lines) it is a **total source-module → target-repo router** (`route_failure`, `DEFAULT_TARGET_REPO = Simard`) that never errors and is unrelated to gap emission/notification — **the real notify-only gap loop lives entirely in `overseer/mod.rs`** (`WorkstreamCoverage` arm `1534-1543` → `act_flag_workstream_gaps` `884-948`). So §28.3's "receiver built, caller never wired" stands as a *remediation-seam* observation (route_failure *could* accept the `"overseer"` briefs), but the current L3 loop is not in `routing.rs`; the verification phase must not chase it. **Loop A (blocked-goal park) dead zone re-confirmed:** the `decide_blocked_goal` ladder (`mod.rs:1603-1631`) has exactly one *closing* rung — `UnblockGoal`, and it fires only for `perpetual && is_no_progress_marker(reason)` (rung 2); a genuine block at recurrence 1–2 that is neither a perpetual false-park nor `needs_review` lands on rung 4 `Report` (terminal), and even the `>=3` `EscalateBlockedGoal` rung is a **dedup'd operator notification, not block removal** (`act_escalate_blocked_goal`, `mod.rs:810-836`). **Loop B (workstream-gap)** is the **only** High-priority "work uncovered" arm routing to neither `LaunchRecipe` nor `FileIssue`; its convergence machinery already exists and is exercised by four sibling arms (DeliveryReady→VerifyAndMergePr, QualityRegression→FileIssue, ProcessHealth/CrossCutting/StepFailure→LaunchRecipe) — a fix **reuses** it, does not redesign the loop. **Regression baseline (run at HEAD 973c294b):** `tests_root_cause`, `tests_gap_scan`, `tests_goal_health`, `ooda_loop::tests_no_progress` → **78 passed, 0 failed**. Verdict unchanged: a **control-loop convergence defect, not a counting/dedup bug** — extend, do not restart.

- **29.2 — Gap-spawn/blocked DECOUPLING re-confirmed live + finalized distinct-goal roster (secondary, VALIDATE @ `973c294b`).** Independently re-read each cited line and ran the gap-scan module. **Gap-spawn is DECOUPLED from the blocked-goal transition** — confirmed at `sensor.rs:299-302` (`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }`): there is **no `blocked → gap` emission loop**; the `overseer-obs:…|goal:blocked:…|workstream-gap` co-occurrence is **two independent non-closing OODA loops observing the SAME under-resourced goal set** (matches DISCOVERIES #5, "two signatures are one problem in two views"). `workstream-gap` is the **true terminal blocker (L3/D3)** — `act_flag_workstream_gaps` is notify-only (`mod.rs:884-948`: no `FileIssue`, no `LaunchRecipe`, no `route_failure`). The **`2×` is honest** and parks in the **dead zone `[2,3)`** (emit at `RECURRING_SIGNATURE_THRESHOLD=2`, `signal.rs:362`; escalate at `RECURRENCE_ESCALATION_THRESHOLD=3`, `root_cause.rs:33`). **Finalized distinct-goal roster (normalization: strip `overseer-obs:` D1 nesting, collapse identical tokens, key on trailing 8-hex slug):** **13 distinct blocked goals** — 6 kgpacks-rs (issue-17 `7f5afcca` focal; parity `f29bb15c`; issue-12 `dbabd65f`; issue-18 `67828479`; issue-23 `982783ea`; issue-25 `822511ca`), 2 simard infra (coverage-to-70 `4d27c91a`; coin harness `09e65e35`), 5 identity personas (atelier `188553ad`, bursar `d0cb8852`, cartographer `fd69391d`, concierge `3719bfd4`, gastronome `15a90819`) — **plus 1 `workstream-gap` family marker = 14 distinct keys**, ALL **Unresolved/blocked**. **Resolved roster: EMPTY.** The raw signature's massive length is **pure multiplicity, not distinct content**: the same ~13-member set re-emitted across observe windows, with `overseer-obs:` nesting and `workstream-gap|workstream-gap` over-aggregation (INV-GAP-KEY, `mod.rs:1371`) inflating the token stream. **Test evidence:** `overseer::tests_gap_scan` **21/21 pass**, including `delegates_blocked_goals_to_goal_health_and_never_reflags_them` (pins the decoupling) and `flagged_gap_never_constructs_an_issue_brief` / notify-only terminality tests — the behavior is **contract-locked**, so L3 is *intended-as-built* and needs a design decision, not a bugfix. Traps re-affirmed: **INV-GAP-KEY** (key any closing-edge ledger on `GapItem.signature`, not the bare `"workstream-gap"` key, or all gaps fold into one issue) and the **§6.2b `store_fact_with_caller_key` remedy trap** (use a count-in-content upsert, or escalation becomes dead code).

- **29.3 — Root-cause synthesis + SPLIT VERDICT (tertiary architect, HEAD `973c294b`): close the investigation with a decision, not another re-observation.** Every load-bearing citation re-read byte-for-byte, overseer suite re-run, nesting collapse reproduced. **Verdict split by layer:** **(a) On the signature / `×2` count → NO FIX** — `overseer-obs:goal:blocked:…-7f5afcca` seen `2×` is an **honest cross-window re-observation tally of a static, unresolved problem set**, provably **prefix nesting, not duplication** (`observation_signature` `mod.rs:1068-1073` is the *sole* producer of the `overseer-obs:` prefix + `\|`-join, wrapping the base `goal:blocked:<slug>` key `mod.rs:1336`; stripping the prefix yields exactly the base key; duplication is *structurally impossible* because Orient merges same-`dedup_key` signals and `keys.dedup()` collapses adjacent equals). Touching the counter/signature would **hide a true signal**; this half is **answered and closed** (H0 REJECTED, H1 SUPPORTED; `cargo test --lib overseer::` → **361 passed, 0 failed**). **(b) On the response the signal indicates → MINIMAL FIX (three additive edges)** — the reason it recurs forever is **an OODA loop that Observes and Decides but never Closes**, via three independent, additive missing graph edges: **D2** (blocked-goal terminal sink never records — `ActOutcome::Reported` is absent from `outcome_records_occurrence` `wiring.rs:612-627`, so Lane-B `recurrence` never leaves 0 and the `>=3` rung is **unreachable dead code**; the absorbing `[2,3)` dead zone); **D3** (workstream-gap notify-only, Act→routing edge never wired); **D1** (self-ingestion re-wrap, the only *growing* loop, `recall_episodic` `wiring.rs:1013-1031` has no self-provenance filter). **Landing-order-safe remediation L0 → D2 → D3 → D1:** **D2 lands first** as a *one-line, test-safe* change (add `ActOutcome::Reported` to `outcome_records_occurrence`; no test pins the exclusion; the first observation still Reports; the change only lets Lane-B `recurrence` climb toward the existing `>=3` gate) — **avoiding the §6.2b `store_fact_with_caller_key` trap** (CallerKey collapses `recall.len()→1` forever ⇒ `>=3` rung dead; if the ratchet is touched at all, use a count-in-content upsert). **D3** = additive recurrence-aware gap ladder (1× Notify / ≥2× LaunchRecipe via the already-built `route_failure` / ≥3× Escalate), keyed on `GapItem.signature` (INV-GAP-KEY) and **never swapping `FlagWorkstreamGaps`** (`tests_gap_scan.rs:852` hard-asserts it). **D1** = single-function self-provenance filter at the write boundary (`write_back_observation` `mod.rs:534-563`), landed **last** for store-boundary safety (so it does not mask a still-open loop by trimming the fingerprint before D2/D3 drain it). Issue-17 (ws2 int8/PQ embed) classed as **real, persistent block the loop cannot resolve, magnitude inflated by an over-counting recurrence lane** — *real block, artifact-inflated magnitude*.

**§29 delta:** verdict unchanged across all twenty-one waves — the `×2` is an **honest, test-locked** cross-window re-observation of a static, under-resourced, non-advancing 13-goal + 1-gap problem set (14 distinct keys), **not** a dedup/replay/collision/counter artifact; the **defect is the response** (three non-closing OODA edges D1/D2/D3), not the counter or the signature. This wave (three VALIDATE dives at HEAD `973c294b`, **zero non-test `.rs` drift** — `git diff f455c06d..HEAD` and `d187e414..HEAD` both empty; suites re-run green: overseer **361/0**, no_progress **25/0**, four-module baseline **78/0**, gap-scan **21/21**) contributes: **(1)** the **`routing.rs` scope correction** that reconciles §28.3 — the live L3 gap loop is entirely in `overseer/mod.rs`; `routing.rs` is a total source→repo router and a *candidate remediation seam*, not the current loop (§29.1); **(2)** the **finalized 14-distinct-key roster** with an **EMPTY resolved roster** and the "massive length is multiplicity, not content" proof, re-confirming the `blocked ⟂ gap` decoupling live at `sensor.rs:299-302` (§29.2); and **(3)** the **crystallized split verdict** — NO FIX to the honest signal, MINIMAL FIX to the response — reducing remediation to **one load-bearing open item: land D2** (one line, test-safe), behind the narrowed L0 WHY prerequisite, then D3 (additive, INV-GAP-KEY, never swap `FlagWorkstreamGaps`), then D1 (self-provenance filter, landed last) (§29.3). D1/D2/D3 remain live and unremediated; the §27.5/§28 fixpoint stands: **Investigation: COMPLETE. Remediation: NOT STARTED — the sole open item is to land D2.** Spawning another parallel re-observation wave is itself an instance of the pathology under study. **No production `.rs` changed; no remediation landed.**

## §30 — Twenty-second-wave net-new findings (HEAD `e5257a33`, zero non-test source drift) — three parallel dives (primary end-to-end signature-assembly→emission→write-back→store re-trace, secondary two-loops + self-feed VALIDATE, tertiary-architect landing-safe remediation + drift reconciliation). The verdict is **unchanged at a 22-wave fixpoint** (honest `×2`, defect-is-the-response). The load-bearing net-new contributions are: **(1)** the primary **elevates the self-ingestion nesting (D1) from a provenance footnote to a first-class structural feedback defect** and supplies the decisive **"gap, not design" proof** — the Signal *notify* path already carries a deliberate anti-self-ingest marker (#2631, `notify.rs:1002-1012`) while the *memory write-back* path has **no analogous guard**, an **asymmetry, not an intentional exemption**; **(2)** the primary pins the exact **untested seam** — none of the green write-back/recall tests assert *the composite must not contain a prior `overseer-obs:` signature*, so the self-ingest case is uncovered, and proposes an **assembly-site D1 variant** (filter `overseer-obs:`-prefixed keys inside `observation_signature`, `mod.rs:1068`, mirroring the notify precedent) alongside the existing write-boundary D1; and **(3)** the tertiary adds a **fourth, independent remediation edge — durable-gate hardening** (persist `WhisperGate.last_delivered`, `guardrails.rs:294`) — named as the most-probable source of *exactly* `2×` (per-process gate reset on daemon restart), landing-safe behind the existing peek→store→commit with in-memory fallback.

- **30.1 — Full five-stage self-ingestion loop re-traced link-by-link with the "gap-not-design" precedent (primary, HEAD `e5257a33`).** Re-grounded every cited line live; `git diff --name-only 6e3113bc..HEAD -- '*.rs'` is EMPTY of production changes (all investigation commits docs-only), so prior `src/overseer/*` citations hold. The observed string is a **`RecurringSignature.signature`** rendered by the summary template at `mod.rs:1361` (`"recurring signature seen {occurrences}× in cognitive memory ({signature})"`), i.e. a value the Overseer *previously stored and then recalled*. **Closed five-stage loop:** ① per-problem `dedup_key` assembly in `classify_signal` (`mod.rs:1238+`) — `GoalBlocked→"goal:blocked:{goal_id}"` (`:1336`), `WorkstreamGap→"workstream-gap"` literal (`:1371`), **`RecurringSignature→sanitize_recalled(signature)` = the recalled string verbatim** (`:1353-1359`); ② composite assembly `observation_signature` (`mod.rs:1068-1073`): `sort_unstable → dedup (adjacent-exact only) → "overseer-obs:"+join("\|")`; ③ emission/write-back gate `write_back_observation` (`mod.rs:534-563`) behind `write_back_gate = WhisperGate::new(900,5)` (`:299`), single call site `wiring.rs:301` passing **`&cycle.problems` unfiltered**; ④ store adapter `record_observation` (`wiring.rs:1076-1091`): `store_episode(content="… [sig:S]", "overseer", {signature:S})`; ⑤ recall + re-derivation `recall_episodic` (`wiring.rs:1013`) + `parse_failure_signature` (`wiring.rs:976`) → `signals_from` counts by `failure_signature`, emits `RecurringSignature` at `occurrences >= RECURRING_SIGNATURE_THRESHOLD(2)` (`signal.rs:455-470,362,463`) → back to ①. **Load-bearing refinement — self-ingestion is real and under-weighted:** `sanitize_recalled` (`capabilities.rs:468-482`) strips control chars and caps at **8192 B** but **does NOT strip the `overseer-obs:` prefix**; the write boundary applies **no recall-derived filter**; `keys.dedup()` collapses only *adjacent exact* duplicates. Therefore each window computes `S_new ⊃ S_prev` (the new composite strictly *contains* the prior `overseer-obs:` signature) → **monotonic nesting**, bounded **only by the 8192 B cap, not by any semantic guard**. Near saturation, `sanitize_recalled` truncates on a byte boundary mid-token, producing *drifting* keys that (a) visually look like the same repeated block and (b) defeat the write-back gate's exact-match dedup and destabilize the `RecurringSignature` count → **recurrence can never converge to a single stable signature**, and each ~8 KB episode bloats the multi-writer graph. **The precedent that proves this is a gap, not a design choice:** the Signal notify path already carries an explicit anti-self-ingest marker (#2631, `notify.rs:1002-1012`) so Simard skips its own notifications; the memory write-back path has **no** analogue — an **asymmetry**. **Untested seam:** green tests `write_back_is_deduplicated_within_window`, `write_back_persists_again_for_a_distinct_signature`, `recurring_signature_emitted_when_two_episodes_share_signature` (`tests_memory_recall.rs`) confirm the *intended* halves; **none asserts the composite must not contain a prior `overseer-obs:` signature**. Proposed minimal guard (assembly-site D1 variant, mirrors the notify precedent): filter `!k.starts_with("overseer-obs:")` before join in `observation_signature` (`mod.rs:1068`), keeping `RecurringSignature` fully live for orient/priority-raising (`mod.rs:1217-1219`) while breaking only the self-referential *storage* feedback; add a regression test asserting `observation_signature` never contains a nested `overseer-obs:` substring even when a `RecurringSignature` problem is present.

- **30.2 — Two non-closing loops + self-feed RE-VALIDATED at HEAD with the `+99` test-only delta corroborating decoupling (secondary, VALIDATE-not-rederive).** `git diff --name-only 6e3113bc..HEAD -- '*.rs'` and `5a85317b..HEAD` both return a **single file — `src/overseer/tests_root_cause.rs` (+99, tests only)** — which ADDS `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and `lane_b_escalates_without_any_lane_a_signal`, **reinforcing** the two-lane decoupling verdict; **no production code changed**, every load-bearing citation re-grounded to live line numbers. **Loop #1 (notify-but-never-launch, `workstream-gap`):** token stamped literal at `mod.rs:1371`, kind `WorkstreamCoverage`/`Priority::High` (`:1368-1373`), origin a backlog-coverage gap (`sensor.rs:288 detect_workstream_gaps`, `GoalUncovered` `:311` — **not** a decomposition failure); Decide arm (`mod.rs:1534-1543`) → `FlagWorkstreamGaps` → `act_flag_workstream_gaps` (`:884`) does exactly three things — peek `gap_gate`, send ONE consolidated operator notification (`:929-930`), commit the gate (`:931-934`) — **no `LaunchRecipe`, no `UnblockGoal`, no issue filed**; it is the **only High-priority Decide arm with no convergence edge** (contrast `StepFailure→LaunchRecipe mod.rs:1565`, blocked-recurrence→`EscalateBlockedGoal :1613`). **Loop #2 (park-without-classify, `goal:blocked:<slug>-<hash>`):** token at `mod.rs:1336` (`— needs human review` appended when `needs_review` `:1339-1343`); the WHY-classification ladder is **double-gated** in `ooda_loop/cycle.rs:582` (Gate A `completion_evidence`, Gate B `no_progress_investigation_enabled()`) — only when BOTH pass do `apply_no_progress_breaker_investigated` + `reinvestigate_bare_blocked_goals` run the reasoner and route `resolution_for_why` (`no_progress_breaker.rs:384`); if either gate fails, control parks a **bare `[OODA-SAFEGUARD] … needs human review`** block — the exact recurring token — and even on success `decide_blocked_goal` (`mod.rs:1603`) only *notifies* (`EscalateBlockedGoal :1613/:1623`, `Report :1630`), never RESOLVES, so `GoalBlocked` (`signal.rs:441`) re-fires next window. **Recurrence dead zone:** Lane A visible at `RECURRING_SIGNATURE_THRESHOLD=2` (`signal.rs:362`) vs Lane B escalation floor `RECURRENCE_ESCALATION_THRESHOLD=3` (`root_cause.rs:33`, gate `mod.rs:1613`) → the absorbing `[2,3)` band, now positively pinned by the two added tests (lanes are decoupled: Lane A = episodic recall count, Lane B = root_cause occurrences). **Self-ingestion (write boundary):** inline `|`-repetition ≠ occurrence count — `write_back_observation` writes the WHOLE problem set including the recall-derived `ProcessHealth` meta-problem with **no recall-derived filter** (`mod.rs:534`), so nested `overseer-obs:…|overseer-obs:…` runs inflate INLINE length only; the authoritative count stays **2×** (`signal.rs:462-467`). **Signal-vs-defect:** `2×` is an **honest cross-window / daemon-restart re-observation**, not a dedup bug (within-window dedup green, gate correctly keyed but **in-memory/per-process** `last_delivered: HashMap` `guardrails.rs:294`). **Unifying pattern — "two signatures, one root problem":** an under-resourced goal oscillates `workstream-gap` (active) ↔ `goal:blocked` (idle); the sibling set (kgpacks issues 12/17/18/23/25, coverage-audit, coin-harness, simard-identity personas) is **one resourcing/convergence problem viewed twice — fix convergence once, not per-goal**. Verification-phase questions raised: confirm `no_progress_investigation_enabled()` default in the live daemon (Gate B), confirm `completion_evidence` is populated on the parking ticks (Gate A), confirm the operator string == `mod.rs:1361` verbatim.

- **30.3 — Split verdict re-validated + fourth remediation edge (durable-gate); corpus reconciliation PASS (tertiary architect, HEAD `e5257a33`).** VALIDATE-don't-re-derive: every load-bearing citation re-read byte-for-byte, overseer suite re-run. **HEAD source-drift check PASS:** `git diff --name-only 6e3113bc..HEAD -- '*.rs'` → **`src/overseer/tests_root_cause.rs` ONLY** (test-only, additive: the two decoupling proofs); **no production source changed**; a 15-row load-bearing citation table re-grounds ✅ exact at HEAD (incl. `observation_signature mod.rs:1068-1073`, base key `mod.rs:1336`, `RecurringSignature`/summary `mod.rs:1353-1363/:1361`, `WorkstreamCoverage` notify-only `mod.rs:1534-1543`, escalation gate `mod.rs:1613`, `record_occurrence` append `mod.rs:1034`, gates `mod.rs:299,304`, store adapter `wiring.rs:1076-1091`, thresholds `signal.rs:362,463` / `root_cause.rs:33,53-55,79-82`, in-memory gate `guardrails.rs:294`, **`ActOutcome::Reported` ABSENT from `outcome_records_occurrence` `wiring.rs:612-627`**, `FlagWorkstreamGaps` hard-pin `tests_gap_scan.rs:865-868`). **Test re-execution green:** `cargo test --lib overseer::` → **361 passed, 0 failed**; `write_back_is_deduplicated_within_window` → ok; `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` → ok; `lane_b_escalates_without_any_lane_a_signal` → ok. **Bottom line (split verdict, unchanged, re-validated):** (a) signature / `×2` count → **NO FIX** (honest cross-window tally; nesting not duplication; suppressing hides a true signal); (b) response the signal indicates → **MINIMAL FIX (three additive edges + one durability edge)**, whole-loop order **L0 → D2 → D3 → D1**. **L0** (prerequisite, no store change): wire the WHY-reasoner so bare parks carry their real WHY; narrow because no-progress investigation is default-on, so only the `completion_evidence` Gate A still admits bare parks; **do not** blind `unblock-all` (operator-rejected, `mod.rs:1588,:1620-1621`). **D2 (LAND FIRST):** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`) so the terminal Rung-4 `Report` sink records an occurrence and Lane-B `recurrence` can climb toward the existing `>=3` gate (today it stays `0` → `>=3` is dead code, the absorbing `[2,3)` dead zone); one-line, no test pins the exclusion, first observation still Reports. **TRAP (do not take):** the §6.2b one-liner `store_fact_with_caller_key(root_cause_signature(...))` at `mod.rs:1034` collapses recall to **1 forever** (`DedupMode::CallerKey`, `library_adapter.rs:885-889`; `recurrence = recall.len()`, `root_cause.rs:79-82`) → makes `>=3` dead code; if the ratchet is touched, use a **count-in-content upsert** (`occurrence_count`/`first_seen`/`last_seen`). **D3:** give `WorkstreamCoverage` a recurrence-aware **additive** Decide arm (`1× Notify / ≥2× LaunchRecipe via the already-built stewardship::route_failure / ≥3× Escalate`), keyed on **`GapItem.signature`** (INV-GAP-KEY trap: never the bare `"workstream-gap"` key `mod.rs:1371`, or all gaps fold into one issue), and **never swap `FlagWorkstreamGaps`** (hard-asserted `tests_gap_scan.rs:865`) — add alongside. **D1 (LAND LAST):** single-function self-provenance filter in `write_back_observation` (`mod.rs:534-563`) that drops `overseer-obs:`-keyed recall-derived problems and/or makes re-wrap idempotent; land after D2/D3 so it does not mask a still-open loop by trimming the fingerprint before the cause is drained. **Durable-gate (independent 4th edge; may land with D2):** persist `WhisperGate.last_delivered` (`guardrails.rs:294`) across restarts — the in-memory/per-process map starts empty after a restart and the still-true condition **re-records**, the **most-probable source of *exactly* `2×`**; additive persistence behind the existing peek→store→commit with in-memory fallback. **Corpus reconciliation PASS** (production `.rs` drift = tests only; every citation re-grounds; question string == `mod.rs:1361`; within-window dedup green; Lane A ⇏ Lane B; §29/§28 verdict + D2→D3→D1 order matches, no re-derivation; §6.2b flagged as trap; D1/D2/D3 all still open). **Dead-ends avoided:** did not touch `kgpacks-rs` issue-17 (observed target, not subject), did not treat inline pipe-repetition as literal count, did not hunt a within-window dedup bug, did not re-derive the stable corpus verdict.

**§30 delta:** verdict unchanged across all twenty-two waves — the `×2` is an **honest, test-locked** cross-window / daemon-restart re-observation of a static, under-resourced, non-advancing 13-goal + 1-gap problem set, **not** a dedup/replay/collision/counter artifact; the **defect is the response** (non-closing OODA edges), not the counter or the signature. This wave (three dives at HEAD `e5257a33`, **zero production `.rs` drift** — `git diff 6e3113bc..HEAD -- '*.rs'` and `5a85317b..HEAD` both return **only** `src/overseer/tests_root_cause.rs`, +99 test-only lines that *corroborate* the two-lane decoupling; suites green: overseer **361/0**, plus the two net-new decoupling tests ok) contributes three net-new sharpenings: **(1)** the self-ingestion nesting (D1) is promoted from a provenance footnote to a **first-class structural feedback defect** with the decisive **"gap, not design" proof** — the notify path's anti-self-ingest marker (#2631, `notify.rs:1002-1012`) exists but the memory write-back path has **no analogue** (asymmetry, not exemption), bounded only by the 8192 B `sanitize_recalled` cap that *degrades* dedup/recurrence near saturation (§30.1); **(2)** the **exact untested seam** is named — no green test asserts the composite must not contain a prior `overseer-obs:` signature — plus an **assembly-site D1 variant** (filter in `observation_signature mod.rs:1068`) and its regression test (§30.1, §30.2); and **(3)** a **fourth remediation edge — durable-gate hardening** (persist `WhisperGate.last_delivered guardrails.rs:294`) — identified as the most-probable source of *exactly* `2×` and landing-safe behind the existing peek→store→commit (§30.3). The split verdict and landing order **L0 → D2 → D3 → D1 (+ durable-gate)** are re-validated unchanged; the sole load-bearing open item remains **land D2** (one line, test-safe), then D3 (additive, INV-GAP-KEY, never swap `FlagWorkstreamGaps`), then D1 (self-provenance filter, landed last), with durable-gate landable alongside D2. D1/D2/D3 remain live and unremediated; the §27.5/§28/§29 fixpoint stands: **Investigation: COMPLETE (fixpoint, re-validated @ `e5257a33`). Remediation: NOT STARTED — the sole open item is to land D2.** Spawning another parallel re-observation wave is itself an instance of the pathology under study. **No production `.rs` changed; no remediation landed.**
