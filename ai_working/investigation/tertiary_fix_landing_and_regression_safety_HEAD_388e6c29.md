# Tertiary Investigation — Minimal Fix Landing & Dedup-Idempotency Regression Safety

**Role:** Tertiary investigator (architect). **Date:** 2026-07-15.
**HEAD:** `388e6c29` (two commits past the prior tertiary doc's `dea65df8`; **both
are docs-only** — `git log`: `388e6c29`/`85b9398a` are investigation consolidations).
**Mandate (tertiary focus):** minimal *count-in-content upsert vs naive-counter*
fix, its exact **landing location** (dedup.rs vs observer.rs write-back seam),
and a **no-regression** argument against the existing dedup-idempotency tests.
**INVESTIGATION-ONLY** — this specifies; it does not implement.

Every claim below was re-grounded to source at HEAD (not doc-to-doc).

---

## 0. Baseline (green before any reasoning)

```
cargo test --lib overseer::observer::      → 12 passed
cargo test --lib overseer::tests_gap_scan::→ 21 passed
cargo test --lib overseer::tests_root_cause::→ 19 passed
cargo test --lib overseer::tests_memory_recall::→ 32 passed
```

The idempotency contract the fix must not break is live and green: `observer.rs`
`dedup_signature_ignores_recipe_and_step_differences` (:394),
`issue_filer_is_idempotent_across_cycles_no_network` (:335),
`brief_to_summary_synthesises_stable_run_id_from_signature` (:381), and
`tests_gap_scan.rs` dedup_key (:857).

---

## 1. Two signatures, two lanes — the landing decision hinges on telling them apart

The single most important architect finding: **there are two unrelated
"signatures" in this codebase, and the recurring `overseer-obs:…` token belongs to
exactly one of them.** Landing the fix in the wrong file fixes nothing and risks
regressing a proven-idempotent path.

| | Stewardship / issue-filing lane | Overseer observation / recall lane |
|---|---|---|
| Signature fn | `failure_signature(kind,text)` `dedup.rs:63` | `observation_signature(problems)` `mod.rs:1068` |
| Shape | `sha256[..8]` hex (16 chars) | **`overseer-obs:{sorted∪dedup keys joined "\|"}`** |
| Purpose | Dedup a GitHub issue via `find_existing` (`dedup.rs:78`) | Key the cognitive-memory episode write-back |
| Idempotency | **Proven** (`observer.rs:335,394`) — `FiledNew` once, `MatchedExisting` after | **Broken across windows** (see §2) |
| Recurs as `overseer-obs:…`? | **No** — hex only | **Yes** — this is the signature in the mandate |

**Verdict:** the recurring `overseer-obs:…` signature is produced **only** in
`overseer/mod.rs`. `dedup.rs` is a **red herring** for this defect: it governs
issue-filing dedup, which is already idempotent and independently tested. **No
change should land in `dedup.rs`.** Landing there would be off-seam and would put
the proven `observer.rs:335/394` contract at risk for zero benefit.

---

## 2. What actually recurs — three seams, confirmed at source

The `overseer-obs:…|overseer-obs:…|workstream-gap|workstream-gap|resource:engineer_spawn`
shape is produced by **three independent defects**, all live at HEAD:

- **D1 — self-observation nesting (emission hygiene).** `wiring.rs:301` calls
  `write_back_observation(&cycle.problems)` with **all** problems, including the
  recall-derived `RecurringSignature` one whose `dedup_key = sanitize_recalled(signature)`
  is itself an `overseer-obs:…` string (`mod.rs:1353-1363`). `observation_signature`
  then folds that key back in via `keys.join("|")` (`mod.rs:1069-1072`) → the literal
  `overseer-obs:…|overseer-obs:…` nesting.
- **D2 — append-only occurrence counter (the "×2" driver + escalation ratchet).**
  `record_occurrence` writes with append-only `mem.store_fact(...)` (`mod.rs:1034`,
  **not** `store_fact_with_caller_key`); `recall_occurrences` returns a Vec whose
  `.len()` becomes `RootCause.recurrence` (`mod.rs:972-997`). Two lanes count
  membership, never a stable field. The episode lane's `write_back_gate` is an
  in-process `HashMap` `WhisperGate(900s,5)` (`guardrails.rs:294`, `mod.rs:299`) that
  **forgets across windows/restarts** — so the same signature re-persists and
  `signal.rs:455-470` counts it `≥ RECURRING_SIGNATURE_THRESHOLD (2)` → `×2`.
