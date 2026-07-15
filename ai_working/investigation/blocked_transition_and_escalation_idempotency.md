# Focused deep-dive: blocked-transition predicate + escalation idempotency

**Investigator focus:** the exact predicate that emits
`overseer-obs:goal:blocked:<slug>-<hash>`, whether escalation is idempotent or
fires every tick, and whether the blocked-goal cluster (kgpacks-rs
12/17/18/23/25 + parity, simard-identity personas, test-coverage audit, coin
benchmark) shares **one** upstream precondition or **N** independent causes.

**Files traced:** `src/overseer/root_cause.rs`, `src/overseer/tests_goal_health.rs`,
`src/goals/store.rs`, plus the supporting chain in `src/overseer/{sensor,signal,mod,wiring}.rs`
and `src/cognitive_memory/{mod,library_adapter}.rs`.

This report **sharpens** the existing `investigation_report.md` and adds one
concrete defect it did not identify: the recurrence counter is **non-idempotent
by construction** (a monotonic write-count ratchet), because `record_occurrence`
uses the non-deduping `store_fact`.

---

## 1. The exact blocked-transition predicate (signature provenance)

The `overseer-obs:goal:blocked:<slug>` signature is produced by a fixed 6-hop
chain. Every hop is pure/deterministic:

| Hop | Site | What it does |
|---|---|---|
| 1. Park | `goal_curation/no_progress_breaker.rs:59` (`NO_PROGRESS_BREAKER_THRESHOLD = 3`) | After **3 consecutive no-action OODA cycles**, the breaker sets `GoalProgress::Blocked("{PREFIX}{count} …needs human review")`. (Also settable by the engineer-brain `MarkGoalBlocked`.) |
| 2. Project | `overseer/sensor.rs:209 blocked_goal_of` | Each active `Blocked` goal → `BlockedGoal { needs_review = is_no_progress_marker \|\| is_brain_failure_marker, perpetual, consecutive_no_action = safeguard_marker_count(reason) }`. |
| 3. Signal | `overseer/signal.rs:441` | One `Signal::GoalBlocked{..}` **per blocked goal** (never consolidated). |
| 4. Orient | `overseer/mod.rs:1324-1345` | Classifies to `ProblemKind::GoalHygiene`, **`dedup_key = format!("goal:blocked:{goal_id}")`** (mod.rs:1336). Priority High iff `needs_review`. |
| 5. Write-back | `overseer/mod.rs:1068-1073 observation_signature` | **`format!("overseer-obs:{}", sorted_deduped_keys.join("|"))`** → the exact `overseer-obs:goal:blocked:<slug>|…` string in the question. Persisted as one episode, gated by `write_back_gate = WhisperGate::new(900, 5)` (≤1 write per 15-min window). |
| 6. Recur-detect | `overseer/signal.rs:455-469` (`RECURRING_SIGNATURE_THRESHOLD = 2`) | On a later tick, recall of **≥2 episodes** sharing a `failure_signature` emits `Signal::RecurringSignature{occurrences}`. **This `2` is the "seen 2×" in the question.** |

So the `<slug>-<hash>` is the goal's dedup key inside the joined observation
signature; the `2×` is `RECURRING_SIGNATURE_THRESHOLD` firing on two recalled
write-back episodes.

`tests_goal_health.rs:374` pins hop 4 (`dedup_key == "goal:blocked:research"`);
`:339-345` pins hop 3 (one `GoalBlocked` per goal).

---

## 2. Escalation idempotency — split verdict

**The escalation ACTION is idempotent within a window. The escalation DECISION
is driven by a NON-idempotent, monotonically-inflating counter.**

### 2a. Action level — idempotent (does NOT fire every tick) ✅
`overseer/mod.rs:810 act_escalate_blocked_goal` gates on
`blocked_goal_gate.peek("escalate:{goal_id}")` where
`blocked_goal_gate = WhisperGate::new(900, 20)` (mod.rs:292). A repeat within the
15-min window returns `GoalHealthSuppressed` (mod.rs:852-878) — no second
operator notification. `tests_goal_health.rs:380` locks "unblocked once, not
escalated." So per goal per 15-min window: **at most one** notify.

### 2b. Decision level — non-idempotent ratchet ❌ (new finding)
`decide_blocked_goal` (mod.rs:1603-1631) routes to `EscalateBlockedGoal` iff
`recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3)` (root_cause.rs:33).
`recurrence` (root_cause.rs:79-82) = **count of prior occurrence facts recalled
from cognitive memory** whose `cause_label` matches the primary.

Those facts are written by `record_occurrence` (mod.rs:1004-1043) via
**plain `store_fact`** (library_adapter.rs:657) — an **unconditional
`CREATE`, no dedup** (mod.rs:340-353 explicitly: only
`store_fact_with_caller_key` dedups; plain `store_fact` accumulates). It is
called once per **effective** act (`outcome_records_occurrence`, wiring.rs:612 —
excludes suppressed no-ops), i.e. once per gate window per goal.

Consequences:
1. **Monotonic lifetime counter, never a streak.** Each acted-on window appends
   one permanent occurrence node; nothing supersedes or prunes them. `recurrence`
   only ever grows.
