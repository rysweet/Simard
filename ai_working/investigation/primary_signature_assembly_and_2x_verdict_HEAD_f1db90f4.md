# Primary Deep Dive — Signature Assembly Provenance & the 2× Real-vs-Bug Verdict

**Role:** PRIMARY investigator.
**HEAD:** `f1db90f4` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Investigation question:** the recurring composite signature
`overseer-obs:goal:blocked:…|…|workstream-gap|resource:engineer_spawn` seen **2×**
in cognitive memory.
**Focus (this wave):** (1) signature **assembly provenance** —
`observation_signature` + the full `run_cycle → write_back_observation →
record_observation` pipeline; (2) the **2× real-vs-bug verdict** adjudicated
specifically through the **WhisperGate 900 s window + per-process `last_delivered`
map**.
**Method:** re-read every load-bearing line at current HEAD (did not trust prior
doc citations — the tree moved `b9f99879 → f1db90f4`); ran the targeted guard suites.

---

## 0. One-line verdict

The `2×` is **real signal, not a WhisperGate bug.** The write-back gate is a
process-local `HashMap<String,i64> last_delivered` (`guardrails.rs:294`) with a
**900 s** window whose *only contract* is "don't re-inject the same observation
every tick." It **never promises global once-only persistence** — so a genuinely
still-blocked condition re-observed after a restart, after a cross-process write,
or `> 900 s` later is **honestly persisted a second time**, and recall then
faithfully counts `occurrences = 2`. Every link in the assembly pipeline is a
deterministic, side-effect-honest projection. **Do not "fix" the gate or the
counter** — collapsing either would suppress a truthful under-throughput signal.

---

## 1. Signature assembly pipeline (verified at HEAD `f1db90f4`)

```
run_cycle()                             mod.rs:384
  ├ observe: status.snapshot + observe_board + workstream_gaps + failure_sink
  │          + fail-closed cognitive recall                    mod.rs:386-438
  ├ signals_from(&observed)             mod.rs:441  (+ recall fold → RecurringSignature)
  ├ orient(&signals,&in_flight)         mod.rs:447  → Vec<Problem>, why=None, dedup-merged
  ├ per-problem root-cause analyze      mod.rs:455-459
  └ decide/gate/classify → CycleReport{ problems, … }
        │
        ▼  (daemon loop, per tick)
write_back_observation(&cycle.problems) mod.rs:534 ; called wiring.rs:301
  ├ guard: recall disabled → None ; problems empty → None      mod.rs:538-545
  ├ signature = observation_signature(problems)                mod.rs:546
  │     keys = problems.map(dedup_key); sort_unstable(); dedup();
  │     "overseer-obs:" + keys.join("|")            ◀── THE STRING IN THE QUESTION   mod.rs:1068-1073
  ├ write_back_gate.peek(&signature, now)           mod.rs:548  (WhisperGate, 900s, cap 5/h — mod.rs:299)
  │     Deliver → record_observation(episode) ; commit(sig)    mod.rs:549-557
  │     _       → Ok(None)  (suppressed; nothing persisted)    mod.rs:559-561
  ▼
record_observation(&episode)            wiring.rs:1076
  └ content = "{content} [sig:{signature}]" ; store_episode(FIXED source) wiring.rs:1084-1090  ◀── persisted
        │
        … later tick / restart / >900s …
        ▼
recall_episodic → parse_failure_signature("[sig:…]")  wiring.rs:976-986,1013-1025
        ▼
signals_from recall fold                signal.rs:455-470
  counts: BTreeMap<&str,u32> of episode.failure_signature ; occurrences ≥ 2 → RecurringSignature   signal.rs:462-467
        ▼
classify_signal (RecurringSignature arm) mod.rs:1353-1361
  summary: "recurring signature seen {occurrences}× in cognitive memory ({signature})"  ◀── verbatim question text
```

This is a **closed feedback loop**: the Overseer's own write-back becomes its own
future recall evidence. Verified links (current line numbers):