- **D3 — workstream-gap has no closing edge.** `WorkstreamGap` → fixed key
  `"workstream-gap"` (`mod.rs:1368-1373`), notify-only decide arm, `gap_gate` with no
  cross-window ledger → the `workstream-gap|workstream-gap` tail never converges.

Membership-delta note (drift check): the newer snapshot's `resource:engineer_spawn`
is a genuine new **primary** key (`Signal::EngineerSpawnRate` → `mod.rs:1267-1272`),
not a signature artifact. It confirms the composite is **volatile-by-membership**:
volatile *content* never leaks into the signature string itself — only the *set of
keys* changes. So the `×2` is a faithful cross-window recurrence of one identical
key-set, **not** a non-idempotent counter self-incrementing within a cycle.

---

## 3. Fix shape: count-in-content upsert vs naive counter

The mandate's "naive counter" and "count-in-content upsert" are **not two ways to
do the same thing** — they resolve two *different* sub-defects, and only one of
them is safe.

### 3.1 The naive-counter trap (reject)
A "naive counter" = keep appending a node per occurrence (today's D2) and read
`.len()`, **or** replace it with `store_fact_with_caller_key(root_cause_signature(...))`
*verbatim*. The latter is the seductive wrong answer: caller-key collapses to **one
live fact**, so `recall.len()` sticks at **1** and escalation at
`RECURRENCE_ESCALATION_THRESHOLD (3)` (`root_cause.rs:33`, `mod.rs:1613`) becomes
**unreachable**. Naive append, conversely, is a monotonic **ratchet** that
over-escalates once the Act path runs. Neither reconciles the two goals
*(stop the ratchet)* and *(still cross 3)*.

### 3.2 Count-in-content upsert (accept — the core systemic fix)
Move the count **into the fact content**, keyed by an upsert:
- **Write** (`record_occurrence`, `mod.rs:1034`): replace append-only `store_fact`
  with `store_fact_with_caller_key(root_cause_signature(entry.key, primary), …)`.
  On hit, deserialize `StoredOccurrence`, `occurrence_count += 1`, refresh
  `last_seen`, re-store the same key (supersede). **One live fact per cause; count
  carried inside.** The upsert primitive already exists and is battle-tested
  (`cognitive_memory/mod.rs:354`; `CallerKey`/supersede in
  `library_adapter.rs:870-913`; used by journal, goal_curation, goals).
- **Read** (`recall_occurrences` / `RootCause.recurrence`): read `occurrence_count`
  from the single live fact instead of `.len()`.
- Add a `last_seen`/`distinct_windows` guard mirroring the 900 s gate so a flapping
  daemon cannot inflate the count **within** one window (in-content idempotency).

**Result:** ratchet gone (idempotent per cause) **and** the counter still advances
monotonically to cross 3 — satisfying *dedup-with-count* and *idempotent
escalation* with **one** record contract.

### 3.3 Prerequisite (D2 is a latch)
The counter fix is **inert** unless the WHY double-gate (`ooda_loop/cycle.rs`
Gate A else `Vec::new()`, Gate B else base ladder) is closed so every
`Blocked(reason)` carries a `NoProgressClass` and the counter can accrue.
§3.2 and the gate-close **must ship together** — fixing either alone changes
nothing observable (counter-only: count stays 0 or over-escalates; gate-only:
revives the append ratchet).

---

## 4. Landing location (the tertiary deliverable)

| Fix | Seam / file:line | Type | Independence |
|---|---|---|---|
| **D2 core** count-in-content upsert | `overseer/mod.rs:1034` (`record_occurrence` write) + `mod.rs:972-997` (`recall_occurrences` read) | `store_fact` → `store_fact_with_caller_key` + `StoredOccurrence.occurrence_count/last_seen` | **Coupled** with WHY-gate |
| **D2 gate** close WHY double-gate | `ooda_loop/cycle.rs` (Gate A/B else-branches) | invariant: no WHY-less block | **Coupled** with D2 core |
| **D3** recurrence-aware closing rung | `overseer/mod.rs:901-934` (gap commit) + existing `launch.rs` seam | record `workstream-gap:{sig}` PriorOccurrence; ≥2× → LaunchRecipe | Independent |
| **D1** write-back hygiene filter | `overseer/mod.rs:534-563` (`write_back_observation`), **before** `observation_signature(problems)` | drop problems whose `dedup_key` starts `overseer-obs:` (recall-derived `RecurringSignature`) | Independent, one-liner |

**Explicitly NOT `dedup.rs`.** The stewardship `failure_signature`/`find_existing`
path is off-seam and already idempotent (§1). Touching it is the primary landing
mistake to avoid.

**Cheapest first, highest-value coupled:** D1 (one-line filter, removes the literal
nested shape) and D3 are independently shippable; the D2 pair is the highest-value
but must be atomic.

---

## 5. No-regression argument (against the named idempotency tests)

The fix set is **orthogonal** to every green idempotency test because they exercise
the *other* lane or an *invariant the fix preserves*:

1. **`observer.rs:394` `dedup_signature_ignores_recipe_and_step_differences`** and
   **`:335` `issue_filer_is_idempotent_…`** — exercise `failure_signature` +
   `process_orchestrator_run` (the **issue-filing** lane). D1–D3 touch **none** of
   `dedup.rs`, `brief_to_summary`, or `StewardshipIssueFiler`. → **untouched, still
   green.** This is the load-bearing reason to keep the fix out of `dedup.rs`.
2. **`observer.rs:381` `brief_to_summary…stable_run_id`** — asserts
   `run_id == overseer-{failure_signature}`. Unrelated to `observation_signature`
   or `record_occurrence`. → **untouched.**
3. **`tests_gap_scan.rs:857` dedup_key == "workstream-gap"** — D3 records a **new**
   `PriorOccurrence` keyed `workstream-gap:{signature}` on a **separate** lane; the
   `Problem.dedup_key` classification (`mod.rs:1371`) is unchanged. → **preserved.**
4. **D1 filter safety** — dropping recall-derived `RecurringSignature` problems from
   the *write-back set* does **not** remove them from `orient`'s output, so the
   priority-raising in `orient` (`mod.rs:1217-1219`) and the `RecurringSignature`
   emission (`signal.rs:462-468`) are untouched: the recurrence **signal** still
   works; only the self-referential **persistence** stops.
5. **D2 upsert vs the two idempotency tests it seems closest to** — `record_occurrence`
   has **no** dedicated idempotency test today (that absence is itself part of the
   defect). The behavioural contract to *add* alongside the fix: "N cycles on the
   same cause under a fake `CognitiveMemoryOps` ⇒ exactly one live fact with
   `occurrence_count == N`; recurrence crosses 3 exactly once ⇒ single escalation."
   Model this on `tests_ranked_recall.rs:127-204` (`caller_key_*` supersede tests),
   which already prove the upsert primitive keeps one live record.

**Idempotency direction of change:** every fix moves the system **toward** stronger
idempotency (upsert replaces append; content-count replaces node-multiplicity;
self-observation stops re-entering the graph). None weakens an existing dedup
guarantee.

---

## 6. Verdict

- The correct fix is **count-in-content upsert** (`store_fact_with_caller_key` +
  in-content `occurrence_count`/`last_seen`), **not** a naive counter — the naive
  forms are a ratchet (append) or a dead-count (verbatim caller-key).
- It lands at the **`overseer/mod.rs` recall/write-back seam** (`record_occurrence`
  :1034 + `recall_occurrences` :972-997), **coupled** with closing the WHY
  double-gate in `ooda_loop/cycle.rs`; **never** in `dedup.rs`.
- Independent companions: D1 one-line write-back filter (`mod.rs:534-563`) and D3
  closing rung (`mod.rs:901-934` + `launch.rs`).
- **No existing dedup-idempotency test regresses** — the issue-filing lane
  (`dedup.rs`, `observer.rs:335/394/381`) and the gap dedup key
  (`tests_gap_scan.rs:857`) are on separate seams the fix does not touch; the fix
  only strengthens idempotency where it is currently absent.
- **Drift:** the prior tertiary VALIDATION (`…_HEAD.md`, at `dea65df8`) remains
  valid at `388e6c29`; the newer `resource:engineer_spawn` token is a genuine new
  primary key (`mod.rs:1267`), not a signature artifact, and does not alter the
  conclusion.
