# Tertiary Investigation — Observe→Dedup→Escalation Pipeline & the Cognitive-Memory vs Stewardship-Store Boundary

**Role:** Tertiary investigator (architect). **Date:** 2026-07-15.
**Focus:** End-to-end `observe → dedup → escalation` pipeline diagram, and a
definitive answer to *"is 'cognitive memory' the same store as the stewardship
dedup/issue store?"* Complements — does not restate — `tertiary_architecture_design.md`
and `tertiary_architecture_VALIDATION_HEAD.md`, which cover the remediation-rung
and WHY-reasoner-wiring fixes. This artifact isolates the **store boundary** and
the **pipeline shape**, which the prior tertiary docs assumed but never drew.

---

## 0. Bottom line (the boundary answer)

**No — cognitive memory and the stewardship store are two physically distinct
stores, with different keys, different idempotency contracts, and different
purposes. The `overseer-obs:…` signature under investigation lives ONLY in
cognitive memory; it is never written to the stewardship (GitHub-issue) store.**

| Axis | **Cognitive memory** | **Stewardship store** |
|---|---|---|
| Physical backend | `CognitiveClientMemoryStore` — dual-write: Python cognitive client **+** local `FileBackedMemoryStore` (`memory_store_adapter/store.rs:28-46`) | GitHub Issues via `GhClient`/`RealGhClient` (`stewardship/gh_client.rs`) |
| Access seam | `CognitiveMemoryOps` → `store_episode` / `store_fact` / `search_facts` | `StewardshipIssueFiler` → `process_orchestrator_run` (`overseer/observer.rs:53-68`) |
| Dedup key | `overseer-obs:{keys.join("\|")}` (episodes, `mod.rs:1068-1073`); `root_cause_signature = "{dedup_key}::{label}"` (facts, `root_cause.rs:53-55`) | `failure_signature` = `sha256(failure_kind\|\|norm(error))[..8]` (`stewardship/dedup.rs:63-75`) |
| Marker embedded | `[sig:{signature}]` in episode content (`wiring.rs:1084`) | `stewardship-signature: {sig}` in issue body (`dedup.rs:79`) |
| Idempotency | **NOT idempotent across windows** — episodes append per 900 s `WhisperGate` window; root-cause facts are **append-only** `store_fact` (no upsert) | **Idempotent** — search-before-file → `FiledNew` once, `MatchedExisting` thereafter (`stewardship/mod.rs:72-...`) |
| What it holds | (a) observation episodes, (b) root-cause `PriorOccurrence` facts = the **recurrence counter** | durable operator findings = **the deduped issue** |
| Who reads it for escalation | `recall_occurrences` → `RootCause.recurrence` → `RECURRENCE_ESCALATION_THRESHOLD` (3) | nobody in the overseer loop; issues are the terminal output |

**Consequence for the `×2`:** the `overseer-obs:…|goal:blocked:…|workstream-gap`
composite is an **episode content string in the cognitive-memory graph**, and the
"seen 2×" count is a *cross-window recurrence tally in cognitive memory* — not a
duplicated GitHub issue and not a stewardship-store artifact. The two stores share
the word "signature" and the search-before-write idiom, but they are not the same
store and must not be reasoned about as one.

---

## 1. End-to-end pipeline diagram (Observe → Orient → Dedup → Root-Cause → Escalate)