| Link | File:line |
|---|---|
| Composite emitter (`sort→dedup→"overseer-obs:"+join`) | `mod.rs:1068-1073` |
| Write-back + gate peek/commit | `mod.rs:534-563` |
| Called from daemon tick | `wiring.rs:301` |
| Token dedup_keys (`resource:engineer_spawn`,`goal:blocked:{id}`,`workstream-gap`) | `mod.rs:1270, 1336, 1371` |
| Recurring-signature summary text | `mod.rs:1353-1361` |
| Recall count + threshold=2 | `signal.rs:455-470`, `:362` |
| Persist marker `[sig:…]` + parse-back | `wiring.rs:1084`, `:976-986` |

---

## 2. The WhisperGate / `last_delivered` adjudication (the core of this wave)

### 2a. What the gate actually guarantees (cited)

`WhisperGate` (`guardrails.rs:291-333`) state:

```rust
struct WhisperGate {
    window_secs: i64,                          // 900 for write_back_gate (mod.rs:299)
    cap_per_hour: usize,                       // 5
    last_delivered: HashMap<String, i64>,      // guardrails.rs:294  ◀── per-PROCESS, in-memory
    deliveries: Vec<i64>,
}
```

`peek` suppresses **only** when `now - last < window_secs` for a signature already
present in `last_delivered` (`guardrails.rs:313-317`). Three structural properties
follow directly from the type:

1. **Process-local.** `last_delivered` is a plain in-memory `HashMap`. It is not
   persisted, not shared, and is **reset to empty on every `WhisperGate::new`**
   (`guardrails.rs:305`) — i.e. on every daemon start. A restart between two ticks
   ⇒ the slot is gone ⇒ the identical observation re-delivers.
2. **Windowed, not permanent.** Past `window_secs` the suppression lapses by
   design (`now - last < window` becomes false). Same signature `> 900 s` later ⇒
   re-delivers.
3. **Commit-after-success only.** The act path `peek … record … commit`
   (`mod.rs:548-557`) consumes the slot only *after* a successful store, so a
   failed write never suppresses a later one. Correct fail-open-on-error shape.

### 2b. The gate's contract is "not chatty", NOT "exactly once"

The doc-comment states the intent precisely: "a persistent condition is recorded
**at most once per window** (never every tick)" (`mod.rs:520-523`). The gate is a
**per-tick anti-flood**, not a global dedup ledger. Re-persisting the same
condition once per 900 s window — or once per process lifetime — is the
**specified behavior**, not a leak.

### 2c. Empirical proof (ran at HEAD)

- `guardrails`/`tests_whisper::whisper_gate_suppresses_an_identical_whisper_within_the_window`
  (`tests_whisper.rs:436-458`, virtual clock): `sig-a` Deliver@0, Suppress@300,
  Suppress@899, **Deliver@901** — same signature re-delivers past the window.
  → **2 deliveries of one signature ⇒ 2 persisted episodes ⇒ recall `occurrences=2`.**
- `tests_memory_recall::write_back_is_deduplicated_within_window`
  (`:797-817`): two identical ticks inside the window ⇒ exactly **one** episode
  persisted (`memory_writes` 1 then 0). Confirms in-window suppression.
- `tests_memory_recall::tick_writes_observation_back_once` (`:779`) and
  `write_back_persists_again_for_a_distinct_signature` (`:820`): once per signature
  per window; distinct signatures both persist.
- `tests_memory_recall::recurring_signature_emitted_when_two_episodes_share_signature`
  + `…not_emitted_for_single_occurrence`: `occurrences` is a faithful episode tally
  against threshold **2**.

Runs at HEAD `f1db90f4`:
`cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed**;
`cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**.

### 2d. Real-vs-bug decision matrix

| Producer of the 2nd episode | Gate behaves as… | Verdict |
|---|---|---|
| Daemon **restart** between ticks (`last_delivered` reset) | as designed (in-mem state) | **real** — the condition genuinely still holds |
| Same condition re-observed **> 900 s** later | as designed (window lapse) | **real** — measured world still stuck |
| **Different process/instance** writes it (no shared gate) | as designed (process-local) | **real** — independent honest observation |
| Same tick / within 900 s, same process | **suppressed** (returns `None`) | would-be dup **prevented** |
| Storage-layer replay / superseded facts | recall reads live only (`include_superseded:false`) | **not** a source — no amplification |

Every path that yields `2×` corresponds to a **genuine second observation of a
still-true condition**. The gate's only job — stop *within-window per-tick*
duplication — it performs correctly (proven by `write_back_is_deduplicated_within_window`).
There is **no gate defect** behind the `2×`.

