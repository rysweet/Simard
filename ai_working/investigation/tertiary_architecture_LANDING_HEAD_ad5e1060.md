# Tertiary (Architect) — Signature-Assembly & Dedup/Gate Architecture + Minimal Safe Fix Landing

**Role:** Tertiary investigator (architecture). **Focus:** diagram the signature-assembly and
dedup/gate architecture; pinpoint the minimal safe fix landing point without redesigning the
OODA loop. Investigation-only — no code changes.

**Grounding:** HEAD `ad5e1060` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
Every line citation below was re-read against live `src/overseer/*` this pass; the prior
synthesis's `6e3113bc..HEAD` source-diff-empty claim still holds (all investigation commits are
docs-only). No source drift.

---

## 1. Verdict alignment (from my architecture seat)

I independently re-traced the pipeline and gate primitive. I **confirm** the standing verdict:
the pipe-delimited string is **not a raw memory key** — it is a *synthesized composite* built by
`observation_signature` (`mod.rs:1068-1073`), and **"seen 2×" is honest re-observation**, not a
dedup/replay/storage/collision defect. The gate that should suppress within a window is real and
correct in-window; the recurrence comes from **(a) window expiry (>900 s)** and, most probably
for *exactly* 2×, **(b) daemon restart clearing the in-memory gate**. The persistence of the
underlying problem *set* is the actual defect surface, in two non-closing observe-and-flag loops.

---

## 2. Signature-assembly architecture (verified)

```
run_cycle
  └─ orient(signals, in_flight)                                   mod.rs:1200
       └─ classify_signal(s) → (kind, priority, dedup_key, sum)   mod.rs:1238
            • goal:blocked:<goal_id>          format!            mod.rs:1336   (goal_id = <slug>-<8hex>, minted at goal creation — NOT hashed here)
            • workstream-gap                  literal            mod.rs:1371   (gaps.len() → summary only)
            • resource:engineer_spawn         literal            mod.rs:1270   ({live} → summary only)
            • <recalled sig> (RecurringSignature) sanitize_recalled mod.rs:1359 (self-observation re-admission)
       └─ Problem { dedup_key, ... }
  └─ write_back_observation(problems)                             mod.rs:534   (single call site: wiring.rs:301)
       └─ signature = observation_signature(problems)             mod.rs:546 → 1068
            keys = problems.map(dedup_key); sort_unstable; dedup;
            "overseer-obs:" + keys.join("|")                      mod.rs:1069-1072
       └─ write_back_gate.peek(signature, now)                    mod.rs:548
            ├─ Deliver  → record_observation(episode); commit     mod.rs:549-557
            └─ Suppress → Ok(None) (nothing persisted this tick)  mod.rs:559-561
```

**Structural facts that matter for the fix:**

- The composite key is a pure function of the *set* of `dedup_key`s. Two of the recurring token
  families carry **constant literal keys** (`workstream-gap` `mod.rs:1371`, `resource:engineer_spawn`
  `mod.rs:1270`); their volatile counts live only in the human summary, so they **cannot**
  perturb dedup/idempotency. `resource:engineer_spawn` is therefore benign membership drift, a
  corroborating third view of under-throughput — not a contradicting signal.
- `keys.dedup()` (`mod.rs:1071`) only collapses **adjacent equal** keys after sort. Distinct gap
  problems all reduce to the single family key `workstream-gap`, so within one signature they
  collapse to one token; the visible `workstream-gap|workstream-gap` run is **cross-episode**
  concatenation of successive write-backs, not intra-signature duplication.

---

## 3. Dedup/gate architecture & durability model (verified)

`WhisperGate` is one reused primitive (`guardrails.rs:291-333`) instantiated four times, each
with an independent keyspace so lanes never interfere:

| Gate | Ctor | Keyspace | Site |
|---|---|---|---|
| `whisper_gate` | `WhisperGate::new(900, 5)` | whisper note sig | mod.rs:286 |
| `blocked_goal_gate` | `WhisperGate::new(900, 20)` | `unblock:<id>` / `escalate:<id>` | mod.rs:292, 780/823 |
| `write_back_gate` | `WhisperGate::new(900, 5)` | composite `overseer-obs:…` | mod.rs:299, 548 |
| `gap_gate` | `WhisperGate::new(900, 200)` | `workstream-gap:<g.signature>` | mod.rs:304, 901/932 |

