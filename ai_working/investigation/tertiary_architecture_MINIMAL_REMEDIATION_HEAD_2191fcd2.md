# Tertiary (Architect) — Minimal Remediation Options, Re-Grounded at HEAD

**Role:** TERTIARY investigator (architecture / remediation design). **Investigation-only — no
production code changed.**
**HEAD:** `2191fcd2` (`docs(investigation): consolidate eighteenth-wave deep dives into §25`).
**Focus (from strategy):** *Minimal remediation options (missing rung / missing launch edge /
dedup fix) without over-engineering* — plus classify `resource:engineer_spawn` as benign drift vs
contradicting signal.
**Method:** VALIDATE-DON'T-RE-DERIVE. Every load-bearing citation independently re-read at
`2191fcd2` (all `src/overseer/*.rs`, `src/ooda_loop/cycle.rs`, `src/overseer/wiring.rs` are clean
at HEAD — `git status --porcelain` empty — so no stale line numbers).

Extends (does not restart): `FINAL_SYNTHESIS.md`, `RECONCILIATION_LEDGER.md`, and the prior
tertiary landing analyses (`..._LANDING_SAFE_REMEDIATION_HEAD_641f9c37.md`,
`..._TWO_LANE_RECONCILIATION_AND_LANDING_HEAD_a68296c6.md`). Their D1/D2/D3 geometry is confirmed
live and unchanged at this HEAD.

---

## 1. What the signature actually is (re-grounded, one paragraph)

The recurring token is **not** an external failure fingerprint — it is the Overseer's own
**observation write-back key** being **recalled and re-counted as evidence of recurrence**:

1. `write_back_observation` stores one episodic memory per non-clean tick, gated by a
   `WhisperGate(900s, 5/hr)` keyed on the composite signature
   (`overseer/mod.rs:534-563`, gate constructed at `mod.rs:299`).
2. The composite is `observation_signature` = sorted-deduped **problem dedup_keys** joined by `|`,
   prefixed `overseer-obs:` (`mod.rs:1068-1073`).
3. The adapter embeds it as a `[sig:overseer-obs:…]` marker in the episode content
   (`wiring.rs:1084`).
4. A later Observe pass recalls that episode and `parse_failure_signature` extracts the marker
   back into `RecalledEpisode.failure_signature` (`wiring.rs:1025`, `wiring.rs:976-987`).
5. `signals_from` counts episodes per `failure_signature` and, at `>= RECURRING_SIGNATURE_THRESHOLD`
   (=2), emits `Signal::RecurringSignature{ signature:"overseer-obs:…", occurrences }`
   (`signal.rs:455-470`, threshold `signal.rs:362`).
6. Orient classifies it `ProblemKind::ProcessHealth / High`, **dedup_key =
   `sanitize_recalled("overseer-obs:…")`** (`mod.rs:1353-1363`), rendered exactly as the tasking
   string *"recurring signature seen 2× in cognitive memory (overseer-obs:…)"* (`mod.rs:1361`).

**Self-feeding nesting (the doubled `overseer-obs:…|overseer-obs:…` prefix in the tasking
signature):** because that recall-derived problem's dedup_key is *itself* an `overseer-obs:…`
string, the next tick's `observation_signature` joins it back in with the other keys
(`mod.rs:1069` iterates **all** problem dedup_keys) → the composite **re-ingests its own prior
key**, so the prefix nests across cycles. This is the mechanical source of the repeated-prefix
membership drift.

---

## 2. Why it parks at exactly 2× (the dead-zone)

Two independent recurrence lanes exist, with **different thresholds and different stores**, and the
composite signature lives in only one of them:

| Lane | Counts | Store | Threshold | Fires |
|---|---|---|---|---|
| **A — episode count** | recalled *episodes* sharing a `[sig:]` marker (incl. `overseer-obs:` self-writebacks) | episodic graph, one node per stored write-back | **2** (`signal.rs:362`) | `RecurringSignature` → ProcessHealth/High |
| **B — root-cause count** | `StoredOccurrence` **facts** matching a problem's dedup_key | `store_fact`, append-only (`mod.rs:1034`) | **3** (`root_cause.rs:33`) | `EscalateBlockedGoal` (`mod.rs:1613`) |

