# Tertiary Investigation (8th wave) — Reconciliation, Drift Detection & the Observe→Orient→Act Pipeline (HEAD `b9f99879`)

**Role:** TERTIARY investigator (architect).
**HEAD:** `b9f99879` — verified. **Date:** 2026-07-15.
**Focus:** reconciliation against `ai_working/investigation/` artifacts, **drift
detection** since the commits those docs were grounded on, and the
**Observe→Orient→Act pipeline diagram** for the recurring signature.
**Method:** independent, line-by-line re-read of every load-bearing seam in
`src/` (did **not** trust the docs' own citations). **Investigation-only — no
code changed.**

---

## 0. Reconciliation verdict — CONFIRM (extend, do not restart)

The prior investigation **re-grounds exactly at HEAD `b9f99879` with zero line
drift.** Every load-bearing citation across the primary/secondary/tertiary
artifacts re-verifies against live source. My independent re-read **confirms**
the three-defect geometry (D1/D2/D3) and the central verdict:

> The `×2` recurring signature is a **faithful cross-window fingerprint of a
> static, unresolved problem set — a REAL re-observation loop, NOT a
> dedup / storage / replay / hash-collision artifact.** It carries one genuine,
> bounded, self-referential write-back defect (D1: nested `overseer-obs:`
> fragments), which remains **open at HEAD.**

No prior conclusion is contradicted. This wave **extends** the record with (a) a
fresh drift ledger at `b9f99879`, and (b) the consolidated Observe→Orient→Act
pipeline diagram (§3).

---

## 1. Drift detection — commit chain and code delta (independently verified)

**Last code change** to the pipeline is `6b2bf5e1` (2026-07-14,
`fix(stewardship): stop recursive issue flood safely (#4063)`).
`src/ooda_loop` last changed at `ad8a2b81` (2026-07-08). **Every commit since —
including `85b9398a → 388e6c29 → 0289572e → 5a85317b → b9f99879` — is
`docs(investigation)`.**

```
git diff --stat 6b2bf5e1..b9f99879 -- src/overseer src/stewardship   → (empty)
git diff --stat 6b2bf5e1..b9f99879 -- src/ooda_loop                  → (empty)
```

**Net code drift since the earliest pinned doc (`85b9398a`) and the prior
tertiary reground (`5a85317b`): ZERO.** Because `src/` is byte-identical, every
line number the older-pinned docs cite is still exact. Re-read directly at HEAD:

| Load-bearing claim | Cited loc | Re-read @ `b9f99879` | Status |
|---|---|---|:--:|
| `observation_signature` = `sort_unstable`→`dedup`→`format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1068-1072` | `mod.rs:1072` literal | ✅ exact |
| `write_back_observation` records **all** problems through one gate; empty-set guard | `mod.rs:534-563` | `mod.rs:534,543,546,554,556` | ✅ exact |
| Recall-derived `RecurringSignature` → Problem, `dedup_key = sanitize_recalled(signature)` | `mod.rs:1353-1363` | `mod.rs:1353,1359` | ✅ exact (self-nesting live) |
| `orient` merges same-key; RecurringSignature co-signal **raises** priority | `mod.rs:1211-1219` | `mod.rs:1211,1217-1218` | ✅ exact |
| `goal:blocked:{goal_id}` key | `mod.rs:1336` | `mod.rs:1336` | ✅ exact |
| `resource:engineer_spawn` fixed key | `mod.rs:1270` | `mod.rs:1270` | ✅ exact |
| `workstream-gap` fixed key | `mod.rs:1371` | `mod.rs:1371` | ✅ exact |
| `WorkstreamCoverage` Decide arm → notify-only `FlagWorkstreamGaps` | `mod.rs:1534-1543` | `mod.rs:1534,1543` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `signal.rs:362,463` | `signal.rs:362,463` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3`; escalate at `recurrence >= 3` | `root_cause.rs:33`; `mod.rs:1613` | `root_cause.rs:33`; `mod.rs:1613` | ✅ exact |
| `record_occurrence` → append-only `store_fact` (ratchet) | `mod.rs:1034` | `mod.rs:1004,1034` | ✅ exact |
| **Recall has NO source-label self-exclusion** (D1 open seam) | `wiring.rs:1013-1031` | `wiring.rs:1025` (`parse_failure_signature(&e.content)` on **every** episode) | ✅ exact — loop OPEN |
| Overseer write-backs stored under `OVERSEER_SOURCE_LABEL="overseer"` | `wiring.rs:952,1088` | `wiring.rs:952,1084-1088` | ✅ exact |
| Proposed D1 fix (`dedup_key.starts_with("overseer-obs:")` filter in write-back) | `tertiary…5a85317b §2.3` | **NOT present** at `mod.rs:534-563` | ✅ fix unimplemented |

**Drift verdict: none.** The one previously-flagged documentation trap
(RECONCILIATION_LEDGER §2 / CONSOLIDATED §6.2b: the `store_fact_with_caller_key`
one-liner is a *never-escalate* trap, superseded by the count-in-content upsert)
**still stands and needs no revision.** The single producer of the
`overseer-obs:` prefix is still `observation_signature` (`mod.rs:1072`) — the D1
prefix-filter discriminator remains exact.

---

## 2. Component boundaries & responsibilities (architect view, re-confirmed)

| Seam | Component | Responsibility | Boundary integrity @ HEAD |
|---|---|---|---|
| **Observe** | `sensor.rs` + `run_cycle` (`mod.rs:384`) | Project raw board/gap/failure snapshot; `recall` starts `None` (`sensor.rs:143`) | ✅ pure projection; recall injected later |
| **Signal** | `signal.rs::signals_from` | Mint one `Signal` per condition; `RecurringSignature` at `occurrences>=2` (`signal.rs:362,455-470`) | ⚠️ counts recall-derived self-echoes (D1 entry) |
| **Orient** | `orient` + `classify_signal` (`mod.rs:1200-1371`) | Signal→Problem; merge same `dedup_key`; recall co-signal raises priority | ⚠️ standalone `overseer-obs:` key never merges → nests |
| **Decide** | `decide_*` (`mod.rs:1534-1613`) | Problem→Intervention; escalate blocked goals at `recurrence>=3` | ⚠️ `WorkstreamCoverage` notify-only (D3 hole); `×2` in dead zone (D2) |
| **Act** | `act_flag_workstream_gaps` (`mod.rs:884`), `notify.rs` | Emit operator notification (email/Signal) | ⚠️ no `FileIssue`/`LaunchRecipe` close edge → gap re-observed |
| **Store** | `write_back_observation` (`mod.rs:534`) → `record_observation` (`wiring.rs:1076`) | Gate on 900 s `WhisperGate`, embed `[sig:…]`, store under `"overseer"` | ⚠️ re-records recall-derived `overseer-obs:` problems (D1 exit) |
| **Recall** | `recall_episodic` (`wiring.rs:1013`) | Read episodes, recover `failure_signature` from **any** content | ❌ **no `source_label` self-exclusion → D1 loop closes here** |
| **Stewardship dedup** | `stewardship/dedup.rs:63` (`sha256`) | GitHub-**issue** dedup only | ✅ orthogonal namespace — NOT on the signature path |

**Structural boundary finding (re-confirmed):** the recurring signature is
composed **entirely** from Overseer Problem `dedup_key`s (the `overseer-obs:`
composite namespace). It **never** touches the stewardship `sha256`
`failure_signature` namespace. The two dedup systems are correctly orthogonal;
`stewardship/routing.rs` routes issues, not memory episodes. Ruled out as origin.

---

## 3. Observe → Orient → Act pipeline diagram (the D1 open loop)

```
                            ┌──────────────────────────── D1 OPEN FEEDBACK LOOP ────────────────────────────┐
                            │                                                                               │
  ┌──────────┐   ┌─────────▼──────────┐   ┌───────────────┐   ┌──────────────┐   ┌───────────┐   ┌─────────┴─────────┐
  │ OBSERVE  │──▶│ SIGNAL             │──▶│ ORIENT        │──▶│ DECIDE       │──▶│ ACT       │   │ STORE (write-back)│
  │ sensor.rs│   │ signals_from       │   │ orient +      │   │ decide_*     │   │ notify.rs │   │ write_back_       │
  │ run_cycle│   │ signal.rs          │   │ classify_sig  │   │ mod.rs       │   │ mod.rs:884│   │ observation       │
  │ mod.rs384│   │                    │   │ mod.rs1200    │   │ 1534 / 1613  │   │           │   │ mod.rs:534        │
  └────┬─────┘   └─────────┬──────────┘   └──────┬────────┘   └──────┬───────┘   └─────┬─────┘   └─────────┬─────────┘
       │ blocked_goals     │ GoalBlocked        │ goal:blocked:{id}  │ escalate iff     │ email/Signal      │ observation_
       │ workstream_gaps   │ WorkstreamGap      │ workstream-gap     │ recurrence>=3    │ notify-ONLY       │ signature()
       │ failure sink      │ EngineerSpawnRate  │ resource:engineer_ │ (root_cause.rs33)│ (no FileIssue,    │ = "overseer-obs:
       │                   │                    │   spawn            │  ── ×2 < 3 ──▶   │  no LaunchRecipe) │    a|b|c"
       │                   │ RecurringSignature │ overseer-obs:…     │  DEAD ZONE (D2)  │ gap re-observed   │ [sig:…] episode
       │                   │ occurrences>=2     │ STANDALONE (never  │                  │ next tick (D3)    │ store_episode(
       │                   │ (signal.rs:362)    │ merges → NESTS)    │                  │                   │  "overseer") 1088
       │                   │        ▲           │ mod.rs:1353-1363   │                  │                   └─────────┬─────────┘
       │                   │        │           └────────────────────┘                  │                             │ persisted
       │              RECALL│        │                                                    │                             ▼
       │      recall_episodic        │                    ┌───────────────────────────────────────────────┐  cognitive memory
       └──────────────◀────┴─────────┴────────────────────┤ recall_episodic (wiring.rs:1013-1031)          │◀───(episodes)
                     recall.failure_signature =            │ failure_signature = parse_failure_signature(   │
                     parse of EVERY episode incl. own      │   &e.content)  ── NO source_label exclusion ── │
                                                           └───────────────────────────────────────────────┘
```

**Loop trace (5 hops, all live @ HEAD):**
1. STORE writes an episode whose `[sig:…]` **is** an `overseer-obs:…` composite (`mod.rs:1072`, `wiring.rs:1084-1088`).
2. RECALL reads it back and parses the composite as a `failure_signature` — **including the Overseer's own write-backs, no self-exclusion** (`wiring.rs:1025`).
3. SIGNAL counts ≥2 identical → `RecurringSignature{ signature:"overseer-obs:…", occurrences }` (`signal.rs:455-470`, threshold 2 at `signal.rs:362`).
4. ORIENT maps it to a **standalone** High `ProcessHealth` problem, `dedup_key = sanitize_recalled("overseer-obs:…")` — no fresh problem shares that prefix, so it never merges (`mod.rs:1353-1363`, `1211`).
5. STORE folds **all** problem keys (including the `overseer-obs:…` one) into a **new** signature → `overseer-obs:…|overseer-obs:…` nests one level deeper, and loops to (1) (`mod.rs:534-563`).

**Why the write-back gate does not break it:** the 900 s `WhisperGate`
(`WhisperGate::new(900,5)`) suppresses only **same-window** duplicates. Across
windows the signature re-delivers by design; while nesting grows, each composite
is a *new* string so the gate always `Deliver`s. Correct behavior, wrong
expectation if treated as a loop breaker. The loop is **open at HEAD**.

---

## 4. Design-quality assessment & structural concerns

1. **Provenance is under-namespaced at the recall boundary.** `record_observation`
   correctly stamps `source_label = "overseer"` (`wiring.rs:1088`), but
   `recall_episodic` **discards** that provenance (`wiring.rs:1024-1029` maps
   `e.content` only, never `e.source_label`). The self-vs-external distinction
   exists in storage but is **erased on read** — the precise architectural cause
   of D1. The cheapest correct fix lives here (drop own-source episodes at
   recall) or at the write-back filter (drop `overseer-obs:` keys); both are
   ~4 lines, orthogonal to D2/D3.
2. **Two-threshold dead zone (D2) is a coupling defect, not a value defect.**
   Episode-count fires at 2 (`signal.rs:362`); root-cause escalation needs 3
   (`root_cause.rs:33`) on a **different** storage lane (append-only `store_fact`,
   `mod.rs:1034`). The visible `×2` can never climb to the `3` bar. Fixing the
   *number* is a trap; the gate+counter must ship **atomically** (count-in-content
   upsert), consistent with RECONCILIATION_LEDGER §2.
3. **`WorkstreamCoverage` is the only High-priority Decide arm with no close
   edge (D3).** Notify-only Act (`mod.rs:1534-1543`, `act_flag_workstream_gaps`)
   leaves the `workstream-gap` token terminal → re-observed every tick. Any
   remediation rung must key on `GapItem.signature` (per-gap), **not** the bare
   `"workstream-gap"` dedup_key (INV-GAP-KEY trap), else all gaps fold into one.
4. **`workstream-gap → resource:engineer_spawn` is NOT an orchestration cycle**
   (confirms secondary S3). No code edge connects them; co-occurrence reflects a
   real under-resourced **state** (engineers ≥8 live **and** coverage incomplete),
   not a spawn loop. Real spawning lives in OODA (`cycle.rs:665`,
   `no_progress.rs:712`), bounded to one guided retry. Steady-state, not defect.

---

## 5. Answer to the mandate

1. **Reconciliation verdict:** **CONFIRM** every prior finding; **extend** with a
   fresh drift ledger. All primary/secondary/tertiary conclusions re-ground
   exactly at HEAD `b9f99879`.
2. **Drift detection:** **ZERO code drift.** `6b2bf5e1..b9f99879` touches no
   `src/` file on the pipeline; the entire chain is `docs(investigation)`. Every
   cited line is byte-identical; the D1 fix is still unimplemented; the §6.2b
   remedy trap remains correctly superseded.
3. **Pipeline diagram:** §3 — the closed 5-hop Observe→Orient→Act(+Store→Recall)
   loop, with the open seam pinpointed at `recall_episodic` (`wiring.rs:1024-1029`,
   provenance erased on read).
4. **Landing order (unchanged):** D2 (atomic gate+counter) → D3 (closing rung) →
   D1 (recall/write-back self-exclusion filter) → convergence gauges. D1 alone
   stops the *nesting shape* but not the `×2` recurrence (the problem set stays
   static — that is D2+D3).
