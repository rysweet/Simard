# Primary Deep-Dive: Signature-Assembly Pipeline Trace (Observe → orient → signal_to_problem → write_back)

**Role:** PRIMARY investigator (fresh, independent trace).
**Branch:** `investigation/recurring-blocked-goals-workstream-gaps`
**HEAD:** `ad5e10606e18b162ef0f0d71edad8e38ecdf5b5f` (`ad5e1060`)
**Scope:** trace how the observed recurring signature is *assembled*, end to end, and
determine what the string is and why it recurs. Every citation below was re-opened
and re-verified against live source at this HEAD (not carried from prior waves).

**Investigation string (the thing "seen 2×"):**
`overseer-obs:goal:blocked:…|…|workstream-gap|…|resource:engineer_spawn|…` with
nested `overseer-obs:…|overseer-obs:…` runs interleaved with raw
`goal:blocked:…`, `workstream-gap`, and `resource:engineer_spawn` tokens.

---

## Verdict (independently confirmed at `ad5e1060`)

**The string is not an external failure signature. It is the Overseer's own
observation write-back signature, recalled and re-narrated by the Overseer as if
it were an incident.** The `2×` is `RECURRING_SIGNATURE_THRESHOLD = 2` — "two of
my own persisted episodes carry the same `[sig:…]` marker" — not two real
incidents. This is a **self-referential recall→narrate→re-persist feedback loop**,
confirmed by a single decisive fact:

> The task's investigation string is **verbatim** the summary produced by
> `classify_signal` for a `Signal::RecurringSignature`:
> `format!("recurring signature seen {occurrences}× in cognitive memory ({signature})")`
> — `src/overseer/mod.rs:1360-1362`.

So the string the investigation is chasing is *literally the Overseer describing
its own memory*, with `{signature}` = a prior `overseer-obs:…` write-back key.

---

## 1. The pipeline, function by function (assembly order)

The signature is assembled once per surviving tick along this exact chain. All
line numbers verified open at HEAD `ad5e1060`.

```
run_cycle (mod.rs:384)
  Observe ─▶ blocked_goals + workstream_gaps + step failures        (:393,:401,:414)
  USE/recall ─▶ pre-signals → RecallKeys → recall_pass              (:423-438)
  signals = signals_from(observed)   [now incl. RecurringSignature] (:441)
  problems = orient(signals, in_flight)                             (:447)
        └─ classify_signal(s) stamps each Problem.dedup_key         (:1204 → :1238)
  per-problem WHY enrichment (does not touch dedup_key/signature)   (:455-459)
  returns CycleReport { problems, … }                              (:482)
        │
        ▼  (wiring.rs:301, sole write-back call site)
write_back_observation(cycle.problems)                             (mod.rs:534)
  signature = observation_signature(problems)                       (:546 → :1068)
        "overseer-obs:" + sorted(dedup()) dedup_keys joined by "|"  (:1069-1072)
  write_back_gate.peek(signature)  [900s, per-process, in-mem]      (:548, :299)
  record_observation(episode)                                       (:554)
        └─ store_episode(content + " [sig:<signature>]", "overseer",…) (wiring.rs:1084-1088)
        │
        ▼  (NEXT tick's recall_pass re-reads this very episode — §3)
```

### 1.1 `run_cycle` — `mod.rs:384`
Ordering that matters for the loop:
- `:423-438` **USE/recall (read side).** Builds *pre-recall* signals+problems to
  derive `RecallKeys` (`:424-426`), then `recall_pass` (`:427`) populates
  `observed.recall`. This step recalls the Overseer's own prior write-back
  episodes — see §3 for the missing source filter.
- `:441` `signals_from(&observed)` now includes any recall-derived
  `Signal::RecurringSignature`.
- `:447` `orient(&signals, &in_flight)` folds signals → problems.
- `:455-459` root-cause WHY enrichment; sets `problem.why` only — **does not alter
  `dedup_key`**, so it cannot change the signature.

