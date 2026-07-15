# Primary Deep Dive — Signature Assembly / Emitter Provenance & the 2× Duplicate-vs-Re-Observation Verdict

**Role:** PRIMARY investigator.
**HEAD:** `f455c06d` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Investigation question:** the recurring composite signature
`overseer-obs:goal:blocked:…|…|workstream-gap|resource:engineer_spawn` reported as
**"recurring signature seen 2× in cognitive memory (…)"**.
**Focus (this wave):** (1) signature **assembly / emitter provenance** — where the
string is built, by whom, from what inputs; (2) the **2× verdict** — is the second
occurrence a genuine **re-observation** (real signal) or a **duplicate** (recording
defect)?
**Method:** re-read every load-bearing line at current HEAD `f455c06d` (the tree
moved since prior waves at `f1db90f4`/`b9f99879`; I re-verified, did not trust prior
citations), confirmed the test-harness `admit` faithfully models the production
`peek`+`commit` path, and reran the two load-bearing guard suites.

---

## 0. One-line verdict

The `2×` is **real re-observation signal, not a duplicate/recording defect.** The
emitter string is a deterministic set-hash of the observed problem membership; the
`2×` is `RecurringSignature.occurrences`, a faithful tally of **two distinct
persisted episodes** that share that signature. The only anti-duplication mechanism
on the write-back path — the `write_back_gate` WhisperGate (900 s window, cap 5/hr,
**process-local** `last_delivered`) — provably suppresses *within-window, same-process*
repeats. A second persisted episode therefore requires a **genuine** trigger: a
`>900 s` gap, a daemon **restart** (the gate resets), or a **distinct process**.
Each is an honest second observation of a still-true condition. **Do not "fix" the
gate, the emitter, or the counter** — collapsing any of them suppresses a truthful
under-throughput signal.

---

## 1. Emitter provenance — who builds the string, from what (verified @ `f455c06d`)

The literal `overseer-obs:…` string has exactly **one** producer, and the "recurring
signature seen 2×" wrapper has exactly **one** producer:

| Concern | Producer | File:line (verified) |
|---|---|---|
| Composite emitter `"overseer-obs:" + sorted∘deduped dedup_keys.join("\|")` | `observation_signature(problems)` | `mod.rs:1068-1073` |
| Token dedup_keys (`goal:blocked:{id}`, `workstream-gap`, `resource:engineer_spawn`) | `classify_signal` arms | `mod.rs:1359,1371` (+ resource/blocked arms) |
| Write-back caller + gate | `write_back_observation` | `mod.rs:534-563` |
| Fixed provenance persist (`[sig:…]`, `source="overseer"`, metadata=signature only) | `record_observation` | `wiring.rs:1076-1091` |
| Recall parse-back of `[sig:…]` | `parse_failure_signature` / `recall_episodic` | `wiring.rs:976-986,1013-1030` |
| Recall fold → `RecurringSignature{occurrences}` (≥2) | `signals_from` | `signal.rs:455-470` (+ threshold `:362`) |
| "recurring signature seen {occurrences}× in cognitive memory ({signature})" | `classify_signal` RecurringSignature arm | `mod.rs:1353-1363` |

**Assembly integrity (no fabrication in the string):**
1. **Set-hash, sorted + deduped.** `observation_signature` does
   `keys.sort_unstable(); keys.dedup();` before `join("|")` (`mod.rs:1069-1072`), so the
   `|`-list *inside* one `overseer-obs:` block is unique and order-independent. The
   many repeated `overseer-obs:…` blocks in the question string are **whole recalled
   episodes concatenated by the recall query**, i.e. the `occurrences` tally — **not**
   intra-signature duplication.
2. **Fixed provenance.** `record_observation` hardcodes `OVERSEER_SOURCE_LABEL =
   "overseer"` (`wiring.rs:952,1088`); source is never caller-chosen. Metadata is a
   validated JSON object carrying only `{"signature": …}` — no secrets/tokens/env.
