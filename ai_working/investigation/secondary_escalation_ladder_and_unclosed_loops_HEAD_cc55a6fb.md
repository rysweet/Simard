# Secondary Investigation — Escalation Ladder, Count-Parks-at-2 Dead-Zone, and Unclosed-Loop Patterns

**HEAD:** `cc55a6fb` (verified). Source drift since prior waves: `git diff --name-only 6e3113bc..HEAD -- '*.rs'`
touched **only** `src/overseer/tests_root_cause.rs` — no non-test `src/overseer/` or `src/ooda_loop/`
source changed. Every citation below re-grounded against live source at this HEAD (no doc-to-doc trust).

**Focus (secondary):** (1) the escalation ladder; (2) why the recurrence count parks at exactly **2**
(the dead-zone); (3) recurring **unclosed-loop** patterns — blocked-goal parking without WHY-classification,
and workstream-gap notify-without-launch. Confirms and refines the prior secondary at `440e024c`.

---

## 1. Two decoupled counters, two different bars — the dead-zone geometry

There are **two independent recurrence counters on two lanes** that never share state:

| Lane | Constant | Location (HEAD `cc55a6fb`) | Counts | Fires at |
|---|---|---|---|---|
| **A — episodic recall** | `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362`; emitted `signal.rs:462-468` | recalled write-back **episodes** whose `failure_signature` string is byte-identical | ≥ **2** |
| **B — semantic root cause** | `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33`; gated `mod.rs:1613` | recalled `PriorOccurrence`s with the same `cause_label` | ≥ **3** |

The visible "**2×**" in the signature (`recurring signature seen 2× in cognitive memory (…)`,
`mod.rs:1360-1361`) is **Lane A** — `Signal::RecurringSignature.occurrences` (`signal.rs:70,464-467`).
The escalation gate in `decide_blocked_goal` reads **Lane B** (`recurrence`, `mod.rs:1608,1613`).

**Why it parks at exactly 2:** Lane A *emits* at occurrences ≥ 2, so the counter becomes *visible* the
moment it reaches 2. But nothing on Lane A ever *escalates or remediates* — the emitted signal only
becomes a `ProcessHealth` advisory problem (see §2). Escalation lives on Lane B and needs 3, keyed on a
different quantity (`cause_label`) that the Lane-A composite never increments. So a signature that recurs
sits **pinned at 2 on the visible lane** while the escalation bar sits at 3 on a lane it can't move. The
"dead-zone" is the interval `[2, 3)` on a counter that has no path to 3.

---

## 2. The escalation ladder, rung by rung — and where it dead-ends

### 2.1 Blocked-goal ladder — `decide_blocked_goal` (`mod.rs:1603-1631`)

This is the **only** closing path for a `goal:blocked:*` problem. Arms in order:

```
recurrence >= 3 (Lane B)          → EscalateBlockedGoal   (mod.rs:1613)   [human]
perpetual && no_progress_marker   → UnblockGoal           (mod.rs:1620)   [blind; re-blocks next cycle]
needs_review                      → EscalateBlockedGoal   (mod.rs:1623)   [human]
else                              → Report                (mod.rs:1630)   [NO-OP]
```

**Dead-zone = `recurrence ∈ {0,1,2}` for any goal that is neither `perpetual+no-progress` nor
`needs_review`.** It falls to `Report` — a no-op — every cycle, forever. Re-observed, re-persisted,
re-classified, re-parked; no convergent action, no trend toward zero. There is **no intermediate
"remediate" rung** between the noise floor and the human-escalation bar. This is the classic
*recurrence dead-zone* anti-pattern.

### 2.2 The "raise-priority" rung is structurally unreachable for the composite (refined)

The intended soft rung is `orient`'s priority-raise: a `RecurringSignature` co-signal merges into an
in-cycle problem **with the same `dedup_key`** and lowers its priority number (`mod.rs:1210-1219`, the
`existing.priority = existing.priority.min(priority)` branch gated on `matches!(s, RecurringSignature)`).

But the `RecurringSignature`'s key is `sanitize_recalled(signature)` where `signature` is the whole-cycle
composite `overseer-obs:…` (`mod.rs:1357`, classify arm), and **no per-goal problem carries that key** —
individual blocked goals key on `goal:blocked:X` (`mod.rs:1336` region). The merge predicate
`p.dedup_key == key` (`mod.rs:1211`) therefore **can never match**. Consequences:

- The 2× never raises the priority of any individual blocked goal it is composed of.
- The meta-problem stands alone → `decide` → `ProblemKind::ProcessHealth` → **`LaunchRecipe`**
  (`mod.rs:1429-1435`) with `task_description = problem.summary` — i.e. the **sanitized composite blob**
  ("fix goal A AND B AND … AND a coverage gap AND an engineer-spawn note"). The **one** cost-bearing
  convergent edge in the whole flow is aimed at a non-actionable aggregate, not at any real goal.

