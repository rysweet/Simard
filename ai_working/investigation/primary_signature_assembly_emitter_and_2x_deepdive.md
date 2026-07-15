# Primary Deep-Dive: Signature Assembly, Emitter file:line, 2× Recurrence Locus, Gate Semantics

**Investigation question:** the recurring signature
`recurring signature seen 2× in cognitive memory (overseer-obs:goal:blocked:…|…|workstream-gap|…|resource:engineer_spawn|…)`
seen 2× in cognitive memory.

**Verdict:** This is a **self-referential feedback artifact**, not an external
defect. The Overseer observes its **own** episodic write-backs, counts them as
"recurring signatures," folds that recall-derived problem back into the next
write-back signature, and — because the dedup gate is **in-memory only and
resets on every process restart** — re-persists the same signature repeatedly.
The `2×` is literally "two persisted write-back episodes carry the same
`[sig:…]` marker," not two independent real-world failures. The nested
`overseer-obs:…overseer-obs:…` structure is signature **compounding** across
cycles/restarts.

All file:line references are on branch
`investigation/recurring-blocked-goals-workstream-gaps` (HEAD).

---

## 1. End-to-end signature assembly flow

```
Observe → signals_from() → orient() → [root-cause] → CycleReport.problems
                                                          │
              recall (episodic) ──────────────────────────┤
                                                          ▼
                                        write_back_observation(&problems)
                                                          │
                                     observation_signature(problems)
                                        = "overseer-obs:" + sorted/deduped
                                          dedup_keys.join("|")
                                                          │
                                     record_observation(): stores episode
                                     content = "<text> [sig:<signature>]"
                                                          │
                                          (persistent memory graph)
                                                          │
     next tick / next restart:  recall_episodic() → parse_failure_signature()
                                     extracts <signature> from "[sig:…]"
                                                          │
                                     signals_from(): counts per failure_signature
                                     ≥ 2 ⇒ Signal::RecurringSignature{sig,occ}
                                                          │
                                     classify_signal(): dedup_key = sanitize(sig)
                                     ⇒ Problem whose dedup_key is the recalled
                                       "overseer-obs:…" string  ── LOOP CLOSES,
                                       folded into next observation_signature.
```

### Exact loci

| Step | File:line | Code |
|------|-----------|------|
| **Signature assembly (the `overseer-obs:` emitter)** | `src/overseer/mod.rs:1068–1073` | `fn observation_signature` → `format!("overseer-obs:{}", keys.join("|"))` (keys = sorted, deduped `Problem.dedup_key`s) |
| Constituent key `goal:blocked:<id>` | `src/overseer/mod.rs:1336` | `format!("goal:blocked:{goal_id}")` (in `classify_signal`, `Signal::GoalBlocked`) |
| Constituent key `workstream-gap` | `src/overseer/mod.rs:1371` | `"workstream-gap".to_string()` (`Signal::WorkstreamGap`) |
| Constituent key `resource:engineer_spawn` (via keyword) | `src/overseer/capabilities.rs:562` | `Signal::EngineerSpawnRate => "engineer_spawn"` (recall keyword; `resource:` prefix from the recall-key builder) |
| **Human string emitter (`recurring signature seen N×…`)** | `src/overseer/mod.rs:1359–1362` | `sanitize_recalled(&format!("recurring signature seen {occurrences}× in cognitive memory ({signature})"))` |
| Marker embed on write | `src/overseer/wiring.rs:1084` | `let content = format!("{} [sig:{}]", episode.content, episode.signature);` |
| Marker parse on recall | `src/overseer/wiring.rs:976–986` + call at `:1025` | `parse_failure_signature("[sig:…]")` → `RecalledEpisode.failure_signature` |
| Write-back call site (feeds full `cycle.problems`) | `src/overseer/wiring.rs:301` → `src/overseer/mod.rs:534–563` | `write_back_observation(&cycle.problems)` |

---

## 2. The 2× recurrence-counting locus

**File:line: `src/overseer/signal.rs:455–470`** (threshold const at
`src/overseer/signal.rs:362`).

```rust
// signal.rs:455
if let Some(snapshot) = &state.recall {
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for ep in &snapshot.episodes {
        if let Some(sig) = &ep.failure_signature {      // parsed from [sig:…]
            *counts.entry(sig.as_str()).or_insert(0) += 1;
        }
    }
    for (signature, occurrences) in counts {
        if occurrences >= RECURRING_SIGNATURE_THRESHOLD {   // == 2 (signal.rs:362)
            out.push(Signal::RecurringSignature { signature, occurrences });
        }
    }
}
```

Key facts:
- The count is over **recalled episodes in one Observe pass**, grouped by
  `failure_signature`. It counts *persisted rows*, not distinct incidents.
