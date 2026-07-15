# Secondary Investigation — 2×→3× Recurrence Dead-Zone & Over-Aggregation of Distinct Goals

**HEAD:** `440e024c` (verified). Diff of `src/overseer/ src/ooda_loop/` from `388e6c29..HEAD`
is **empty** — only docs changed since the prior waves. Every citation below re-confirmed
against live `src/` (no doc-to-doc trust). **Zero source drift** — all prior file:line
citations hold at this HEAD.

**Focus (secondary):** (1) characterize the `2×→3×` recurrence dead-zone at
`root_cause.rs:33`; (2) characterize the over-aggregation of distinct goals into ONE composite
blocked signature. Confirm-not-contradict prior `FINAL_SYNTHESIS.md`.

---

## 1. The two thresholds live on two decoupled lanes (re-confirmed)

| Lane | Threshold | Location | Counter is… |
|---|---|---|---|
| **A — episodic recall** | `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362`, fired `signal.rs:462-468` | # of recalled write-back **episodes** whose `failure_signature` string is byte-identical |
| **B — semantic root cause** | `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33`, gated `mod.rs:1613` | # of recalled `PriorOccurrence`s with the same `cause_label` |

The `2×` in the investigation artifact is **Lane A** (`Signal::RecurringSignature.occurrences`,
`signal.rs:70,464-467`). The escalation gate in `decide_blocked_goal` reads **Lane B**
(`recurrence`, `mod.rs:1608,1613`). **They never share a counter.** This is the "cross-lane
visibility gap" — not a single mis-set threshold.

---

## 2. Dead-zone geometry — CONFIRMED, and it is worse than "no rung"

`decide_blocked_goal` (`mod.rs:1603-1631`) is the ONLY closing path for a `goal:blocked:*`
problem. Its arms, in order:

```
recurrence >= 3 (Lane B)            → EscalateBlockedGoal        (mod.rs:1613)
perpetual && no_progress_marker     → UnblockGoal  (blind; re-blocks next cycle)  (mod.rs:1620)
needs_review                        → EscalateBlockedGoal        (mod.rs:1623)
else                                → Report       (no-op)       (mod.rs:1630)
```

**The dead zone is `recurrence ∈ {0,1,2}` for any goal that is neither `perpetual+no-progress`
nor `needs_review`.** Such a goal falls to `Report` — a no-op — **on every cycle, forever**. It
is re-observed, re-persisted, re-classified, and re-parked with no convergent action and no trend
toward zero.

### 2.1 New finding — the "raise-priority" rung is STRUCTURALLY UNREACHABLE for the composite

The strategy framed the dead zone as "between raise-priority and escalate." The intended
raise-priority mechanism is `orient` (`mod.rs:1211-1219`): a `RecurringSignature` co-signal
merges into an in-cycle problem **with the same `dedup_key`** and lowers its priority number
(raises importance).

**But the `RecurringSignature.dedup_key` is the whole-cycle composite `overseer-obs:…`
(`mod.rs:1359`), which equals NO per-goal key** (`goal:blocked:X`, `mod.rs:1336`). The merge
predicate `p.dedup_key == key` (`mod.rs:1211`) can therefore **never** match. Consequently:

- The `2×` Lane-A signal never raises the priority of any of the individual blocked goals it is
  composed of.
- The meta-problem instead stands alone → `decide` → `ProblemKind::ProcessHealth` →
  `LaunchRecipe` (`mod.rs:1353-1363` classify; ProcessHealth→LaunchRecipe arm), i.e. the ONE
  cost-bearing convergent action in the whole flow is aimed at the meaningless composite blob,
  not at any real goal.

So the dead zone is not merely "priority raised but not escalated." For the composite it is
**"priority never raised AND never escalated"** — the raise-priority rung the strategy assumed
exists is unreachable by construction. The `2×` produces **zero** remediation pressure on the
actual goals.

### 2.2 Is an intermediate rung warranted? (strategy priority-2 answer)

Yes — but **conditioned on benign-explanation classification**, not on the raw count. The final
`Report` arm is *correct* for a deliberate operator-set / upstream-dependency block (respecting a
deliberate block is a feature, per `mod.rs:1597-1598`). The defect is that the code cannot
distinguish "benignly parked" from "genuinely stuck but unmarked," so both collapse to `Report`.

