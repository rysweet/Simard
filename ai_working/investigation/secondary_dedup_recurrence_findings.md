# Secondary Investigation — Cognitive-Memory Dedup / Recurrence Path

**Focus:** Classify the "recurring signature seen 2× in cognitive memory
(`overseer-obs:...`)" as a **real re-observation loop** vs. a **dedup/storage
artifact**.

**Verdict (high confidence): REAL re-observation loop — NOT a dedup/storage/replay
bug.** The within-window dedup gate is working correctly; the `×2` is a faithful
count of two genuine write-back episodes of an unchanged problem set across two
windows. Additionally, I found a **self-referential feedback path** (recall-derived
`overseer-obs:` problems are written back into new observation signatures) that
explains the *nested* `overseer-obs:` tokens visible in the signature.

---

## 1. The mechanism, end to end (with citations)

### 1a. Signature construction (deterministic, correct)
`observation_signature` (`src/overseer/mod.rs:1068-1073`) builds the key as the
**sorted, de-duplicated** problem `dedup_key`s joined by `|`, prefixed
`overseer-obs:`. `keys.sort_unstable(); keys.dedup();` — so identical observations
collapse to one signature; different observations stay distinct. This is correct
and stable.

### 1b. Within-window dedup gate (correct — this is the "dedup" people worry about)
`write_back_observation` (`mod.rs:534-563`) gates every write through
`write_back_gate` = `WhisperGate::new(900, 5)` (`mod.rs:299`): a **15-minute
window** keyed on the signature, cap 5/hour. `peek` returns `SuppressDuplicate`
when `now - last < window_secs` (`guardrails.rs:312-317`). The slot is committed
**only after a successful store** (`mod.rs:555-556`), so a failed write never
suppresses a later one.

**Test proof it dedups correctly within a window:**
`write_back_is_deduplicated_within_window` (`tests_memory_recall.rs:797-817`) —
two identical ticks ⇒ `memory_writes` 1 then 0 ⇒ exactly one episode persisted.

### 1c. Storage layer does NOT dedup (by design → cross-window accumulation)
`OverseerMemoryAdapter::record_observation` (`wiring.rs:1076-1091`) issues an
**unconditional** `store_episode(content, "overseer", metadata)`; the adapter
(`library_adapter.rs:609-628`) forwards straight to `store_episode` with **no
query-before-store and no signature upsert**. Contrast the procedural fix
(#2298, `tests_pr_2298_idempotency.rs`) which made `store_procedure` an idempotent
upsert keyed on `name`. **Observation episodes have no equivalent.**

Consequence: once the 15-min window expires (or the daemon restarts — the
`WhisperGate.last_delivered` HashMap is in-memory, per-process, `guardrails.rs:294`),
the same static problem set is written **again as a new episode node** carrying the
**same** `overseer-obs:...` signature.

### 1d. Recurrence count derivation (the "seen 2×")
On a later tick, `recall_pass` (`mod.rs:498-516`) pulls episodes; `signals_from`
counts episodes sharing a `failure_signature` and, at `RECURRING_SIGNATURE_THRESHOLD
= 2` (`signal.rs:362`, `signal.rs:463-464`), emits
`Signal::RecurringSignature { signature, occurrences }`.

**Test proof:** `recurring_signature_emitted_when_two_episodes_share_signature`
(`tests_memory_recall.rs:471-491`) — 2 episodes with the same signature ⇒
`occurrences: 2`. A single occurrence does not (`:494-506`).

That signal maps (`mod.rs:1353-1363`) to a High-priority `ProcessHealth` problem
whose summary is **verbatim the investigation string**:
`"recurring signature seen {occurrences}× in cognitive memory ({signature})"`.

**So `occurrences = N` literally means "N distinct write-back episodes carrying
this signature currently sit in the graph" — a true recurrence count, not a
duplicate-node miscount.**

---

## 2. Why "2×" specifically — and why it's real, not an artifact

- A **tick** is far shorter than the 900 s gate window, so per-tick re-observation
  is correctly suppressed (1b). The count only advances **once per window** (or per
  restart). `×2` ⇒ the identical problem set survived **two** windows unresolved.
- `×2` is **above per-tick noise** (threshold 2) but **below**
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) — a **recurrence dead
  zone**: recorded and re-flagged, but no escalation rung fires. Combined with the
  observe-and-flag-without-a-closing-action pattern (workstream-gap /
  goal-board-health paths never resolve the blocked cluster), the same set recurs
  indefinitely, oscillating around low occurrence counts.
