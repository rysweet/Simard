# Secondary Investigation — Self-observation nesting vs. true duplication; token classification (re-grounded to HEAD)

**Role:** SECONDARY investigator (patterns / self-observation nesting / token taxonomy / reconciliation).
**HEAD:** `bbddd23a` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Focus:** (1) Is the verbatim repetition of the `goal:blocked` set and the nested
`overseer-obs:…|overseer-obs:…` fragments intended self-observation re-ingestion or a
defect inflating the signature? (2) Which tokens are benign membership drift vs.
structurally load-bearing? (3) Reconcile prior `ai_working/investigation/` findings,
**re-grounded to current HEAD source** (not trusting the docs' own citations).

**Drift check (mandatory re-grounding):**
`git diff --name-only dea65df8..HEAD -- '*.rs'` → **only `src/overseer/tests_root_cause.rs`**
(a test file). `f1db90f4..HEAD` and `f9cefec1..HEAD` → **empty** for `.rs`. So every
load-bearing *source* line the prior waves cite is byte-identical at HEAD. I independently
re-read each one below; all verify **exactly**.

---

## 0. One-line verdict

The doubled `overseer-obs:…|overseer-obs:…` and the recurring `goal:blocked` membership are
**self-observation nesting (D1), not true duplication and not a counting bug.** The `×2` is
an **honest** Lane-A occurrence tally. Nested `overseer-obs:…` recalled fragments are the
**load-bearing / signature-inflating** tokens; `workstream-gap` and `resource:engineer_spawn`
are **benign membership drift** (fixed-literal dedup_keys, volatile fields confined to the
human summary). Fix the write-boundary self-feed and the missing convergence rung — **not**
the counter.

---

## 1. Re-grounded citation table (independently re-read at HEAD `bbddd23a`)

| Claim | Loc @ HEAD | Re-verified |
|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`format!("overseer-obs:{}", keys.join("\|"))` | `overseer/mod.rs:1068-1073` | ✅ exact |
| `write_back_observation` builds signature over **all** `problems`, **no** exclusion of recall-derived problems | `overseer/mod.rs:534-563` | ✅ exact (see §2) |
| `RecurringSignature` dedup_key = `sanitize_recalled(signature)` (an `overseer-obs:…` string); summary = `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` | `overseer/mod.rs:1353-1363` | ✅ exact — **verbatim match to the investigation-question string** |
| `orient` merges same-`dedup_key` signals into one problem; `RecurringSignature` only *raises priority* | `overseer/mod.rs:1200-1221` | ✅ exact |
| Recall→`RecurringSignature` emitted only at `occurrences >= RECURRING_SIGNATURE_THRESHOLD` | `overseer/signal.rs:455-470` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2` | `overseer/signal.rs:362` | ✅ exact |
| `workstream-gap` dedup_key = fixed literal; `{gaps.len()}` only in summary | `overseer/mod.rs:1368-1372` | ✅ exact |
| `resource:engineer_spawn` dedup_key = fixed literal; `{live}` only in summary | `overseer/mod.rs:1268-1272` | ✅ exact |
| `write_back_gate = WhisperGate::new(900, 5)` (in-memory 900 s window / 5-per-hr) | `overseer/mod.rs:299`; `:546-556` | ✅ exact |
| Within-window dedup test | `overseer/tests_whisper.rs:437` `whisper_gate_suppresses_an_identical_whisper_within_the_window` | ✅ present |

**Conclusion:** the prior investigation's source citations are sound at HEAD. This wave
**extends and sharpens**, it does not restart (per RECONCILIATION_LEDGER §0).

---

## 2. Nesting vs. true duplication — the mechanism, verified end-to-end

The doubled `overseer-obs:…|overseer-obs:…` is **self-observation feedback (D1)**, a
closed loop where the write-back's own output re-enters its own input:

1. **Assemble** — `observation_signature(problems)` set-hashes the whole tick's problem
   membership into `overseer-obs:<sorted|deduped dedup_keys>` (`mod.rs:1068-1073`).
2. **Persist** — `write_back_observation` stores that composite as an episode
   (`mod.rs:546-557`), gated only by the **in-memory** `WhisperGate(900,5)`.
3. **Recall** — a later tick recalls the episode; `signals_from` groups recalled episodes by
   `failure_signature` and emits `Signal::RecurringSignature{signature,occurrences}` at
   `≥2` (`signal.rs:455-470`). The `signature` here **is** the earlier `overseer-obs:…` string.
4. **Re-admit** — `classify_signal` turns it into a `Problem` whose **`dedup_key` is that
   `overseer-obs:…` string** (`mod.rs:1359`).
5. **Re-nest** — that problem is in `cycle.problems`, so the **next** `write_back_observation`
   passes it straight back into `observation_signature` with **no filter** (`mod.rs:534-563`).
   Its `overseer-obs:…` dedup_key becomes one of the joined keys → the composite now contains
   `overseer-obs:…|overseer-obs:…`. Each recall level nests one deeper.

**Why this is nesting and not per-token duplication (decisive structural proof):**
`orient` merges any two same-`dedup_key` signals into a single `Problem` (`mod.rs:1211`),
and `observation_signature`'s `keys.dedup()` collapses adjacent equals (`mod.rs:1071`).
Therefore **each family key can appear at most once per snapshot.** A literal
`workstream-gap|workstream-gap` (or repeated `overseer-obs:`) inside one composite is
**impossible from true duplication** — it can *only* arise from **nested recalled
`overseer-obs:…` fragments**, each a distinct string (it embeds its own `workstream-gap`)
that survives `dedup()`. The observed doubling is a positive fingerprint of the D1 self-feed.

**Intended vs. defect:** memory-backed cross-window recurrence *is* intended
(`signal.rs:449-453` design note) — recalling one's own prior observations is by design.
The **defect** is the absence of a **write-boundary exclusion** for recall-derived
`RecurringSignature` dedup_keys, which lets the meta-observation re-ingest itself and inflate
the signature. This is the classic *self-observation feedback* anti-pattern (PATTERNS.md).
**Fix seam:** filter recall-derived (`overseer-obs:` / `RecurringSignature`) keys out of the
set fed to `observation_signature` at `mod.rs:546` — a symptom-seam fix, orthogonal to the
counter.

---

## 3. Token classification — benign drift vs. load-bearing (re-grounded)

| Token | dedup_key mint @ HEAD | Volatile field | In signature? | Classification |
|---|---|---|---|---|
| `goal:blocked:<goal_id>` | `mod.rs` `format!("goal:blocked:{goal_id}")` | `consecutive_no_action`, `needs_review` (summary/priority only) | goal_id is a **stable ID** | **Load-bearing** — the persistent membership set that IS the problem |
| `overseer-obs:…` (nested) | `mod.rs:1359` `sanitize_recalled(signature)` | `occurrences` (summary only) | the whole recalled string enters | **Load-bearing / signature-inflating** — the D1 artifact; the token that manufactures the doubling |
| `workstream-gap` | `mod.rs:1371` fixed literal | `gaps.len()` (summary only) | fixed literal | **Benign membership drift** — appears/disappears with coverage, never forks per-token identity |
| `resource:engineer_spawn` | `mod.rs:1270` fixed literal | `{live}` (summary only) | fixed literal | **Benign membership drift / telemetry** — NOT a new signal source |

**`resource:engineer_spawn` re-grounded (the historically drifty one):** despite being a
live-count signal, `{live}` lands **only** in the summary `"elevated engineer spawn ({live}
live)"` (`mod.rs:1271`); the dedup_key is the fixed string. No volatile component leaks into
the signature. It is an *effect/early-warning* of saturation (`ENGINEER_SPAWN_THRESHOLD=8`
equals the `max_concurrent_engineers=8` admission cap — a **state coupling, not a data-flow
edge** to `goal:blocked`). Classify **benign**, confirmed at HEAD. Do not spin a
spawn-failure hypothesis.

