# Final Synthesis — Recurring `overseer-obs:…|goal:blocked:…|workstream-gap` Signature

**Investigation:** Why the composite overseer signature was "seen 2×" in cognitive memory.
**HEAD:** `5a85317b` — `git diff --name-only 6e3113bc..HEAD -- '*.rs'` is **EMPTY** (zero source
drift; every investigation commit is docs-only, so all `src/overseer/*` line citations hold).
**Status:** Complete and re-validated across five investigation waves. Every load-bearing
citation independently re-grounded to live `src/overseer/`. The question string matches
`mod.rs:1361` verbatim: `"recurring signature seen {occurrences}× in cognitive memory ({signature})"`.

The five required synthesis outputs follow (also emitted as JSON at the end).

---

## Output 1 — Executive Summary

The string is **not a raw memory key**: it is the Overseer's own observation write-back
signature (`observation_signature`, `overseer/mod.rs:1068-1073`) — the cycle's problem
`dedup_key`s, sorted, deduped, joined by `|`, prefixed `overseer-obs:`. **"Seen 2×" is a real,
honest re-observation of a near-static, unresolved problem set across two window-gated
write-back passes — not a dedup, storage, replay, or collision bug.** The problem set persists
because two "observe-and-flag" loops never close: blocked goals are **parked without a WHY
classification** (the resolution ladder is double-gated off, `ooda_loop/cycle.rs:582-702`) and
`workstream-gap` coverage gaps are **only notified, never launched or filed**
(`WorkstreamCoverage` is the sole High-priority Decide arm with no `launch.rs` edge,
`mod.rs:1534-1543`). The count parks in a **recurrence "dead zone"**: 2× is above one-off noise
but below the escalation bar of 3, with no remediation rung between. The newly observed
`resource:engineer_spawn` token is **benign membership drift** (a fixed literal key whose
volatile count lives only in the summary) — it corroborates rather than contradicts the verdict.

---

## Output 2 — Detailed Explanation

### 2.1 What emits each token (provenance)

