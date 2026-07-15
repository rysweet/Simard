# Tertiary Investigation — Two-loop (Lane-A / Lane-B) architecture, self-observation isolation, and the minimal landing-order-safe fix

**Role:** TERTIARY investigator (architect focus)
**HEAD:** `1de21e71` (all citations re-grounded against current source — not trusted from prior waves)
**Focus (as assigned):** Diagram the two-loop architecture, verify self-observation isolation
(Lane-A vs. Lane-B *and* Lane-A vs. itself), locate every dedup/idempotency gate, and specify a
**minimal, landing-order-safe** fix for duplicate persistence and/or over-aggregation.
**Empirical grounding:** citations confirmed by direct `view` of each region; the two isolation
tests (`tests_root_cause.rs:490,536`) and the two write-back tests (`tests_memory_recall.rs:797,820`)
exist and are referenced below.

---

## Verdict (one line)

Lane-A and Lane-B are **correctly isolated from each other** (tested), but Lane-A is **not isolated
from itself**: the recall→signal→orient→write-back path is a *closed cycle with no self-provenance
boundary*, and the one gate that could damp it (`write_back_gate`) is defeated because every
generation **mutates** the signature. The architecturally-correct, landing-order-safe fix is a
**single-function write-boundary filter** that drops recall-derived (`overseer-obs:`-keyed)
meta-problems before `observation_signature` is computed. It closes the self-feed and restores the
gate's dedup power without touching the honest counter, without cross-file plumbing, and in any
landing order.

---

## A. The two-loop architecture (diagram)

```
                          ┌───────────────────────────  ONE OODA TICK (run_cycle, mod.rs:~410-489) ──────────────────────────┐
                          │                                                                                                    │
 board / sensors ─▶ Observe ─▶ ObservedState                                                                                  │
                          │        │                                                                                           │
                          │        │ (1) pre-recall signals+problems → RecallKeys        mod.rs:424-426                        │
                          │        ▼                                                                                           │
                          │   recall_pass(keys)  mod.rs:498-515  ── recall_episodic ──▶ caps.memory.recall_episodic           │
                          │        │                                (wiring.rs:1013-1031)                                      │
                          │        ▼                                                                                           │
                          │   ObservedState.recall = MemorySnapshot{ episodes:[RecalledEpisode{failure_signature,…}] }        │
                          │        │                                                                                           │
   ┌──── LANE A (composite / whole-board recurrence) ─────────────────────────────────────────────────────────────────┐      │
   │                     ▼                                                                                              │      │
   │   signals_from(state)  signal.rs:455-470                                                                          │      │
   │     count episodes by failure_signature; if count ≥ RECURRING_SIGNATURE_THRESHOLD(=2, signal.rs:362)             │      │
   │       → Signal::RecurringSignature{ signature:"overseer-obs:…", occurrences }   ◀── "seen 2×" is this count      │      │
   │                     │                                                                                              │      │
   │                     ▼  classify_signal  mod.rs:1353-1363                                                          │      │
   │       ProblemKind::ProcessHealth, Priority::High, dedup_key = sanitize_recalled(signature)                        │      │
   │         (sanitize_recalled, capabilities.rs:468-482, does NOT strip "overseer-obs:")                              │      │
   │                     │                                                                                              │      │
   │                     ▼  orient  mod.rs:1200-1235                                                                    │      │
   │       merge-into-same-key branch (1211-1221) is DEAD for the composite (key never equals a bare goal key)         │      │
   │         → RecurringSignature ALWAYS becomes a STANDALONE ProcessHealth meta-problem                               │      │
   │                     │                                                                                              │      │
   │        ┌────────────┴───────────────┐                                                                             │      │
   │        ▼                            ▼                                                                             │      │
   │   decide → LaunchRecipe        (meta-problem flows into cycle.problems)                                            │      │
   │   (mod.rs:1429-1435, cost-       │                                                                                 │      │
   │    bearing, gated by            │                                                                                 │      │
   │    max_launches_per_cycle=2,    │                                                                                 │      │
   │    mod.rs:283,607-611)          │                                                                                 │      │
   │                                 ▼                                                                                 │      │
   │            write_back_observation(&cycle.problems)   wiring.rs:301 / mod.rs:534-563                                │      │
   │              signature = observation_signature(problems)  mod.rs:1068-1073                                         │      │
   │                = "overseer-obs:" + sort/dedup(all dedup_keys).join("|")                                            │      │
   │              ── GATE: write_back_gate.peek/commit (WhisperGate::new(900,5), mod.rs:299) ──▶ record_observation     │      │
   │                                                        (wiring.rs:1076-1091: store_episode +"[sig:…]")            │      │
   │                                 │                                                                                 │      │
   │                                 └───────────────── writes an episode the NEXT tick recalls ────────────────┐      │      │
   └──────────────────────────────────────────────────────────────────────────────────────────────────────────┼──────┘      │
                          │                                                                                     │             │
   ┌──── LANE B (per-problem root-cause recurrence) ───────────────────────────────────────────┐               │             │
   │   for each problem: recall_occurrences(dedup_key)  mod.rs:972-997  (semantic store_fact)   │               │             │
   │     → root_cause::analyze → why.recurrence; ≥ RECURRENCE_ESCALATION_THRESHOLD(=3,          │               │             │
   │        root_cause.rs:33) → EscalateBlockedGoal (mod.rs decide path)                        │               │             │
   │   record_occurrence(entry)  mod.rs:1004-1043  → store_fact (NOT episodic, NO "[sig:…]")    │               │             │
   └───────────────────────────────────────────────────────────────────────────────────────────┘               │             │
                          │                                                                                     │             │
                          └─────────────────────────────────────────────────────────────────────────────────────────────────┘
                                                                                                                └─ SELF-FEED EDGE
                                                                                                                   (Lane-A → Lane-A,
                                                                                                                    no provenance gate)
```

