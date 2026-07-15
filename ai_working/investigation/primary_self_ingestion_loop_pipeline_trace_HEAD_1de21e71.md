# PRIMARY deep-dive — self-ingestion feedback loop behind the recurring `overseer-obs:` signature

- **Investigator role:** PRIMARY
- **HEAD:** `1de21e71`
- **Question:** why does an `overseer-obs:...` signature recur ("seen 2×") in cognitive memory, with the reported blob mixing nested `overseer-obs:goal:blocked:...` fragments and bare `goal:blocked:...` / `workstream-gap` / `resource:engineer_spawn` fragments?
- **Focus (as assigned):** trace the full pipeline `signal → classify_signal → observation_signature (mod.rs:1068-1073) → write-back gate (mod.rs:292-299, guardrails.rs:291-333) → recall → Recur`.

## Verdict

**The Overseer re-ingests its own observation write-backs.** There is no
self-provenance filter on the episodic recall path, so an episode the Overseer
itself wrote (`source_label = "overseer"`, content carrying `[sig:overseer-obs:…]`)
is recalled, its `overseer-obs:…` marker is lifted as a `failure_signature`,
counted, and — once **two** such episodes share a signature
(`RECURRING_SIGNATURE_THRESHOLD = 2`, i.e. the reported **"seen 2×"**) — re-raised
as a `Signal::RecurringSignature`. That signal's `dedup_key` keeps the
`overseer-obs:` prefix (sanitize does not strip it), and `observation_signature`
unconditionally **re-wraps** it with another `overseer-obs:` prefix on the next
write-back. The result is the exact reported anomaly: an outer `overseer-obs:`
signature whose joined keys already contain `overseer-obs:goal:blocked:…`
fragments. Each generation mutates the signature, so the 15-minute
`write_back_gate` never suppresses it — the loop is unbounded.

This is a **closed feedback loop / recurrence defect**, not a benign dedup key.

## The pipeline, link by link (all citations at HEAD `1de21e71`)

### 1. Write-back embeds a recoverable self-marker (the loop's seed)
`wiring.rs:1076-1091` — `record_observation` stores the episode as
`content = "{content} [sig:{signature}]"` with a fixed `source_label = "overseer"`
(`OVERSEER_SOURCE_LABEL`, `wiring.rs:952`). The signature it embeds is the
`overseer-obs:…` string from `observation_signature`.

### 2. Recall lifts the Overseer's own marker as a `failure_signature` — NO provenance filter
`wiring.rs:1013-1031` — `recall_episodic` maps every recalled episode to
`RecalledEpisode { failure_signature: parse_failure_signature(&e.content), … }`.
- `parse_failure_signature` (`wiring.rs:976-986`) extracts whatever sits in
  `[sig:…]` — it does **not** care who wrote it.
- Critically, this method **does not exclude `source_label == "overseer"`**.
  The Overseer's own episodes come straight back in.

### 3. `signals_from` counts the self-signature and fires at 2 → **"seen 2×"**
`signal.rs:455-470` — episodes are bucketed by `failure_signature`; any bucket with
`occurrences >= RECURRING_SIGNATURE_THRESHOLD` (`= 2`, `signal.rs:359-362`) emits
`Signal::RecurringSignature { signature: "overseer-obs:…", occurrences }`.
The threshold value **is** the reported "2×".

### 4. `classify_signal` keeps the `overseer-obs:` prefix in the dedup_key
`mod.rs:1353-1360` — for `Signal::RecurringSignature` the dedup_key is
`sanitize_recalled(signature)`. `sanitize_recalled` (`capabilities.rs:468-482`)
only strips control chars and caps length — **it does not strip or reject the
`overseer-obs:` self-provenance prefix**. So the problem's `dedup_key` is literally
`overseer-obs:goal:blocked:…`.

### 5. Orient folds it in as a first-class problem
`mod.rs:1210-1231` — the recall-derived problem either merges into a same-key
problem (raising priority, `:1217-1219`) or is pushed as a new `Problem` whose
`dedup_key` is the `overseer-obs:…` string. It now sits alongside the tick's
**fresh** bare-key problems (`goal:blocked:…`, `workstream-gap:…`,
`resource:engineer_spawn`).