So for the composite the dead-zone is worse than "raise-but-not-escalate": it is **"never raised AND
never escalated,"** exerting **zero remediation pressure on the real goals**.

---

## 3. Unclosed-loop pattern #1 — blocked-goal parking WITHOUT WHY-classification

The WHY reasoner + self-resolving ladder (auto-complete / heal-precondition / defer-upstream /
spawn-one-engineer / escalate-with-WHY) lives in `cycle.rs:582-702`. It is **double-gated**:

- **Gate 1 (data):** the entire block is `if let Some(source) = &memories.completion_evidence` at
  `cycle.rs:582`. If `completion_evidence` is `None`, the `else` at `cycle.rs:700-701` sets
  `breaker_dropped = Vec::new()` — **no WHY classification runs at all**; a stalled goal keeps whatever
  bare `[OODA-SAFEGUARD] … needs human review` block it already had.
- **Gate 2 (kill-switch):** inside, `no_progress_investigation_enabled()` (`no_progress.rs:203`, defaults
  **ON**; `SIMARD_NO_PROGRESS_INVESTIGATE=off` disables) chooses between the investigated ladder and the
  base verify-once breaker (`cycle.rs:684-698`), which only re-blocks bare.

**Anti-pattern:** *park-then-forget without classifying WHY.* The system fails **open to a bare park** —
when evidence is absent the goal is parked with no cause label, so it can never satisfy Lane-B's
`cause_label`-keyed escalation (§1) either. The park is created on one lane and the only escalation reads a
label that the bare park never sets. This is the "Classify-then-route the stall, don't park it"
anti-pattern (PATTERNS.md): the stall is parked, not classified-and-routed.

Note the third "3" in the system: `NO_PROGRESS_BREAKER_THRESHOLD = 3` (`no_progress_breaker.rs:59`,
reused as `INVESTIGATED_BREAKER_THRESHOLD`, `no_progress.rs:1148`) — 3 idle cycles to *create* the park.
So parks arrive on a 3-idle-cycle cadence, then sit in the recurrence dead-zone indefinitely.

---

## 4. Unclosed-loop pattern #2 — workstream-gap NOTIFY-WITHOUT-LAUNCH

`WorkstreamGap` signal → `ProblemKind::WorkstreamCoverage` (`mod.rs:1367-1372`) → intervention
`FlagWorkstreamGaps` (`mod.rs:1534-1543`) → `act_flag_workstream_gaps` (`mod.rs:884-948`).

`act_flag_workstream_gaps` does exactly one external action: `notifier.notify(&notification)`
(`mod.rs:929-930`), then commits the dedup gate (`mod.rs:931-934`). **There is no `launch.rs` /
spawn / recipe edge.** `WorkstreamCoverage` is the **only High-priority Decide arm whose intervention
never launches work** — contrast `ProcessHealth`/`CrossCutting`/`StepFailure`, which all route to
`LaunchRecipe`. The gap is *reported to a human* and then suppressed by the `gap_gate` `WhisperGate`
for the dedup window; the underlying uncovered workstream is never *converted into work*.

**Anti-pattern:** *observe-and-flag without a closing action* (PATTERNS.md). The loop is unclosed:
the same gap re-surfaces each Observe pass, is either re-notified (fresh) or silently suppressed
(within window, `mod.rs:900-908`), and no cycle ever launches the missing workstream. The gap can
persist across unbounded write-back passes.

---

## 5. The self-feed that keeps the loops alive (cross-reference, primary-owned)

Both unclosed loops are perpetuated by the write-back self-observation:
`observation_signature` (`mod.rs:1068-1073`) folds the **entire cycle problem set** — every `dedup_key`,
sorted+deduped+`|`-joined, prefixed `overseer-obs:` — into ONE episode persisted each cycle
(`write_back_observation`, `mod.rs:534-563`; call site `wiring.rs:301`). Because the `RecurringSignature`
meta-problem is itself a `Problem` in the set, its `overseer-obs:…` key re-enters the *next*
`observation_signature`, so the composite (blocked-goal tokens + `workstream-gap` + `resource:engineer_spawn`)
is re-observed and re-written every cycle. Recall counts **episodes by exact composite string**
(`signal.rs:456-460`), so the count ticks toward 2 and pins there (§1). This is the "Self-observation
feedback" pattern (PATTERNS.md) and is the mechanism by which parked goals and un-launched gaps stay
*visible-but-unresolved* forever.