The composite `overseer-obs:…` key only ever exists on **Lane A**. It parks at 2 because:

- Lane A's write-back gate stores **at most one** episode per identical composite per 900 s window
  (`mod.rs:548-556`). Reaching 3 requires the *entire* problem-set membership to stay byte-identical
  across ≥3 windows.
- The composite is the **full sorted membership** of every dedup_key. Real membership drifts every
  window (a gap closes, a goal unblocks, and — per §1 — the nested `overseer-obs:` key itself
  mutates), so a given exact composite recurs across ~2 windows before the string changes and the
  count resets. It clears the noise floor (2) but structurally can't reach Lane B's bar (3).

**Verdict on "2×": honest signal on a self-referential input.** The *count* is arithmetically
correct (two real write-back episodes did exist); the *defect* is that Lane A is allowed to count
the Overseer's own observations as recurrence evidence and to drive a `LaunchRecipe` off a nonsense
task_description. It is neither a dedup bug nor a storage-replay artifact — it is a **missing
input filter + two missing closing actions**.

---

## 3. The three live design gaps (unchanged at HEAD)

| ID | Seam @ HEAD `2191fcd2` | Nature |
|---|---|---|
| **D1 — self-ingestion** | composite at `mod.rs:1068-1073` re-includes recall-derived `overseer-obs:*` problem keys (`mod.rs:1353-1359`) | Lane A feeds itself; drives spurious `ProcessHealth→LaunchRecipe` (`mod.rs:1429-1435`) on a meaningless brief |
| **D2 — starved escalation** | WHY accrual double-gated (`cycle.rs:582-583`) **and** Lane-B counter is append-only `store_fact` with no upsert (`mod.rs:1034`) read by `>= 3` (`mod.rs:1613`) | Blocked-goal root-cause recurrence rarely accrues → plain blocks park at `Intervention::Report` (`mod.rs:1630`) |
| **D3 — notify-without-launch** | `WorkstreamCoverage` Decide arm returns notify-only `FlagWorkstreamGaps` (`mod.rs:1534-1543`); gap dedup_key is the **bare** `"workstream-gap"` (`mod.rs:1371`) | Gaps are emailed every window, never converted into a workstream → they persist and keep re-entering the composite |

The sibling `StepFailure` arm **does** launch (`mod.rs:1549-1581`), proving the notify-only gap is a
local omission, not a house style.

---

## 4. `resource:engineer_spawn` classification → **benign co-symptom, not a contradicting signal**

`resource:engineer_spawn` is minted by `Signal::EngineerSpawnRate{live}` → `ProblemKind::
ResourcePressure`, **Priority::Normal**, dedup_key `resource:engineer_spawn` (`mod.rs:1267-1272`),
and Decides to `Intervention::Escalate` (passive; `mod.rs:1444-1446`). It is **passive telemetry**
with **no causal edge** to `workstream-gap` or the write-back loop. Its presence in the composite is
pure membership drift (elevated live-engineer count happened to co-occur). **Recommendation: no
coupling fix.** Treat it as a co-symptom; do not build a spawn↔gap feedback control — that would be
over-engineering against a correlation.

---

## 5. Minimal remediation options (ranked; investigation-only)

Design bias: **ruthless simplicity + additive changes**. Each option is independently landable.

### Option 1 (RECOMMENDED, smallest, highest leverage) — Close D1: exclude self-writeback keys from the composite
**Change:** in `observation_signature` (`mod.rs:1068-1073`), filter out any dedup_key with the
`overseer-obs:` prefix before `join("|")`. One `.filter()` line.
**Effect:** breaks the self-ingestion nesting at its source; Lane A can no longer manufacture
`overseer-obs:` recurrence about itself; the doubled-prefix drift disappears; spurious
`ProcessHealth→LaunchRecipe` on nonsense briefs stops.
**Cost/risk:** trivial. Additive. No existing assertion depends on `overseer-obs:` appearing inside
its own successor composite. **New** test: `observation_signature_excludes_self_writeback_keys`.
**Over-engineering flag:** resist the temptation to also strip the marker at recall time or add a
provenance enum — the single emit-side filter is sufficient and is the ruthless-simple cut.