### 6. `observation_signature` RE-WRAPS — produces the nested prefix
`mod.rs:1068-1073` — `format!("overseer-obs:{}", keys.join("|"))` over the sorted,
deduped `dedup_key`s. Because one of those keys is already `overseer-obs:goal:blocked:…`,
the emitted signature is `overseer-obs:( … overseer-obs:goal:blocked:… | goal:blocked:… … )`
— **the exact structure of the reported blob**: an outer `overseer-obs:` wrapping a
mix of nested `overseer-obs:goal:blocked:…` fragments and fresh bare
`goal:blocked:…` / `workstream-gap` / `resource:engineer_spawn` keys.

### 7. The write-back gate does NOT break the loop
`mod.rs:534-563` + `guardrails.rs:291-343` — `write_back_gate` (`WhisperGate::new(900,5)`,
`mod.rs:299`) only suppresses the **byte-identical** signature within its 900 s window
(`peek`, `guardrails.rs:312-323`). But every loop generation **mutates** the signature
(the nested prefix grows and the aggregated fresh-key set churns tick to tick), so each
generation is a *new* gate key → `Deliver` → persisted → recallable again.
The gate throttles repeats of one static signature; it cannot stop a signature that
changes shape every generation.

### 8. Recur
The freshly-persisted, deeper-nested `overseer-obs:` episode is recalled next pass
(step 2), re-counted (step 3), re-raised (step 4)… The recurrence counter inflates and
the signature accretes prefixes without bound. This is the "Recur" edge.

## Why the reported blob looks the way it does
- **Repeated identical key-groups** across the blob = the same standing set of blocked
  goals re-observed across multiple windows/generations (each write-back re-aggregates the
  live board).
- **Nested `overseer-obs:goal:blocked:…` fragments** = step 6 re-wrapping a recall-derived
  dedup_key that already carried the prefix (steps 2-4). This is the fingerprint of
  self-ingestion; a first-generation, self-clean signature could never contain
  `overseer-obs:` *inside* the joined keys.
- **Bare `workstream-gap` / `resource:engineer_spawn` keys** = fresh, non-recall problems
  co-aggregated into the same observation on the same tick (`signal.rs:393-397, 472-`,
  gap path `mod.rs:884-948`).

## Root cause (single sentence)
The episodic recall path (`recall_episodic`, `wiring.rs:1013-1031`) has **no
self-authorship exclusion**, and neither `sanitize_recalled` (`capabilities.rs:468-482`)
nor `observation_signature` (`mod.rs:1068-1073`) treats an already-`overseer-obs:`-prefixed
key specially — so the Overseer recalls, re-classifies, and re-wraps its own write-backs
into an ever-nesting recurring signature.

## Fix options (smallest → most complete; for the remediation rung, not applied here)
1. **Exclude own provenance on recall** *(preferred, root cause)* — in `recall_episodic`,
   drop episodes whose `source_label == OVERSEER_SOURCE_LABEL` (or whose parsed
   `failure_signature` starts with `overseer-obs:`) before they reach the
   `failure_signature` count. The write-back stays first-class in the graph for *other*
   readers, but the Overseer stops counting itself. Requires plumbing `source_label`
   through `store_episode`/recall (it is written at `wiring.rs:1088` but not returned by
   `recall_episodes_ranked`); the `overseer-obs:` prefix check needs no plumbing and is a
   safe belt-and-suspenders guard.
2. **Reject self-prefixed recurring signals at the admission boundary** — in
   `classify_signal`/`signals_from`, skip any `failure_signature` beginning with
   `overseer-obs:` so an observation signature can never become a `RecurringSignature`.
3. **Idempotent wrapping** — make `observation_signature` refuse to double-wrap
   (strip a leading `overseer-obs:` from any incoming key before join). This stops the
   *nesting* but not the recurrence-count inflation, so it is a mitigation, not a cure —
   pair with (1) or (2).

Recommendation: **(1) + (2)** — cut the loop at recall *and* at the signal-admission
boundary; add (3) as cheap defence-in-depth.

## Confidence
**High.** Every edge of the loop is a direct code citation at HEAD `1de21e71`; the
nested-prefix structure of the reported blob is reproducible only by the self-ingestion
path (steps 2→4→6), and the "seen 2×" equals `RECURRING_SIGNATURE_THRESHOLD` exactly.
