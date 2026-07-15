# Consolidated Findings — Recurring `goal:blocked` + `workstream-gap` Signature

**Investigation:** the overseer signature seen 2× in cognitive memory:
`overseer-obs:goal:blocked:…|…|workstream-gap|workstream-gap`
**Branch / HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `dea65df8`
**Date:** 2026-07-15  **Status:** Complete — re-validated against current source (HEAD `dea65df8`).

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
Every claim below is re-grounded to a current line in `src/overseer/` (re-verified at
HEAD `dea65df8`; all prior root-cause citations still hold — the one superseded item is
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
