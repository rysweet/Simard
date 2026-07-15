# Primary Deep Dive — Signature Assembly, the Emission Pipeline, and the Recurrence/Idempotency Gate

**Role:** PRIMARY investigator.
**HEAD:** `3fac68a5` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Investigation question:** the recurring composite signature
`overseer-obs:goal:blocked:…|…|workstream-gap|resource:engineer_spawn` reported as
**"recurring signature seen 2× in cognitive memory (…)"**.
**Focus (this wave):** (1) **signature assembly + the emission pipeline** — end-to-end,
which function builds the string, from what inputs, and every hop from build → gate →
persist → recall → re-emit; **name the emitter**. (2) the **recurrence/idempotency
gate** — what it guarantees, and whether the `2×` is a genuine re-observation or a
recording defect.
**Method (validate-don't-re-derive):** the last PRIMARY doc was authored at `f455c06d`;
HEAD advanced by two documentation-only commits (`d6ba8b25`, `3fac68a5`). I re-read
every load-bearing line **directly from live `src/` at HEAD `3fac68a5`** (did not trust
prior citations), confirmed line numbers, and **reran the two guard suites myself**.

---

## 0. One-line verdict

The `2×` is **real re-observation signal, not a duplicate/recording defect.** The
emitter is **`observation_signature(problems)` (`src/overseer/mod.rs:1068-1073`)** — a
deterministic, sorted-and-deduped set-hash of the observed problem membership. The `2×`
is `RecurringSignature.occurrences`, a faithful tally of **two distinct persisted
episodes** that share that signature. The sole anti-duplication mechanism on the
write-back path — the `write_back_gate` `WhisperGate` (900 s window, cap 5/hr,
**process-local, volatile** `last_delivered`) — provably suppresses within-window,
same-process repeats. A second persisted episode therefore requires a **genuine**
trigger: a `>900 s` gap, a daemon **restart**, or a **distinct process**. Each is an
honest re-observation of a still-true condition. **Do not "fix" the emitter, the gate,
or the counter** — collapsing any of them suppresses a truthful under-throughput signal.
The defect is downstream (missing closing edge), owned by the secondary/tertiary lanes.

---

## 1. The emitter — named, with full provenance (verified @ `3fac68a5`)

The literal `overseer-obs:…` string has exactly **one** producer; the "recurring
signature seen 2×" wrapper has exactly **one** producer. Each hop re-verified live:

| Pipeline stage | Producer (named) | File:line @ `3fac68a5` |
|---|---|---|
| **① Token synthesis** — `goal:blocked:{goal_id}` | `classify_signal` `GoalBlocked` arm | `mod.rs:1336` |
| **① Token synthesis** — `workstream-gap` (bare) | `classify_signal` `WorkstreamGap` arm | `mod.rs:1371` |
| **① Token synthesis** — `resource:engineer_spawn` | `classify_signal` `EngineerSpawnRate` arm | `mod.rs:1270` |
| **② Composite emitter** — `"overseer-obs:" + sorted∘deduped dedup_keys.join("\|")` | **`observation_signature(problems)`** | **`mod.rs:1068-1073`** |
| **③ Human body** — `sanitize_recalled`-cleaned one-liner | `observation_content(problems)` | `mod.rs:1079-1089` |
| **④ Emission caller + idempotency gate** | `write_back_observation` | `mod.rs:534-563` |
| **④ Gate primitive** — `peek`/`commit`/`admit` | `WhisperGate` | `guardrails.rs:312-342` |
| **⑤ Persist adapter** — fixed provenance, `[sig:…]` marker, one `store_episode` | `record_observation` | `wiring.rs:1076-1090` |
| **⑥ Recall parse-back** of `[sig:…]` | `parse_failure_signature` / `recall_episodic` | `wiring.rs:976-986, 1013-1030` |
| **⑦ Recall fold** → `RecurringSignature{occurrences}` (≥2) | `signals_from` | `signal.rs:459-470` (threshold `:362`) |
| **⑧ "recurring signature seen {occurrences}× …"** | `classify_signal` `RecurringSignature` arm | `mod.rs:1353-1363` |

**Assembly integrity (no fabrication inside the string):**

1. **Set-hash, sorted + deduped.** `observation_signature` does
   `keys.sort_unstable(); keys.dedup();` before `join("|")` (`mod.rs:1069-1072`), so the
   `|`-list *inside* one `overseer-obs:` block is unique and order-independent. The many
   repeated `overseer-obs:…` blocks in the queried string are **whole recalled episodes
   concatenated by the recall query** (the `occurrences` tally) — **not** intra-signature
   duplication.
2. **Fixed provenance.** `record_observation` hardcodes `OVERSEER_SOURCE_LABEL =
   "overseer"` (`wiring.rs:952, 1086`); source is never caller-chosen, so a hostile
   payload cannot spoof an author. Metadata is a validated JSON object carrying only
   `{"signature": …}` — no secrets/tokens/env.
3. **Sanitized content.** Every problem summary passes through `sanitize_recalled` in
   `observation_content` (`mod.rs:1082`), and the recall summary is `sanitize_recalled`
   again at the classify boundary (`mod.rs:1359-1362`) — defence-in-depth against
   recalled-text re-entry in a multi-writer graph.
4. **Honest membership.** The signature is stable **iff** the observed problem set is
   stable; tokens are pure projections of observed state (blocked goals, workstream gaps,
   engineer-spawn resource), never synthesized.

**Closed feedback loop (by design, #2628):** the Overseer's own write-back becomes its
own future recall evidence — `run_cycle → write_back_observation → record_observation
(store_episode)` … later `recall_episodic → parse_failure_signature → signals_from fold
→ RecurringSignature`. Deliberate stewardship memory, not an accidental echo.

---

## 2. The recurrence/idempotency gate — what it guarantees

`write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`). The emission caller
`write_back_observation` (`mod.rs:546-562`) computes the signature, then `peek`s:

- `peek` (`guardrails.rs:312-323`) returns `SuppressDuplicate` **only** when the
  signature is in `last_delivered` **and** `now - last < window_secs (900)`; otherwise
  `Deliver` (subject to the 5/hr cap).
- `commit` (`guardrails.rs:328-333`) runs **only after a successful store**
  (`mod.rs:554-556`) — a failed store never consumes the slot (correct
  fail-open-on-error shape).

Three properties follow directly from the type:

- **Process-local & volatile.** `last_delivered: HashMap<String,i64>`
  (`guardrails.rs:294`) is plain in-memory state, initialized empty in `new`
  (`:305`). Not persisted, not shared. A **restart** between two ticks erases the slot ⇒
  the identical observation re-delivers.
- **Windowed, not permanent.** Past 900 s the `now - last < window` test is false ⇒ the
  same signature re-delivers by design.
- **Commit-after-success.** Idempotent within a window; never wrongly suppresses a later
  write after an error.

The doc-comment states the contract exactly: a persistent condition is recorded "at most
once per window (never every tick)" (`mod.rs:520-523`). The gate is a **per-tick
anti-flood**, *not* a global once-only ledger. Re-persisting once per 900 s window, or
once per process lifetime, is the **specified** behavior.

`admit` = `peek` then `commit`-on-`Deliver` (`guardrails.rs:336-342`) — byte-identical to
the production `peek`+`commit` sequence, so the whisper/recall tests faithfully model the
real write-back path.

**Single delivery ⇒ single episode.** `record_observation` calls `store_episode`
**exactly once** per `Deliver` (`wiring.rs:1084-1089`), returning one `node_id`. No path
persists two episodes from one admitted tick. So `occurrences = 2` cannot arise from a
non-idempotent *episodic* write; it requires two separate admitted deliveries. (The
Lane-B non-idempotency concern from prior waves lives in semantic `store_fact`, not this
episodic write-back — out of scope, not the source of the `2×`.)

---

## 3. The 2× adjudication — decision matrix (every path to the 2nd episode)

| Producer of the 2nd episode | Gate behavior | Verdict |
|---|---|---|
| Daemon **restart** between ticks (`last_delivered` reset in `new`) | as designed (volatile state) | **real** — condition genuinely still holds |
| Same condition re-observed **> 900 s** later | as designed (window lapse) | **real** — measured world still stuck |
| **Distinct process/instance** writes it (no shared gate) | as designed (process-local) | **real** — independent honest observation |
| Same process, within 900 s, same signature | `SuppressDuplicate` → `Ok(None)` | duplicate **prevented** |
| Storage replay / superseded facts | episodic recall reads live rows only | **not** a source — no amplification |

Every `2×`-producing path is a **genuine second observation of a still-true condition.**
The gate's sole duty — stop within-window per-tick duplication — is performed correctly
(proven §4). **There is no emitter or gate defect behind the `2×`.**

---

## 4. Empirical proof — reran by me @ HEAD `3fac68a5`

`cargo test --lib -- overseer::tests_whisper::whisper_gate overseer::tests_memory_recall`
→ **34 passed; 0 failed** (first-hand, this wave). Load-bearing cases:

- `whisper_gate_suppresses_an_identical_whisper_within_the_window` — `sig` Deliver,
  Suppress in-window, re-Deliver past window ⇒ two episodes ⇒ recall `occurrences = 2`.
- `write_back_is_deduplicated_within_window` — two identical in-window ticks ⇒ **exactly
  one** episode persisted (proves in-window idempotency of the emission path).
- `write_back_persists_again_for_a_distinct_signature` / `tick_writes_observation_back_once`
  — once per signature per window; distinct signatures both persist.
- `recurring_signature_emitted_when_two_episodes_share_signature` /
  `…_not_emitted_for_single_occurrence` / `…_ignores_episodes_without_signature` /
  `recurring_signature_is_additive_not_replacing` / `…_problem_summary_is_sanitized` —
  `occurrences` is a faithful, threshold-2 (`signal.rs:362`), sanitized, additive tally.

---

## 5. Reconciliation with prior verdicts (no re-derivation)

| Prior wave | This wave (@ `3fac68a5`) |
|---|---|
| `primary_signature_assembly_emitter_and_2x_verdict_HEAD_f455c06d`: `2×` real signal; single fixed-provenance emitter; one episode per delivery | **CONFIRM** — re-read every cited line; all moved intact; reran guards (34 green) |
| `primary_signature_emitter_and_2x_semantics`: `2×` = recall episode count, not a gate counter | **CONFIRM** |
| `secondary_ooda_closure_and_deadzone_HEAD_3fac68a5`: `git diff f455c06d..HEAD -- src/` empty; both OODA arms non-closing; 2-vs-3 dead zone | **ADOPT** — source is byte-identical; the emitter/gate are correct, the defect is the missing closing edge (out of my scope) |
| Lane-B durability (`store_fact`) is the only recording concern; naïve `CallerKey` swap is a trap | **ADOPT** — out of scope; episodic write-back is correctly windowed, not the ratchet in question |

**No contradictions.** PRIMARY contribution this wave: a re-grounded, end-to-end
**named-emitter** pipeline map at current HEAD (①→⑧) proving the string has a single,
fixed-provenance, sanitized producer, and that `record_observation` stores **exactly one**
episode per admitted delivery — so the `2×` cannot be manufactured on the write path and
is necessarily a real re-observation.

---

## 6. Recommendation (diagnosis only — the blocked goals are OUT OF SCOPE)

**No change to `observation_signature`, `write_back_observation`, `write_back_gate`
(`WhisperGate`), `record_observation`, or `RECURRING_SIGNATURE_THRESHOLD`.** The assembly
is honest, provenance is fixed, and the gate is correct and idempotent within its window.
The `2×` is a truthful under-throughput signal that a blocked-goal cluster is genuinely
static. Per settled prior/secondary waves, remediation lies **elsewhere**: give
`WorkstreamCoverage` a **closing edge** (keyed on `GapItem.signature`, not the bare
`workstream-gap` dedup_key), insert a WHY-gated rung in `decide_blocked_goal` between
`Report` and the ≥3 escalation, and harden **Lane-B** semantic recording via a
count-in-content upsert (not the naïve `CallerKey` swap). **Do NOT** persist the write-back
gate, silence the `2×`, or merge the recurrence lanes.

---

## 7. Verification performed (@ HEAD `3fac68a5`)

- Re-read directly at HEAD: `mod.rs:286-304, 518-563, 1068-1089, 1258-1272, 1336,
  1353-1373`; `guardrails.rs:288-342`; `wiring.rs:952, 973-986, 1013-1030, 1076-1090`;
  `signal.rs:362, 459-470`.
- Confirmed `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`) and
  `admit == peek + commit-on-Deliver` (`guardrails.rs:336-342`).
- `cargo test --lib -- overseer::tests_whisper::whisper_gate overseer::tests_memory_recall`
  → **34 passed; 0 failed**.

**Bottom line:** the composite
`overseer-obs:…|workstream-gap|resource:engineer_spawn` is one `observation_signature`
write-back key produced by a single, fixed-provenance, sanitized emitter
(`mod.rs:1068-1073`); `record_observation` persists exactly one episode per admitted
delivery; the `2×` is `RecurringSignature.occurrences`, a faithful count of two distinct
persisted episodes. The `write_back_gate`'s process-local `last_delivered` + 900 s window
suppresses within-window same-process repeats (proven) and, by design, re-delivers past
the window / across restarts / across processes (proven). The `2×` is **real
re-observation, not a duplicate defect.**