```
                         O V E R S E E R   T I C K   (one OODA cycle)
 ┌──────────────────────────────────────────────────────────────────────────────────────┐
 │                                                                                        │
 │  OBSERVE (sensor.rs)                                                                    │
 │   ├─ detect_workstream_gaps ─────────────► WorkstreamGap signal                         │
 │   ├─ goal health / no-progress ──────────► GoalProgress::Blocked(reason)                │
 │   └─ recall_episodic (cognitive mem) ────► RecurringSignature (occurrences ≥ 2)         │
 │                                                    (signal.rs:362,463; thr = 2)         │
 │                                                                                        │
 │  ORIENT — map signals → Problem{ kind, dedup_key, summary }  (mod.rs:1340-1373)         │
 │   ├─ WorkstreamGap        → WorkstreamCoverage, dedup_key = "workstream-gap"            │
 │   ├─ Blocked(reason)      → GoalHygiene,        dedup_key = "goal:blocked:{slug}-{h}"   │
 │   └─ RecurringSignature   → ProcessHealth,      dedup_key = sanitize_recalled(sig)      │
 │                             ─────────────────────────────────► *nests* an "overseer-obs:"│
 │                                                                key back into a Problem   │
 │                                                                                        │
 │  ┌──────────────────────────  ROOT-CAUSE (root_cause.rs)  ──────────────────────────┐  │
 │  │ recall = recall_occurrences(dedup_key)   ◄────────────── COGNITIVE MEMORY (read)  │  │
 │  │ recurrence = recall.filter(cause==primary).len()                                  │  │
 │  │ analyze() → RootCause{ primary, recurrence, source }                              │  │
 │  └───────────────────────────────────────────────────────────────────────────────────┘│
 │                                                                                        │
 │  DECIDE  (mod.rs Decide table)                                                          │
 │   ├─ GoalHygiene(blocked): recurrence ≥ 3 ? EscalateBlockedGoal : UnblockGoal(+WHY)     │
 │   │                          (root_cause.rs:33 RECURRENCE_ESCALATION_THRESHOLD)         │
 │   ├─ ProcessHealth/CrossCutting/StepFailure → LaunchRecipe ──► launch.rs (converges)    │
 │   └─ WorkstreamCoverage → FlagWorkstreamGaps ── notify-only ── NO launch edge  ✗        │
 │                                                                                        │
 │  ACT                                                                                    │
 │   ├─ launch / merge / verify (closing actions)                                          │
 │   ├─ act_flag_workstream_gaps: peek/commit gap_gate (900 s window) + notify only        │
 │   ├─ record_occurrence(entry, outcome) ─────────────────────► COGNITIVE MEMORY (write,  │
 │   │                          store_fact, APPEND-ONLY, no upsert)   root-cause facts     │
 │   └─ file deduped issue (defects only) ─────────────────────► STEWARDSHIP STORE (write, │
 │                              process_orchestrator_run, IDEMPOTENT)  GitHub issue        │
 │                                                                                        │
 │  WRITE-BACK  write_back_observation (mod.rs:534-563)                                     │
 │   signature = observation_signature(problems) = "overseer-obs:{sorted keys | joined}"   │
 │   write_back_gate.peek(signature, 900 s) == Deliver ?                                    │
 │        record_observation(episode) ─────────────────────────► COGNITIVE MEMORY (write,  │
 │                              store_episode, per-window)   observation episode           │
 │        └─ episode content embeds the WHOLE composite incl. any recalled overseer-obs:   │
 │           and workstream-gap keys  → THIS is the "×2" record under investigation        │
 └──────────────────────────────────────────────────────────────────────────────────────┘

  Legend:  ─► data/control flow    COGNITIVE MEMORY = CognitiveClientMemoryStore (Python client + local file)
           STEWARDSHIP STORE = GitHub issues via GhClient
```

### 1.1 The self-nesting feedback edge (why the composite grows an `overseer-obs:` prefix)

There is exactly **one** cycle in the diagram: `recall_episodic → RecurringSignature
→ ProcessHealth Problem (dedup_key = sanitize_recalled(sig)) → write_back_observation
→ observation episode (content re-embeds that overseer-obs: key) → recall_episodic`.
Because ORIENT turns a *recalled* signature back into a *Problem*, and write-back
persists *all* problems (including that recall-derived one), the `overseer-obs:` key
is folded into the **next** observation signature — producing the literal
`overseer-obs:…|overseer-obs:…` shape. This is a **cognitive-memory-internal**
feedback loop; the stewardship store is not on this cycle at all.

---

## 2. Component boundaries & responsibilities

Three components, three responsibilities, two stores:

1. **`overseer/*` (Observe→Orient→Decide→Act):** the control loop. Owns
   `observation_signature`, the root-cause recurrence read, the Decide table, and
   the write paths. It is the *only* writer to cognitive memory in this flow, and
   it delegates issue filing to stewardship.
