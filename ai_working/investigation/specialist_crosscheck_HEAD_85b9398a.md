# Specialist Cross-Check — prior artifacts & commits 6e3113bc / dea65df8 vs. live source @ HEAD 85b9398a

**Role:** Specialist (knowledge-archaeologist). **Mandate:** confirm whether the recurring
`overseer-obs:…|goal:blocked:<slug>-<hash>|…|workstream-gap` signature was already
root-caused and whether prior conclusions still hold — extend, do not restart.
**Method:** re-read every load-bearing line in `src/` at current HEAD; did not trust doc citations.

## 0. Verdict

**Already root-caused; conclusions still valid; do NOT re-derive.** Commits `6e3113bc`,
`dea65df8`, and `85b9398a` are **documentation-only** (`git show --stat`), so no source
changed under the analysis. Every load-bearing citation re-verifies **exactly** at HEAD
`85b9398a`. The prior `RECONCILIATION_LEDGER.md` (written at `dea65df8`) still holds one
commit later. The single wrong *remedy* (§6.2b `store_fact_with_caller_key` trap) was
already corrected to a count-in-content upsert in `85b9398a`. No open contradictions remain.

## 1. Independently re-verified at HEAD 85b9398a

| Claim (prior docs) | Loc | Status |
|---|---|---|
| `observation_signature` = `sort_unstable`→`dedup`→`overseer-obs:{join("\|")}` | `mod.rs:1068-1073` | ✅ exact |
| `record_occurrence` persists via **append-only** `store_fact` (caller_key=None) | `mod.rs:1034` | ✅ exact |
| `goal:blocked:{goal_id}` built here; `<slug>-<8hex>` is the upstream goal_id | `mod.rs:1336` | ✅ exact |
| `workstream-gap` constant dedup key (evidence-independent) | `mod.rs:1371` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps`, no launch/file edge | `mod.rs:1534-1543` | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `signal.rs:362,463` | ✅ exact |

## 2. New provenance link the cross-check nails (extends prior docs)

The investigation **question text itself is Simard-emitted**: `signal_to_problem`'s
`Signal::RecurringSignature` arm formats the literal
`"recurring signature seen {occurrences}× in cognitive memory ({signature})"`
(`mod.rs:1353-1362`). The `2×` is produced upstream at `signal.rs:455-469`, which counts
**recalled EPISODES** (`snapshot.episodes[*].failure_signature`, `capabilities.rs:614`) that
share a signature — **not** the root-cause occurrence *facts*. Because the recalled episodes
are the Overseer's own append-only `overseer-obs:` write-backs, recall re-counts them and, at
2 episodes (= `RECURRING_SIGNATURE_THRESHOLD`), re-emits the RecurringSignature. This is the
**self-referential write-back (D1)** the CONSOLIDATED already names — now traced end-to-end:
`write_back_observation` (episode) → recall → `signal.rs:457-463` (count episodes) →
`mod.rs:1353-1362` (emit "seen 2×").

## 3. Two counter lanes stay decoupled — both prior verdicts are consistent

- **Lane A (visible `×2`)**: append-only episodes, counted by `signal.rs`. `2×` = two genuine
  observation events (two 900 s windows or a process restart of the non-durable
  `write_back_gate`), **not** a duplicate-write-of-one-cycle. Matches
  `primary_signature_provenance_and_idempotency.md §2`.
- **Lane B (escalation)**: append-only occurrence facts (`mod.rs:1034`) counted in
  `root_cause::analyze`, gated at `>=3`. `2×` sits below the bar with no gap-remediation rung.
  Matches `CONSOLIDATED_FINDINGS.md` D2 dead-zone.

The primary "2× is expected append behavior" and the consolidated "real re-observation loop
driven by unresolved defects" are **not** in conflict: the episode append is expected; the
problem set stays static because D1/D2/D3 never resolve it.

## 4. Recommendation

Do not restart or re-root-cause. The committed record + HEAD docs are the authoritative
finding. Proceed to implementation in the already-documented dependency order
(D2 gate+counter atomically → D3 closing rung → D1 write-back filter → convergence gauges).
The only residual doc hygiene item — committing the §6.2b correction — was resolved in
`85b9398a`.