### 1.2 `orient` — `mod.rs:1200`
- `:1204` `classify_signal(s)` → `(kind, priority, key, summary)`.
- `:1207-1209` dedup against Simard in-flight refs. The `overseer-obs:…` key never
  matches engineer refs, so the recurring-signature problem **survives** into
  write-back.
- `:1211-1220` same-key merge; a `RecurringSignature` co-signal raises the matched
  problem's priority (`:1217-1219`). Standalone it becomes its own High problem
  carrying the recalled key.
- `:1222-1230` pushes `Problem { dedup_key = key, why: None, … }`.

### 1.3 `classify_signal` (= the task's "signal_to_problem") — `mod.rs:1238`
This is where each *constituent token* of the composite is minted:

| Token in the signature | Arm | file:line | Construction |
|---|---|---|---|
| `goal:blocked:<goal_id>` | `Signal::GoalBlocked` | `mod.rs:1336` | `format!("goal:blocked:{goal_id}")` |
| `workstream-gap` (constant) | `Signal::WorkstreamGap` | `mod.rs:1371` | literal `"workstream-gap"` — one key per Observe pass, evidence-independent |
| `resource:engineer_spawn` | `Signal::EngineerSpawnRate` | `mod.rs:1270` | `"resource:engineer_spawn"` constant |
| nested `overseer-obs:…` | `Signal::RecurringSignature` | `mod.rs:1353-1363` | `dedup_key = sanitize_recalled(signature)` — the recalled composite, re-admitted |

The `RecurringSignature` arm (`:1353-1363`) is the load-bearing one: its
**summary** (`:1360-1362`) is the exact task string, and its **dedup_key**
(`:1359`) is the recalled `overseer-obs:…` composite, which then folds back into
the *next* signature (§4, the nesting mechanism).

### 1.4 `observation_signature` — `mod.rs:1068`
```rust
let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
keys.sort_unstable();
keys.dedup();                 // collapses only ADJACENT equal keys
format!("overseer-obs:{}", keys.join("|"))
```
Two consequences visible in the observed string:
- `sort` + `dedup` collapse **adjacent** duplicates only. Distinct-but-similar keys
  (e.g. multiple `goal:blocked:*` slugs, or a recalled `overseer-obs:…` that is not
  byte-identical to a raw key) all survive — hence the long `|`-joined run.
- Because a recalled `overseer-obs:…` key is one of the `dedup_keys`, the new
  signature is `overseer-obs:` + `…overseer-obs:…` = the observed **nesting**.

### 1.5 `write_back_observation` — `mod.rs:534`
- `:538-540` no-op if recall disabled; `:543-544` no-op on a clean tick (empty
  problems) — a clean tick writes nothing.
- `:546` build the composite signature.
- `:548-556` `write_back_gate` (peek → store → commit): a 900 s / 5-per-window gate
  (`:299`). Within-window identical signatures are suppressed (`_ => Ok(None)`,
  `:559-561`); the dedup slot is consumed only after a successful store (`:556`).
- `:554` `record_observation` persists the episode.

### 1.6 `record_observation` — `wiring.rs:1076`
```rust
let content = format!("{} [sig:{}]", episode.content, episode.signature);
let metadata = serde_json::json!({ "signature": episode.signature });
self.mem.store_episode(&content, OVERSEER_SOURCE_LABEL, Some(&metadata))
```
`OVERSEER_SOURCE_LABEL = "overseer"` (`wiring.rs:952`). The composite signature is
embedded twice: as a `[sig:…]` marker in the content **and** as typed metadata.

---

## 2. Where the count "2×" comes from — `signal.rs:455-470`

