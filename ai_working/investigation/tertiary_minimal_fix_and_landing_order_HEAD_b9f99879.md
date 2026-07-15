# Tertiary Investigation (9th wave) — Minimal Contained Fix Design & Dependency-Correct Landing Order

**Role:** TERTIARY investigator (architect). **Mandate:** minimal, contained fix
design with a dependency-correct landing order — or a justified no-fix. **Advisory
only; no code changed.**
**HEAD:** `b9f99879` (verified). **Baseline for drift:** `6b2bf5e1` (last code
change to the pipeline, `fix(stewardship): stop recursive issue flood safely #4063`).
**Method:** independent line-by-line re-read of every load-bearing seam (did **not**
trust prior docs' citations); reconstructed the composite signature by hand; verified
the fix seams have the data they need in-hand.

---

## 0. Verdict — CONFIRM + EXTEND (no-fix is NOT justified; ship the contained fix)

The prior eight waves re-ground **exactly** at HEAD with **zero source drift**. My
independent re-read confirms the three-defect geometry and the central conclusion:

> The `2×` signature is a **faithful cross-window fingerprint of a static, unresolved
> problem set — a REAL re-observation loop, NOT a dedup / storage / replay /
> hash-collision artifact.** One genuine, bounded, self-referential write-back defect
> (D1) is **open at HEAD**; two convergence holes (D2/D3) keep the problem set static.

A **no-fix verdict is not defensible**: D1 is a live self-feeding loop whose composite
key nests one level deeper every cycle (`overseer-obs:…|overseer-obs:…`), and D2/D3
guarantee the observed `2×` set never trends to zero. This wave **extends** the record
with (a) a re-verified fix-seam data-availability proof (§2) and (b) a tightened,
dependency-correct landing order (§4).

---

## 1. Drift & citation re-verification (independent, at HEAD `b9f99879`)

`git diff --stat 6b2bf5e1..HEAD -- src/overseer src/stewardship src/ooda_loop` → **empty.**
Every commit since `6b2bf5e1` is `docs(investigation)`. All lines below were re-read
directly (not copied from docs):

| Load-bearing claim | Re-read @ HEAD | Status |
|---|---|:--:|
| Composite emitter `format!("overseer-obs:{}", keys.join("\|"))` after `sort_unstable`+`dedup` | `mod.rs:1068-1073` | ✅ exact |
| Write-back gate: `peek`→`Deliver`→`record_observation`→`commit`; empty-set guard | `mod.rs:543,546,548-556` | ✅ exact |
| `RecurringSignature` → standalone High `ProcessHealth`, key `sanitize_recalled(signature)` | `mod.rs:1353-1363` | ✅ exact (self-nesting live) |
| `WorkstreamGap` → `WorkstreamCoverage`, bare key `"workstream-gap"` | `mod.rs:1368-1373` | ✅ exact |
| `WorkstreamCoverage` Decide arm → **notify-only** `FlagWorkstreamGaps` (no `LaunchRecipe`) | `mod.rs:1534-1543` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A, fires at `:463`) | `signal.rs:362` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B) | `root_cause.rs:33` | ✅ exact |
| Write-back stamps fixed provenance `OVERSEER_SOURCE_LABEL="overseer"` | `wiring.rs:952,1088` | ✅ exact |
| Recall parses `failure_signature` from **every** episode's `content`; **no source-label exclusion** | `wiring.rs:1024-1029` (`:1025`) | ✅ exact — **D1 loop OPEN** |
| Proposed D1 self-exclusion filter | **absent** at `mod.rs:534-563` and `wiring.rs:1024` | ✅ fix unimplemented |

**Drift verdict: ZERO.** Every cited line is byte-identical to the pinned docs.

---

## 2. NEW architectural evidence — the D1 fix needs no new plumbing

The cheapest correct D1 fix is a **self-source exclusion at the recall boundary**, and
this wave proves the required provenance is **already in hand there**:

- `recall_episodes_ranked` returns `Vec<CognitiveEpisode>` (`cognitive_memory/mod.rs:542`).
- `CognitiveEpisode` carries a **public** `source_label: String` field
  (`memory_cognitive.rs:47-53`), populated end-to-end (`library_adapter.rs:559`).
- At the open seam `recall_episodic` (`wiring.rs:1024-1029`) the loop binds `e`, but maps
  **only** `e.content` and `e.node_id` — it **discards `e.source_label`**.

So provenance exists in storage (`"overseer"`), survives the round-trip, and is present
on the exact line that drops it. **No schema change, no new field, no new query** is
required to close D1 — only *consulting* a value already bound. This sharpens the prior
"~4 lines" estimate to a **single-predicate filter** on data in scope.

---

## 3. The three defects (contained scopes, no overlap)

