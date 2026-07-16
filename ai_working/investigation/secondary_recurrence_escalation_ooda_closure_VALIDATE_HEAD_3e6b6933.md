# Secondary Investigation — Recurring-Pattern Detection, Escalation Ladder & OODA Loop Closure

**Role:** SECONDARY investigator.
**Focus:** Recurrence detection, escalation ladder, and OODA loop closure across
`tests_memory_recall.rs / root_cause.rs / intervention.rs / guardrails.rs`.
**HEAD:** `3e6b6933`.
**Drift check:** `git diff --name-only 6e3113bc..HEAD -- 'src/overseer/*.rs'` = **only
`tests_root_cause.rs`** (NOT one of my four focus files). Every citation below is
re-grounded live at HEAD `3e6b6933` and holds.
**Mandate followed:** Per the frozen-code warning I *validated and re-pinned* rather than
re-derived. Prior verdict (2× is an honest cross-window re-observation; the counter is not
the defect — the closing action is) is **confirmed** with fresh line evidence, and I add one
**new drift finding** (doc/impl divergence) the prior waves did not pin.

---

## 1. The OODA loop, traced across the four focus files (validated)

| Phase | Site (HEAD 3e6b6933) | Behavior |
|---|---|---|
| **Observe** | `tests_memory_recall.rs:461-468` (`snapshot_with_episodes`) | recall episodes carry a `failure_signature`. |
| **Orient (detect)** | `signals_from` → `Signal::RecurringSignature { occurrences }` | fires at **≥2** shared-signature episodes. Test-proven: `recurring_signature_emitted_when_two_episodes_share_signature` (`tests_memory_recall.rs:471-491`, `occurrences: 2`); negative at 1 (`:494-507`); ignores unsignatured episodes (`:510-525`). |
| **Orient (promote)** | `orient()` | a `RecurringSignature` becomes a **High** priority `Problem` — proven `orient_raises_recurring_signature_to_high_priority` (`tests_memory_recall.rs:564-579`) and summary is sanitized before egress (`:582-596`). |
| **Decide** | `decide` / `signal_to_problem` (`mod.rs`) | routes each Problem to an `Intervention`. |
| **Act** | `mod.rs` act arms + `guardrails.rs` WhisperGate | notify / self-heal / escalate, gated for dedup. |

**Two independent recurrence counters (cross-lane, re-confirmed):**

- **Lane A — observation episodes** (drives the operator-visible `×2`): `RecurringSignature`,
  threshold **2** (`tests_memory_recall.rs:471-491`).
- **Lane B — root-cause occurrences** (drives escalation): `RootCause.recurrence`, computed in
  `root_cause.rs:79-82` as *count of recalled `PriorOccurrence` whose `cause_label` == primary*,
  threshold **3** = `RECURRENCE_ESCALATION_THRESHOLD` (`root_cause.rs:33`).

The two are wired independently: `decide_blocked_goal` reads **Lane B**
(`mod.rs:1469` → `problem.why.recurrence`, sourced from `root_cause::analyze` at `mod.rs:457`).
The visible `×2` (Lane A) therefore says **nothing** about whether Lane B reached 3. **The
"dead zone at 2" is a cross-lane visibility gap, not a broken threshold.** Verdict from prior
waves **CONFIRMED**.

---

## 2. The escalation ladder — re-pinned, and its missing rung

`decide_blocked_goal` (`mod.rs:1603-1631`) is the entire ladder for a blocked goal:

```
recurrence >= 3 (RECURRENCE_ESCALATION_THRESHOLD)  -> EscalateBlockedGoal { why }   (mod.rs:1613)
perpetual && is_no_progress_marker(reason)         -> UnblockGoal (self-heal)       (mod.rs:1620)
needs_review                                        -> EscalateBlockedGoal { why }   (mod.rs:1623)
otherwise                                           -> Report (bare, no action)      (mod.rs:1630)
```

**Anti-pattern CONFIRMED — "recurrence dead zone":** a goal at Lane-B `recurrence == 2` that is
neither a `perpetual` no-progress park nor `needs_review` falls straight through to
`Intervention::Report` — a **non-closing** terminal. Between "seen twice" (real, above noise)
and the escalation bar of 3 there is **no remediation rung**. This is the exact gap the prior
"recurrence dead zone" pattern named; I have now pinned it to the `else → Report` arm at
`mod.rs:1630`.

