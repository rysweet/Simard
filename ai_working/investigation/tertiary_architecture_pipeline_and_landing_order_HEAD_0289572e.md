# Tertiary (Architect) Deep Dive — End-to-End Pipeline & Dependency-Safe Remediation Landing Order

**Investigation:** issue #4087 — "recurring signature seen 2× in cognitive memory (overseer-obs:…)"
**Role:** TERTIARY / architect
**HEAD re-grounded:** `0289572e` (source identical to `388e6c29` / `85b9398a`;
`git diff --name-only 85b9398a HEAD` = 12 files, **all under `ai_working/`**, **zero `src/` changes**).
**Method:** token-by-token trace against source with file:line citations. No doc-to-doc trust —
every claim below re-confirmed against current source.

---

## 1. Verdict (up front)

- The `×2` is a **real, honest re-observation of a static blocked set**, not a hash/count/replay
  artifact — BUT it is **self-referential**: the recalled signature is the Overseer's *own*
  observation write-back, folded back into the next observation. This is a genuine
  **self-observation feedback loop**, not a display glitch.
- Three distinct, confirmed structural defects exist (**D1 idempotency**, **D2 meta write-back
  feedback**, **D3 recurrence dead-zone**). They are entangled and must land in a specific order.
- **Dependency-safe landing order: D2 → D1 → D3** (with the classify/decide meta-guard co-landing
  with D2).

---

## 2. End-to-End Pipeline (source-cited)

```
                        ┌────────────────────────────────────────────────────────────┐
                        │  OODA tick: run_cycle()  (src/overseer/mod.rs:~410–489)      │
                        └────────────────────────────────────────────────────────────┘

 (A) OBSERVE ──────────────────────────────────────────────────────────────────────────────
     StatusReader.snapshot() → ObservedState
       blocked_goals[], workstream_gaps[], live_engineers, …

 (B) PRE-RECALL keys (mod.rs:425–426)
     pre_signals = signals_from(state)              (signal.rs:366)
     pre_problems = orient(pre_signals, in_flight)  (mod.rs:1200)
     keys = RecallKeys::from_signals(...)           (capabilities.rs:528)
            keywords = per-signal tokens (signal_keyword, capabilities.rs:556)
            signatures = pre_problems' dedup_keys   (capabilities.rs:533)

 (C) RECALL PASS (mod.rs:498; wiring MemoryRecallOps at wiring.rs:988)
     recall_episodic → RecalledEpisode {
        failure_signature = parse_failure_signature(content)   (wiring.rs:976,1025)
        # recovers "overseer-obs:…" from the "[sig:…]" marker written at (G)
     }

 (D) SIGNALS (incl. recall-derived)   signals_from(observed)   (signal.rs:366,455–470)
     Count recalled episodes by failure_signature (BTreeMap).
     If a signature has ≥ RECURRING_SIGNATURE_THRESHOLD (=2, signal.rs:362) copies:
        push Signal::RecurringSignature { signature, occurrences }   (signal.rs:464)
     # NOTE: signature here can be an "overseer-obs:…" written by THIS overseer at (G).

 (E) ORIENT → PROBLEMS   orient(signals, in_flight)   (mod.rs:1200)
     classify_signal(RecurringSignature) →
        kind     = ProblemKind::ProcessHealth        (mod.rs:1357)
        priority = High
        dedup_key= sanitize_recalled(signature)      (mod.rs:1359)   ← the "overseer-obs:…" itself
     Same-key merge raises priority (mod.rs:1211–1219).

 (F) DECIDE + GATE   decide(problem) (mod.rs:1402); gate() (mod.rs:569)
     ProblemKind::ProcessHealth → Intervention::LaunchRecipe {
        task_description = problem.summary
           = "recurring signature seen 2× … (overseer-obs:…)"      (mod.rs:1431)
     }                                                              ← D3 mis-routing
     ProblemKind::GoalHygiene(blocked) → decide_blocked_goal(...)   (mod.rs:1449,1605)
        recurrence≥3 → Escalate; perpetual+marker → Unblock;
        needs_review → Escalate; ELSE → Report                      ← D3 dead-zone

 (G) WRITE-BACK   write_back_observation(problems)   (mod.rs:534–563)
     signature = observation_signature(problems)     (mod.rs:1068–1073)
        = "overseer-obs:" + sorted,deduped dedup_keys joined by "|"
     WhisperGate.peek(signature, now)  → Deliver only if not seen in window
     record_observation(episode)       (wiring.rs:1076–1091)
        content = "{content} [sig:{signature}]"  → store_episode(source_label="overseer")
     WhisperGate.commit(signature, now)  (append-only; NO signature-keyed upsert) ← D1

            └──────────────────── feeds back into (C) next tick ───────────────────┘
```