---

## 3. Assembly-side integrity checks (no fabrication in the string)

1. **Intra-signature uniqueness.** `observation_signature` does
   `sort_unstable()` then `dedup()` (`mod.rs:1070-1071`), so the `|`-list inside a
   single `overseer-obs:` block is unique+sorted. The repeated `overseer-obs:…`
   blocks in the question are **separate recalled episodes concatenated by the
   recall query**, consistent with `occurrences = 2` (two whole episodes) — not
   intra-signature duplication.
2. **Provenance is fixed, not caller-chosen.** `record_observation` hardcodes
   `OVERSEER_SOURCE_LABEL` and emits only a `{signature}` metadata object — no
   secrets/env (`wiring.rs:1081-1090`).
3. **Content is sanitized.** `observation_content` runs every problem summary and
   the whole line through `sanitize_recalled` (`mod.rs:1079-1089`) — defence in
   depth against recalled-text re-entry.
4. **Set-hash honesty.** The signature is stable iff the observed problem
   membership is stable; `blocked_goals` is a pure projection of
   `GoalProgress::Blocked` (`sensor.rs`, per prior waves) — no fabrication.

---

## 4. Reconciliation with prior verdicts (no re-derivation)

| Prior claim | This wave |
|---|---|
| `primary_signature_emitter_and_2x_semantics`: `2×` = recall episode count, not a gate counter; dup arises from in-mem 900 s gate | **CONFIRM** at HEAD `f1db90f4`; re-verified every cited line + reran gate/recall suites |
| `tertiary_lane_isolation_…_f9cefec1`: `×2` is **intended signal, not a recording defect**; Lane A ⊥ Lane B | **CONFIRM** — my gate-level adjudication independently reaches the same verdict from the WhisperGate/`last_delivered` angle |
| Recording concern lives only in **Lane-B** durability (non-idempotent `store_fact`), not the `×2` | **ADOPT** — out of this wave's scope; the write-back path's `store_episode` append is *correctly* windowed by the gate, not the ratchet in question |
| Naïve `store_fact_with_caller_key` "fix" is a trap | **CONFIRM** — irrelevant to the write-back gate, which must stay windowed |

**No contradictions.** New PRIMARY contribution: an explicit, type-grounded
adjudication that the WhisperGate's process-local `last_delivered` + 900 s window
is **working to spec**, and that every mechanism producing the second episode is a
genuine re-observation — so the `2×` is **real, not a gate bug**.

---

## 5. Recommendation (diagnosis only — the underlying goals are OUT OF SCOPE)

**No change to `WhisperGate`, `write_back_gate`, `observation_signature`, or the
recall counter.** The assembly is honest and the gate is correct. The `2×` is a
truthful under-throughput signal that the measured world (a blocked-goal cluster)
is genuinely static. Per prior settled waves, the real remediation lies elsewhere:
give `WorkstreamCoverage` a **closing edge**, reconcile issue-closed goals out of
`Blocked` (D0) and fail loud when completion-evidence is absent, and harden
**Lane-B** recording via a caller-key upsert (not the naïve swap). **Do NOT**
persist the write-back gate, silence the `2×`, or merge the recurrence lanes.

---

## 6. Verification performed

- Re-read at HEAD `f1db90f4`: `mod.rs:384-475, 534-563, 1068-1073, 1264-1371`;
  `guardrails.rs:267-343`; `wiring.rs:295-311, 973-986, 1013-1025, 1076-1091`;
  `signal.rs:70, 362, 448-470`.
- `cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed, 0 failed**.
- `cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**
  (incl. `write_back_is_deduplicated_within_window`,
  `tick_writes_observation_back_once`,
  `recurring_signature_emitted_when_two_episodes_share_signature`).

**Bottom line:** the composite `overseer-obs:…|workstream-gap|resource:engineer_spawn`
is one `observation_signature` write-back key; the `2×` is
`RecurringSignature.occurrences` — a faithful count of two persisted episodes. The
WhisperGate's process-local `last_delivered` + 900 s window is behaving exactly as
specified (in-window suppression proven; past-window re-delivery proven). The `2×`
is **real signal, not a gate defect.**