### Option 2 — Close D3 as an *additive* recurrence rung (missing launch edge)
**Change:** keep `decide(WorkstreamCoverage) == FlagWorkstreamGaps` for first-seen/below-threshold
(preserves the hard assertion `tests_gap_scan.rs::decide_routes_workstream_coverage_to_flag_gaps`);
**add** a second rung that, when a **per-gap** `GapItem.signature` (NOT the bare `"workstream-gap"`
constant — `mod.rs:1371`) has recurred `>= 2×` on Lane A, routes that gap through the existing
`launch.rs` edge already proven by `StepFailure` (`mod.rs:1549-1581`).
**Effect:** gaps stop being emitted-but-never-closed; they leave the composite once a workstream
covers them. This is the true "missing launch edge."
**Cost/risk:** medium. Must key on `GapItem.signature` (the Act gate already does at
`mod.rs:901,932`) or all gaps fold into one launch. **New** tests only; existing two gap-scan
assertions stay green because the base arm is unchanged.
**Over-engineering flag:** do NOT swap the Decide arm to always-launch (breaks the routine-risk
assertion and launches on first sight). Additive rung only.

### Option 3 — Close D2 as an *atomic pair* (missing escalation rung)
**Change (must ship together):** (a) open the WHY accrual so blocked-goal recurrence actually
counts (`cycle.rs:582-583`), and (b) make the Lane-B counter a **count-in-content upsert** rather
than append-only `store_fact` (`mod.rs:1034`) so `recurrence >= 3` (`mod.rs:1613`) can be reached.
**Effect:** repeatedly re-parked plain blocks escalate their root cause instead of sitting at
`Report` forever.
**Cost/risk:** highest. **Latch hazard:** closing the gate without de-ratcheting escalates on a
broken counter; de-ratcheting via a `CallerKey`/single-live-fact write collapses `recall.len()` to
1 and makes `>= 3` dead code (`RECONCILIATION_LEDGER.md §2`). Ship both halves or neither.
**Over-engineering flag:** this is the one option that can grow unbounded. If time-boxed, prefer
Options 1+2 and defer D2; D2 is the deepest but least directly responsible for the tasking
signature (which is Lane A, not Lane B).

### Option 0 (rejected) — Raise `RECURRING_SIGNATURE_THRESHOLD` from 2→3
Suppresses the symptom (2× stops rendering) while leaving self-ingestion and the two unclosed loops
intact; also breaks `#2628`'s "two-or-more is the floor" contract (`signal.rs:359-362`). **Reject:
tuning a threshold to hide an honest count is a silent-degradation anti-pattern.**

---

## 6. Recommended landing order & scope

1. **Option 1 (D1 filter)** — ship first, alone. Smallest diff, stops the self-referential growth,
   immediately quiets the tasking signature at its root.
2. **Option 2 (D3 additive rung)** — ship second; independent of Option 1, closes the gap loop.
3. **Option 3 (D2 atomic pair)** — only if the blocked-goal escalation starvation is independently
   prioritized; larger and latch-sensitive. Not required to resolve the tasking signature.

`resource:engineer_spawn`: **no change** (§4).

**Minimal viable fix for the tasking signature specifically = Option 1 alone.** Options 2/3 address
the *upstream* non-convergence that keeps feeding real problems into the composite, but the
`overseer-obs:`-prefixed self-recurrence is fully severed by the D1 emit-side filter.

---

## 7. Regression floor (must stay green; where noted, update in same diff)

- `overseer::tests_gap_scan` — Option 2 must **add** tests, not edit
  `decide_routes_workstream_coverage_to_flag_gaps` / the `RiskClass::Routine` pin.
- `overseer::tests_goal_health`, `overseer::tests_root_cause` — Option 3's atomic pair changes
  asserted accrual/escalation behavior; update those assertions in the same diff.
- `overseer::tests_memory_recall` — Option 1 must not regress the `RecurringSignature` emit for
  **genuine external** signatures (only `overseer-obs:` self-keys are excluded).

---

## 8. Bounded scope / dead-ends respected

Stopped at the `store_episode` / `store_fact` seam (no cognitive-memory backend internals). Did not
investigate the agent-kgpacks-rs issue-17 embedding work, the test-coverage/benchmark goals, or the
individual `goal:blocked:*` slugs — they are membership drift, not causes. **No fix landed; options
only.**