**Nuance (from tertiary lane-isolation, re-grounded):** because `observation_signature` is a
**set-hash over the whole tick's membership**, `workstream-gap`/`engineer_spawn` are benign
as *tokens* yet, as *co-members*, they **fork the composite Lane-A identity** under drift.
That fork is confined to the self-fed **advisory Lane-A**; **Lane-B escalation keys on the
per-problem `dedup_key`** and is immune. So membership drift is a benign-but-latent
*precision* defect in Lane-A, never a correctness defect in escalation.

---

## 4. Reconciliation with prior waves (extend, don't restart)

- **RECONCILIATION_LEDGER §0/§4 holds at HEAD:** root-cause analysis is sound; the one
  correction is the §6.2b remedy **trap** — literal `store_fact_with_caller_key(root_cause_signature)`
  collapses recall to 1 forever (`DedupMode::CallerKey`), making the escalation rung
  (`mod.rs:1613`) dead code. Use **count-in-content upsert** instead. *(Not my seam, but I
  re-confirm it stands — do not adopt the literal one-liner.)*
- **Three-defect geometry D1/D2/D3 holds.** My focus is **D1** (self-observation write-back
  nesting), which I re-verify live at `mod.rs:534-563` + `mod.rs:1359` + `signal.rs:455-470`.
  The write boundary has **no recall-derived exclusion** — defect present at HEAD.
