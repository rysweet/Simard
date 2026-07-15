# Synthesis — Recurring `overseer-obs:…|goal:blocked:…|workstream-gap` Signature

**Investigation:** why the composite overseer signature was "seen 2×" in cognitive memory.
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `85b9398a`
**Status:** Complete. Every load-bearing citation re-verified against live `src/overseer/`.

The five required synthesis outputs follow (also emitted as JSON at the end).

---

## Output 1 — Executive Summary

The string is **not a raw memory key**: it is the overseer's own observation write-back
signature (`observation_signature`, `overseer/mod.rs:1068-1073`) — the cycle's problem
`dedup_key`s, sorted, deduped, joined by `|`, prefixed `overseer-obs:`. "Seen 2×" is a
**real, honest re-observation of a static, unresolved problem set across two window-gated
write-back passes — not a dedup, storage, or replay bug.** The problem set never changes
because two "observe-and-flag" loops never close: blocked goals are **parked without a WHY
classification** (the resolution ladder is double-gated off, `ooda_loop/cycle.rs:582-702`)
and `workstream-gap` coverage gaps are **only notified, never launched or filed**
(`WorkstreamCoverage` is the sole High-priority Decide arm with no `launch.rs` edge,
`mod.rs:1534-1543`) — both sitting in a **recurrence "dead zone"** where 2× is above one-off
noise but below the escalation bar of 3, with no remediation rung.

---

## Output 2 — Detailed Explanation

### 2.1 What emits each token (provenance)