The escalation itself is well-formed where it *does* fire: `EscalateBlockedGoal` carries the
one-line WHY (`intervention.rs:64-70`), so the operator sees *why*, not a bare symptom — the
closing edge exists, it is simply gated to recurrence ≥ 3.

### NEW (proven) — the dead zone is *self-sealing*: escalation is UNREACHABLE, not merely late

I traced how Lane B accrues and found the dead zone is worse than "no rung between 2 and 3" — it
**can never reach 3**:

- Lane B only increments via `record_occurrence` (`mod.rs:1004`), whose sole call site is
  `wiring.rs:279`, **gated by `outcome_records_occurrence(&outcome)`** (`wiring.rs:276`).
- `outcome_records_occurrence` (`wiring.rs:612-627`) accrues **only for effective actions**:
  `Launched | Merged | Deployed | IssueFiled | Escalated | Whispered | GoalUnblocked |
  GoalEscalated | ConflictResolved | GoalTransferred | Audited`.
- A dead-zone blocked goal takes `Intervention::Report` (`mod.rs:1630`) →
  `ActOutcome::Reported` (`mod.rs:658`). **`Reported` is NOT in that set** → `record_occurrence`
  is **never called** → Lane B never increments for that goal.

**Consequence:** a blocked goal that lands on the bare-`Report` arm produces no occurrence, so
its recalled `recurrence` stays put and can **never climb from 2 to 3**. The `recurrence >= 3`
escalation at `mod.rs:1613` is therefore **dead code for exactly the goals that fall into the
dead zone** — the loop is self-sealing, not just slow. Only goals *already* being unblocked or
escalated (which accrue) can ever reach the threshold. This sharpens the prior "recurrence dead
zone" pattern from "missing rung" to a **proven unreachable-escalation** defect.

The identical trap hits `workstream-gap`: `act_flag_workstream_gaps` returns
`ActOutcome::WorkstreamGapsFlagged` (`mod.rs:917/944`), which is **also absent** from
`outcome_records_occurrence` — so gap flagging never accrues Lane B either, and a coverage gap
can never escalate by recurrence. D3's "notify-only, never closes" now has a second mechanism:
even the recurrence-escalation backstop is structurally unreachable for it.

---

## 3. The WorkstreamCoverage loop never closes — and a NEW doc/impl drift finding

**Decide arm** (`mod.rs:1534-1543`): `ProblemKind::WorkstreamCoverage` →
`Intervention::FlagWorkstreamGaps { gaps }`. It is the **only** High-priority Decide arm with no
`launch.rs` edge (siblings `StepFailure`, `ProcessHealth`, `CrossCutting` all reach
`LaunchRecipe`). CONFIRMED.

**Act arm** (`act_flag_workstream_gaps`, `mod.rs:884-948`): peeks the gap gate (`:902`),
sends **one consolidated operator notification** (`notifier.notify`, `:925-930`), then commits
the gate (`:931-934`). That is the *entire* action — **no `IssueFiler::file`, no `launch`**. The
gap is observed-and-flagged, never closed. CONFIRMED (this is defect **D3**).

### NEW finding — the intervention doc promises a closing action the code does not take

`intervention.rs:71-78` (the `FlagWorkstreamGaps` doc comment) states the action will:

> "...notify the operator on BOTH channels (email + Signal) with the specifics **AND file one
> deduped issue per gap.** ... Capability: `notify::OperatorNotifier` + **`IssueFiler::file`**."

But `act_flag_workstream_gaps` (`mod.rs:884-948`) **never calls `IssueFiler::file`** and never
launches. The documented closing edge (file-one-issue-per-gap) was **specified but never wired**.
This is a *doc/impl divergence* on top of D3: the intent to close the loop is captured in the
contract, so the remediation is a faithful completion of the stated design, not a new feature.
Prior waves flagged D3 as "notify-only"; **this pins the smoking gun that the closing action was
always intended** — lowering remediation risk.

---

## 4. Why the counter is honest (guardrails re-validated)

`WhisperGate` (`guardrails.rs:291-333`) is a peek→commit dedup gate:
- `last_delivered` is an **in-memory `HashMap`** (`guardrails.rs:294`), initialized **empty** in
  `new()` (`:305`).
- `peek` suppresses within `window_secs` (`:312-324`); `commit` records (`:328-333`).