| Token | Emitter | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<hash>` | `signal_to_problem`, `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; `<slug>-<8hex>` **is the goal_id**, minted at goal creation |
| `workstream-gap` (constant) | `signal_to_problem`, `WorkstreamGap` arm — `mod.rs:1371` | literal `"workstream-gap"`; `gaps.len()` goes to the summary only |
| `resource:engineer_spawn` (constant) | `signal_to_problem`, `EngineerSpawnRate` arm — `mod.rs:1267-1272` | literal `"resource:engineer_spawn"`; `{live}` count goes to the summary only |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:455-469`, admitted `mod.rs:1353-1363` | `sanitize_recalled(signature)` written back into the next signature (self-observation) |

**Assembly (once per surviving tick):** `run_cycle → orient → signal_to_problem` stamps each
`Problem.dedup_key` → `write_back_observation(problems)` (`mod.rs:534`, single call site
`wiring.rs:301`) → `observation_signature` builds the composite → `record_observation`
(`wiring.rs:1076-1091`) writes one episode via `store_episode(content, "overseer", {sig})`.

### 2.2 Why it recurs "2×" (the counter is honest)

- The composite episode is written **at most once per 900 s window** — `write_back_gate =
  WhisperGate::new(900, 5)` (`mod.rs:299`), a peek→store→commit gate. Within-window dedup is
  **proven** by `write_back_is_deduplicated_within_window` (green at HEAD).
- A second identical episode only appears when the gate did **not** suppress it: (a) **>900 s
  later** — the same set legitimately re-recorded in a new window; or (b) **after a daemon
  restart** — the gate's `last_delivered` map is in-memory/per-process (`guardrails.rs:294`),
  so it starts empty and the still-true condition re-records. (b) is the most probable source
  of *exactly* 2×.
- "2×" is `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`); `Signal::RecurringSignature`
  fires at `occurrences >= 2` (`signal.rs:463`). **Verdict: a REAL cross-window re-observation
  loop** (H1 confirmed, H0 rejected).

### 2.3 Two decoupled counter lanes (the key structural insight)

The visible `×2` and the escalation counter live on **different storage lanes**:

- **Lane A — observation episodes** (drives the visible `×2`): `record_observation →
  store_episode` (unconditional), keyed on the composite signature, +1 per 900 s window,
  counted by `RecurringSignature.occurrences` (threshold **2**). *This is the number in the
  question.*
- **Lane B — root-cause occurrences** (drives escalation): `record_occurrence → store_fact`
  (unconditional, append-only, `mod.rs:1034`), keyed on `occurrence_concept(dedup_key)`, +1 per
  ACT touching the cause, counted by `RootCause.recurrence = recall.len()` (threshold **3**,
  `root_cause.rs:33`; `mod.rs:1613`).

The lanes are decoupled — the operator-visible `×2` says **nothing** about whether Lane B
reached 3. The "dead zone at 2" is really a **cross-lane visibility gap**.

### 2.4 Why goals stay blocked (root cause A)

The no-progress breaker fires after 3 idle OODA cycles (`no_progress_breaker.rs:59`) and
historically parks with a **bare** reason "…needs human review" — *what* but not *why*. The
corrective vocabulary exists (`NoProgressClass` + `resolution_for_why`,
`no_progress_breaker.rs:384-417`): `AlreadyComplete→MarkDone`, `Obsolete→Drop`,
`MissingPrecondition→Heal`, `UpstreamDependency→Defer`, `UnclearCriteria`/`GenuinelyStuck→
human`. **Only the last two should ever reach a human.** But the WHY reasoner
(`cycle.rs:582-702`) is **double-gated and fails open to bare-park**:
- **Gate A** — `completion_evidence.is_some()`; if `None`, the whole breaker block collapses to
  `Vec::new()` (no classification, no ladder).
- **Gate B** — `no_progress_investigation_enabled()` (default `true`; env-off → legacy park).
- **No invariant** ties a `Blocked` reason to a `NoProgressClass` (INV-WHY is violable today,
  proven by `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`).

So all six stall classes collapse to the same bare park → the goal re-parks every window → the
recurring `goal:blocked` population. Canonical incident: seven `kgpacks-rs` goals parked as "no
progress" when the work was **already done** (issues CLOSED, PRs MERGED) — the safeguard misread
*done* as *stuck*.

### 2.5 What `workstream-gap` is (root cause B)

A **backlog-coverage gap**, NOT zero-workstream decomposition (`sensor.rs:288-320`):
`detect_workstream_gaps` flags a p1/p2 **active, non-blocked** goal with no
assignee/PR/branch/session (`GoalUncovered`), a high-signal open issue with no PR
(`IssueUncovered`), or a live anomaly with no fix (`AnomalyUnaddressed`). **Blocked goals are
skipped** (`sensor.rs:300-302`; routed via `goal_health`). Decomposition producing `<2`
sub-goals is a *separate, loud* path (`MIN_SUBGOALS=2`) that emits **no** `workstream-gap`. The
Act path `act_flag_workstream_gaps` (`mod.rs:884-948`) **only notifies the operator** — files
no issue, launches no workstream. `WorkstreamCoverage` is the **only** High Decide arm with no
`launch.rs` edge; siblings (`ProcessHealth`, `CrossCutting`, `StepFailure`) all reach
`LaunchRecipe`. Repeated `workstream-gap|workstream-gap` = multiple distinct gap problems each
carrying the **bare family key** `"workstream-gap"` (`mod.rs:1371`, which erases per-gap
identity), concatenated across episodes — `dedup()` only collapses *adjacent* equal keys within
one signature.

### 2.6 Two signatures, one problem (and `resource:engineer_spawn`)

An under-resourced important goal **oscillates**: `workstream-gap` (GoalUncovered) while active
with no workstream → once the breaker parks it, it leaves gap-scan and reappears as
`goal:blocked`. This is why personas, the coverage audit, the coin harness, and kgpacks appear
in **both** recurring families and co-occur in the same composite. `resource:engineer_spawn`
(`ResourcePressure`, `mod.rs:1267-1272`) is the **third view of the same under-throughput
condition**: the system *is* spawning engineers, yet goals stay blocked and gaps stay uncovered.
It is benign membership drift — its `{live}` count lands only in the summary, never in the
signature key — so it does not perturb dedup/idempotency.

### 2.7 Three independent defects (D1/D2/D3) — all UNMERGED at HEAD

| Defect | Seam | Symptom in the signature |
|---|---|---|
| **D1** emission hygiene | `write_back` re-emits recall-derived `RecurringSignature` (`wiring.rs:301`) | nested `overseer-obs:…\|overseer-obs:…` runs |
| **D2** escalation counter + gate | Lane B append-only ratchet (`mod.rs:1034`) **behind** the WHY double-gate | blocked goals never escalate *or* over-escalate & latch |
| **D3** closing edge | `WorkstreamCoverage` has no `launch.rs` edge; `gap_gate` (`mod.rs:304`) has no cross-window ledger | the `workstream-gap\|workstream-gap` tail, forever |

**The latch:** D2's counter and its accrual gate are a coupled pair — fixing *either alone
changes nothing observable*. Ratchet trap: naively replacing `store_fact` with
`store_fact_with_caller_key(root_cause_signature(...))` collapses recall to **1 forever**
(`DedupMode::CallerKey` keeps one live fact per key), making `recurrence >= 3` **dead code**.
The correct fix carries the count **in the fact content** (count-in-content upsert). Fix-landing
grep at HEAD confirms D1/D2/D3 all still unmerged.

---

## Output 3 — Visual Aids

### 3.1 Signature assembly pipeline
```
run_cycle ─▶ orient ─▶ signal_to_problem  (stamps Problem.dedup_key each)
                              │  goal:blocked:<goal_id>        (mod.rs:1336)
                              │  workstream-gap                (mod.rs:1371, constant)
                              │  resource:engineer_spawn       (mod.rs:1270, constant)
                              │  overseer-obs:<recalled>       (mod.rs:1353-1363, self-obs)
                              ▼
              write_back_observation(problems)                 (mod.rs:534 / wiring.rs:301)
                              │
                              ▼
              observation_signature(problems)                  (mod.rs:1068-1073)
              keys.sort_unstable(); keys.dedup();
              "overseer-obs:" + keys.join("|")
                              │
                    [ write_back_gate 900s ]  ◀── in-memory, per-process (guardrails.rs:294)
                              │ (peek→store→commit)
                              ▼
              record_observation ─▶ store_episode(content, "overseer", {sig})  (wiring.rs:1076)
