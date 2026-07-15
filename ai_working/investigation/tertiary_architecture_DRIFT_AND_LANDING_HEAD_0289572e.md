# Tertiary (Architect) Deep Dive — Drift Check, Fix-Landing Status & Pipeline Recurrence Diagram

**Investigation:** "recurring signature seen 2× in cognitive memory (overseer-obs:…|goal:blocked:…|workstream-gap|resource:engineer_spawn)"
**Role:** TERTIARY / architect
**HEAD verified:** `0289572e` (`git rev-parse HEAD`)
**Method:** every load-bearing claim re-confirmed against current source with file:line citations; no doc-to-doc trust.

---

## 1. Verdict (up front)

- **REAL BUG — expected/benign in mechanism, harmful in effect.** The `2×` recurring
  signature is a **faithful re-observation of a static, unresolved problem set**, folded
  back through the Overseer's *own* observation write-back. It is **not** a hash / storage /
  replay artifact. It is a genuine **self-observation feedback loop + non-convergent
  remediation dead-zone**.
- **All three structural defects (count-in-content idempotency, gap-quarantine, write-back
  self-observation guard) remain LIVE and UNMERGED at HEAD `0289572e`.** Verified by direct
  source read + absence greps below.
- **Prior consolidated findings reconcile with source — zero material drift.** One
  cosmetic drift item (cross-wave D-label divergence) is already reconciled in
  `CONSOLIDATED_FINDINGS.md §12.3`; I re-confirm that reconciliation and normalize the
  labels below.
- **Dependency-safe landing order holds: loop-breaker → idempotent-counter → closing-rung**
  (see §4). The count-in-content counter and its WHY-gate must ship **atomically**.

---

## 2. Drift report — consolidated findings vs. current source @ `0289572e`

`git diff --name-only 85b9398a HEAD -- src/` = **EMPTY** → no `src/` change since the prior
validation waves; all divergence is docs-only under `ai_working/`. Every cited defect is
therefore still live. Independent re-verification of each load-bearing citation:

| Claim (consolidated docs) | Cited loc | Re-read @ HEAD | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`"overseer-obs:"+join("\|")` | `overseer/mod.rs:1068-1073` | ✅ exact | no drift |
| write-back folds the **whole `problems` slice** with **no provenance filter** | `overseer/mod.rs:534-563` (sig at :546) | ✅ exact; `grep 'starts_with("overseer-obs'` in mod.rs = **no match** | no drift; fix absent |
| `record_occurrence` uses non-idempotent append-only `store_fact` | `overseer/mod.rs:1004-1043` (write at :1034) | ✅ exact | no drift; fix absent |
| `StoredOccurrence` carries **no** count / first_seen / last_seen field | `overseer/mod.rs:1180-1185` | ✅ 4 fields: signature, cause_label, action, outcome | no drift; fix absent |
| `recall_occurrences` counts prior facts filtered by `signature == dedup_key` | `overseer/mod.rs:972-997` | ✅ exact; `recurrence = recall.len()` semantics | no drift |
| `WorkstreamCoverage` → notify-only `FlagWorkstreamGaps`; no launch/issue edge | `overseer/tests_gap_scan.rs:853-869`; act path `mod.rs:895-934` | ✅ exact | no drift; fix absent |
| gap path dedups **within-window only** (`gap_gate.peek/commit`), no cross-window ledger | `overseer/mod.rs:901-933` | ✅ exact | no drift; fix absent |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (detect); emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact | no drift |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (escalate) | `overseer/root_cause.rs:33` | ✅ exact | no drift |
| `decide_blocked_goal` falls through to `Intervention::Report` in the 2≤rec<3 band | `overseer/mod.rs:1603-1631` (fallthrough at :1630) | ✅ exact | no drift; dead-zone live |

**Drift conclusion:** the consolidated analysis is **fully consistent with HEAD**. The only
non-cosmetic caveat previously flagged (`RECONCILIATION_LEDGER §2`) — that the naïve
`store_fact_with_caller_key` remedy would collapse `recall.len()` to 1 forever and make
escalation dead code — is **still valid** and is why the corrected remedy is **count-in-content**,
not a bare caller-key upsert. No new drift introduced.

---

## 3. Pipeline recurrence diagram (Observe → aggregate → write-back → recurrence)

