# Primary Deep Dive — Signature Emitter / Token Assembly & 2× Counter Semantics (WhisperGate window, recall fold)

**Role:** PRIMARY investigator.
**HEAD:** `3fac68a5` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Investigation question:** the recurring composite signature
`overseer-obs:goal:blocked:…|…|workstream-gap|resource:engineer_spawn` reported as
**"recurring signature seen 2× in cognitive memory (…)"**.
**Focus (this wave):** the **signature emitter / token assembly** (how the string is
built, from which tokens, by whom) and the **2× counter semantics** — specifically the
**WhisperGate window** arithmetic and the **recall fold** that produces `occurrences`.
**Method:** treat prior citations as untrusted; re-read every load-bearing line at
current HEAD `3fac68a5`; confirm src did not drift since the last primary wave
(`f455c06d`); re-run the two guard suites.

---

## 0. One-line verdict

The `2×` is **real re-observation signal, not a duplicate/recording defect.** The
emitter is a single deterministic **set-hash** of observed-problem membership. The
`2×` is `RecurringSignature.occurrences`, produced by the **recall fold** in
`signals_from` (a count of two distinct persisted episodes sharing the signature) —
it is a *separate counter* from anything the WhisperGate maintains. The gate's
process-local, half-open **[0, 900) s** dedup window provably suppresses within-window,
same-process repeats; a second persisted episode therefore requires an honest trigger
(>900 s gap, daemon restart, or distinct process). **Do not touch the emitter, the
gate, or the threshold** — each is correct; suppressing any collapses a truthful
under-throughput signal. No source change this wave.

---

## 1. Drift check — the tree has not moved since the last primary wave

`git diff --stat f455c06d..HEAD -- src/` is **empty**; the only commits since
`f455c06d` are docs (`d6ba8b25`, `3fac68a5`). I still re-read the load-bearing lines
directly (below) rather than inheriting citations. Every cited line is **byte-identical**
at HEAD `3fac68a5`. This is a verification wave that re-grounds the standing verdict on
the current tree and adds a fresh emitter/token-assembly + counter-semantics analysis.

---

## 2. Emitter / token assembly — verified @ `3fac68a5`

| Concern | Producer | File:line (verified this wave) |
|---|---|---|
| Composite emitter `"overseer-obs:" + sorted∘deduped dedup_keys.join("\|")` | `observation_signature(problems)` | `mod.rs:1068-1073` |
| Token `workstream-gap` (WorkstreamGap dedup_key) | `classify_signal` | `mod.rs:1371` |
| Token `goal:blocked:{id}` / `resource:engineer_spawn` | `classify_signal` blocked/resource arms | `mod.rs` (same match) |
| Write-back caller + gate | `write_back_observation` | `mod.rs:534-563` |
| Fixed-provenance persist (`[sig:…]`, `source="overseer"`, metadata=signature only) | `record_observation` | `wiring.rs:1076-1091` (`OVERSEER_SOURCE_LABEL` `:952`) |
| Recall parse-back of `[sig:…]` | `parse_failure_signature` | `wiring.rs:976` |
| Recall fold → `occurrences` (≥2) | `signals_from` | `signal.rs:455-470` (threshold `:362`) |
| `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` | `classify_signal` RecurringSignature arm | `mod.rs:1353-1363` |

**Token-assembly integrity (new detail this wave):**

1. **Set-hash, not a multiset.** `observation_signature` does
   `keys.sort_unstable(); keys.dedup();` **before** `join("|")` (`mod.rs:1069-1072`).
   Consequence: within a single `overseer-obs:` block the `|`-token list is
   **order-independent and duplicate-free**. Therefore the *many repeated
   `overseer-obs:…` blocks* in the question string are **not** intra-signature
   duplication — they are **whole recalled episodes concatenated by the recall
   query**, i.e. the visual manifestation of the `occurrences` tally.
2. **Tokens are pure projections of observed state.** Each token is a `Problem.dedup_key`
   (`mod.rs:1069`); `dedup_key`s are assigned by `classify_signal` arms
   (`workstream-gap` at `mod.rs:1371`, blocked/resource elsewhere in the same match).
   The membership is stable **iff** the observed problem set is stable — nothing is
   synthesized.