**Detection brittleness corollary:** because the recall key is the *whole-cycle* composite (a logical AND
of the full membership set), any churn (one goal resolves / one new goal blocks / a gap opens or closes)
mutates the composite and **resets the recall count to 1** — a chronically re-blocking goal in a churning
board can evade Lane-A detection entirely. Recurrence is tracked at whole-cycle granularity when it should
be per-`dedup_key`.

---

## 6. Design rationale observed

- The composite signature was designed for **within-window write-back dedup** (#2628, `mod.rs:1064-1067`):
  "two identical observations → same signature → gate de-dups." That goal *is* met for the exact-same-set
  case. The unintended consequence: "identical" was defined at whole-cycle granularity — too coarse for
  recurrence *detection* and useless as a *remediation* unit.
- `Report` as the default blocked-goal arm deliberately encodes "respect a deliberate operator/dependency
  block" (`mod.rs:1597-1598`). It becomes a trap only because the WHY class that would distinguish
  "benignly parked" from "genuinely stuck but unmarked" is **not wired into the arm** — and (from §3) is
  often never computed at all.
- The gap path notifies through the *same* mandatory operator notifier the merge/goal-health paths use
  (`mod.rs:923-924`) — consistent operator-visibility design — but stops at *visibility*, never crossing
  into *action*.

---

## 7. Concerns / minimal remediation directions (investigation-only — nothing landed)

1. **Add ONE intermediate remediation rung to `decide_blocked_goal`, gated on WHY class, not raw count.**
   At first *proven* recurrence for a `goal:blocked` whose WHY carries no benign explanation, route to a
   resolution action instead of `Report`; reserve human `EscalateBlockedGoal` for `UnclearCriteria`/
   genuinely-stuck. Couple this with un-gating the WHY reasoner (§3) — the rung is useless if the class is
   never computed. (Fixes the dead-zone + park-without-classification together.)
2. **Give `WorkstreamCoverage` a launch edge** (or convert its `FlagWorkstreamGaps` to notify+launch for
   a single decomposed gap). Closes the notify-without-launch loop. Keep it small — launch ONE bounded
   sub-goal per fresh gap, reuse the existing spawn capability, don't build a parallel launcher.
3. **Key recurrence per `dedup_key`, not per whole-cycle composite** (fixes detection brittleness in §5)
   — larger change; flag as such.
4. **Don't point `LaunchRecipe` at the composite** (§2.2): if a meta-problem must launch, target a single
   decomposed member.
5. **Coupling warning:** any Lane-B threshold/accrual change must ship atomically with its counter or
   `recurrence>=3` becomes dead code / latches; likewise the 2× rung + WHY-ungating are a coupled pair.

**Over-engineering guardrail:** the counter is *honest* — fix the loop, not the counter. Prefer the two
minimal edges (rung #1 + launch edge #2) over a recurrence-engine rewrite.

---

## 8. Questions for verification phase

1. Assert with a unit test that the `orient` merge (`mod.rs:1211`) **never** matches an `overseer-obs:`
   key against a `goal:blocked:` key (static reading says never; prove it).
2. Confirm the composite `RecurringSignature`'s `LaunchRecipe` is actually *admitted* by `gate()` under
   default autonomy/budget — determines whether the "LaunchRecipe at the blob" harm is **live** or latent.
   (Open from prior waves, still unanswered.)
3. Regression: two cycles with a **one-goal-different** blocked set must NOT emit `RecurringSignature`
   (demonstrates detection brittleness); a per-key variant SHOULD (demonstrates fix direction).
4. Confirm the `Report` default arm is reached *only* for genuinely-benign blocks once a WHY class is
   wired — i.e. the proposed 2× rung would not swallow deliberate operator blocks.
5. Confirm `act_flag_workstream_gaps` has no launch/spawn edge under any config path (static reading:
   only `notifier.notify`, `mod.rs:929-934`).

**Verdict (secondary):** The escalation ladder has a **structural dead-zone at count 2**: Lane A makes the
count *visible* at 2 but has no remediation rung, while the only escalation (Lane B) needs 3 on a counter
Lane A never increments — so a recurring signature pins at 2 forever. Two unclosed loops feed it:
(a) blocked goals are **parked without WHY-classification** (double-gated ladder at `cycle.rs:582-702`
fails open to a bare park), and (b) workstream gaps are **notified without ever launching** work
(`act_flag_workstream_gaps`, the sole High-priority Decide arm with no launch edge). The write-back
self-observation (`observation_signature`) re-persists the whole composite each cycle, keeping both loops
perpetually visible-but-unresolved. Minimal fix direction: add one WHY-gated remediation rung + one gap
launch edge; do not touch the counter. Zero non-test source drift at HEAD `cc55a6fb`. Investigation-only.