3. **Sanitized content.** Every problem summary and the whole line pass through
   `sanitize_recalled` in `observation_content` (`mod.rs:1079-1089`), and the recall
   summary is `sanitize_recalled`-cleaned again at the classify boundary
   (`mod.rs:1359-1362`) — defence-in-depth against recalled-text re-entry in a
   multi-writer graph.
4. **Honest membership.** The signature is stable **iff** the observed problem set is
   stable; the tokens are pure projections of observed state (blocked goals, workstream
   gaps, engineer-spawn resource), not synthesized.

**Closed feedback loop (by design, #2628):** the Overseer's own write-back becomes
its own future recall evidence — `run_cycle → write_back_observation →
record_observation(store_episode)` … later `recall_episodic → parse_failure_signature
→ signals_from fold → RecurringSignature`. This is deliberate stewardship memory, not
an accidental echo.

---

## 2. The 2× adjudication — duplicate vs re-observation (core of this wave)

### 2a. The only dedup mechanism, and exactly what it guarantees

`write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`). `peek`
(`guardrails.rs:312-323`) returns `SuppressDuplicate` **only** when the signature is
present in `last_delivered` **and** `now - last < window_secs (900)`; else `Deliver`
(subject to the 5/hr cap). `commit` (`:328-333`) is called **only after a successful
store** (`mod.rs:549-557`). Three properties follow directly from the type:

- **Process-local & volatile.** `last_delivered: HashMap<String,i64>`
  (`guardrails.rs:294`) is plain in-memory state, initialized empty in `new`
  (`:305`). Not persisted, not shared. A **restart** between two ticks erases the
  slot ⇒ the identical observation re-delivers.
- **Windowed, not permanent.** Past 900 s the `now - last < window` test is false ⇒
  the same signature re-delivers by design.
- **Commit-after-success.** A failed store never consumes the slot, so it cannot
  wrongly suppress a later write (correct fail-open-on-error shape).

The doc-comment states the contract exactly: a persistent condition is recorded "at
most once per window (never every tick)" (`mod.rs:520-523`). The gate is a **per-tick
anti-flood**, *not* a global once-only ledger. Re-persisting once per 900 s window,
or once per process lifetime, is the **specified** behavior.

`admit` (the test convenience) is `peek` then `commit`-on-Deliver
(`guardrails.rs:336-342`) — identical to the production sequence, so the whisper
tests below faithfully model the real write-back path.

### 2b. Single delivery ⇒ single episode (no artificial doubling)

`record_observation` calls `store_episode` **exactly once** per Deliver
(`wiring.rs:1086-1090`), returning one `node_id`. There is **no path** where one
gate-admitted tick persists two episodes. So `occurrences = 2` cannot arise from a
non-idempotent *episodic* write; it requires two separate admitted deliveries. (The
Lane-B non-idempotency concern from prior waves lives in semantic `store_fact`, not
this episodic write-back — out of scope and not a source of the `2×`.)

### 2c. Empirical proof (reran @ HEAD `f455c06d`)

- `overseer::tests_whisper::whisper_gate_suppresses_an_identical_whisper_within_the_window`
  (`tests_whisper.rs:437-458`, virtual clock): `sig-a` Deliver@0, **Suppress@300**,
  **Suppress@899**, **Deliver@901** — the same signature re-delivers **only** past the
  window. ⇒ two persisted episodes of one signature ⇒ recall `occurrences = 2`.
- `overseer::tests_memory_recall::write_back_is_deduplicated_within_window`
  (`:797-817`): two identical ticks inside the window ⇒ **exactly one** episode
  persisted (`memory_writes` 1 then 0). Confirms in-window suppression.
- `tick_writes_observation_back_once` (`:779`) and
  `write_back_persists_again_for_a_distinct_signature` (`:820`): once per signature per
  window; distinct signatures both persist.
- `recurring_signature_emitted_when_two_episodes_share_signature` /
  `…_not_emitted_for_single_occurrence` /
  `recurring_signature_ignores_episodes_without_signature`: `occurrences` is a faithful
  episode tally against threshold **2** (`signal.rs:362`).

Suite results at HEAD `f455c06d`:
`cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed, 0 failed**;
`cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**.

### 2d. Decision matrix — every path that yields the 2nd episode