```

### 3.2 Two decoupled counter lanes
```
                LANE A (visible ×2)                 LANE B (escalation)
   record_observation                       record_occurrence
        │ store_episode (uncond.)                │ store_fact (uncond., append-only ratchet)
        │ key = composite signature              │ key = occurrence_concept(dedup_key)
        │ +1 per 900s window                     │ +1 per ACT touching cause
        ▼                                        ▼
   RecurringSignature.occurrences >= 2      RootCause.recurrence = recall.len() >= 3
        │  (signal.rs:463)                       │  (root_cause.rs:33; mod.rs:1613)
        ▼                                        ▼
   "seen 2×"  ◀── DECOUPLED ──▶  escalate?   (WHY double-gate starves accrual → never 3)
                    dead-zone / cross-lane visibility gap
```

### 3.3 The oscillation — one problem, three views
```
   active, no workstream  ──breaker parks (3 idle cycles)──▶  Blocked (bare, no WHY)
         │  emits                                                   │  emits
         ▼                                                          ▼
    workstream-gap  ◀────── unblocked / reactivated ──────    goal:blocked
    (GoalUncovered, notify-only)                              (skipped by gap-scan)
         └───────────────  resource:engineer_spawn  ───────────────┘
              (ResourcePressure: engineers spawn, throughput stalls — same root cause)
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
  changes. A *correct* count that never trends to zero points at a **missing convergence rung**,
  not a counting defect.
- **Two decoupled storage lanes.** The operator-visible `×2` (Lane A, episodes) is independent
  of the escalation counter (Lane B, occurrences). The "dead zone at 2" is a **cross-lane
  visibility gap**, not a single-threshold problem. Any remediation rung must sit on the
  **episode lane** (first proven at ×2), not the occurrence lane (≥3).