**Reading the diagram:** Lane-A is a *cross-tick* loop closed through the episodic store
(`store_episode` → next tick's `recall_episodic`). Lane-B is an *intra-tick* enrichment lane closed
through the semantic-fact store (`store_fact` → next tick's `recall_occurrences`). They never share a
counter or a store row type.

---

## B. Every dedup / idempotency gate (inventory with exact location)

| # | Gate | Location | Keyed on | State durability | What it stops | Why it fails here |
|---|------|----------|----------|------------------|---------------|-------------------|
| G1 | `write_back_gate` (`WhisperGate::new(900,5)`) | `mod.rs:299`; internals `guardrails.rs:291-333`; used `mod.rs:546-556` | full `observation_signature` string | **in-memory only** (`HashMap`/`Vec`, `guardrails.rs:294-295`) — **resets on restart** | re-persisting a **byte-identical** signature within 900 s | signature **mutates every generation** (nested prefix grows) → each gen is a *new key* → `Deliver` every time |
| G2 | `orient` in-cycle merge | `mod.rs:1211-1221` | `problem.dedup_key` equality | per-tick (rebuilt each cycle) | duplicate problems in one tick | **dead for composite**: `overseer-obs:g1\|g2` never equals a bare `goal:blocked:g1` |
| G3 | `orient` in-flight dedup | `mod.rs:1207-1209` | `key` vs engineers' `refs` | per-tick | fighting an engineer already on it | irrelevant to the self-loop (no engineer owns a meta-key) |
| G4 | Lane-B `recall_occurrences` exact-match | `mod.rs:983` (`o.signature == dedup_key`) | per-problem `dedup_key` | store-durable (semantic facts) | cross-problem key bleed | works correctly; keeps Lane-B immune to board churn |
| G5 | `blocked_goal_gate` / `gap_gate` / `whisper_gate` | `mod.rs:286,292,304` | respective signatures | in-memory | flooding those act-paths | not on the write-back path; out of scope |

**Key architectural finding:** the *only* idempotency gate on the self-feed edge is **G1**, and G1 is
structurally defeated by signature mutation. G2 (which would otherwise collapse a recalled signature
into an existing problem) is **dead for the composite** because the composite key shape can never equal
a single problem's key. So the loop has **no effective idempotency boundary**.

---

## C. Self-observation isolation: verified

**Lane-A ⟂ Lane-B (isolated — TESTED, PASS).**
- Different stores: Lane-A uses `store_episode` (`wiring.rs:1088`); Lane-B uses `store_fact`
  (`mod.rs:1034`). No episodic `[sig:…]` writer exists outside `record_observation` — confirmed by
  reading both writers; the only `[sig:…]` producer is `wiring.rs:1084`.
- Different counters: Lane-A counts `failure_signature` in `signals_from` (`signal.rs:455-470`,
  floor 2); Lane-B counts `PriorOccurrence` in `root_cause::analyze` (floor 3, `root_cause.rs:33`).
- Asserted by `tests_root_cause.rs:490` (`loud_lane_a_…_does_not_feed_lane_b_recurrence`) and its
  converse `:536` (`lane_b_escalates_without_any_lane_a_signal`). **Isolation A↔B holds.**

**Lane-A ⟂ Lane-A (NOT isolated — the leak, UNTESTED).**
- `recall_episodic` (`wiring.rs:1013-1031`) maps **every** recalled episode to a `RecalledEpisode`
  and lifts `parse_failure_signature` (`wiring.rs:976-986`) with **no `source_label` filter** — even
  though the write path fixes provenance to `OVERSEER_SOURCE_LABEL` (`wiring.rs:952,1088`), the read
  path (`recall_episodes_ranked`) neither returns nor filters provenance. The Overseer therefore
  counts **its own** write-backs.
- `sanitize_recalled` (`capabilities.rs:468-482`) strips only control chars + caps length; it does
  **not** strip the `overseer-obs:` self-prefix. So the recalled signature survives as a `dedup_key`
  and re-enters `observation_signature` (`mod.rs:1069`), which re-wraps → `overseer-obs:overseer-obs:…`
  (the nested tokens in the investigation string).
- **No test asserts A→A isolation.** The two isolation tests cover only A↔B; the write-back tests
  (`tests_memory_recall.rs:797,820`) feed *per-problem* signatures, never a composite
  `overseer-obs:g1|g2|…` back through recall. This is a **test gap on the exact defect edge.**

**"seen 2×" semantics (architectural confirmation):** the count is the number of recalled episodes in
**one** snapshot sharing the identical composite `failure_signature` (`signal.rs:457-459`), thresholded
at `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`). It is **not** a WhisperGate delivery count and
**not** two recall passes. Confirmed by reading the count loop directly.

---

## D. Feedback / starvation risk assessment

1. **Unbounded signature accretion (primary risk).** Each generation nests one more `overseer-obs:`
   and re-aggregates the live board, so G1 never dedups and the episodic store grows a new row every
   window. The signature length is bounded only by the 8192-byte cap in `sanitize_recalled`
   (`capabilities.rs:455,472`) — a *ceiling*, not convergence.
2. **Resource amplification (conditional).** The standalone `ProcessHealth` meta-problem routes to
   `LaunchRecipe` (`mod.rs:1429-1435`), cost-bearing and gated by `max_launches_per_cycle = 2`
   (`mod.rs:283,607-611`). If admitted, it spends launch budget on a self-referential meta-string;
   if perpetually held by the cap, it starves *real* problems of the two launch slots. Either way the
   self-loop degrades the tick's useful throughput. (Whether it is admitted in production is Secondary's
   open Q2 — architecturally, both outcomes are harmful.)