| Producer of the 2nd episode | Gate behavior | Verdict |
|---|---|---|
| Daemon **restart** between ticks (`last_delivered` reset in `new`) | as designed (volatile state) | **real** — condition genuinely still holds |
| Same condition re-observed **> 900 s** later | as designed (window lapse) | **real** — measured world still stuck |
| **Distinct process/instance** writes it (no shared gate) | as designed (process-local) | **real** — independent honest observation |
| Same process, within 900 s, same signature | `SuppressDuplicate` → `Ok(None)` | duplicate **prevented** |
| Storage replay / superseded facts | episodic recall reads live rows only | **not** a source — no amplification |

Every `2×`-producing path is a **genuine second observation of a still-true
condition**. The gate's sole duty — stop within-window per-tick duplication — is
performed correctly (proven). **There is no emitter or gate defect behind the `2×`.**

---

## 3. Reconciliation with prior verdicts (no re-derivation)

| Prior wave | This wave (@ `f455c06d`) |
|---|---|
| `primary_signature_assembly_and_2x_verdict_HEAD_f1db90f4`: `2×` real signal; process-local 900 s gate; assembly honest | **CONFIRM** — re-verified every cited line moved intact to HEAD; reran both suites |
| `primary_signature_emitter_and_2x_semantics`: `2×` = recall episode count, not a gate counter | **CONFIRM** |
| `tertiary_lane_isolation_…`: `×2` is **intended signal, not a recording defect**; Lane A ⊥ Lane B | **CONFIRM** — reached independently from the emitter-provenance + single-episode-per-delivery angle |
| Lane-B durability (`store_fact`) is the only recording concern; naïve caller-key swap is a trap | **ADOPT** — out of scope; episodic write-back is correctly windowed, not the ratchet in question |

**No contradictions.** New PRIMARY contribution this wave: a provenance-first audit
proving the emitter has a single, fixed-provenance, sanitized producer, and that
`record_observation` stores **exactly one** episode per admitted delivery — so the
`2×` cannot be manufactured on the write path and is necessarily a real re-observation.

---

## 4. Recommendation (diagnosis only — the blocked goals are OUT OF SCOPE)

**No change to `observation_signature`, `write_back_observation`, `write_back_gate`
(WhisperGate), `record_observation`, or `RECURRING_SIGNATURE_THRESHOLD`.** The
assembly is honest, provenance is fixed, and the gate is correct. The `2×` is a
truthful under-throughput signal that a blocked-goal cluster is genuinely static.
Per settled prior waves, remediation lies **elsewhere**: give `WorkstreamCoverage` a
**closing edge**, reconcile issue-closed goals out of `Blocked` (fail loud when
completion evidence is absent), and harden **Lane-B** semantic recording via a
caller-key upsert (not the naïve swap). **Do NOT** persist the write-back gate,
silence the `2×`, or merge the recurrence lanes.

---

## 5. Verification performed (@ HEAD `f455c06d`)

- Re-read: `mod.rs:286-304, 518-563, 1064-1089, 1345-1373`;
  `guardrails.rs:291-342`; `wiring.rs:952, 973-986, 1013-1030, 1076-1091`;
  `signal.rs:362, 440-470`.
- Confirmed `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`) and
  `admit == peek + commit-on-Deliver` (`guardrails.rs:336-342`).
- `cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed, 0 failed**.
- `cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**
  (incl. `write_back_is_deduplicated_within_window`,
  `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
  `recurring_signature_emitted_when_two_episodes_share_signature`).

**Bottom line:** the composite `overseer-obs:…|workstream-gap|resource:engineer_spawn`
is one `observation_signature` write-back key produced by a single, fixed-provenance,
sanitized emitter; `record_observation` persists exactly one episode per admitted
delivery; the `2×` is `RecurringSignature.occurrences`, a faithful count of two
distinct persisted episodes. The `write_back_gate`'s process-local `last_delivered` +
900 s window suppresses within-window same-process repeats (proven) and, by design,
re-delivers past the window / across restarts / across processes (proven). The `2×`
is **real re-observation, not a duplicate defect.**
