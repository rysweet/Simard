# Tertiary Investigation (re-run) — Blocked/Gap Observation + Dedup Pipeline, Idempotency Boundaries, and Why the Single-Counter Fix Is a Trap

**Role:** Tertiary investigator (architect).
**HEAD:** `85b9398a` (the two commits after `dea65df8` are investigation docs only — **no fix is merged; every defect below is live**).
**Focus:** End-to-end blocked/gap observation → signature → dedup → cognitive-memory write pipeline with idempotency boundaries marked; a dependency-correct remediation scope; and the code-grounded reason the naive single-counter fix is a trap.
**Method:** Every claim re-grounded in named functions/lines at current HEAD, then one concrete goal (`fix-agent-kgpacks-rs-issue-17-ws2-...`) traced end-to-end.

---

## 0. Bottom line

The investigated string is **not** an external failure that recurred. It is the Overseer **recalling its own prior observation write-back**. The composite `overseer-obs:<sorted dedup_keys joined by |>` is deliberately built as a write-back **dedup key** (`observation_signature`, `mod.rs:1068-1073`), stored as an episode (`wiring.rs:1076-1091`), then **re-parsed on recall as a `failure_signature`** (`wiring.rs:976-986`, `1013-1031`) and counted (`signal.rs:455-470`). Two such episodes ⇒ `RecurringSignature{occurrences:2}` ⇒ a new `ProcessHealth` Problem whose `dedup_key` **is that same composite** (`mod.rs:1353-1363`), which is then folded back into the next write-back's signature — producing the literal nested `overseer-obs:…|…|overseer-obs:…` shape seen in the payload.

So the `×2` is a **faithful cross-window recurrence count of the Overseer's own bookkeeping**, amplified by a **self-referential nesting edge**. It is a cognitive-memory-internal loop; the stewardship (GitHub-issue) store is not on this cycle at all.

The single-counter fix is a trap because the signature aggregates **three independent defects on three seams** into one opaque string, and the one counter that actually gates escalation lives in the **weakest-idempotency store** while being **starved shut** by an upstream gate. Fixing the counter alone changes nothing observable.

---

## 1. End-to-end pipeline with idempotency boundaries

```
  OVERSEER TICK (one OODA cycle) — src/overseer/mod.rs::run_cycle (405–489)

  OBSERVE
   ├ goal health / no-progress ──────────► Signal::GoalBlocked{goal_id}
   ├ detect_workstream_gaps (sensor.rs) ─► Signal::WorkstreamGap{gaps}
   └ recall_pass → recall_episodic ──────► counts by failure_signature
        (mod.rs:498-516; wiring.rs:1013-1031)   (signal.rs:455-470, thr=2)
        └► Signal::RecurringSignature{signature, occurrences>=2}
             ▲                                            │
             │   ══════ SELF-REFERENTIAL FEEDBACK EDGE ══════
             │                                            ▼
  ORIENT  signal_to_problem (mod.rs:1296-1385)  →  Problem{kind, dedup_key, summary}
   ├ GoalBlocked        → GoalHygiene,        dedup_key = "goal:blocked:{goal_id}"   (1336)
   ├ WorkstreamGap      → WorkstreamCoverage, dedup_key = "workstream-gap"           (1371)
   └ RecurringSignature → ProcessHealth,      dedup_key = sanitize_recalled(sig)     (1359)
                          └─ pipes survive sanitize (capabilities.rs:468-482) → NESTS "overseer-obs:"

  ROOT-CAUSE  recall_occurrences(dedup_key) → recurrence = recall.len()  ◄─ COG-MEM read
              (mod.rs:455-459, 972-997; threshold=3 root_cause.rs:33)

  DECIDE  decide(problem) (mod.rs:1296+, 1603+)
   ├ GoalHygiene(blocked): recurrence>=3 ? EscalateBlockedGoal : self-heal/report  (1613)
   ├ StepFailure         → LaunchRecipe  ────────────────► CLOSING EDGE ✔          (1565)
   └ WorkstreamCoverage  → FlagWorkstreamGaps  ── notify-only, NO launch edge ✘    (1534-1543)

  ACT
   ├ act_flag_workstream_gaps: gap_gate.peek/commit (900s window) + notify only    (884-948)
   ├ record_occurrence → store_fact  ── APPEND-ONLY, no upsert ── COG-MEM write     (1004-1042)
   └ (defects) file deduped issue → process_orchestrator_run ── STEWARDSHIP write   (observer.rs:53-68)

  WRITE-BACK  write_back_observation (mod.rs:534-563)
   signature = observation_signature(problems) = "overseer-obs:{sorted keys|joined}"   (1068-1073)
   write_back_gate.peek(signature, 900s)==Deliver ? record_observation(episode)
        └ episode content embeds "[sig:<whole composite incl. nested overseer-obs:>]"  (wiring.rs:1084)
        └───────────────────────────────► COG-MEM write  ==> becomes next tick's recall input
```