| Token | Emitter | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<hash>` | `signal_to_problem`, `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; the `<slug>-<8hex>` **is the goal_id**, minted upstream at goal creation |
| `workstream-gap` (constant) | `signal_to_problem`, `WorkstreamGap` arm — `mod.rs:1371` | literal `"workstream-gap"` — one consolidated, evidence-independent key per Observe pass |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:455-469`, admitted `mod.rs:1353-1359` | `sanitize_recalled(signature)` written back into the next signature (self-observation) |

**Assembly (once per surviving tick):** `run_cycle → orient → signal_to_problem` stamps
each `Problem.dedup_key` → `write_back_observation(problems)` (`mod.rs:534`, single call site
`wiring.rs:301`) → `observation_signature` builds the composite → `record_observation`
(`wiring.rs:1076-1091`) writes one episode via `store_episode(content, "overseer", {sig})`.

### 2.2 Why it recurs "2×" (the counter is honest)

- The composite episode is written **at most once per 900 s window** — `write_back_gate =
  WhisperGate::new(900, 5)` (`mod.rs:299`), a peek→store→commit gate. Within-window dedup is
  **proven** by `write_back_is_deduplicated_within_window` (`tests_memory_recall.rs:797-817`).
- Therefore a second identical episode only appears when the gate did **not** suppress it:
  (a) **>900 s later** — the same problem set legitimately re-recorded in a new window, or
  (b) **after a daemon restart** — the gate's `last_delivered` map is in-memory/per-process
  (`guardrails.rs:294`), so it starts empty and the still-true condition re-records. This is
  the most probable source of *exactly* 2×.
- "2×" is the `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`); `Signal::RecurringSignature`
  fires at `occurrences >= 2` (`signal.rs:463`). **Verdict: a REAL re-observation loop.**

### 2.3 Two decoupled counter lanes (the key structural insight)

The visible `×2` and the escalation counter live on **different storage lanes**:

- **Lane A — observation episodes** (drives the visible `×2`): `record_observation →
  store_episode` (unconditional), keyed on the composite signature, incremented once per
  900 s window, counted by `RecurringSignature.occurrences` (threshold **2**). *This is the
  number in the question.*
- **Lane B — root-cause occurrences** (drives escalation): `record_occurrence → store_fact`
  (unconditional, `mod.rs:1034`), keyed on `occurrence_concept(dedup_key)`, incremented once
  per ACT touching the cause, counted by `RootCause.recurrence = recall.len()` (threshold
  **3**, `root_cause.rs:33`; `mod.rs:1613`).

The lanes are decoupled — the operator-visible `×2` says **nothing** about whether Lane B
reached 3. The "dead zone at 2" is really a **cross-lane visibility gap**.

### 2.4 Why goals stay blocked (root cause A)

The no-progress breaker fires after 3 idle OODA cycles (`no_progress_breaker.rs:59`) and
historically parks with a **bare** reason "…needs human review" (`:75`) — *what* but not
*why*. The corrective vocabulary exists (`NoProgressClass` + `resolution_for_why`,
`no_progress_breaker.rs:384-417`): `AlreadyComplete→MarkDone`, `Obsolete→Drop`,
`MissingPrecondition→Heal`, `UpstreamDependency→Defer`, `UnclearCriteria`/`GenuinelyStuck→
human`. **Only the last two should ever reach a human.** But the WHY reasoner
(`cycle.rs:582-702`) is **double-gated and fails open to bare-park**:
- **Gate A** — `completion_evidence.is_some()`; if `None`, the whole breaker block collapses
  to `Vec::new()` (no classification, no ladder).
- **Gate B** — `no_progress_investigation_enabled()` (default `true`; env-off → legacy park).
- **No invariant** ties a `Blocked` reason to a `NoProgressClass`.

So all six stall classes collapse to the same bare park → the goal re-parks every window →
the recurring `goal:blocked` population. The canonical incident: seven `kgpacks-rs` goals
parked as "no progress" when the work was **already done** (issues CLOSED, PRs MERGED) — the
safeguard misread *done* as *stuck* (`no_progress_why.rs` header).

### 2.5 What `workstream-gap` is (root cause B)

A **backlog-coverage gap**, NOT zero-workstream decomposition (`sensor.rs:288-320`):
`detect_workstream_gaps` flags a p1/p2 **active, non-blocked** goal with no
assignee/PR/branch/session (`GoalUncovered`), a high-signal open issue with no PR
(`IssueUncovered`), or a live anomaly with no fix (`AnomalyUnaddressed`). **Blocked goals are
skipped** (`sensor.rs:300-302`; routed via `goal_health`). Decomposition producing `<2`
sub-goals is a *separate, loud* path (`decompose.rs`, `MIN_SUBGOALS=2`) that emits **no**
`workstream-gap`. The Act path `act_flag_workstream_gaps` (`mod.rs:884-948`) **only notifies
the operator** — files no issue, launches no workstream. `WorkstreamCoverage` is the **only**
High Decide arm with no `launch.rs` edge; siblings (`ProcessHealth`, `CrossCutting`,
`StepFailure`) all reach `LaunchRecipe`. Repeated `workstream-gap|workstream-gap` = multiple
distinct gap problems each carrying the **bare family key** `"workstream-gap"` (`mod.rs:1371`,
which erases per-gap identity), concatenated across episodes — `dedup()` only collapses
*adjacent* equal keys within one signature.

### 2.6 Two signatures, one problem

An under-resourced important goal **oscillates**: `workstream-gap` (GoalUncovered) while
active with no workstream → once the breaker parks it, it leaves gap-scan and reappears as
`goal:blocked`. This is why personas, the coverage audit, the coin harness, and kgpacks
appear in **both** recurring families and co-occur in the same composite.

### 2.7 Three independent defects (D1/D2/D3)

| Defect | Seam | Symptom in the signature |
|---|---|---|
| **D1** emission hygiene | `write_back` re-emits recall-derived `RecurringSignature` (`wiring.rs:301`) | nested `overseer-obs:…\|overseer-obs:…` runs |
| **D2** escalation counter + gate | Lane B append-only ratchet (`mod.rs:1034`) **behind** the WHY double-gate | blocked goals never escalate *or* over-escalate & latch |
| **D3** closing edge | `WorkstreamCoverage` has no `launch.rs` edge; `gap_gate` (`mod.rs:304`) has no cross-window ledger | the `workstream-gap\|workstream-gap` tail, forever |

**The latch:** D2's counter and its accrual gate are a coupled pair — fixing *either alone
changes nothing observable*. Note the ratchet trap: naively replacing `store_fact` with
`store_fact_with_caller_key(root_cause_signature(...))` collapses recall to **1 forever**
(`DedupMode::CallerKey` keeps one live fact per key, `library_adapter.rs:885-889`), making
`recurrence >= 3` **dead code**. Correct fix carries the count **in the fact content**.

---

## Output 3 — Visual Aids

### 3.1 Signature assembly pipeline
```
run_cycle ─▶ orient ─▶ signal_to_problem  (stamps Problem.dedup_key each)
                              │  goal:blocked:<goal_id>   (mod.rs:1336)
                              │  workstream-gap           (mod.rs:1371, constant)
                              │  overseer-obs:<recalled>  (mod.rs:1353-1359, self-obs)
                              ▼
              write_back_observation(problems)            (mod.rs:534 / wiring.rs:301)
                              │
                              ▼
              observation_signature(problems)             (mod.rs:1068-1073)
              keys.sort_unstable(); keys.dedup();
              "overseer-obs:" + keys.join("|")
                              │
                    [ write_back_gate 900s ]  ◀── in-memory, per-process (guardrails.rs:294)
                              │ (peek→store→commit)
                              ▼
              record_observation ─▶ store_episode(content, "overseer", {sig})   (wiring.rs:1076)
```

### 3.2 Two decoupled counter lanes
```
                LANE A (visible ×2)                 LANE B (escalation)
   record_observation                       record_occurrence
        │ store_episode (uncond.)                │ store_fact (uncond., ratchet)
        │ key = composite signature              │ key = occurrence_concept(dedup_key)
        │ +1 per 900s window                     │ +1 per ACT touching cause
        ▼                                        ▼
   RecurringSignature.occurrences >= 2      RootCause.recurrence = recall.len() >= 3
        │  (signal.rs:463)                       │  (root_cause.rs:33; mod.rs:1613)
        ▼                                        ▼
   "seen 2×"  ◀── DECOUPLED ──▶  escalate?   (WHY double-gate starves accrual → never 3)
                    dead-zone / cross-lane visibility gap
