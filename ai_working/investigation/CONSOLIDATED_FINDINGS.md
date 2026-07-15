# Consolidated Findings — Recurring `goal:blocked` + `workstream-gap` Signature

**Investigation:** the overseer signature seen 2× in cognitive memory:
`overseer-obs:goal:blocked:…|…|workstream-gap|workstream-gap`
**Branch / HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `5a85317b`
**Date:** 2026-07-15  **Status:** Complete — re-validated against current source through seven waves
(latest HEAD `5a85317b`; **every** investigation commit is **docs-only** — `git diff --name-only
6e3113bc..HEAD -- '*.rs'` is empty, all changes confined to `ai_working/`, so every source citation
still holds and no fix is merged). Fifth-wave net-new findings (incl. the `resource:engineer_spawn`
membership drift) are folded into §11; sixth-wave net-new findings (the end-to-end pipeline trace and
the **D2→D1→D3 dependency-safe landing order**, plus a reconciliation of the two waves' D-numbering)
are folded into §12; seventh-wave net-new findings (the falsifiable **H0–H8 hypothesis set**, the
executed **per-hypothesis verification matrix**, and the **minimal contained signature-path fix** with
a zero-drift re-grounding) are folded into §13.

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

Every claim below is re-grounded to a current line in `src/overseer/` (re-verified at
HEAD `5a85317b`; all prior root-cause citations still hold exact — the one superseded item is
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

**Fix status (unchanged, re-confirmed at HEAD `5a85317b`):** all seven investigation commits remain
**documentation-only**; defects **D1/D2/D3 stay live in source**. No remediation merged — §6's
three-defect fix, landed in the §12.4-refined **D2→D1→D3 dependency-safe order**, is all remaining
scoped work. **INVESTIGATION-ONLY** — this wave formalizes and verifies; it does not implement.