**Recommended rung (investigation-only; not landed):** at first *proven* recurrence (Lane-A `2×`,
or a Lane-B `2`) for a `goal:blocked` whose WHY class carries **no benign explanation**
(`NoProgressClass`, uncovered-precondition, etc.), route down a *resolution* action rather than
`Report`. Reserve human `EscalateBlockedGoal` for `UnclearCriteria`/`GenuinelyStuck`. This is the
`PATTERNS.md` "Classify-then-route the stall, don't park it" rung, placed at 2×. It must be gated
on the WHY reasoner (`cycle.rs`), which today fails open to bare-park (prior DISCOVERIES #4) — so
the rung and the WHY-ungating are a coupled pair.

---

## 3. Over-aggregation of distinct goals into ONE composite signature — CONFIRMED

`observation_signature` (`mod.rs:1068-1073`) takes **the entire cycle's problem set**
(`write_back_observation(&cycle.problems)`, `wiring.rs:301`; `write_back_observation`,
`mod.rs:534-563`), collects every `dedup_key`, `sort_unstable` + `dedup`, and `join("|")`. One
write-back **episode per cycle** is persisted (`mod.rs:550-554`) carrying that single composite
string. Recall then counts **episodes by exact composite string** (`signal.rs:456-460` builds a
`BTreeMap<&str,u32>` keyed on the whole `failure_signature`; `parse_failure_signature`
`wiring.rs:976-986` recovers the whole `[sig:…]` blob verbatim).

So all of `kgpacks-rs #12/#17/#18/#23/#25`, `simard-identity {atelier,bursar,cartographer,…}`,
`audit-test-coverage`, `coin-benchmark`, `advance-parity`, plus `workstream-gap` and
`resource:engineer_spawn`, are aggregated into **one** signature — the observed blob. Per the
strategy warning, per-goal identity is **not lost** (each goal ID survives as its own `|`-token),
so this is *expected co-occurrence aggregation*, **not** a token-duplication bug. But the
aggregation granularity produces two real harms:

### 3.1 Harm A — Detection brittleness (false negatives on recurrence)

The recall key is the composite = a logical **AND** of the whole membership set. RecurringSignature
fires only when **two cycles share a byte-identical composite** — i.e. the *entire* blocked/gap set
is unchanged. Any churn (one goal resolves, one new goal blocks, a gap opens/closes) mutates the
composite → recall count resets to 1 → **no** RecurringSignature, even for a goal that has been
re-blocking every single cycle. Recurrence is tracked at whole-cycle granularity when it should be
**per-`dedup_key`**. A single chronically-stuck goal in a churning environment can evade Lane-A
detection indefinitely.

### 3.2 Harm B — Non-actionability of the composite as a remediation unit

When the composite *does* recur and reaches `LaunchRecipe` (§2.1), the `task_description` is the
whole blob. "Fix goal A AND B AND … AND a coverage gap AND an engineer-spawn resource note" is not
a well-formed recipe brief. The composite is a *diagnostic aggregate*, not an actionable unit — the
one convergent edge in the flow is pointed at something no engineer can execute against.

### 3.3 Interaction with the D1 feedback loop (cross-reference, primary-owned)

Because the RecurringSignature meta-problem is itself a `Problem` in `cycle.problems`, its composite
`overseer-obs:…` key re-enters the **next** `observation_signature` → nesting +
`workstream-gap|workstream-gap` doubling (primary D1, `secondary_writeback_feedback…388e6c29.md`
§1). Over-aggregation is the *substrate* that makes the composite a single fat token that visually
merges on the outer join. My finding **corroborates** D1 and adds: the same over-aggregation is
what makes §2.1's merge-key never match. **Same root mechanism, two symptoms.**

---

## 4. Design rationale observed

- The composite signature was designed for **within-window write-back dedup** (`#2628`,
  `mod.rs:1064-1067`): "two identical observations → same signature → gate de-dups." That goal is
  met *for the exact-same-set case*. The unintended consequence is that "identical" was defined at
  whole-cycle granularity, which is too coarse for recurrence *detection* and *remediation*.
- `Report` as the default blocked-goal arm encodes "respect a deliberate block"
  (`mod.rs:1597-1598`) — a legitimate design choice that becomes a trap only because the WHY class
  that would distinguish benign-vs-stuck is not wired into the arm.

---

## 5. Potential concerns / recommendations (investigation-only, nothing landed)

1. **Key recurrence per `dedup_key`, not per composite.** Detection (Lane A) should count episodes
   per individual problem key so a single re-blocking goal trips `2×` regardless of cycle-mate
   churn. (Fixes Harm A. Larger change — persist per-key markers or write one episode per problem.)
2. **Add a 2× remediation rung gated on WHY class** (§2.2), not a bare count bump — else benign
   operator blocks get over-escalated.
3. **Do not point `LaunchRecipe` at the composite.** If a meta-problem must launch, it should
   target a single decomposed member, or the flow should route to per-goal resolution instead
   (Harm B).
4. **Coupling warning (from RECONCILIATION_LEDGER, re-affirmed):** any Lane-B threshold or accrual
   change must ship atomically with its counter or `recurrence>=3` becomes dead code / latches.

---

## 6. Questions for verification phase

1. Confirm no path lowers a per-goal problem's priority from the composite RecurringSignature —
   i.e. verify empirically that the `orient` merge at `mod.rs:1211` never matches an
   `overseer-obs:` key against a `goal:blocked:` key (static reading says never; assert with a
   unit test feeding both).
2. Confirm the composite RecurringSignature's `LaunchRecipe` is actually *admitted* by `gate()`
   under default autonomy/budget (determines whether Harm B is live vs latent) — same open
   question as prior wave, still unanswered.
3. Add a regression test: two cycles with a **one-goal-different** blocked set must NOT emit
   RecurringSignature (demonstrates Harm A), and a per-key-keyed variant SHOULD (demonstrates the
   fix direction).
4. Confirm the `Report` default arm is only reached for genuinely-benign blocks once a WHY class
   is wired — i.e. that the proposed 2× rung would not swallow deliberate operator blocks.

**Verdict (secondary):** The `2×→3×` dead-zone is CONFIRMED and is *deeper* than "no rung between
raise and escalate" — for the composite self-observation the raise-priority rung is **structurally
unreachable** (`overseer-obs:` key can never merge with a `goal:blocked:` key at `mod.rs:1211`),
so the `2×` exerts zero pressure on the real goals and the sole convergent edge (`LaunchRecipe`)
targets the meaningless composite. Over-aggregation is CONFIRMED as *expected* co-occurrence
aggregation (no identity loss) but carries two real harms: recurrence-detection brittleness
(whole-cycle-exact key → false negatives under churn) and composite non-actionability. Both trace
to the same whole-cycle-granularity `observation_signature`, corroborating primary D1. Zero source
drift at HEAD `440e024c`. Investigation-only — no change landed.