**Durability = per-process, in-memory only.** `last_delivered: HashMap<String,i64>` and
`deliveries: Vec<i64>` (`guardrails.rs:294-295`) are heap state on the `Overseer` struct,
constructed empty in `Overseer::new` (`mod.rs:305`). There is **no persistence, no rehydrate,
no cross-restart ledger**. Consequences:

1. **Window model is correct** for a *live* daemon: `peek` suppresses while
   `now - last < window_secs` (`guardrails.rs:312-317`). In-window dedup is a proven invariant.
2. **Restart is a silent gate reset.** After a restart the map is empty, so a still-true
   condition re-delivers immediately — regardless of the 900 s window. This is the most likely
   producer of *exactly* 2× (one pre-restart episode + one post-restart episode).
3. The gate conflates "already told you this window" (volatile, correct to forget) with "this
   condition has been open across restarts" (durable, should persist). **This conflation is the
   architectural gap**, not the window arithmetic.

Note the interesting asymmetry: `gap_gate` keys on the **per-gap** `g.signature`
(`mod.rs:901`), which *does* preserve gap identity — but only in volatile memory, so it too
forgets across restarts and has no cross-window ledger. Lane B (root-cause occurrences,
`store_fact` `mod.rs:1034`) is the *only* lane with **durable** append-only accrual; it is
starved by the WHY double-gate (see §4). So the system has one durable counter (Lane B, gated
off) and three volatile gates (reset on restart) — the recurrence is the predictable product of
that split.

---

## 4. The two non-closing loops (where the problem set persists)

```
Loop 1 — goal:blocked (WHY-gated resolution)
  GoalBlocked → GoalHygiene problem → decide_blocked_goal            mod.rs:1447-1483, 1603-1631
     recurrence>=3 → EscalateBlockedGoal        (but Lane B rarely reaches 3 — starved)
     perpetual & no-progress → UnblockGoal
     needs_review → EscalateBlockedGoal
     else → Report                              ◀── PARK, no WHY class, re-observed next window
  Root cause: WHY reasoner double-gated + fails-open to bare park (ooda_loop/cycle.rs WHY block);
  no invariant binds a Blocked reason to a NoProgressClass. All stall classes collapse to bare park.

Loop 2 — workstream-gap (coverage flag)
  WorkstreamGap → WorkstreamCoverage → FlagWorkstreamGaps            mod.rs:1534-1543
     act_flag_workstream_gaps → notifier.notify(...) ONLY            mod.rs:884-948
     (docstring mod.rs:881-883: "never create GitHub issues or backlog items")
  Root cause: WorkstreamCoverage is the ONLY High-priority Decide arm with NO launch.rs edge.
  Siblings ProcessHealth/CrossCutting/StepFailure all reach LaunchRecipe (mod.rs:1429-1443,1549+).
```

Both loops **observe and flag** but never emit the **closing edge** (classify-and-resolve for
Loop 1; launch/file-a-workstream for Loop 2). The condition stays true → the composite signature
re-forms every window → "recurring signature." The recurrence count is honest telemetry *of a
real open backlog*, so raising the escalation bar or muting the signal would hide a true signal —
the fix must add the missing closing edge, not silence the observation.

---

## 5. Minimal safe fix — landing points (scope-bounded)

Three independent seams; each is a small, local edge addition. **None require OODA redesign.**
Ordered by safety/independence. This is landing guidance for a *later* implementation task — I
did not implement.

### D3 — the missing closing edge for `workstream-gap` (lowest risk, highest signal)
- **Landing:** `decide` `WorkstreamCoverage` arm (`mod.rs:1534-1543`) and/or
  `act_flag_workstream_gaps` (`mod.rs:884-948`).
