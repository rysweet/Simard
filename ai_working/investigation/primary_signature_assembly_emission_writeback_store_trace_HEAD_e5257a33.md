# Primary Deep-Dive — Signature Assembly → Emission → Write-Back → Memory-Store Path

**Role:** PRIMARY investigator.
**Focus:** Trace the *full* path that produces the recurring
`overseer-obs:…|goal:blocked:…|workstream-gap` signature "seen 2× in cognitive memory".
**HEAD:** `e5257a33`. All citations re-grounded live at this HEAD (`git diff 6e3113bc..HEAD -- '*.rs'`
is empty; every investigation commit is docs-only, so prior `src/overseer/*` line citations hold).

The observed string is the argument to the summary template at
`mod.rs:1361` — `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` —
i.e. it is a **`RecurringSignature.signature`** value that was previously stored by the Overseer's
own write-back and then recalled.

---

## 1. The end-to-end path (five stages, one closed loop)

```
                        ┌───────────────────────── ONE OODA tick ─────────────────────────┐
 Observe ─► signals_from ─► orient ─► [dedup_key stamped] ─► write_back_observation ─► record_observation
   ▲            (signal.rs)   (mod.rs:1200)   (mod.rs:1238+)     (mod.rs:534)            (wiring.rs:1076)
   │                                                                                          │
   │                                                                                    store_episode
   │                                                                              content="… [sig:S]"
   │                                                                                          │
   └──────── recall_episodic ◄── parse_failure_signature ◄──────────── cognitive-memory graph ┘
             (wiring.rs:1013)      (wiring.rs:976)                       (multi-writer)
```

### Stage 1 — Per-problem `dedup_key` assembly  (`classify_signal`, `mod.rs:1238`)
Each `Signal` is mapped to `(kind, priority, dedup_key, summary)`:

| Signal | dedup_key | Site |
|---|---|---|
| `GoalBlocked{goal_id}` | `format!("goal:blocked:{goal_id}")` | `mod.rs:1336` |
| `WorkstreamGap{gaps}` | literal `"workstream-gap"` (count → summary only) | `mod.rs:1371` |
| `EngineerSpawnRate{live}` | literal `"resource:engineer_spawn"` (count → summary only) | `mod.rs:1267` |
| **`RecurringSignature{signature}`** | **`sanitize_recalled(signature)`** — the *recalled string verbatim* | `mod.rs:1353‑1359` |

`goal_id` is a `<slug>-<8hex>` minted at goal creation (e.g. `…-7f5afcca`), which is why the tokens
are stable across cycles.

### Stage 2 — Composite signature assembly  (`observation_signature`, `mod.rs:1068‑1073`)
```rust
let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
keys.sort_unstable();
keys.dedup();                                   // removes ADJACENT exact duplicates only
format!("overseer-obs:{}", keys.join("|"))
```
So the signature is `overseer-obs:` + the sorted/deduped problem `dedup_key`s joined by `|`.

### Stage 3 — Emission / write-back gate  (`write_back_observation`, `mod.rs:534‑563`)
- Only runs when recall is enabled and `problems` is non-empty.
- Builds `ObservationEpisode { content = observation_content(problems), signature }`.
- Gated by `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`): peek → store → **commit only
  after a successful store**. At most one write per 900 s window per **exact** signature; cap 5/window.
- Single call site: `wiring.rs:301`, `write_back_observation(&cycle.problems)` — **all** problems,
  unfiltered.

### Stage 4 — Memory-store adapter  (`record_observation`, `wiring.rs:1076‑1091`)
```rust
let content  = format!("{} [sig:{}]", episode.content, episode.signature);
let metadata = serde_json::json!({ "signature": episode.signature });
let node_id  = self.mem.store_episode(&content, OVERSEER_SOURCE_LABEL, Some(&metadata))?;
```
The signature is embedded **in the episode content** as a `[sig:…]` marker (the read path carries no
typed signature field) and duplicated in validated JSON metadata. Provenance is fixed
(`source_label = "overseer"`, never caller-chosen).