- **Not a replay/dedup artifact:** `dedup()` in the signature collapses only
  *adjacent equal* keys within one signature (`mod.rs:1071`), and the write-back
  gate provably suppresses within-window duplicates (test 1b). The 2 episodes are
  two **separate, legitimate** write-back passes of a **static** problem set.

---

## 3. NEW finding — self-referential write-back feedback (design concern)

`write_back_observation(&cycle.problems)` (`wiring.rs:301`) writes back **all**
cycle problems, **including the recall-derived `RecurringSignature` problem**.
That problem's `dedup_key` is `sanitize_recalled(signature)` = the prior
`overseer-obs:...` string (`mod.rs:1359`), which differs from the base
`goal:blocked:*` keys, so `orient` (`mod.rs:1210-1221`) does **not** merge it away —
it is admitted as a distinct problem and folded into the **next** observation
signature.

**Result:** the next signature becomes
`overseer-obs:[ ...goal:blocked keys..., "overseer-obs:goal:blocked:...|..." ]` —
i.e. an `overseer-obs:` token **nested inside** the composite. This exactly matches
the investigation-question signature, which contains `overseer-obs:` repeated
internally between `goal:blocked:*` runs. The Overseer is recalling and
re-observing **its own bookkeeping**.

- **Bounded, not runaway:** growth is throttled by the 15-min gate, the recall
  budget/limit, `orient`'s same-key merge, and the `×2` threshold. It stabilizes
  into a small family of nested signatures each "seen ~2×" — consistent with the
  observed data.
- **Still a smell:** the write-back is documented as recording *stewardship over
  Simard's problems* (`mod.rs:518-523`), not the Overseer's own recalled
  observations. The presence of `sanitize_recalled` at this exact admission
  boundary (`mod.rs:1359`) shows the authors already treat recalled signatures as
  untrusted — yet they are still written back, polluting future signatures.

---

## 4. Patterns / anti-patterns

- **Observe-and-flag without a closing action** (PATTERNS.md anti-pattern): the
  recurrence is faithfully recorded but never resolved → permanent low-count
  recurrence.
- **Recurrence dead zone:** threshold-2 emit vs. threshold-3 escalate, with no rung
  between → recurs forever at `×2`.
- **No storage-layer idempotency for observation episodes** (asymmetric with the
  #2298 procedural upsert) → unbounded same-signature node accumulation for a
  long-lived unresolved problem (mitigated only by recall LIMIT / consolidation).
- **Self-observation feedback:** recall-derived problems re-enter the write-back set.

## 5. Integration points
- `signals_from` / `RECURRING_SIGNATURE_THRESHOLD` (`signal.rs`) → `orient`
  (`mod.rs:1210`) → `RecurringSignature` problem (`mod.rs:1353`) →
  `write_back_observation` (`wiring.rs:301`) → `record_observation`
  (`wiring.rs:1076`) → `store_episode` (`library_adapter.rs:609`) → recall
  (`recall_pass`, `mod.rs:498`) → back to `signals_from`. **This closes a loop.**
- Escalation coupling: `RECURRENCE_ESCALATION_THRESHOLD` (`root_cause.rs:33`) is the
  primary handoff into the blocked-goal health path (owned by the primary/tertiary
  investigators).

## 6. Questions for verification phase
1. **Should `write_back_observation` exclude recall-derived `RecurringSignature`
   problems** (kind `ProcessHealth`, key prefixed `overseer-obs:`) to stop the
   Overseer observing its own bookkeeping? (Removes the nested-signature pollution;
   preserves genuine recurrence signalling.)
2. **Should observation episodes get a signature-keyed idempotent upsert** (like
   #2298) or a bounded retention/consolidation, so a long-unresolved problem does
   not accumulate unbounded same-signature nodes?
3. Confirm empirically whether the two episodes arose from **two 15-min windows in
   one run** or **two daemon restarts** (both are "real", but the latter shows the
   in-memory gate offers no cross-restart protection — `guardrails.rs:294`).
4. Confirm the dead-zone remedy is owned elsewhere (root_cause escalation): at
   `×2` nothing acts — is a rung intended between emit(2) and escalate(3)?

**Bottom line:** the `×2` is a *correct* recurrence count of a genuinely
re-observed static problem set. The dedup gate is not broken. The real issues are
(a) an unresolved problem set that recurs by design because nothing closes it, and
(b) a self-referential write-back that nests `overseer-obs:` signatures — both
design concerns, neither a dedup/storage defect.