- **Minimal change:** after the consolidated operator notification, add ONE closing action per
  fresh gap — file a deduped stewardship item / `LaunchRecipe` brief routed through
  `stewardship/routing.rs::route_failure` (total-over-inputs, falls back to `rysweet/Simard`,
  `routing.rs:39-52`), reusing the existing per-gap `gap_gate` key (`mod.rs:901`) so it stays
  idempotent. **Scope boundary:** do not change gap *detection* (`sensor.rs:288-320`) or the
  notification format — only add the launch/file edge that every sibling High arm already has.

### D1 — emission hygiene (self-observation re-admission)
- **Landing:** the write-back call site (`wiring.rs:301`) / `observation_signature` input
  filter (`mod.rs:1068-1073`) — exclude recall-derived `RecurringSignature`-sourced problems
  (`mod.rs:1353-1363`) from the composite so the Overseer stops folding its own prior
  observation back into the next signature (the nested `overseer-obs:…|overseer-obs:…` runs).
- **Minimal change:** filter one problem kind out of the key set before `join("|")`. Pure,
  local, no gate/counter change. **Scope boundary:** filter only the emission key; leave the
  priority-raising merge in `orient` (`mod.rs:1217-1219`) intact.

### D2 — durable recurrence accrual for escalation (most coupled — land last, land whole)
- **Landing (counter):** `record_occurrence`/`store_fact` seam (`mod.rs:1034`) — Lane B.
- **Landing (gate):** the WHY double-gate that starves accrual (`ooda_loop/cycle.rs` breaker/WHY
  block) so a `Blocked` reason is always bound to a `NoProgressClass` (INV-WHY).
- **Coupling warning (verified against the ratchet note):** the counter and its accrual gate are
  a coupled pair — fixing *either alone changes nothing observable*. Do **not** naively swap
  `store_fact` for a `CallerKey`-deduped write: `DedupMode::CallerKey` keeps one live fact per
  key, collapsing `recall.len()` to 1 forever and making `recurrence >= 3` (`mod.rs:1613`,
  `root_cause.rs:33`) dead code. Correct minimal form is **count-in-content upsert**. **Scope
  boundary:** keep the threshold semantics; do not re-plumb the OODA breaker itself.

### Durability gap (cross-cutting, optional, do NOT expand scope)
The in-memory gate reset (`guardrails.rs:294`, §3) is a *separate* concern. The minimal safe
disposition is to **leave the window gates volatile** (they are correct for a live daemon) and
let the D1/D2/D3 closing edges remove the *persistent condition* that makes restart re-emission
visible. Persisting `write_back_gate`/`gap_gate` state across restarts is a larger change with
its own correctness surface (stale-slot pruning, clock skew) and is **out of minimal scope**;
record it as a follow-up, not part of the safe fix.

---

## 6. Recurrence-threshold / escalation-bar appropriateness

- The visible `×2` is **Lane A** (`RecurringSignature`, threshold `2`, `signal.rs`), decoupled
  from **Lane B** escalation (threshold `3`, `mod.rs:1613`). `2` is above one-off noise but
  below the escalation bar of `3`, with **no remediation rung between** — the "dead zone."
- **Assessment:** the thresholds themselves are *reasonable* and should **not** be moved as the
  fix. Moving the bar to `2` would escalate honest transient re-observations; the real gap is
  the missing closing edges (D1/D2/D3) and the cross-lane visibility split, not the numbers. The
  `2×` should be treated as a *true low-grade signal of an open backlog*, resolved by closing the
  loops — not by re-tuning the recurrence/escalation thresholds.

---

## 7. Scope boundary (explicit)

In-scope for the minimal safe fix: the three closing-edge seams (D1 emission filter, D2
count-in-content + WHY-binding, D3 gap launch/file edge). **Out of scope:** redesigning the
OODA/OODA-loop architecture, re-tuning recurrence/escalation thresholds, persisting the volatile
window gates, and altering gap *detection* or signature *format*. The composite-signature and
`WhisperGate` primitives are sound; the defect is three absent resolution/launch edges plus the
Lane-B starvation, each fixable locally.
