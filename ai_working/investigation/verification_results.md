# Verification Phase — Practical Tests for the "recurring signature seen 2×" Hypotheses

**Scope:** Execute practical verification tests for the hypotheses about the
recurring `overseer-obs:...` signature "seen 2×" in cognitive memory.

**Verdict: H1 CONFIRMED (high confidence).** The `×2` is a *faithful cross-window
recurrence count* of a genuinely re-observed static problem set — **not** a
dedup/storage/replay defect. Every confirming test passes; every refuting
condition is empirically excluded.

---

## H1 — "composite `overseer-obs:` signature is sorted+deduped `dedup_key`s joined by `|`; ×2 = same static set written back across two windows"

### Test method: trace_code + run tests + standalone reproduction

**Environment:** `cargo test -p simard --lib` (package `simard` owns `src/overseer`).
Full overseer suite: **359 passed, 0 failed.**

| # | Test / probe | Verifies | Result |
|---|---|---|---|
| 1 | `observation_signature` @ `mod.rs:1068-1073` (trace) | `sort_unstable(); dedup(); join("\|")`, `overseer-obs:` prefix | ✅ matches claim exactly |
| 2 | Standalone reproduction (rustc) | same set/any order → identical signature; distinct sets → distinct; no adjacent dup keys | ✅ PASS |
| 3 | `write_back_is_deduplicated_within_window` (`tests_memory_recall.rs:797-817`) | 2 identical ticks in one window → `memory_writes` 1 then 0; exactly 1 episode persisted | ✅ ok |
| 4 | `whisper_gate_suppresses_an_identical_whisper_within_the_window` (`tests_whisper.rs:437-458`) | same sig at t=300/899 → `SuppressDuplicate`; **t=901 → `Deliver`** (same sig re-delivers past the 900 s window) | ✅ ok |
| 5 | `whisper_gate_caps_whispers_per_rolling_hour` (`tests_whisper.rs:461-478`) | 4th distinct sig/hour capped; budget frees next hour | ✅ ok |
| 6 | `recurring_signature_emitted_when_two_episodes_share_signature` (`:471-491`) | 2 episodes same sig → `RecurringSignature{occurrences:2}` | ✅ ok |
| 7 | `recurring_signature_not_emitted_for_single_occurrence` (`:494-506`) | 1 episode → no signal | ✅ ok |
| 8 | `write_back_persists_again_for_a_distinct_signature` (`:819+`) | different observations → distinct sigs → both recorded | ✅ ok |
| 9 | `orient_raises_recurring_signature_to_high_priority` | signal → High `ProcessHealth` problem | ✅ ok |

### Confirming evidence — all validated
- Signature builder is deterministic, sorted, deduped, `|`-joined, `overseer-obs:`-prefixed (probes 1–2).
- Within-window dedup provably suppresses intra-window duplicates (probes 3–4).
- Gate = `WhisperGate::new(900, 5)` (`mod.rs:299`) enforces ≤1 write / 15 min per signature ⇒ `×2` ⇒ ≥2 distinct windows (probe 4: t=901 re-delivers).

### Refuting conditions — all excluded
- **"Single write yields count 2 / double-increment"** → excluded: probe 3 shows 1 write = 1 episode; probe 7 shows single occurrence emits nothing.
- **"`dedup()` not applied → adjacent duplicate keys in signature"** → excluded: probe 2 asserts no `a|a` adjacency; `keys.dedup()` present at `mod.rs:1071`.
- **"Two identical-signature episodes inside one 15-min window"** → excluded: probe 4 returns `SuppressDuplicate` for the whole window; commit happens only after a successful store (`mod.rs:555-556`).

---

## Corroborated secondary findings (citations re-verified)

- **Count semantics:** `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`);
  maps (`mod.rs:1353-1363`) to a High `ProcessHealth` problem whose summary is
  **verbatim** the investigation string:
  `"recurring signature seen {occurrences}× in cognitive memory ({signature})"`.
  Confirmed against source. So `occurrences=N` = N distinct write-back episodes
  in the graph — a true recurrence count.

- **Recurrence "dead zone":** emit at 2 vs. `RECURRENCE_ESCALATION_THRESHOLD = 3`
  (`root_cause.rs:33`, confirmed) → recorded/re-flagged at `×2` but no escalation
  rung fires → recurs indefinitely (no closing action).

- **Self-referential write-back (nested `overseer-obs:` tokens):**
  `write_back_observation(&cycle.problems)` (`wiring.rs:301`, confirmed) writes
  back **all** problems including the recall-derived `RecurringSignature` one,
  whose `dedup_key = sanitize_recalled(signature)` (`mod.rs:1359`, confirmed) is
  the prior `overseer-obs:...` string. `orient`'s same-key merge does not remove
  it, so it nests inside the next signature — exactly the shape in the
  investigation signature (`overseer-obs:` repeated between `goal:blocked:*` runs).

- **No storage-layer idempotency for observation episodes:** the WhisperGate is
  in-memory / per-process (`guardrails.rs:294`), so cross-window (or
  cross-restart) re-observation of an unresolved static set legitimately appends
  new same-signature episodes. Asymmetric with the #2298 procedural upsert.

---

## Bottom line
The dedup gate is **not broken**. `×2` is a correct recurrence count of a
genuinely re-observed, unresolved problem set. The real (design) issues are:
(1) observe-and-flag with **no closing action** + a threshold-2/escalate-3 **dead
zone** ⇒ permanent low-count recurrence; (2) **self-referential write-back** that
nests `overseer-obs:` signatures. Both are design concerns, neither a
dedup/storage defect.