- **One lever, several surface symptoms.** kgpacks parity, the coverage audit, the persona
  cluster, the coin harness, and `resource:engineer_spawn` are **not** independent goal bugs —
  they funnel through one unwired classification rung: the **bare no-progress park with no WHY
  token**, plus the notify-only gap edge.
- **The de-ratchet "obvious fix" is a trap.** `store_fact_with_caller_key(root_cause_signature)`
  makes `recall.len()` stick at 1 → escalation becomes dead code. Counts must be carried **in
  fact content** (count-in-content upsert), not in node multiplicity.
- **The overseer observes its own bookkeeping.** Recall-derived `RecurringSignature` problems
  are written back into future signatures, nesting `overseer-obs:` tokens — a real (bounded)
  feedback smell. `sanitize_recalled` shows authors already treat recalled signatures as
  untrusted at the read boundary, yet still write them back. **D1 (exclude recall-derived
  `overseer-obs:*` at the write boundary) is the highest-leverage fix** — it breaks the nesting
  loop at the source and stops membership drift from re-nesting.
- **`WorkstreamCoverage` is dual-path quarantined.** It is the only High kind cut off from
  **both** closing seams the codebase already owns — `FileIssue` (observer→`Report`) and
  `LaunchRecipe`/notify (acting→`FlagWorkstreamGaps`, `mod.rs:1543`).
- **`workstream-gap` ≠ decomposition failure.** It is a backlog-coverage gap; zero/invalid
  decomposition is a separate, loud path (`MIN_SUBGOALS=2`) that emits no gap token.
- **`resource:engineer_spawn` is benign membership drift, not code drift.** Fixed literal key,
  volatile `{live}` count confined to the summary; it never enters the signature. Its appearance
  only in the later snapshot confirms the two occurrences are overlapping-but-different snapshots
  of a *near*-static (not byte-static) set — corroborating the real re-observation verdict.