2. **`cognitive_memory` / `memory_store_adapter` (the recurrence store):** the
   `CognitiveMemoryOps` seam behind `CognitiveClientMemoryStore`. Dual-writes to a
   Python cognitive client and a local `FileBackedMemoryStore` (`store.rs:28`), so
   the runtime always functions even if the Python client is down. Holds both
   observation episodes and root-cause facts. **This is "cognitive memory."**
3. **`stewardship/*` (the issue store):** `process_orchestrator_run` — validate →
   route module→repo → `failure_signature` → search-before-file. Idempotent by
   construction. Its output (a GitHub issue) is *terminal*: `mod.rs` docstring line
   16 and `observer.rs:20` both state filed issues are **not fed back into the goal
   board**. **This is the stewardship store — a different store.**

**Boundary integrity:** the two stores never share a key namespace. Cognitive
memory keys are human-readable slugs (`overseer-obs:…`, `…::label`); stewardship
keys are opaque `sha256[..8]` hex. A record in one cannot be looked up in the
other. The only conceptual overlap is the *pattern* (search-before-write); the
implementations, backends, and idempotency guarantees differ.

---

## 3. Interface analysis (the three write contracts, ranked by correctness)

| Write path | Interface | Contract | Verdict |
|---|---|---|---|
| Issue filing | `IssueFiler::file` → `process_orchestrator_run` | search-before-file; `FiledNew`→`MatchedExisting` | ✅ **Correct dedup.** Truly idempotent; stable synthetic `run_id = overseer-{sig}` (`observer.rs:79`) keeps identity stable across re-observation. |
| Observation write-back | `MemoryRecall::record_observation` → `store_episode` | `WhisperGate(900 s)` peek/commit dedup | ⚠️ **Window-scoped only.** Idempotent *within* 900 s; **re-persists across windows and across daemon restarts** (`write_back_gate` is an in-process `HashMap`, `guardrails.rs:294`). This is the mechanism that lets the `×2` accrue. |
| Root-cause occurrence | `store_fact` (append-only) | none — every call appends a new fact | ❌ **No dedup.** `recurrence = recall.len()` counts distinct facts, so repeated ACT on the same cause **ratchets** the count. Escalation therefore depends on *how many times ACT ran*, not on *how many distinct cycles observed the cause*. |

The asymmetry is the core architectural smell: **the store that is terminal
(stewardship issues) is idempotent, while the store that drives a threshold
decision (cognitive-memory recurrence) is not.** A counter that gates escalation
should have the strongest idempotency contract; here it has the weakest.

---

## 4. Structural concerns (architecture-level, store-boundary-specific)