3. **Restart-reset duplicate (secondary, bounded).** G1's in-memory state (`guardrails.rs:294-295`)
   clears on daemon restart, so one identical write-back can re-persist per restart even for a *stable*
   signature. This is **bounded** (≤1 duplicate per stable signature per restart) and is a *distinct,
   lower-severity* issue from the unbounded nesting. It is not the driver of the reported blob.
4. **Non-convergence under board churn (over-aggregation).** Because the composite keys the *whole*
   board, any single membership change mints a new signature, so genuinely-stuck goals can recur
   forever without the composite firing (Secondary F2). The composite is simultaneously *too eager*
   (self-nesting) and *too blind* (misses per-goal recurrence) — Lane-B already covers the latter
   precisely. This argues the composite should be **telemetry, not an actor.**

---

## E. Minimal, landing-order-safe fix (architect recommendation)

### Design constraints for "landing-order-safe"
- **Single function, no cross-file plumbing** (so it cannot half-land or depend on a signature/schema
  change elsewhere).
- **Idempotent and additive**: harmless if the recall-side provenance fix (Primary option 1) also
  lands, in either order.
- **Does not touch the honest counter** (`signal.rs:455-470`) — Secondary's documented trap.
- **Purely local to the write boundary**, the last choke point before persistence.

### The fix — write-boundary self-provenance filter

In `write_back_observation` (`mod.rs:534-563`), drop recall-derived meta-problems (those whose
`dedup_key` begins with the self-prefix `overseer-obs:`) **before** computing the signature:

