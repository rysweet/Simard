# Secondary Investigation — persistent blocked-goal park (no WHY) & workstream-gap/engineer_spawn coupling

**Role:** SECONDARY investigator (patterns / blocked-goal resolution ladder / gap-spawn coupling)
**HEAD:** `9fd1ea0a` (ledger baseline was `dea65df8`)
**Mandate:** EXTEND / VALIDATE — not restart.
**Provenance check:** `git diff --name-only dea65df8..HEAD -- '*.rs'` = **`src/overseer/tests_root_cause.rs` only** (a test file). Every production `.rs` citation from prior waves is byte-identical and re-verifies exactly at HEAD. This wave **validates, regrounds, and adds two refinements.**

---

## Verdict (one line)

The recurring `goal:blocked:<slug>` + `workstream-gap` + `resource:engineer_spawn` composite is a **faithful re-observation of a stable, under-resourced problem set with no convergence rung on the observation lane** — confirming prior findings. **Two refinements:** (1) the "WHY reasoner is unwired" framing is **stale at HEAD** — the investigated breaker *and* the bare-park re-investigation sweep are both wired by default; (2) the `goal:blocked` signature is **WHY-agnostic**, so its recurrence cannot be read as evidence of a bare park at all.

---

## PART A — Persistent blocked-goal park

### A1. Signature is dedup-keyed on `goal_id` ONLY — carries no WHY token (NEW, load-bearing)

`Signal::GoalBlocked` orients to a `Problem` with **`dedup_key = format!("goal:blocked:{goal_id}")`** (`overseer/mod.rs:1336`). The WHY class (`ALREADY-COMPLETE`, `UNCLEAR-CRITERIA`, `GENUINELY-STUCK`, …) lives only in the goal's block-reason *text* and in `problem.why`; it is **never** part of the dedup_key, and the observation signature is built purely from sorted/deduped dedup_keys (`observation_signature`, `mod.rs:1068-1073`).

**Consequence:** the recurring `goal:blocked:<slug>` token is **invariant to whether the park is bare or WHY-bearing**. A goal correctly classified `UnclearCriteria` (→ human), `GenuinelyStuck` (→ human), or `UpstreamDependency` (→ defer until upstream lands) **legitimately stays blocked and re-emits the identical signature every window**. Therefore the focus premise — "bare no-progress, **no WHY classification**" — **cannot be confirmed from the signature**; the recurrence looks the same either way. The persistent `goal:blocked` recurrence is the expected fingerprint of *terminally-classified-but-unresolved* work, not proof of a missing classification.

### A2. The WHY reasoner IS wired at HEAD — prior "unwired lever" framing is stale (REFINEMENT)

Prior artifacts (`blocked_transition_and_escalation_idempotency.md §3`; `DISCOVERIES.md #4`) named an **unwired/degraded WHY reasoner** as the "single lever." At live HEAD this is **no longer accurate**:

- `cycle.rs:583` gates on `no_progress_investigation_enabled()` — **default ON** (`no_progress.rs:203-207`; `SIMARD_NO_PROGRESS_INVESTIGATE=off` is an opt-out kill-switch).
- `cycle.rs:599-608` calls **`apply_no_progress_breaker_investigated`** with a real, production `DeterministicNoProgressReasoner::new(source_ref)` (`cycle.rs:593-594`), a `CloneRepoHealer`, and a `QueueingEngineerDispatcher`. So on the transition cycle a stall is classified and routed down `resolution_for_why` — **not** parked bare.
- `cycle.rs:627-636` **additionally** calls **`reinvestigate_bare_blocked_goals`** — the issue-#17 sweep that scans the board for goals still in a **bare** `[OODA-SAFEGUARD] … needs human review` block and re-runs the *same* reasoner + ladder over them, un-blocking on a non-terminal rung or authoring a WHY-bearing reason otherwise.

So bare parks are **actively upgraded to WHY-bearing every cycle**. The live gate that actually produces / preserves *bare, unexplained* parks is different and narrower:

- **The whole breaker + reinvestigation block is gated on `memories.completion_evidence == Some`** (`cycle.rs:582`). Absent that memory pair (non-daemon callers, or a daemon config without the completion-evidence source), **neither parking nor reinvestigation runs** — so any pre-existing bare park persists untouched and no WHY is ever produced. **This — not an unwired reasoner — is the real "no WHY classification" condition.**
- The env kill-switch path (`cycle.rs:684-698`) falls back to `apply_no_progress_breaker`, whose ladder authors the **bare** `no_progress_blocked_reason` ("…consecutive no-action cycles; needs human review", `no_progress_breaker.rs:75,123`) with no WHY.