### Idempotency boundaries (where the contract holds vs. breaks)

| # | Boundary | Key | Idempotency | Verdict |
|---|---|---|---|---|
| B1 | Stewardship issue filing | `failure_signature = sha256(kind‖norm(err))[..8]` (`dedup.rs:63-75`) | search-before-file → `FiledNew` once, `MatchedExisting` after (`observer.rs:53-68`), stable `run_id=overseer-{sig}` (`:79`) | ✅ **Truly idempotent.** Terminal output; not on the loop. |
| B2 | Observation write-back (episode) | `observation_signature` composite (`mod.rs:1068`) | `write_back_gate` = in-process `WhisperGate(900s/5)` | ⚠️ **Window-scoped only.** Re-persists across windows and across daemon restarts (in-proc `HashMap`). This is how `×2` accrues. |
| B3 | Root-cause occurrence (fact) — **the escalation counter** | `root_cause_signature = "{dedup_key}::{label}"` (`root_cause.rs:53-55`) | `store_fact` **append-only, no upsert** (`mod.rs:1034`); `recurrence = recall.len()` | ❌ **No idempotency.** Counts nodes, not distinct cycles → ratchets on every ACT. |
| B4 | Recall→signal detection | parsed `[sig:…]` marker (`wiring.rs:976`) | none — counts whatever episodes B2 persisted | ⚠️ **Amplifier.** Re-ingests B2's own output as a "failure signature." |
| B5 | Workstream-gap recurrence | `workstream-gap:{signature}` in `gap_gate` (`mod.rs:901-933`) | intra-window `WhisperGate` only; **no cross-window store** | ❌ **No ledger.** Structurally cannot cross any recurrence threshold — a permanent 2× dead zone. |

**The core architectural smell:** the boundary that is *terminal and harmless* (B1) is the only one that is idempotent, while the boundary that *drives a threshold escalation decision* (B3) has the weakest contract. Idempotency strength is inverted relative to consequence.

---

## 2. One goal traced end-to-end: `issue-17-ws2`

1. Goal `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed` is parked no-progress ⇒ `Signal::GoalBlocked{goal_id}` ⇒ Problem `dedup_key="goal:blocked:...-7f5afcca"`, `GoalHygiene`.
2. Root-cause: `recall_occurrences("goal:blocked:...-7f5afcca").len()`. Because ACT is gated shut (the WHY double-gate, `ooda_loop/cycle.rs:582-702`, and `decide_blocked_goal` only self-heals/reports below threshold, `mod.rs:1620+`), `record_occurrence` rarely runs for this key ⇒ `recurrence` stays **< 3** ⇒ **never escalates**. The goal stays blocked and re-emits its `goal:blocked` key every cycle.
3. Write-back: this key joins the sorted composite. The **same blocked set persists across 900 s windows** ⇒ B2 persists the identical `overseer-obs:…` episode again ⇒ two episodes ⇒ `RecurringSignature{occurrences:2}` for that composite ⇒ Problem whose `dedup_key` is the composite ⇒ nested into the next composite. **This is why `issue-17-ws2` also appears in the `workstream-gap|workstream-gap` cluster: the same cycle that can't close the block also can't close the gap** (B5), and both keys ride the same write-back episode.