- **Token-classification wave (`…0289572e`) holds unchanged** at HEAD: three token groups are
  co-aggregated members of one composite, not three signatures; blocks share one common root
  cause (unwired convergence rung in a `[2,3)` dead zone); gap/spawn are §11 benign drift.
- **`×2` counting locus (reconciled):** Lane-A observation episodes drive the visible `×2`
  (`RECURRING_SIGNATURE_THRESHOLD=2`, `signal.rs:362,463`); Lane-B root-cause occurrences drive
  escalation (`RECURRENCE_ESCALATION_THRESHOLD=3`). The `×2` lands in the `[2,3)` gap. The
  counter is honest; audit the closing action.

---

## 5. Patterns / anti-patterns

- **Meta-pattern (holds):** *the recurrence count is honest — audit the closing action, not the counter.*
- **Anti-pattern present — Self-observation feedback (D1):** write-back re-ingests its own
  recall-derived `overseer-obs:` tokens because the write boundary (`mod.rs:546`) does not
  exclude `RecurringSignature`-derived dedup_keys. *This directly answers the nesting question.*
- **Anti-pattern — Recurrence dead zone:** `×2 ∈ [2,3)` with no auto-remediation rung.
- **Benign membership drift (§11):** `workstream-gap`, `resource:engineer_spawn`.

## 6. Integration points

`overseer/mod.rs:534-563` (write boundary — D1 fix seam) · `:1068-1073` (set-hash) ·
`:1200-1221` (orient merge/dedup — the "at most once per snapshot" invariant) ·
`:1353-1372` (RecurringSignature / workstream-gap classify) · `:1268-1272` (engineer_spawn) ·
`signal.rs:362,455-470` (recall→emit, threshold) · `tests_whisper.rs:437` (within-window dedup).

## 7. Questions for verification phase

1. **Confirm the D1 fix seam:** exclude recall-derived (`overseer-obs:`/`RecurringSignature`)
   dedup_keys from the set passed to `observation_signature` at `mod.rs:546` — verify it stops
   the nesting without suppressing legitimate first-order recurrence detection.
2. **Confirm no `.rs` behavioral drift** beyond `tests_root_cause.rs` since `dea65df8`
   (already checked; re-assert at merge time).
3. **Confirm** `resource:engineer_spawn` at snapshot B came from real elevated telemetry
   (convergence class) vs. a one-off spike (incidental drift) — a goal-board/telemetry check,
   not a source check.
4. **Confirm** the within-window dedup (`WhisperGate(900,5)`) is the *only* thing gating the
   `×2` and that daemon restart resets in-memory gate state (would re-emit sooner than 900 s).