**Net:** the classification machinery exists and is default-wired; bare parks are a **degraded-configuration** artifact (evidence source absent or kill-switch engaged), not a missing subsystem. Verification phase should confirm the production daemon actually supplies `completion_evidence` (if it does, sustained bare parks should be self-healing and the reviewer should look at the reasoner's *classification accuracy*, not its existence).

### A3. Why terminally-classified goals still recur — the missing convergence rung (VALIDATED, sharpened)

Even with the reasoner wired, the named cluster recurs because the terminal WHY classes **intentionally keep the goal blocked**:

| Goal(s) | Likely class (per `no_progress_why.rs`) | Ladder rung | Stays blocked? |
|---|---|---|---|
| kgpacks-rs #12/17/18/23/25, parity | `AlreadyComplete` / `MissingPrecondition` | auto-complete / heal+retry | should clear — if it recurs, classification or done-gate is failing |
| coverage-audit-to-70% | `UnclearCriteria` | guided engineer → **human** | **yes** (uncheckable done-gate) |
| coin benchmark harness | `MissingPrecondition` / `UpstreamDependency` | heal / **defer** | **yes** until upstream lands |
| simard-identity personas | `GoalUncovered` (active) / `UnclearCriteria` (blocked) | gap-notify / human | **yes** |

For the `UnclearCriteria` / `GenuinelyStuck` / `UpstreamDependency`-defer cases, staying blocked is *correct* — but there is **no rung that converges the recurring observation signal**. `decide_blocked_goal` (`mod.rs:1603-1631`) only ever **escalates once per window** (gated) or **reports**; it never marks the observation lane resolved. So the signature re-emits every write-back window forever. **Same root shape as the gap loop (Part B): observe-without-closing on the observation lane.**

### A4. Escalation counter (VALIDATED, unchanged)

`recurrence = problem.why.recurrence` (`mod.rs:1469`) derives from `recall_occurrences(&problem.dedup_key)` (`mod.rs:456,972`), i.e. `recall.len()`. Occurrences are written by `record_occurrence` via **non-idempotent `store_fact`** (`mod.rs:1004-1043`, store at `:1034`) — an append-only ratchet, one node per effective act/window. Escalation fires at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3)` (`mod.rs:1613`; threshold in `root_cause.rs`). **The `×2` in the signature is on a *different lane*** (the observation write-back episodes, `RECURRING_SIGNATURE_THRESHOLD = 2`) than this escalation counter — the two-lane separation from prior waves holds exactly. **CallerKey trap warning still applies** (see `RECONCILIATION_LEDGER §2`): the de-ratchet fix must be a count-in-content upsert, not a bare `store_fact_with_caller_key`, or escalation becomes dead code.

---

## PART B — workstream-gap / resource:engineer_spawn coupling

### B1. `act_flag_workstream_gaps` is notify-only — no closing edge (VALIDATED at HEAD)

`act_flag_workstream_gaps` (`mod.rs:884-946`) in full: peeks `gap_gate` (`WhisperGate::new(900, 200)`, `:304`) keyed `workstream-gap:{g.signature}` (`:901`), sends **one consolidated operator notification** (email + Signal, `:929-930`), commits the gate (`:932-933`). **No `launch`, no `file_issue`, no engineer spawn.** The Decide arm (`mod.rs:1534-1543`) merely carries the `WorkstreamGap` evidence verbatim into `Intervention::FlagWorkstreamGaps`. Contrast the sibling `StepFailure` arm (`:1549-1580`) which produces a real `Intervention::LaunchRecipe` — **proving `WorkstreamCoverage` is uniquely the High-family arm with no launch edge.** The 15-min dedup window makes a persistently-uncovered item re-notify each window forever ⇒ **missing convergence rung, not a counting bug.**

### B2. Signature emission is DECOUPLED from notification (NEW nuance)

The `workstream-gap` token enters the composite signature at **Orient** — `signal_to_problem` mints `dedup_key = "workstream-gap"` (`mod.rs:1371`) and `write_back_observation(&cycle.problems)` (`wiring.rs:301`) writes `observation_signature` over the oriented problems, gated only by `write_back_gate` (`WhisperGate::new(900,5)`, `mod.rs:299,546`). This is **independent of the Act phase**, which can be *held entirely* when `!self.gap_scan_enabled` (`mod.rs:596-598`, `SIMARD_OVERSEER_GAP_SCAN` opt-out) — "no notification, no issue even though gaps were observed."

**Consequence:** the `workstream-gap` token can recur in the signature **even when zero notifications fire** (gap-scan disabled). So the loop is "observe-into-signature without *any* closing action" — a strictly weaker precondition than "notify-without-closing." Any remediation must key on `GapItem.signature` (per-gap), **not** the bare `"workstream-gap"` dedup_key (**INV-GAP-KEY trap**, `mod.rs:1371`), else all gaps fold into one issue.

### B3. `resource:engineer_spawn` is benign passive telemetry; no causal edge to the gap (VALIDATED)

`resource:engineer_spawn` maps to `ProblemKind::ResourcePressure`, `Priority::Normal`, dedup_key `"resource:engineer_spawn"` (`mod.rs:1268-1272`); Decide arm → `Intervention::Escalate { reason }` (`mod.rs:1444-1446`). **There is no code path** from `workstream-gap` to `engineer_spawn` or back. They co-occur in the composite signature only because **both predicates held in one window**: backlog uncovered **AND** engineers saturated (≥8 live). This is a **single under-resourced STATE**, not an orchestration cycle — and it unifies the whole signature: `goal:blocked` (idle stuck) + `workstream-gap` (active uncovered) + `resource:engineer_spawn` (no spare executors) are three symptoms of one resourcing/convergence deficit. **Actual spawning lives in the OODA loop** (`no_progress.rs` `SpawnEngineer` rung, bounded to one guided retry via `mark_guided_retry`, `:717`), not at the overseer boundary — no unfulfilled-spawn defect there.

---

## Patterns / anti-patterns (this focus)

- **Observe-and-flag without a closing action** (PATTERNS.md) — confirmed on BOTH lanes: the gap loop (`act_flag_workstream_gaps`) and the blocked-goal observation lane (`decide_blocked_goal` never resolves the recurring signal). *Same shape, two surfaces.*
- **Recurrence dead zone** (PATTERNS.md) — `×2` sits above one-off noise, below the `3` escalation bar, with no auto-remediation rung for coverage gaps. Confirmed.
- **Two signatures, one root problem** (PATTERNS.md) — a goal oscillates `workstream-gap` (active/uncovered, Blocked explicitly skipped in `sensor.rs`) ↔ `goal:blocked` (idle). Confirmed; explains why the same entities appear in both families.
- **Signature-invariant recurrence** (NEW) — because `goal:blocked:<slug>` omits the WHY token, a *correctly-classified* terminal block is indistinguishable from a bare park in the signal. Don't infer "unclassified" from recurrence.

---

## Integration points

- Orient → `signal_to_problem` (`mod.rs:1262-1371`) mints all three tokens' dedup_keys.
- Write-back → `wiring.rs:301` → `write_back_observation` (`mod.rs:534-563`) → `observation_signature` (`:1068`). **Only lane that feeds the visible composite signature.**
- Act → `execute`/`plan` (`mod.rs:588-671`) gates: goal-health (`:588`), gap-scan (`:596`), autonomy (`:600`), then `act_flag_workstream_gaps` (`:671→884`).
- OODA breaker → `cycle.rs:582-702` (double-gated on `completion_evidence` + `no_progress_investigation_enabled`), reasoner+reinvestigation from `ooda_loop/no_progress.rs`.

---

## Questions for verification phase

1. **Does the production daemon supply `memories.completion_evidence`?** If yes, sustained bare parks should not exist (reinvestigation clears them) — then reviewers must audit the *reasoner's classification accuracy* (e.g. kgpacks recurring despite `AlreadyComplete` implies the done-gate/verify path is misfiring), not the breaker's wiring. If no, that absence is the concrete "no WHY" root cause. *(Confirm which; the signature alone cannot tell us — see A1.)*
2. Confirm the remediation rung for gaps keys on `GapItem.signature`, not the bare `"workstream-gap"` dedup_key (INV-GAP-KEY, `mod.rs:1371,1543`).
3. Confirm the D2 fix (escalation gate + occurrence counter) ships atomically and reads a count-in-content field, not `recall.len()` (avoids the CallerKey dead-code trap).
4. Confirm `gap_scan_enabled` state in production: if disabled, gaps recur in the signature with **zero** operator visibility (B2) — a silent-degradation surface worth a gauge.

---

## Reconciliation with prior artifacts

- **Extends** `secondary_gap_and_spawn_HEAD_440e024c.md` (B1/B3 identical conclusions, regrounded at HEAD 9fd1ea0a) and adds B2 (emission/notification decoupling).
- **Refines** `blocked_transition_and_escalation_idempotency.md §3` and `DISCOVERIES.md #4`: the "unwired WHY reasoner" lever is **stale**; the live gate is `completion_evidence` presence + the env kill-switch (A2). Root-cause *shape* (bare park → no self-resolution) is unchanged where the gate is off; the *mechanism* description is corrected.
- **Confirms** `RECONCILIATION_LEDGER.md` two-lane framing, INV-GAP-KEY, and CallerKey trap without contradiction.