- `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) is the floor, so the
  minimum reportable value is exactly `2×` — matching the observed string.
- Because write-back episodes are keyed on `overseer-obs:<joined keys>`
  (`mod.rs:1072`) and re-persisted across restarts (see §3), the two counted
  episodes are typically the Overseer's **own** prior write-backs, giving the
  self-referential `overseer-obs:…` signature.

### Compounding / nesting mechanism (why the signature contains nested `overseer-obs:` and repeated blocks)
- `classify_signal` sets the RecurringSignature problem's `dedup_key` to the
  **recalled signature itself**: `src/overseer/mod.rs:1359`
  (`sanitize_recalled(signature)`).
- `orient` keeps that problem (`mod.rs:1200–1235`); the full `cycle.problems`
  set is handed to `write_back_observation` (`wiring.rs:301`).
- `observation_signature` then joins **all** dedup_keys, including the
  `overseer-obs:…` one, into the *next* signature (`mod.rs:1069–1072`),
  yielding `overseer-obs:…|overseer-obs:…|…`. Each cycle/restart can re-embed
  the previous signature → the observed multi-block, multi-prefix string.
- `sort_unstable()` + `dedup()` (`mod.rs:1070–1071`) only collapse *adjacent
  exact-duplicate* keys within one assembly; distinct-but-overlapping
  `overseer-obs:…` variants and the repeated `workstream-gap` / `goal:blocked:*`
  blocks survive because they differ by embedded content, so the string grows.

---

## 3. Dedup / window / restart gate semantics

Gate type: `WhisperGate` — `src/overseer/guardrails.rs:291–343`.

```rust
// guardrails.rs
struct WhisperGate { window_secs: i64, cap_per_hour: usize,
                     last_delivered: HashMap<String,i64>, deliveries: Vec<i64> }

peek(sig, now):                                   // guardrails.rs:312
  if last_delivered[sig] exists && now - last < window_secs
        → SuppressDuplicate                        // dedup window
  if (deliveries within last 3600s) >= cap_per_hour
        → SuppressCapReached                        // rolling per-hour cap
  else → Deliver
commit(sig, now): last_delivered[sig]=now; deliveries.push(now);
                  retain(t > now-3600)              // prune rolling hour
```

Instances and their (window_secs, cap_per_hour) — `src/overseer/mod.rs:286–304`:

| Gate | window | cap/hr | mod.rs |
|------|--------|--------|--------|
| `whisper_gate` | 900s | 5 | :286 |
| `blocked_goal_gate` | 900s | 20 | :292 |
| **`write_back_gate`** (episodic write-back) | **900s** | **5** | :299 |
| **`gap_gate`** (workstream-gap notify) | **900s** | **200** | :304 |

Semantics relevant to the recurrence:
- **Dedup window = 900s (15 min).** Within one running process, an identical
  `overseer-obs:…` signature is written back at most once per 15 min
  (`mod.rs:548` peek → `:556` commit only after a successful store).
- **Restart gate = NONE (the root enabler).** `WhisperGate::new`
  (`guardrails.rs:301`) initialises `last_delivered`/`deliveries` **empty**;
  there is no load/save of gate state anywhere. The gate is purely in-memory.
  On every Overseer daemon **restart** the 15-min window resets, so the same
  `overseer-obs:…` write-back is stored **again** into the *persistent* memory
  graph. Two restarts within a recall horizon ⇒ two identical `[sig:…]`
  episodes ⇒ `occurrences == 2` at `signal.rs:463` ⇒ the `2×` signal.
- Write-back gate is deliberately conservative (5/hr) but is bypassed by
  restarts; the memory graph has no equivalent write-side idempotency on
  `signature`, so the persistent store accumulates duplicates the ephemeral
  gate was meant to prevent.

---

## 4. Root cause & minimal-fix pointers (for the fixer, not implemented here)

**Root cause:** persistent-store write amplification gated only by an
**ephemeral** dedup window, combined with the Overseer folding its own
recall-derived `overseer-obs:` signature back into the next write-back
signature. Persistent memory + non-persistent gate + self-ingestion = the
compounding `overseer-obs:…` "recurring 2×" artifact.

Candidate fixes (any one breaks the loop; ordered by locality):
1. **Store-side idempotency:** in `record_observation`
   (`wiring.rs:1076–1091`), upsert/dedup on the `signature` metadata (or query
   before `store_episode`) so identical `[sig:…]` episodes are not duplicated
   across restarts. Directly kills the `occurrences==2` inflation.
2. **Break self-ingestion:** exclude `Signal::RecurringSignature`-derived
   problems (dedup_key starting `overseer-obs:`) from `observation_signature`
   in `write_back_observation` (`mod.rs:546`) / `observation_signature`
   (`mod.rs:1068`). Stops the `overseer-obs:…overseer-obs:…` nesting/growth.
3. **Persist the write-back dedup window** across restarts (give
   `write_back_gate` durable `last_delivered` keyed by signature), so a restart
   no longer re-opens the 15-min window for an already-recorded signature.

Fixes 1 and 2 are complementary: (1) stops count inflation, (2) stops signature
growth. Recommend both.

---

## Evidence index (verbatim loci)
- Signature assembly: `src/overseer/mod.rs:1068–1073`
- Human "N×" string: `src/overseer/mod.rs:1359–1362`
- 2× counting: `src/overseer/signal.rs:455–470`; threshold `signal.rs:362`
- Marker embed/parse: `src/overseer/wiring.rs:1084`, `:976–986`, `:1025`
- Write-back path & gate: `src/overseer/mod.rs:534–563`; call `wiring.rs:301`
- Gate type/semantics: `src/overseer/guardrails.rs:291–343`
- Gate config (windows/caps): `src/overseer/mod.rs:286–304`
- Self-ingestion (dedup_key = recalled sig): `src/overseer/mod.rs:1359`,
  merge in `orient` `mod.rs:1200–1235`