Because `last_delivered` is per-process, a **daemon restart clears it**, so a still-true
composite re-records exactly once in the new process → the honest `×2`. This is the most probable
source of *exactly* 2×, and it is a correct-by-design consequence, **not** a dedup bug. The gate
covers the *notification/observation* lane only; it has **no cross-window persistent ledger**,
which is why `workstream-gap|workstream-gap` survives every cycle (the D3 tail). CONFIRMED.

---

## 5. Root cause of the blocked `kgpacks` goal (my lane's contribution)

`root_cause.rs::analyze` (`:65-115`) is pure/deterministic and **always returns a usable WHY**
(`:71-73` inject `unknown_candidate` when empty). So a blocked goal is *never* actionless for
lack of a WHY at the analyzer. The failure is **downstream at the ladder**: `recurrence` is
Lane B (`root_cause.rs:79-82`), and unless it reaches 3 or the goal is a `perpetual`
no-progress park / `needs_review`, `decide_blocked_goal` degrades to bare `Report`
(`mod.rs:1630`). Combined with the WHY reasoner being double-gated off upstream
(`ooda_loop/cycle.rs`, per prior waves — outside my four files, not re-pinned here), a
self-resolvable stall collapses to a bare park and re-parks every window → the recurring
`goal:blocked` population. **Confirms** the prior "classify-then-route the stall, don't park it"
pattern; the blocker is a *routing/gating* gap, not missing analysis.

`resource:engineer_spawn` corroborates (benign membership drift; not chased). The spawn↔gap
self-feed is real but is **one under-resourcing problem in two views**, not two defects.

---

## 6. Reconciliation with prior waves & minimal remediation (my lane only)

**Agreements (validated, not re-derived):**
- 2× = honest cross-window re-observation (Lane A, threshold 2). ✔
- Escalation is Lane B, threshold 3; the 2-vs-3 gap is cross-lane visibility. ✔
- `WorkstreamCoverage` is the sole non-closing High arm (D3). ✔
- The counter is not the defect; the **closing action** is. ✔

**Do NOT** apply `store_fact → store_fact_with_caller_key` (RECONCILIATION_LEDGER trap): it
collapses Lane B recall to 1 forever, making `recurrence >= 3` (`mod.rs:1613`) dead code. Carry
occurrence count **in fact content** instead. (Not re-litigated — inherited as settled.)

**Minimal, landing-safe rungs my four files point to (design only, no code changed):**
1. **Close the WorkstreamCoverage loop:** wire the *already-documented* `IssueFiler::file`
   into `act_flag_workstream_gaps` (`mod.rs:884-948`) — one deduped issue per fresh gap, exactly
   as `intervention.rs:71-78` already promises. Lowest-risk because it fulfills the existing
   contract; the gap gate already de-dupes fresh vs. suppressed.
2. **Fill the recurrence dead zone:** add a remediation/escalation rung at first *proven*
   recurrence (Lane-B `recurrence == 2`) for signals with no benign explanation, so
   `decide_blocked_goal`'s `else` arm (`mod.rs:1630`) is not the terminal for a twice-seen
   blocked goal. Do **not** lower the threshold-3 escalation — add a distinct lighter rung.

Both are additive edges, testable against the existing `tests_memory_recall.rs` suite without
touching the OODA architecture.

---

## 7. Questions for the verification phase

1. Does any Decide arm consume `RecurringSignature` (Lane A) into a *closing* action, or does the
   High-priority `Problem` from `orient` (`tests_memory_recall.rs:564-579`) only ever re-notify?
   (My trace found no closing consumer; verify against the full `decide` match.)
2. **RESOLVED during this investigation (see §2, NEW):** `record_occurrence` (`mod.rs:1004`) is
   gated by `outcome_records_occurrence` (`wiring.rs:612-627`), which excludes `Reported` and
   `WorkstreamGapsFlagged`. So dead-zone blocked goals and flagged gaps **never accrue Lane B**
   → `mod.rs:1613` escalation is unreachable for them. Verification should independently confirm
   this outcome-set exclusion and whether it is intended.
3. Confirm the `tests_root_cause.rs` drift since `6e3113bc` does not alter
   `RECURRENCE_ESCALATION_THRESHOLD` semantics (constant still `= 3` at `root_cause.rs:33`; it is
   unchanged, but the test delta should be diffed).