```
        ┌──────────────────────────────────────────────────────────────────────┐
        │  OODA tick: run_cycle()                          (overseer/mod.rs)     │
        └──────────────────────────────────────────────────────────────────────┘

 (A) OBSERVE     StatusReader.snapshot() → ObservedState
                 { blocked_goals[], workstream_gaps[], live_engineers, … }

 (B) RECALL      recall_pass(keys)                          (mod.rs:498-516)
                 recall_episodic → episodes carry failure_signature
                 recovered from the "[sig:overseer-obs:…]" marker written at (G)
                                                   ▲
                                                   │  self-referential edge
 (C) SIGNAL      signals_from(observed)            │        (signal.rs:366,463)
                 count recalled episodes by signature (BTreeMap)
                 occurrences >= RECURRING_SIGNATURE_THRESHOLD(=2)  ──► 
                    Signal::RecurringSignature{ signature, occurrences }
                    NOTE: signature can be an "overseer-obs:…" THIS overseer wrote

 (D) ORIENT      orient(signals,in_flight)                  (mod.rs:1200)
                 classify_signal(RecurringSignature) →
                    dedup_key = sanitize_recalled(signature)  ← the "overseer-obs:…" ITSELF
                 blocked goals → dedup_key = "goal:blocked:<slug>-<hash>"
                 workstream gaps → dedup_key = "workstream-gap"
                 spawn-rate     → dedup_key = "resource:engineer_spawn"

 (E) WHY         recall_occurrences(dedup_key)              (mod.rs:456,972-997)
                 recurrence = live facts whose signature == dedup_key   ← Lane B counter

 (F) DECIDE      decide(problem)                            (mod.rs:1603-1631)
                 blocked goal, 2 ≤ recurrence < 3, not perpetual, not needs_review
                    ─► Intervention::Report            ◄── DEAD-ZONE (no close, no escalate)
                 WorkstreamCoverage ─► FlagWorkstreamGaps ◄── NOTIFY-ONLY (no launch/issue edge)
                 recall-derived meta RecurringSignature ─► ProcessHealth/LaunchRecipe
                    task = "recurring signature seen 2× (overseer-obs:…)"  ◄── mis-routed

 (G) WRITE-BACK  write_back_observation(problems)           (mod.rs:534-563)
                 signature = observation_signature(problems)       (mod.rs:1068-1073)
                    = "overseer-obs:" + sorted,deduped dedup_keys joined by "|"
                    ── NO provenance filter: the (D) meta dedup_key is folded back in
                 WhisperGate.peek(signature) → Deliver only if unseen in window
                 record_observation → store_episode(content "… [sig:{signature}]")
                 record_occurrence(entry) → store_fact  (mod.rs:1004-1043)  ◄── Lane A
                    APPEND-ONLY: no signature-keyed upsert, StoredOccurrence has no count
                    └──► every window writes a fresh identical-signature fact
                                                   │
                                                   └──── feeds (B) next tick ────► recurrence
```

**Two independent counter lanes (both true, different storage):**
- **Lane A — observation episodes** (mod.rs:534-563): produces the visible `×2`; append-only
  per WhisperGate window → the `2` is **write cadence across windows**, not two real-world
  recurrences.
- **Lane B — root-cause occurrences** (mod.rs:1004-1043): feeds escalation via
  `recall_occurrences().len()`; also append-only, so the count is likewise cadence-inflated.

**The 2× is real re-observation, not a replay artifact:** within a window the WhisperGate
suppresses duplicates (mod.rs:548-561); across windows an unchanged blocked set legitimately
re-emits. `2` = two temporally distinct Observe passes over the same static blocked state,
mechanically amplified by the append-only lanes and the self-referential (G)→(B) edge.

---

## 4. Fix set — dependency-correctness & unmerged status (validated)

Canonical fixes (the strategy's names in **bold**; cross-wave D-labels reconciled per
`CONSOLIDATED_FINDINGS.md §12.3`):