- **C1 — Counter store lacks the idempotency its consumer assumes.**
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) is compared against a
  count derived from an append-only fact store. The threshold semantics ("seen 3
  *distinct* times") are silently violated by the append-per-ACT write. Fix belongs
  at the store contract (count-in-content upsert), not at the threshold.

- **C2 — The escalation counter (cognitive memory) and the operator-visible
  recurrence (episode `×2`) are two different numbers in the *same* store.** The
  episode lane (`observation_signature`, window-gated) and the fact lane
  (`root_cause_signature`, append-only) are decoupled: the `×2` an operator sees on
  the episode says nothing about whether the escalation counter reached 3. A reader
  of cognitive memory cannot infer escalation state from the visible episode count.

- **C3 — `WorkstreamCoverage` has no store at all for cross-window recurrence.**
  Its only memory is the 900 s `gap_gate` window (`mod.rs:901-933`); nothing writes
  a gap to cognitive memory. So `workstream-gap|workstream-gap` is a *stateless*
  re-observation, structurally unable to ever cross a recurrence threshold — a
  "2× dead zone" by construction (confirmed in `DISCOVERIES.md §3`).

- **C4 — Self-nesting feedback (D1) is a cognitive-memory hygiene defect, cheaply
  fixable at the write-back boundary.** Filtering recall-derived problems
  (`dedup_key.starts_with("overseer-obs:")`) out of `observation_signature` removes
  the nested shape without touching either threshold. It is orthogonal to the
  store-boundary and to stewardship entirely.

---

## 5. Recommendations for understanding (not implementation)

1. **Treat "cognitive memory" and "stewardship store" as two separate subsystems
   whenever reading this signature.** The `overseer-obs:` string is a
   cognitive-memory episode key; the stewardship store never sees it. Any fix aimed
   at "the dedup store" must first name *which* store.
2. **Read the `×2` as a cognitive-memory cross-window recurrence, not a duplicate
   write.** It is a real loop count (window-gated episode + append-only fact),
   consistent with the secondary finding that `×2` = two genuine cycles, not one
   double-persisted cycle. The stewardship store's true idempotency is a red
   herring for this symptom.
3. **The load-bearing architectural fix is to give the escalation counter the
   idempotency the terminal store already has** (count-in-content upsert keyed on
   `root_cause_signature`), gated behind closing the WHY double-gate so the counter
   can accrue at all — as detailed in `tertiary_architecture_VALIDATION_HEAD.md §2`.
   This artifact adds *why*: the counter and the terminal store are different stores
   with inverted idempotency guarantees; align them.
4. **Add a cross-window recurrence store for `workstream-gap`** (mirror the
   root-cause `PriorOccurrence` in cognitive memory) so C3's dead zone gains a
   threshold to cross. This is the store-boundary framing of the §2.3 remediation
   rung.

---

## 6. Cross-check vs prior artifacts

| Prior claim | This artifact | Status |
|---|---|---|
| `×2` is a faithful cross-window recurrence, not a storage artifact (`tertiary…VALIDATION_HEAD §0`) | Confirmed via store contracts: episode window-gate + append-only fact = accrual, not duplication | ✅ consistent |
| Stewardship issue filing is idempotent (`observer.rs`, `secondary…findings`) | Confirmed and contrasted: it is the *only* idempotent store of the three write paths | ✅ consistent |
| `workstream-gap` has no cross-window ledger (`DISCOVERIES §3`) | Reframed as a **missing store** (C3): gap never enters cognitive memory | ✅ consistent, new framing |
| Self-nesting `overseer-obs:` (D1) (`tertiary…VALIDATION_HEAD §2.4`) | Located precisely as the single feedback edge in the pipeline (§1.1) | ✅ consistent, adds the diagram |

No prior conclusion is contradicted or made stale. This artifact's net-new
contributions are: (a) the single end-to-end pipeline diagram, (b) the explicit
two-store boundary table (§0), and (c) the inverted-idempotency framing (§3–§4):
the terminal store is idempotent while the threshold-driving store is not.

---

## 7. Evidence ledger

| Claim | Source |
|---|---|
| Observation write-back → `store_episode` on `CognitiveMemoryOps` | `overseer/wiring.rs:1076-1091`; `mod.rs:534-563` |
| `observation_signature = "overseer-obs:{sorted keys \| joined}"` | `overseer/mod.rs:1068-1073` |
| Root-cause occurrence → append-only `store_fact` | `overseer/mod.rs:1004-1042` |
| `recurrence = recall_occurrences(...).len()`; threshold = 3 | `overseer/mod.rs:972-997`; `root_cause.rs:33` |
| Cognitive store = Python client + local file dual-write | `memory_store_adapter/store.rs:28-46,236` |
| Stewardship pipeline (validate→route→signature→search→file) | `stewardship/mod.rs:1-16,66-...` |
| `failure_signature = sha256(kind\|\|norm(err))[..8]`; `stewardship-signature:` marker | `stewardship/dedup.rs:63-81` |
| Idempotent issue outcome (`FiledNew`/`MatchedExisting`) | `overseer/observer.rs:53-68`; stable `run_id` at `:79` |
| Filed issues NOT fed back into goal board (terminal) | `stewardship/mod.rs:16`; `overseer/observer.rs:20` |
| `WorkstreamCoverage` → notify-only, no launch edge; gap_gate 900 s only | `overseer/mod.rs:1534-1543,884-948,304` |
| `RecurringSignature` emits at occurrences ≥ 2 | `overseer/signal.rs:362,463` |
| Stewardship issue store handle assembled separately from memory | `overseer/wiring.rs:1137-1140` |