Verdict: **real repeated cycles, not a double-persist of one cycle** — but "real" only in the sense that a *stuck* board is re-observed forever. The recurrence is a **liveness/convergence failure surfaced as a recurring-failure signature**, not a distinct new fault.

---

## 3. Why the single-counter fix is a trap (code-grounded)

The payload looks like one problem (one signature). It is **three defects on three seams**, and any fix that treats "the counter" as one thing is wrong:

- **D1 — emission/nesting (B2+B4).** Write-back persists the recall-derived `RecurringSignature` problem, whose `dedup_key` is an `overseer-obs:` string, so the signature nests and mutates each cycle (`mod.rs:546` over `problems` that include the `1359` problem). A single-counter fix on the recurrence tally does nothing to this; the composite keeps growing.
- **D2 — escalation counter is simultaneously a ratchet and a dead-zone (B3 + accrual gate).** `store_fact` append-only + `recurrence=len()` (`mod.rs:1034, 972-997`) *over*-counts when ACT runs; the WHY double-gate + sub-threshold `decide_blocked_goal` (`mod.rs:1613-1620`, `cycle.rs:582-702`) mean ACT usually **doesn't** run, so it *under*-counts to 0. **Naively adding a caller-key upsert makes it worse:** the secondary proved a plain `store_fact_with_caller_key(root_cause_signature)` collapses to one live fact so `recall.len()` sticks at **1**, and escalation at threshold 3 becomes **permanently unreachable**. The count must move **into the fact content** (upsert `occurrence_count + first/last_seen`), read from the field — *and* the accrual gate must be closed, or the counter is dead code.
- **D3 — no closing edge for `workstream-gap` (B5).** `WorkstreamCoverage → FlagWorkstreamGaps` is **notify-only** (`mod.rs:1534-1543`); `gap_gate` has no cross-window ledger. No counter value ever converges the gap. A counter fix on D2 cannot touch D3.

These three share the composite string but need three different fixes on three seams. **"Fix the dedup with a count" is under-specified by two seams and actively regressive at the third.** That is the trap.

---

## 4. Dependency-correct remediation scope

```
  (must ship ATOMICALLY — they form a latch)
  ┌────────────────────────────────────────────────────────────┐
  │  R1  Close the WHY double-gate (blocked-transition)         │  cycle.rs:582-702
  │      → every Blocked reason carries a NoProgressClass;       │  fail LOUD on None
  │        counter can finally accrue                            │
  │                       ▲ prerequisite                         │
  │  R2  Count-in-content occurrence record (idempotent counter)│  mod.rs:1004-1042
  │      → caller-key upsert on root_cause_signature;            │  read occurrence_count,
  │        one live fact/cause; escalation reads the FIELD       │  not recall.len()
  └────────────────────────────────────────────────────────────┘
  (independently shippable)
  ┌────────────────────────────────────────────────────────────┐
  │  R3  Recurrence-aware closing rung for workstream-gaps      │  mod.rs:931-934,1534-1543
  │      → record gap PriorOccurrence; >=2× → LaunchRecipe via   │  reuse launch.rs seam;
  │        existing seam; >=3×/unsafe → one operator escalation  │  classify at LaunchRecipe tier
  ├────────────────────────────────────────────────────────────┤
  │  R4  Write-back hygiene: exclude recall-derived problems    │  mod.rs:534-546
  │      (dedup_key.starts_with("overseer-obs:")) before signing │  one-line filter; kills nesting
  ├────────────────────────────────────────────────────────────┤
  │  R5  Convergence gauges + extend B2 dedup to (sig,window)   │  activity.rs / guardrails.rs:294
  │      → prove the fix holds; guard cross-restart inflation    │
  └────────────────────────────────────────────────────────────┘
```