3. **Fixed provenance + sanitation.** `source_label` is hardcoded `"overseer"`
   (`wiring.rs:952,1088`), never caller-chosen; metadata is a validated JSON object
   carrying **only** `{"signature": …}`. Content is `sanitize_recalled`-cleaned in
   `observation_content` (`mod.rs:1079-1089`) and again at the classify boundary
   (`mod.rs:1359-1362`) — defence-in-depth for a multi-writer graph.

---

## 3. 2× counter semantics — two independent counter systems (core contribution)

The critical clarification this wave: **the `2×` does not come from any counter inside
the WhisperGate.** There are two disjoint counter systems, and only one surfaces as the
`2×`.

### 3a. Counter system A — the recall fold (produces the `2×`)

`signals_from` (`signal.rs:455-470`) builds `counts: BTreeMap<&str, u32>`, incrementing
once per recalled episode whose `failure_signature` (parsed from the `[sig:…]` marker)
equals the signature. When `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (=2,
`signal.rs:362`) it emits `Signal::RecurringSignature { signature, occurrences }`.
`occurrences` is therefore a **faithful tally of distinct persisted episodes** sharing
the signature — the `2×` the report renders. Each episode contributes **+1 exactly
once** (`signal.rs:459`); a single persisted row (one `store_episode` node_id,
`wiring.rs:1086-1090`) cannot be double-counted.

### 3b. Counter system B — the WhisperGate internals (never surface as the `2×`)

`WhisperGate` holds `last_delivered: HashMap<String,i64>` and `deliveries: Vec<i64>`
(`guardrails.rs:294-295`). These drive **suppression decisions only** — they are never
read by the recall fold and never rendered to the operator. They gate *how often an
episode may be written*, indirectly bounding how fast counter A can grow; they are not
counter A.

**Why this matters:** a reader could wrongly assume the `2×` is a gate "duplicate
counter" and try to reset/persist it. It is not. Persisting the gate (system B) would
merely throttle honest re-observations feeding system A — hiding a true signal.

### 3c. The window arithmetic — exact half-open boundary

`peek` (`guardrails.rs:312-323`) suppresses **iff** the signature is in `last_delivered`
**and** `now - last < window_secs` (strict `<`, `guardrails.rs:314`). With
`window_secs = 900` (`mod.rs:299`) this is a **half-open [0, 900) s** suppression window:

- gap `< 900 s` → `SuppressDuplicate` (no episode; counter A does not advance)
- gap `== 900 s` → `900 < 900` is **false** → `Deliver` (episode persisted; counter A +1)
- gap `> 900 s` → `Deliver`

The test confirms the boundary exactly (virtual clock): `Deliver@0`, `Suppress@300`,
`Suppress@899`, `Deliver@901`
(`tests_whisper::whisper_gate_suppresses_an_identical_whisper_within_the_window`).
`commit` runs **only after a successful store** (`mod.rs:555-556`), so a failed write
never consumes the slot (fail-open-on-error). `admit` = `peek` + `commit`-on-`Deliver`
(`guardrails.rs:336-342`), identical to the production `peek`/`commit` sequence — so the
whisper tests faithfully model the real write-back path.

### 3d. Decision matrix — every path that advances counter A to 2

| Producer of the 2nd episode | Gate (system B) behavior | Verdict |
|---|---|---|
| Daemon **restart** between ticks (`last_delivered` reset empty in `new`, `guardrails.rs:305`) | as designed (volatile, process-local) | **real** — condition genuinely still holds |
| Same condition re-observed **≥ 900 s** later | as designed (half-open window lapses) | **real** — world still stuck |
| **Distinct process/instance** (no shared gate) | as designed (process-local `HashMap`) | **real** — independent honest observation |
| Same process, same signature, gap `< 900 s` | `SuppressDuplicate` → `Ok(None)` (`mod.rs:559-561`) | duplicate **prevented** — counter A does **not** advance |
| Storage replay / superseded facts | episodic recall reads live rows only | **not** a source — no amplification |

Every path that reaches `occurrences = 2` is a **genuine second observation of a
still-true condition**. There is no emitter or gate defect behind the `2×`.

---

## 4. Empirical re-run @ HEAD `3fac68a5`

- `cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed, 0 failed**
  (suppress-within-window boundary + per-hour cap).