```rust
if let Some(snapshot) = &state.recall {
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for ep in &snapshot.episodes {
        if let Some(sig) = &ep.failure_signature {          // the [sig:…] marker
            *counts.entry(sig.as_str()).or_insert(0) += 1;
        }
    }
    for (signature, occurrences) in counts {
        if occurrences >= RECURRING_SIGNATURE_THRESHOLD {    // == 2  (signal.rs:362)
            out.push(Signal::RecurringSignature { signature, occurrences });
        }
    }
}
```
`ep.failure_signature` is recovered by `parse_failure_signature` (`wiring.rs:976`),
which extracts the `[sig:…]` marker the Overseer *itself* wrote in §1.6. So the
count is literally "how many of my own persisted episodes carry this signature."
`RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) is the `2×`.

---

## 3. The defect that closes the loop: no source-exclusion at recall — CONFIRMED

The Overseer's recall does **not** filter out its own `"overseer"`-sourced
episodes. `recall_episodic` (`wiring.rs:1013`) calls:

```rust
self.mem.recall_episodes_ranked(&keys.query(), limit, RecallWeightSet::default())
```

and the adapter (`cognitive_memory/mod.rs:542-550`) is a pure keyword search with
**no source parameter and no `OVERSEER_SOURCE_LABEL` exclusion**:

```rust
fn recall_episodes_ranked(&self, query, limit, _weights) -> ... {
    let keywords = query.split_whitespace()...;
    self.search_episodes_by_keywords(&keywords, limit)     // no source filter
}
```

**Result:** episodes written by §1.6 (`source_label = "overseer"`, carrying a
`[sig:overseer-obs:…]` marker) are recalled by §1.1, counted by §2, and re-narrated
by §1.3. The write side stamps `"overseer"` provenance but the read side never
uses it. This is the single concrete emission-hygiene defect (prior waves' **D1**),
independently re-confirmed here.

---

## 4. The nesting mechanism: `sanitize_recalled` does NOT collapse it — CONFIRMED

`sanitize_recalled` (`capabilities.rs:468-482`) only replaces control chars with
spaces and caps length (`RECALLED_TEXT_MAX_LEN`). It does **not** strip an
`overseer-obs:` prefix, dedup segments, or reject a self-sourced signature:

```rust
for c in s.chars() {
    if c.is_control() { out.push(' ') } else { out.push(c) }   // prefix preserved
}
```

So when a recalled `overseer-obs:…` composite becomes a problem `dedup_key`
(`mod.rs:1359`) and re-enters `observation_signature` (`mod.rs:1069`), the next
persisted signature is `overseer-obs:…|overseer-obs:…` — each surviving cycle adds
another layer until the length cap truncates it. This exactly reproduces the
nested/interleaved runs in the observed string.

---

## 5. Why the constituent tokens co-occur and persist (not a counting bug)

The composite is an *honest* fingerprint of a problem set that never changes,
because two observe-and-flag loops never close (verified upstream in prior waves;
re-cited here as the driver, not re-derived):

- **`goal:blocked:*`** — blocked goals are re-observed every board read
  (`signal.rs:440-448`) and re-flagged as `GoalBlocked` because the no-progress
  breaker parks them without a resolving WHY classification (resolution ladder
  gated off). They never leave the blocked population, so the same
  `goal:blocked:<id>` keys reappear each window.
- **`workstream-gap`** — the `WorkstreamGap` arm emits a single **bare constant**
  key (`mod.rs:1371`) with per-gap identity erased; the act path only *notifies*
  (no `launch.rs` edge for `WorkstreamCoverage`), so gaps are never closed and
  reappear. Multiple distinct gaps concatenate as `workstream-gap|workstream-gap`
  because `dedup()` (`mod.rs:1071`) collapses only adjacent equal keys and these
  arrive interleaved across recalled composites.
- **`resource:engineer_spawn`** — a Normal-priority `ResourcePressure` signal
  (`mod.rs:1270`) that recurs whenever live engineer count is elevated; it rides
  along in the same tick's problem set and thus the same signature.

Because none of these problems ever resolve, every window re-emits the same
`dedup_key` set → the same composite signature → the write-back gate suppresses it
*within* a window but re-persists it in a *new* window (or after a daemon restart,
since `write_back_gate.last_delivered` is in-memory/per-process). Two persisted
episodes → §2 fires at `occurrences == 2`.

---

## 6. Independent confirmation summary

| Claim | Evidence (re-opened at `ad5e1060`) | Status |
|---|---|---|
| Task string == RecurringSignature summary | `mod.rs:1360-1362` | ✅ verbatim match |
| `2×` == threshold, from recalled-episode count | `signal.rs:362,455-470` | ✅ |
| Composite = sorted/deduped dedup_keys, `overseer-obs:` prefix | `mod.rs:1068-1072` | ✅ |
| Write-back embeds `[sig:…]`, source `"overseer"` | `wiring.rs:952,1084-1088` | ✅ |
| Recall re-reads that marker as `failure_signature` | `wiring.rs:976,1025` | ✅ |
| **Recall has no source exclusion** (self-feed) | `cognitive_memory/mod.rs:542-550`; `wiring.rs:1013-1021` | ✅ D1 confirmed |
| **`sanitize_recalled` does not collapse nesting** | `capabilities.rs:468-482` | ✅ nesting confirmed |
| Constituent token emitters | `mod.rs:1270,1336,1371` | ✅ |
| WHY enrichment cannot change signature | `mod.rs:455-459` (sets `why` only) | ✅ |

---

## 7. Minimal, surgical fix candidates (diagnosis-only; not applied)

Ordered by blast radius. These follow directly from §3/§4 and target the *loop*,
not the honest counter.

1. **Exclude self-sourced episodes from the recurrence count (fixes D1 / the
   nesting root).** In `signal.rs:455-470`, skip episodes whose recovered
   signature has the `overseer-obs:` prefix (or thread source through
   `RecalledEpisode` and skip `OVERSEER_SOURCE_LABEL`). This stops the Overseer
   from counting its own bookkeeping as an incident and stops new
   `overseer-obs:…` keys from entering `observation_signature`. Smallest change,
   highest leverage. Guard with a test asserting a store containing only
   `overseer`-sourced composite episodes yields **zero** `RecurringSignature`.

2. **Refuse to re-admit a self-signature as a problem key (defense in depth).** In
   `classify_signal`'s `RecurringSignature` arm (`mod.rs:1353-1363`), if
   `signature` already starts with `overseer-obs:`, drop or collapse it rather than
   using it as a `dedup_key`. Prevents nesting even if (1) is bypassed.

3. **Do not carry the `overseer-obs:` prefix into a new signature.** In
   `observation_signature` (`mod.rs:1068`), strip any leading `overseer-obs:` from
   constituent keys before joining, so the output has exactly one prefix layer.
   Cosmetic relative to (1)/(2) but caps signature growth.

**Do not** "fix" this by changing the threshold or de-ratcheting the escalation
counter — §2 shows the count is honest; the loop is the bug. (Prior waves document
the de-ratchet trap: `store_fact_with_caller_key` collapses recall to 1 forever.)

---

## 8. Remaining unknowns

- **Restart vs. new-window as the source of exactly-2×.** Both produce 2 persisted
  episodes; the signature alone cannot distinguish them (no restart/window
  timestamps captured). `write_back_gate` state is in-memory (`mod.rs:299`), so a
  restart is the most likely exact-2× source.
- **Whether the blocked goals underlying the constituent `goal:blocked:*` tokens
  were genuinely stuck or false-parked** is a driver established upstream (the
  kgpacks "already done, misread as stuck" cluster) and is not re-derived here.
- **Live recall LIMIT/consolidation** bounds how large the nested composite can
  grow before truncation; the actual on-store magnitude was not measured.
- **Deliverable scope** assumed diagnosis + prioritized fix candidates; no code
  change was applied in this pass (consistent with the prior docs-only commits).