**Landing order (dependency-correct):** R1+R2 atomic (the latch that unblocks every `goal:blocked:*` row and every simard-identity persona goal) → R3 (converges the `workstream-gap` family + personas) → R4 (removes the nested shape) → R5 (regression guard). R3/R4/R5 are independently valuable but never substitute for the R1+R2 latch.

---

## 5. Cross-check vs prior artifacts (revalidation verdict)

| Prior claim | This re-run @ 85b9398a | Status |
|---|---|---|
| `observation_signature` = sort→dedup→`overseer-obs:{join |}` | `mod.rs:1068-1073` exact | ✅ still valid |
| Self-nesting via recall-derived Problem `dedup_key` | `mod.rs:1353-1363`; pipes survive `sanitize_recalled` (`capabilities.rs:468-482`) | ✅ live |
| `record_occurrence` append-only ratchet | `mod.rs:1034` | ✅ live |
| `recurrence=len()`, escalate at 3 | `mod.rs:972-997`, `1613`; `root_cause.rs:33` | ✅ live |
| WorkstreamCoverage notify-only, no launch edge | `mod.rs:1534-1543` | ✅ live |
| gap_gate window-only, no cross-window ledger | `mod.rs:901-933` | ✅ live |
| `×2` is real cross-window recurrence, not a storage duplicate | confirmed via B1–B5 contracts | ✅ consistent |
| Naive caller-key upsert is a trap (collapses to len()=1) | reaffirmed as the D2 anti-fix | ✅ consistent |

**No prior conclusion is stale.** The two post-`dea65df8` commits are documentation; the entire defect surface (D1–D3) is live at `85b9398a`. Net-new in this re-run: the **idempotency-boundary table (B1–B5)** mapped onto the pipeline, the **inverted-idempotency framing** (terminal store idempotent, escalation store not), and the end-to-end `issue-17-ws2` trace tying the `goal:blocked` and `workstream-gap|workstream-gap` clusters to the *same* stuck cycle.

---

## 6. Evidence ledger

| Claim | Source @ HEAD |
|---|---|
| `observation_signature` composite | `src/overseer/mod.rs:1068-1073` |
| write-back persists all problems incl. recall-derived; 900s gate | `mod.rs:534-563` |
| RecurringSignature → ProcessHealth, dedup_key=sanitize_recalled(sig) | `mod.rs:1353-1363` |
| GoalBlocked→`goal:blocked:{id}`; WorkstreamGap→`workstream-gap` | `mod.rs:1336, 1371` |
| WorkstreamCoverage → notify-only FlagWorkstreamGaps | `mod.rs:1534-1543` |
| decide_blocked_goal escalates at recurrence>=3 | `mod.rs:1603-1620`; `root_cause.rs:33` |
| record_occurrence append-only store_fact | `mod.rs:1004-1042` |
| recurrence=recall_occurrences(...).len() | `mod.rs:972-997` |
| RecurringSignature emit threshold=2 | `signal.rs:362, 455-470` |
| recall episodic parses `[sig:…]` marker | `wiring.rs:976-986, 1013-1031` |
| record_observation embeds `[sig:{signature}]` | `wiring.rs:1076-1091` |
| sanitize_recalled preserves `|`, caps length only | `capabilities.rs:468-482` |
| RecurringSignature signal_keyword returns full signature (re-queries same episode) | `capabilities.rs:556-572` |
| failure_signature = sha256(kind‖norm)[..8]; `stewardship-signature:` marker | `stewardship/dedup.rs:63-81` |
| idempotent issue outcome; stable run_id | `overseer/observer.rs:53-68, 79` |