- **Latch risk if the gate opens naively.** With the WHY gate shut, Lane B never increments
  (today's symptom); if opened without de-ratcheting, it ratchets monotonically and
  **over-escalates and latches forever** — so the counter fix and the WHY-gate fix must ship
  atomically.

---

## Output 5 — Remaining Unknowns

- **Live daemon Gate-A state.** Whether `completion_evidence` was actually `None` in the daemon
  that produced these signatures (i.e. whether the WHY ladder ever ran for the kgpacks cluster)
  is inferred from code, not observed from runtime telemetry.
- **Restart vs. window as the source of exactly-2×.** Both a >900 s new window and a daemon
  restart produce 2×; which one produced *this* observation is not distinguishable from the
  signature alone (no restart/window timestamps captured).
- **Escalation un-latch behavior.** Whether the escalation DECISION latches at recurrence≥3 and
  never un-latches once a cause clears is analyzed from code but not confirmed against a goal
  that actually crossed the bar (the blocked-goal cluster sits below it).
- **Per-gap attribution.** Whether the `WorkstreamCoverage` *problem* should key on
  `workstream-gap:<GapItem.signature>` (already used at the gate, `mod.rs:901`) instead of the
  bare constant is a design decision not yet validated for downstream count attribution.
- **Cross-restart episode inflation magnitude.** The unbounded same-signature episode
  accumulation across restarts is real in principle; its actual volume (bounded by recall
  LIMIT/consolidation) has not been measured on the live store.
- **Scope of ask.** The deliverable is assumed to be diagnosis + prioritized remediation
  candidates (not implementation); no fix has been merged (all investigation commits are
  docs-only; D1/D2/D3 confirmed unmerged at HEAD `5a85317b`).

---

```json
{
  "executive_summary": "The string is the Overseer's own observation write-back signature (observation_signature, overseer/mod.rs:1068-1073): the cycle's problem dedup_keys, sorted/deduped/'|'-joined, prefixed 'overseer-obs:'. 'Seen 2x' is a real, honest cross-window re-observation of a near-static unresolved problem set — NOT a dedup/storage/replay/collision bug (H1 confirmed, H0 rejected). The set persists because two observe-and-flag loops never close: blocked goals bare-park with no WHY class (double-gated ladder, cycle.rs:582-702) and workstream-gaps are notify-only (WorkstreamCoverage is the sole High Decide arm with no launch edge, mod.rs:1534-1543). 2x parks in a dead zone between detection threshold 2 and escalation threshold 3. resource:engineer_spawn is benign membership drift.",
  "detailed_explanation": "Provenance: overseer-obs prefix+join = observation_signature (mod.rs:1068-1073); goal:blocked:<goal_id> = GoalBlocked arm (mod.rs:1336); workstream-gap constant = WorkstreamGap arm (mod.rs:1371); resource:engineer_spawn constant = EngineerSpawnRate arm (mod.rs:1267-1272); nested overseer-obs = recall-derived RecurringSignature written back (mod.rs:1353-1363, wiring.rs:301). Two decoupled lanes: Lane A episodes (store_episode, +1 per 900s window, threshold 2, the visible x2) vs Lane B occurrences (store_fact append-only, threshold 3, escalation). Root cause A: WHY reasoner double-gated (Gate A completion_evidence.is_some(); Gate B investigation_enabled) fails open to bare-park; no invariant ties Blocked to a NoProgressClass; kgpacks incident = 7 goals parked as stuck when work was done. Root cause B: workstream-gap is a backlog-coverage gap (sensor.rs:288-320), notify-only via act_flag_workstream_gaps (mod.rs:884-948), no FileIssue/LaunchRecipe. Three defects D1 (emission hygiene), D2 (escalation counter+gate coupled; de-ratchet trap requires count-in-content), D3 (no closing edge) all unmerged at HEAD 5a85317b.",
  "visual_aids": "Four diagrams: (1) signature assembly pipeline run_cycle->orient->signal_to_problem->write_back_observation->observation_signature->[900s gate]->record_observation->store_episode; (2) two decoupled counter lanes A(episodes,>=2) vs B(occurrences,>=3) with dead-zone gap; (3) oscillation triangle workstream-gap<->goal:blocked<->resource:engineer_spawn = one under-throughput problem in three views; (4) blocked-goal resolution ladder with Gate A/Gate B fail-open to bare park.",
  "key_insights": [
    "The count is honest — audit the missing closing action, not the counter.",
    "Two decoupled storage lanes; the dead zone at 2 is a cross-lane visibility gap; remediation rung must sit on the episode lane.",
    "One lever, several symptoms: kgpacks/coverage-audit/personas/coin-harness/engineer_spawn all funnel through the bare no-progress park with no WHY.",
    "De-ratchet obvious fix is a trap: store_fact_with_caller_key makes recall.len() stick at 1; carry count in fact content.",
    "Overseer observes its own bookkeeping (nested overseer-obs); D1 exclude-recall-derived-at-write-boundary is highest leverage.",
    "WorkstreamCoverage is dual-path quarantined (no FileIssue and no LaunchRecipe).",
    "workstream-gap != decomposition failure; resource:engineer_spawn is benign membership drift not code drift.",
    "Latch risk: WHY-gate and counter fixes must ship atomically or Lane B over-escalates and latches."
  ],
  "remaining_unknowns": [
    "Whether completion_evidence was actually None in the live daemon (Gate-A state inferred, not observed).",
    "Restart vs >900s new window as the source of exactly-2x (indistinguishable from the signature alone).",
    "Whether escalation latches at recurrence>=3 and never un-latches (analyzed from code, not confirmed on a goal that crossed the bar).",
    "Whether WorkstreamCoverage should key per-gap (workstream-gap:<signature>) vs the bare constant.",
    "Actual cross-restart episode inflation volume on the live store (unmeasured).",
    "Whether the ask is diagnosis-only or diagnosis+implementation (assumed diagnosis + prioritized candidates; D1/D2/D3 unmerged)."
  ],
  "head": "5a85317b",
  "source_drift": "none (6e3113bc..HEAD -- '*.rs' empty)",
  "verdict": "Honest cross-window re-observation of a genuinely re-observed near-static problem set; every defect is design-level, none a dedup/storage bug. 17 targeted tests + the 365-test overseer suite all green."
}
```