### The runaway (D2) shown concretely

Because (E) sets `dedup_key = "overseer-obs:…"` for a recall-derived RecurringSignature, that key
becomes one of the keys joined in the NEXT `observation_signature` at (G):

```
tick n   : problems = {goal:blocked:A, …, workstream-gap, resource:engineer_spawn}
           sig_n = overseer-obs:goal:blocked:A|…|resource:engineer_spawn|workstream-gap
tick n+1 : recall fires RecurringSignature{signature=sig_n, occ=2}
           → problem dedup_key = sig_n
           sig_{n+1} = overseer-obs: … | overseer-obs:goal:blocked:A|…|workstream-gap
                        ^ prior whole keyset folded back in as ONE giant nested key
```

This is exactly the shape of the observed composite blob (repeated `overseer-obs:…` prefixes
concatenated, with `workstream-gap` and `resource:engineer_spawn` as leaf constituents).

---

## 3. Confirmed Defects (architect lens)

### D1 — Non-idempotent write-back (store bloats; recurrence count is a cadence artifact)
- `record_observation` (wiring.rs:1076) unconditionally `store_episode`s a NEW node; there is **no
  signature-keyed upsert**. The only dedup is the in-process `WhisperGate` window (mod.rs:548),
  which lapses (~per-window), so an unchanged blocked set writes fresh identical-signature
  episodes over time.
- Recall then counts those duplicates as "occurrences" (signal.rs:456–459). So
  `occurrences = 2` measures **write cadence across windows**, not two distinct real-world
  recurrences.
- **Component boundary at fault:** the memory-store stud (`MemoryRecall::record_observation`) is
  keyed by node identity, not by observation signature.

### D2 — Recall-derived meta-problem write-back (self-observation feedback loop)
- A RecurringSignature problem whose `dedup_key` is an `overseer-obs:*` string (mod.rs:1359) is a
  **meta-observation** (the Overseer observing its own bookkeeping), yet it is treated as a
  first-class problem and folded into the next `observation_signature` (mod.rs:1069).
- Result: unbounded nesting, memory-graph pollution, and operator-surface noise. This is the
  **root cause** of the `overseer-obs:overseer-obs:…` nesting.
- **Interface at fault:** `observation_signature`/`observation_content` (mod.rs:1068,1079) accept
  the full `problems` slice with no provenance filter; recall-derived problems are indistinguishable
  from freshly-sensed ones at the write boundary.

### D3 — Recurrence dead-zone + meta mis-routing (missing closing-action rung)
- Two thresholds are intentionally staggered:
  `RECURRING_SIGNATURE_THRESHOLD = 2` (signal.rs:362) detects recurrence;
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (root_cause.rs:33) gates escalation. A test even pins the
  ordering (`tests_signature_verification.rs:164`).
- For a non-perpetual, non-`needs_review` blocked goal at `recurrence == 2`,
  `decide_blocked_goal` (mod.rs:1615–1632) falls through every arm to **`Intervention::Report`** —
  neither remediation (`UnblockGoal`) nor escalation (`EscalateBlockedGoal`). **Dead zone.**
- Separately, the recall-derived meta problem is routed as `ProblemKind::ProcessHealth →
  LaunchRecipe` (mod.rs:1431) with `task_description = "recurring signature seen 2× (overseer-obs:…)"`
  — a **nonsensical remediation** (a fix recipe pointed at the Overseer's own bookkeeping string,
  not at the underlying blocked goals).
- Net: the constituents stay blocked pass after pass; the only "action" is a Report and a
  self-referential recipe launch — non-convergence.

---

## 4. Minimal Fixes & Dependency-Safe Landing Order

**Order: D2 → D1 → D3** (meta-guard co-lands with D2). Rationale is a true dependency chain, not
preference:

### Step 1 — D2 (loop-breaker): exclude recall-derived meta-problems from write-back
- Change: before computing `observation_signature`/`observation_content` (mod.rs:546,551), filter
  out problems that are recall-derived meta-observations — i.e., evidence is (solely)
  `Signal::RecurringSignature` **or** `dedup_key.starts_with("overseer-obs:")`.