2. **Escalation latches on.** Once `recurrence ≥ 3`, the goal routes to
   `EscalateBlockedGoal` on **every** future window forever — it can never fall
   back to self-heal (`UnblockGoal`) even after the underlying condition changes.
3. **Recurrence magnitude is a write-count, not an incident-count.** A goal
   resolved and legitimately re-blocked later inherits its old count; recurrence
   conflates "how many windows we ever touched this" with "genuine recurrence."

**Is the observed 2× a real loop or a write artifact? Both — and that matters.**
The `overseer-obs:` write-back IS window-gated, so "2×" means observed across
≥2 distinct 15-min windows = the goals ARE genuinely, persistently blocked (real
loop, not a within-tick duplicate). BUT the *recurrence number* that drives
escalation is inflated by non-deduping writes, so its magnitude is partly an
artifact. The goals recur for real; the counter over-counts by construction.

**Actionable fix:** switch `record_occurrence` to
`store_fact_with_caller_key(root_cause_signature(problem, primary), …)` — the
`root_cause_signature` helper (root_cause.rs:53) already exists for exactly this
key. That makes recurrence a deduped/superseding incident signal (matching the
#2329 pattern) instead of an accumulating write-count.

### 2c. `store.rs` role
`goals/store.rs` is the durable substrate, not a transition site: `upsert_record`
(line 291) keys on `slug` (idempotent put), and `FileBackedGoalStore.put`
reload-under-flock-then-persist (233-253) is cross-process safe. It faithfully
**persists** the `GoalProgress::Blocked` set by the breaker; it neither sets nor
clears the blocked state and adds no idempotency defect. The transition lives
upstream (breaker) and the escalation counter lives downstream (cognitive memory).

---

## 3. Shared upstream precondition: one MECHANISM, N root causes

**One shared choke-point mechanism; N distinct upstream causes; a single
architectural lever.**

- **Shared mechanism (the convergence point):** every goal in the cluster enters
  `blocked` through the **same** predicate — the no-progress breaker parking it
  with a **bare `is_no_progress_marker` reason** (`needs_review = true`,
  `perpetual`, hop 1–2 above). They all present identically as
  `goal:blocked:<slug>` with **no WHY token**.
- **N distinct root causes** (per `no_progress_why.rs` `NoProgressClass`,
  corroborated by the existing report §1.3):
  - kgpacks-rs parity + #12/#17/#18/#23/#25 → `AlreadyComplete` (issues CLOSED /
    PRs MERGED, goal never marked done) and/or `MissingPrecondition` (governed
    repo never cloned).
  - Test-coverage-to-70% audit → `UnclearCriteria` (no done-gate-checkable
    artifact).
  - Coin benchmark harness → `MissingPrecondition` / `UpstreamDependency`.
  - simard-identity personas → primarily `GoalUncovered` workstream-gap; when
    blocked, `UnclearCriteria`.
- **The single lever (the real shared precondition):** the breaker collapses all
  N causes onto the **same non-self-resolving bare marker** because the
  **no-progress WHY reasoner is unwired/degraded**, so `resolution_for_why` never
  runs and none of the four auto-resolvable classes route down their ladder. Wire
  and correct the WHY reasoner (+ `reinvestigate_bare_blocked_goals` for legacy
  bare parks) and the cluster splits into independently-resolving goals instead of
  a single recurring `goal:blocked` population.

So the answer to "single upstream precondition vs N independent causes" is:
**N independent *causes* funneled through 1 shared *mechanism*, remediable at 1
shared *lever*** (WHY-reasoner wiring), with the escalation counter's
non-idempotent ratchet (§2b) as an independent, compounding defect that keeps
already-escalated goals latched even once their cause is gone.

---

## 4. Evidence ledger (this focus)

| Claim | Source |
|---|---|
| Breaker fires at 3 no-action cycles → bare "needs human review" park | `goal_curation/no_progress_breaker.rs:59` |
| Blocked→signal projection; `needs_review`/`consecutive_no_action` derivation | `overseer/sensor.rs:204-233` |
| One `GoalBlocked` per goal; RecurringSignature@≥2 | `overseer/signal.rs:441-469`; `RECURRING_SIGNATURE_THRESHOLD=2` :362 |
| `dedup_key = goal:blocked:{id}`; observation signature `overseer-obs:{keys}` | `overseer/mod.rs:1336`, `:1068-1073` |
| Write-back window-gated (≤1/15min) | `overseer/mod.rs:299,534-563` |
| Escalate action deduped per goal/window | `overseer/mod.rs:810-854`, `:292` |
| Escalate decision needs recurrence≥3 | `overseer/mod.rs:1603-1631`; `root_cause.rs:33,79-82` |
| `record_occurrence` uses non-deduping `store_fact` (unconditional CREATE) | `overseer/mod.rs:1034`; `cognitive_memory/library_adapter.rs:657-683`; `mod.rs:340-353` |
| Occurrence recorded only for effective (non-suppressed) acts | `overseer/wiring.rs:276-280,612-627` |
| Existing dedup key helper available for the fix | `overseer/root_cause.rs:53-55` |
| Goal store is idempotent upsert-by-slug, flock-safe (no transition logic) | `goals/store.rs:291-300,233-253` |