| ID | Defect | Live seam | Blast radius |
|---|---|---|---|
| **D1** | Overseer recalls its **own** `overseer-obs:` write-backs and counts them as recurring → composite key self-nests | `wiring.rs:1024-1029` (read) / `mod.rs:546` (write) | Recall boundary only |
| **D2** | **Cross-lane dead zone**: Lane A fires at 2 (`signal.rs:362`), Lane B escalates at 3 (`root_cause.rs:33`) on a *separate* counter → `2×` can never reach the `3` bar | `signal.rs:362,463` ↔ `root_cause.rs:33` | Escalation coupling |
| **D3** | `WorkstreamCoverage` is the **only** High Decide arm with **no close edge** — notify-only, no `LaunchRecipe`/`FileIssue`/backlog rung | `mod.rs:1534-1543`; `launch.rs` has zero gap refs | Gap remediation |

D1 changes the signature **shape** (stops nesting). D2+D3 change the signature
**persistence** (let the set converge to zero). All three are required to end the `2×`
recurrence; each is independently landable.

---

## 4. Minimal contained fix design + dependency-correct landing order

**Reject the naïve counter trap.** Raising `RECURRING_SIGNATURE_THRESHOLD` 2→3
(`signal.rs:362`) only hides the visible symptom; the problem set is still static and
the two counters still count different things across two lanes. The fix must add the
**missing remediation rung**, not move the bar.

Landing order below is dependency-ordered so each step is independently mergeable,
regression-safe, and does not depend on a later step:

**Step 1 — D2 first (atomic gate+counter, unblocks honest escalation).**
Make Lane A and Lane B share one honest occurrence count via a **count-in-content
upsert** (per RECONCILIATION_LEDGER §2), so a persistently-recurring signature actually
increments the escalation counter instead of sitting at `2×` forever. This must ship
**atomically** (gate and counter together) — a partial change re-creates a
never-escalate trap. *Rationale for ordering first:* until escalation can climb,
neither D1 nor D3 has an observable convergence gauge to prove closure.

**Step 2 — D3 (build the closing rung for gaps).**
Give `WorkstreamCoverage` a real close edge in Decide (`mod.rs:1534-1543`): route to a
`LaunchRecipe`/backlog-file intervention keyed on **`GapItem.signature` (per-gap)**,
**not** the bare `"workstream-gap"` dedup_key (INV-GAP-KEY trap — a family key folds all
gaps into one and erases per-gap identity/convergence). This is the missing remediation
rung for Loop (b). *Depends on nothing earlier; ordered after D2 only so its convergence
is measurable.*

**Step 3 — D1 (self-source exclusion, stops the nesting shape).**
At `recall_episodic` (`wiring.rs:1024-1029`), **skip episodes whose
`e.source_label == OVERSEER_SOURCE_LABEL`** (own write-backs) — a single-predicate filter
on data already in scope (§2). Equivalent alternative: drop `dedup_key`s with the
`"overseer-obs:"` prefix at `write_back_observation` (`mod.rs:546`) before signing.
Prefer the recall-side filter (narrower blast radius, preserves the write-back for
external/audit consumers). *Ordered last:* D1 alone stops the nesting **shape** but not
the `2×` **recurrence** (the set stays static until D2+D3 let it converge), so it is the
lowest-urgency of the three despite being the "true self-loop."

**Step 4 — convergence gauges (verification, not a code defect).**
Add an assertion/telemetry that the `goal:blocked:*` and `workstream-gap` signal counts
**trend toward zero** after D2/D3, proving the rungs close. This is the acceptance test
for the whole fix, distinct from any threshold.

**Out of scope / confirmed non-defects (do NOT "fix"):**
- `resource:engineer_spawn` — fixed literal key (`mod.rs:1270`), one stable member; benign
  membership drift, a read-back of the ladder's own guided spawns. Not a spawn cycle.
- kgpacks-rs issue numbers / Simard identity personas / external-CVE slugs in the goal_ids
  — **payload text**, not causal subjects.
- `stewardship/dedup.rs` `sha256` — orthogonal GitHub-issue namespace; not on the
  signature path.

---

## 5. Answer to the mandate

1. **No-fix justified?** **No.** D1 is a live self-feeding loop; D2/D3 keep the set
   static. A contained, low-risk fix exists.
2. **Minimal contained fix:** three independently-landable changes — D2 atomic
   gate+counter (count-in-content upsert), D3 per-gap closing rung on
   `WorkstreamCoverage`, D1 recall-side self-source filter (data already in scope, §2).
3. **Dependency-correct landing order:** **D2 → D3 → D1 → convergence gauges.**
4. **Explicitly rejected:** raising the `2→3` threshold (naïve counter trap). The honest
   count needs a convergence rung, not a higher bar.
5. **Drift:** ZERO code drift `6b2bf5e1..b9f99879`; the fix remains unimplemented; every
   cited line is byte-identical.