- `cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**, incl.
  `write_back_is_deduplicated_within_window`,
  `tick_writes_observation_back_once`,
  `write_back_persists_again_for_a_distinct_signature`,
  `recurring_signature_emitted_when_two_episodes_share_signature`,
  `recurring_signature_not_emitted_for_single_occurrence`,
  `recurring_signature_ignores_episodes_without_signature`,
  `recurring_signature_is_additive_not_replacing`,
  `recurring_signature_problem_summary_is_sanitized`.

These jointly prove: (a) within-window same-process repeats persist **one** episode
(counter A stays 1); (b) a distinct signature persists independently; (c) `occurrences`
is a threshold-2 episode tally, additive and sanitized.

---

## 5. Reconciliation with prior verdicts (no contradictions)

| Prior wave | This wave (@ `3fac68a5`) |
|---|---|
| `primary_signature_assembly_emitter_and_2x_verdict_HEAD_f455c06d`: `2×` real; process-local 900 s gate; assembly honest; one episode per delivery | **CONFIRM** — re-verified every cited line unchanged (src diff since `f455c06d` is empty); reran both suites |
| `primary_signature_emitter_and_2x_semantics`: `2×` = recall episode count, not a gate counter | **CONFIRM + EXTEND** — made the two-counter-system distinction (fold vs gate) and the exact half-open [0,900) boundary explicit |
| tertiary lane-isolation: `×2` is intended signal, not a recording defect; Lane A ⊥ Lane B | **CONFIRM** (reached independently from the token-assembly + fold angle) |
| Lane-B (`store_fact`) durability is the only recording concern; naïve caller-key swap is a trap | **ADOPT** — out of scope; episodic write-back is correctly windowed |

**New PRIMARY contribution this wave:** (1) explicit **two-counter-system** framing —
the `2×` is the recall-fold `occurrences` (system A), *not* a WhisperGate internal
(system B), pre-empting a plausible "reset the dedup counter" misfix; (2) the exact
**half-open [0, 900) s** window boundary with the strict-`<` derivation and matching
`Deliver@901`/`Suppress@899` evidence; (3) a fresh drift check proving the standing
verdict holds byte-for-byte at current HEAD.

---

## 6. Recommendation (diagnosis only — the blocked goals themselves are OUT OF SCOPE)

**No change** to `observation_signature`, `write_back_observation`, `write_back_gate`
(`WhisperGate`), `record_observation`, the recall fold, or
`RECURRING_SIGNATURE_THRESHOLD`. The emitter is a single fixed-provenance, sanitized
set-hash; the `2×` is an honest recall-fold tally of two genuine episodes; the gate's
half-open window is correct. The `2×` is a truthful under-throughput signal that a
blocked-goal cluster is genuinely static. Per settled prior waves, remediation lies
**elsewhere**: give `WorkstreamCoverage` a **closing edge**, reconcile issue-closed
goals out of `Blocked` (fail loud when completion evidence is absent), and harden
**Lane-B** semantic recording via a caller-key upsert (not the naïve swap). **Do NOT**
persist the write-back gate, silence the `2×`, or merge the recurrence lanes.

---

## 7. Verification performed (@ HEAD `3fac68a5`)

- `git diff --stat f455c06d..HEAD -- src/` → empty (no source drift).
- Re-read: `mod.rs:286-304, 518-563, 1064-1089, 1345-1373`;
  `guardrails.rs:288-343`; `wiring.rs:952, 976, 1076-1091`; `signal.rs:355-372, 440-470`.
- Confirmed `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`), strict-`<`
  suppression (`guardrails.rs:314`), commit-after-success (`mod.rs:555-556`),
  `admit == peek + commit-on-Deliver` (`guardrails.rs:336-342`),
  `occurrences` fold with threshold 2 (`signal.rs:456-467, 362`).
- `cargo test --lib overseer::tests_whisper::whisper_gate` → **2 passed, 0 failed**.
- `cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**.

**Bottom line:** the composite `overseer-obs:…|workstream-gap|resource:engineer_spawn`
is one deterministic set-hash emitted by a single fixed-provenance, sanitized producer.
The `2×` is the recall-fold `occurrences` — a faithful count of two distinct persisted
episodes — and is **independent of** the WhisperGate's own suppression counters. The
gate's process-local, half-open [0, 900) s window suppresses within-window same-process
repeats (proven) and, by design, re-delivers at ≥900 s / across restarts / across
processes (proven). The `2×` is **real re-observation, not a duplicate defect.**