```rust
// mod.rs, inside write_back_observation, replacing the `let signature = …` line:
let own: Vec<Problem> = problems
    .iter()
    .filter(|p| !p.dedup_key.starts_with("overseer-obs:")) // never fold our own recall back in
    .cloned()
    .collect();
if own.is_empty() {
    return Ok(None); // nothing but our own echoes this tick — write nothing
}
let signature = observation_signature(&own);
let now = now_secs();
// … content built from `own`, gate/peek/commit unchanged …
```

(Build `observation_content` from `own` too, `mod.rs:551`, so the persisted body matches the signature.)

### Why this is the correct architectural cut
- **Kills the nesting at the source:** the composite can never again contain an `overseer-obs:` key,
  so `observation_signature` stops growing → the signature becomes **stable** across ticks for a stable
  board → **G1 (`write_back_gate`) regains its dedup power** (the 900 s window now actually suppresses
  the repeat). One fix repairs both "duplicate persistence" (via re-enabled G1) and "over-aggregation
  self-feed" (via the filter).
- **Cuts the self-feed edge, keeps the honest signal:** Lane-A still fires `RecurringSignature` for
  *genuine* recurring board states (fresh bare keys); it simply stops **recording its own meta-problem**
  as new evidence. The "seen 2×" counter is untouched.
- **Order-independent / belt-and-suspenders with Primary's recall-side fix:** if Primary option 1
  (exclude `source_label == OVERSEER_SOURCE_LABEL` in `recall_episodic`) lands too, this filter is a
  redundant second guard at a different seam — no conflict, no ordering requirement. If only one lands,
  the loop is still cut. This is exactly the defence-in-depth posture the two prior waves recommend.
- **No plumbing:** unlike the recall-side fix (which needs `source_label` returned through
  `recall_episodes_ranked`), this reads a field already present on `Problem` (`dedup_key`) — zero
  schema/interface change, minimal blast radius.

### Deliberately out of scope of the minimal fix (flag, don't bundle)
- **Restart-reset duplicate (D.3):** persisting G1's `last_delivered`, or content-deduping in
  `store_episode`, is a *separate* change with its own store-schema implications. Bounded severity;
  land it independently later. Bundling it would break the "single-function, no-plumbing" property.
- **Demote composite `ProcessHealth → LaunchRecipe` to advisory/telemetry** (Secondary Q3b): a policy
  change to `decide` (`mod.rs:1429`). Complementary and defensible, but it is a *routing* decision, not
  the loop cut; keep it a separate reviewable change so the loop fix can land first and alone.

### Required test to close the gap (ship with the fix)
Add an A→A isolation test (none exists — Section C): drive a cycle that write-backs a composite, recalls
it, fires `RecurringSignature`, and asserts the meta-problem's `overseer-obs:` key is **excluded** from
the next `observation_signature` (i.e. the signature does not nest and G1 suppresses the repeat within
the window). Mirror the harness of `tests_memory_recall.rs:820`
(`write_back_persists_again_for_a_distinct_signature`) but assert **no** distinct nested signature is
produced.

---

## F. Integration points (for the remediation rung)

`mod.rs:299` (G1 ctor) · `mod.rs:534-563` (**edit site**) · `mod.rs:546,551` (signature/content build) ·
`mod.rs:1068-1073` (`observation_signature`) · `mod.rs:1200-1235` (orient; dead merge branch 1211-1221) ·
`mod.rs:1353-1363` (classify RecurringSignature) · `mod.rs:1429-1435` (decide → LaunchRecipe) ·
`signal.rs:362,455-470` (counter — DO NOT TOUCH) · `guardrails.rs:291-333` (G1 internals, in-memory) ·
`wiring.rs:301` (write-back call) · `wiring.rs:952,976-986,1013-1031,1076-1091` (provenance/recall seam) ·
`capabilities.rs:455,468-482` (`sanitize_recalled` — does not strip prefix) · `root_cause.rs:33` (Lane-B floor).

## G. Confidence
**High.** Every architectural edge, gate, and isolation claim is a direct code citation at HEAD
`1de21e71`, independently re-read (not inherited). The A↔B isolation is test-backed; the A→A leak and
its test gap are confirmed by the absence of any composite-signature recall test. The proposed fix is
local, additive, and order-independent by construction.