- Co-land the **classify/decide meta-guard**: a RecurringSignature whose signature is
  `overseer-obs:*` must not become an actionable `ProblemKind::ProcessHealth` (it is an
  observability artifact, not a remediable process fault).
- **Why first:** it is the only change that stops the *growth of new distinct signatures*. It has
  **no dependency** on D1/D3 and the smallest blast radius (a filter at one boundary).
- **Must precede D1:** making the store idempotent *before* breaking the loop would not help —
  each nesting level is a *different* signature, so upsert can't collapse a moving target. Idempotency
  without D2 still bloats.

### Step 2 — D1 (make recurrence meaningful): signature-keyed idempotent upsert
- Change: `MemoryRecall::record_observation` upserts one node per signature (bump a
  last-seen/occurrence counter) instead of appending a new node per window.
- **Depends on D2:** only once the signature set is stable (meta-free) does "one node per
  signature" describe a fixed target, and only then does the recall `occurrences` count reflect
  distinct observation windows rather than write cadence.
- **Must precede D3:** D3's escalation rung consumes the occurrence count; escalating on a
  cadence-inflated count (pre-D1) would be noisy/incorrect.

### Step 3 — D3 (add the missing rung): first-recurrence remediation/escalation
- Change: introduce a closing action for the `RECURRING_SIGNATURE_THRESHOLD ≤ recurrence <
  RECURRENCE_ESCALATION_THRESHOLD` band — e.g., at first genuine (non-meta) recurrence, route the
  blocked goal to a bounded remediation or a single operator escalation instead of `Report`
  (mod.rs:1632). Keep the two-tier design but ensure the lower tier is not a no-op.
- **Depends on D1+D2:** the rung must fire on a *meaningful* count (post-D1) over a *meta-free*
  signature (post-D2). Landing D3 first would escalate on artifacts and on nonsensical
  `overseer-obs:*` strings.

### Landing-order dependency graph
```
   D2 (exclude meta write-back + classify/decide guard)   ← land 1st (loop-breaker, no deps)
     │  breaks nesting; stabilizes the signature set
     ▼
   D1 (signature-keyed idempotent upsert)                 ← land 2nd (needs stable signatures)
     │  makes "occurrences" mean distinct windows
     ▼
   D3 (first-recurrence remediation/escalation rung)      ← land 3rd (needs meaningful count + meta-free sig)
```

---

## 5. Structural Concerns / Notes

- **`resource:engineer_spawn` interaction:** it is an ordinary leaf `dedup_key`
  (mod.rs:1270, `EngineerSpawnRate` → `resource:engineer_spawn`) and participates in
  `observation_signature` like any other key; it introduces **no new dedup-key mechanism**. It only
  appears in the blob because it co-occurs in the static problem set. No special handling needed
  beyond D2 (it is a legitimate sensed problem, not a meta one).
- **`workstream-gap`:** stable, evidence-independent key (mod.rs:1371); routed to
  `FlagWorkstreamGaps` (mod.rs:1545), which notifies but does not close the gap. It is a
  legitimate constituent, not a meta problem — leave it in write-back; it is a symptom of the same
  non-convergence, addressed by D3's closing-action philosophy, not by exclusion.
- **Provenance is sound at the store seam:** `source_label` is fixed to `"overseer"`
  (wiring.rs:1088) and recalled text is `sanitize_recalled`-cleaned at the admission boundary
  (mod.rs:1359, 1082) — so this is **not** a security/injection issue; it is a control-flow
  feedback issue.
- **This is an investigation.** No source change is landed here: the fixes are non-trivial
  (touch the write boundary, the store stud, and the decide ladder) and warrant the normal
  development workflow, not a drive-by edit.

---

## 6. Re-grounding of prior findings
- All prior `ai_working/investigation/` verdicts on provenance, feedback loop, and dead-zone are
  **consistent with current source** and remain valid at HEAD `0289572e`.
- Confirmed **docs-only drift since `85b9398a`** (`git diff --name-only 85b9398a HEAD` → only
  `ai_working/*`). Source at `388e6c29` and `0289572e` is byte-identical to `85b9398a` for `src/`.
- No prior remedy is superseded; the D2→D1→D3 ordering above supersedes any doc that listed the
  three fixes without a dependency order.