```

### 3.3 The oscillation — two signatures, one problem
```
   active, no workstream  ──breaker parks (3 idle cycles)──▶  Blocked (bare, no WHY)
         │  emits                                                   │  emits
         ▼                                                          ▼
    workstream-gap  ◀────── unblocked / reactivated ──────    goal:blocked
    (GoalUncovered, notify-only)                              (skipped by gap-scan)
```

### 3.4 Blocked-goal resolution ladder (exists but gated off)
```
 breaker fires ─▶ [Gate A completion_evidence? ]──None──▶ Vec::new()  (no ladder)
                       │ Some
                       ▼
                  [Gate B investigation enabled?]──off──▶ legacy bare park
                       │ on
                       ▼
        classify NoProgressClass ─▶ resolution_for_why
          AlreadyComplete→MarkDone   Obsolete→Drop   MissingPrecondition→Heal
          UpstreamDependency→Defer   UnclearCriteria/GenuinelyStuck→HUMAN (only these 2)
```

---

## Output 4 — Key Insights

- **The count is honest — audit the closing action, not the counter.** The signature is a
  deterministic, provably within-window-deduped fingerprint of a problem set that never
  changes. A *correct* count that never trends to zero points at a **missing convergence
  rung**, not a counting defect.
- **Two decoupled storage lanes.** The operator-visible `×2` (Lane A, episodes) is
  independent of the escalation counter (Lane B, occurrences). The "dead zone at 2" is a
  **cross-lane visibility gap**, not a single threshold problem.
- **One lever, four surface symptoms.** kgpacks parity, the coverage audit, the persona
  cluster, and the coin harness are **not** four independent goal bugs — they all funnel
  through one unwired classification rung: the **bare no-progress park with no WHY token**.
- **The de-ratchet "obvious fix" is a trap.** `store_fact_with_caller_key(root_cause_
  signature)` makes `recall.len()` stick at 1 → escalation becomes dead code. Counts must be
  carried **in fact content** (count-in-content upsert), not in node multiplicity.
- **The overseer observes its own bookkeeping.** Recall-derived `RecurringSignature` problems
  are written back into future signatures, nesting `overseer-obs:` tokens — a real feedback
  smell (`sanitize_recalled` shows authors already treat recalled signatures as untrusted at
  the read boundary, yet still write them back).
- **`WorkstreamCoverage` is dual-path quarantined.** It is the only High kind cut off from
  **both** closing seams the codebase already owns — `FileIssue` (observer→`Report`,
  `observer.rs:120`) and `LaunchRecipe`/notify (acting→`FlagWorkstreamGaps`, `mod.rs:1543`).
- **`workstream-gap` ≠ decomposition failure.** It is a backlog-coverage gap; zero/invalid
  decomposition is a separate, loud path (`MIN_SUBGOALS=2`) that emits no gap token.
- **Latch risk if the gate opens naively.** With the WHY gate shut, Lane B never increments
  (today's symptom); if opened without de-ratcheting, it monotonically ratchets and
  **over-escalates and latches forever** — so §6.2b counter and §6.3 WHY-gate must ship
  atomically.

---

## Output 5 — Remaining Unknowns

- **Live daemon Gate-A state.** Whether `completion_evidence` was actually `None` in the
  daemon that produced these signatures (determines whether the WHY ladder ever ran for the
  kgpacks cluster) is inferred, not directly observed from runtime telemetry.
- **Restart vs. window as the source of exactly-2×.** Both a >900 s new window and a daemon
  restart produce 2×; which one produced *this* observation is not distinguishable from the
  signature alone (no restart/window timestamps captured).
- **Escalation un-latch behavior.** Whether the escalation DECISION latches at recurrence≥3
  and never un-latches once a cause clears is analyzed from code but not confirmed against a
  goal that actually crossed the bar (blocked-goal cluster sits below it).
- **Per-gap attribution.** Whether the `WorkstreamCoverage` *problem* should key on
  `workstream-gap:<GapItem.signature>` (already used at the gate, `mod.rs:901`) instead of the
  bare constant is a design decision not yet validated for downstream count attribution.
- **Cross-restart episode inflation magnitude.** The unbounded same-signature episode
  accumulation across restarts is real in principle; its actual volume (bounded by recall
  LIMIT/consolidation) has not been measured on the live store.
- **Scope of ask.** Whether the deliverable is diagnosis-only or diagnosis-plus-implementation
  is assumed to be diagnosis + prioritized remediation candidates; no fix has been merged
  (both prior commits are docs-only).