### Stage 5 — Recall & re-derivation (loop closure)
- `recall_pass` → `recall_episodic` (`wiring.rs:1013`) pulls ranked episodes, and
  `parse_failure_signature(&e.content)` (`wiring.rs:976`) extracts the `[sig:…]` back into
  `RecalledEpisode.failure_signature`.
- `signals_from` (`signal.rs:455‑470`) counts recalled episodes by `failure_signature`; when
  `occurrences >= RECURRING_SIGNATURE_THRESHOLD (2)` (`signal.rs:362,463`) it emits
  `Signal::RecurringSignature { signature, occurrences }`.
- Back to Stage 1: that signal's `dedup_key` becomes `sanitize_recalled(signature)`.

**This is a closed feedback loop:** the Overseer stores its own observation signature, later recalls
it, and re-admits it as a problem key — which then feeds the *next* observation signature.

---

## 2. What "seen 2×" actually means (confirms prior verdict)

`2` = `RECURRING_SIGNATURE_THRESHOLD`. A second identical episode reaches the store only when the
900 s gate did **not** suppress it — either (a) a legitimately new window >900 s later, or (b) a daemon
restart (the gate's `last_delivered` map is in-memory/per-process, `guardrails.rs`), which is the most
probable source of *exactly* 2×. The counter is **honest**: it reflects a near-static, unresolved
problem set re-observed across two window-gated passes. This **confirms** the existing
`FINAL_SYNTHESIS` verdict (real cross-window re-observation, not a dedup/storage/replay/collision bug)
and its two non-closing-loop root causes (blocked goals parked without a WHY; `workstream-gap`
notified but never launched/filed).

---

## 3. Load-bearing REFINEMENT — the self-ingestion feedback is real and under-weighted

The prior synthesis lists "nested `overseer-obs:…` fragments" only as a provenance-table row. The
primary trace shows this is a **structural feedback defect**, and it is what produces the *repeated /
nested* `overseer-obs:goal:blocked:…` blocks visible in the reported string:

1. Recall returns episodes whose `failure_signature = S_prev` (a full `overseer-obs:…` string).
2. ≥2 share `S_prev` → `RecurringSignature{S_prev}`.
3. `orient` creates `Problem{ dedup_key = sanitize_recalled(S_prev) }`. **`sanitize_recalled` strips
   control chars and caps at 8192 B (`capabilities.rs:468‑482`) but does NOT strip the
   `overseer-obs:` prefix.**
4. `write_back_observation(&cycle.problems)` includes that problem (no filter, `wiring.rs:301`), so
   `observation_signature` computes `overseer-obs:` + join(`S_prev`, `goal:blocked:X`,
   `workstream-gap`, …) = **`S_new ⊃ S_prev`** — the new signature strictly *contains* the old one.
5. `S_new` is stored, recalled next window, and the cycle repeats → **monotonic nesting**.

Why the observed string shows the *same block repeated* rather than collapsing: `observation_signature`
uses `keys.dedup()`, which removes only **adjacent exact** duplicates after sort. The nested layers
(`overseer-obs:goal:blocked:X`, `overseer-obs:overseer-obs:goal:blocked:X`, …) are **distinct
strings**, so they never collapse; near the 8192‑byte cap, `sanitize_recalled` truncates on a byte
boundary, cutting mid-token and yielding *different* truncated keys — which visually look like the same
repeated block and defeat exact-match dedup.

### Consequences
- **Growth is bounded** (good): the 8192‑byte `sanitize_recalled` cap prevents unbounded blow-up.
  Growth is bounded by size, **not** by any semantic guard.
- **Dedup/recurrence degrade near saturation** (bad): once the signature saturates ~8 KB, truncation
  makes near-saturation variants unstable, so the write-back gate's exact-match dedup and the
  `RecurringSignature` occurrence count operate on drifting keys — recurrence can never *converge* to a
  single stable signature, and each ~8 KB episode bloats the multi-writer graph.
- This is **distinct from, and compounds,** the two non-closing loops: the loops keep the base problem
  set alive; the missing self-ingest guard mutates its signature every window.

### The precedent that proves this is a gap, not a design choice
The Signal **notify** path already carries a deliberate **anti-self-ingest marker** (#2631,
`notify.rs:1002‑1012`) so Simard's inbound Signal processor skips its *own* notifications. The
**memory write-back** path has **no** analogous guard — an asymmetry, not an intentional exemption.

---

## 4. Verification performed

- Re-grounded every cited line live at HEAD `e5257a33` (assembly, gate, adapter, parse-back,
  re-derivation, sanitize).
- Confirmed `sanitize_recalled` does **not** strip `overseer-obs:` (`capabilities.rs:468‑482`) — the
  prefix survives into the next `observation_signature`.
- Confirmed the single write-back call site passes `&cycle.problems` **unfiltered** (`wiring.rs:301`)
  — no exclusion of `overseer-obs:`-prefixed / `RecurringSignature`-derived keys.
- Confirmed `keys.dedup()` collapses only adjacent exact duplicates (`mod.rs:1070`), so nested layers
  persist.
- Relevant green tests confirm the *intended* halves: `write_back_is_deduplicated_within_window`,
  `write_back_persists_again_for_a_distinct_signature`, `recurring_signature_emitted_when_two_episodes
  _share_signature` (`tests_memory_recall.rs`). None asserts the *composite must not contain a prior
  `overseer-obs:` signature* — i.e. the self-ingest case is untested.

---

## 5. Minimal, landing-safe remediation (single guard, mirrors existing precedent)

Exclude recall-derived keys from the Overseer's own observation signature so it cannot ingest itself —
the memory-path analogue of the notify anti-self-ingest marker.

Preferred site — `observation_signature` (`mod.rs:1068`), filter before join:
```rust
let mut keys: Vec<&str> = problems.iter()
    .map(|p| p.dedup_key.as_str())
    .filter(|k| !k.starts_with("overseer-obs:"))   // never re-absorb our own prior signature
    .collect();
```
(Equivalently/additionally, skip `Signal::RecurringSignature`-derived problems when assembling the
write-back set.) This keeps `RecurringSignature` fully live for **orient/priority-raising** (its
intended job, `mod.rs:1217‑1219`) while breaking only the self-referential *storage* feedback. It does
not touch the two-loop root causes — those remain separate remediation rungs (add a WHY-classified
resolution rung for parked blocked goals; give `WorkstreamCoverage` a launch/file edge).

**Regression guard to add:** a test asserting `observation_signature(problems)` never contains a
nested `overseer-obs:` substring even when a `RecurringSignature` problem is present.

---

## JSON summary

```json
{
  "head": "e5257a33",
  "observed_string_origin": "RecurringSignature.signature rendered by mod.rs:1361 summary template",
  "path": [
    "classify_signal dedup_key (mod.rs:1238+)",
    "observation_signature composite (mod.rs:1068-1073)",
    "write_back_observation + WhisperGate(900,5) (mod.rs:534-563)",
    "record_observation store_episode content='… [sig:S]' (wiring.rs:1076-1091)",
    "recall_episodic + parse_failure_signature (wiring.rs:1013,976)",
    "signals_from RecurringSignature @occurrences>=2 (signal.rs:455-470)"
  ],
  "seen_2x_verdict": "honest cross-window re-observation of a near-static unresolved problem set; RECURRING_SIGNATURE_THRESHOLD=2; likely exactly-2 from per-process gate reset on restart",
  "primary_refinement": "self-ingestion feedback is real: RecurringSignature dedup_key = sanitize_recalled(prior overseer-obs signature) is re-absorbed by observation_signature (no prefix strip, no write-back filter), so each window nests another overseer-obs: layer; bounded only by the 8192B sanitize cap, degrading dedup/recurrence near saturation",
  "no_guard_confirmed": {
    "sanitize_recalled_strips_prefix": false,
    "write_back_filters_recall_keys": false,
    "dedup_collapses_nested_layers": false,
    "notify_path_has_anti_self_ingest_marker": true
  },
  "remediation": "filter out overseer-obs:-prefixed / RecurringSignature-derived dedup_keys in observation_signature (mod.rs:1068); add regression test asserting no nested overseer-obs: substring",
  "unchanged_root_causes": [
    "blocked goals parked without WHY classification (resolution ladder double-gated)",
    "workstream-gap notified but never launched/filed (WorkstreamCoverage has no launch edge)"
  ]
}
```