| Fix (canonical) | Seam / loc | Merged? | Evidence of absence |
|---|---|---|---|
| **Write-back self-observation guard** (loop-breaker): filter recall-derived `overseer-obs:*` / RecurringSignature-only problems before `observation_signature` | `mod.rs:546` boundary | **NO** | `grep 'starts_with("overseer-obs'` in `mod.rs` → no match; `write_back_observation` passes the full slice unfiltered |
| **Count-in-content + WHY-gate** (idempotency guard, **atomic latch**): signature-keyed caller-key upsert whose content holds `occurrence_count`/`first_seen`/`last_seen`; escalation reads that field, not `recall.len()` | `record_occurrence` `mod.rs:1004-1043`; `StoredOccurrence` `mod.rs:1180-1185`; read `mod.rs:972-997` | **NO** | `record_occurrence` still calls plain `store_fact` (append); `StoredOccurrence` has 4 fields, **no count** |
| **Gap-quarantine closing rung** (D3): first-recurrence remediation/escalation for `WorkstreamCoverage`; cross-window ledger keyed on `GapItem.signature` (not bare `"workstream-gap"`); + `decide_blocked_goal` dead-zone rung | gap act `mod.rs:895-934`; decide `mod.rs:1603-1631`; test pins notify-only `tests_gap_scan.rs:853` | **NO** | gap path only `notifier.notify` (`mod.rs:929-930`) + within-window `gap_gate`; `decide_blocked_goal` falls to `Report` at `mod.rs:1630` |

**Dependency-safe landing order (re-validated dependency-correct):**

```
 [1] Write-back self-observation guard  (loop-breaker; no deps, smallest blast radius)
       │  stops NEW distinct nested "overseer-obs:overseer-obs:…" signatures from forming;
       │  stabilizes the signature set so the next fix has a fixed target
       ▼
 [2] Count-in-content + WHY-gate        (ATOMIC latch — must ship together)
       │  only over a meta-free, stable signature set does "one node per signature +
       │  incremented count" describe a fixed target; makes recurrence mean distinct
       │  windows instead of write cadence
       ▼
 [3] Gap-quarantine closing rung        (needs a meaningful count over a meta-free sig)
          fires remediation/escalation at first genuine recurrence instead of Report /
          notify-only; converges the workstream-gap|workstream-gap tail
```

**Why the order is a true dependency chain, not preference:**
- Fix [1] **must precede** [2]: making the store idempotent before breaking the loop cannot
  help — each nesting level is a *different* signature, so an upsert cannot collapse a moving
  target; idempotency without the loop-breaker still bloats.
- Fix [2] **must precede** [3]: the closing rung consumes the occurrence count; escalating on
  a cadence-inflated count (pre-idempotency) would be noisy/incorrect. It must also ship the
  count and its WHY-gate **atomically** — fixing either alone changes nothing observable
  (the latch).
- The naïve `store_fact_with_caller_key` shortcut for [2] is a **trap** (collapses
  `recall.len()`→1 forever → escalation dead code); the corrected remedy is **count-in-content**.
  Re-confirmed against `library_adapter.rs` CallerKey semantics via `RECONCILIATION_LEDGER §2`.

---

## 5. Structural notes

- **`resource:engineer_spawn`** and **`workstream-gap`** are ordinary leaf `dedup_key`s
  (spawn-rate signal; `mod.rs:1371` gap key), aggregate **members** of the composite
  signature — not separate signatures and not meta-observations. They introduce no new
  dedup mechanism; they appear in the blob only because they co-occur in the static problem
  set. `workstream-gap` recurrence is a symptom of the missing closing rung (fix [3]), not a
  candidate for write-back exclusion.
- **Provenance is sound at the store seam:** `source_label = "overseer"` is fixed by the
  adapter and recalled text is `sanitize_recalled`-cleaned at admission — this is a
  **control-flow feedback** issue, **not** a security/injection issue.
- **This is an investigation deliverable — no source change landed.** The three fixes touch
  the write boundary, the occurrence store, and the decide ladder; they warrant the normal
  development workflow, not a drive-by edit.

---

## 6. Reconciliation of prior findings

- All prior `ai_working/investigation/` verdicts on signature provenance, the self-observation
  feedback loop, the two counter lanes, and the recurrence dead-zone are **consistent with
  source at HEAD `0289572e`**. No conclusion is superseded.
- Confirmed **docs-only drift** since `85b9398a` (`git diff --name-only 85b9398a HEAD -- src/`
  = empty). Defects remain live.
- The only reconciliation item is **label divergence** across waves (each wave numbered the
  same three defects D1/D2/D3 differently); `CONSOLIDATED_FINDINGS.md §12.3` already maps them
  and this deliverable uses the fixes' canonical names to avoid the ambiguity.
