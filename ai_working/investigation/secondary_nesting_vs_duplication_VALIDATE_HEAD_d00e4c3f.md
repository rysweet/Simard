# Secondary (VALIDATE-not-rederive) — D1 self-observation nesting vs. per-token duplication; landed-fix status

**Role:** SECONDARY investigator (patterns / dedup_key + `keys.dedup()` structural proof / prior-art & landed-fix reconciliation).
**HEAD:** `d00e4c3f` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Mandate:** *Validate*, not re-derive. Re-ground the D1-vs-duplication classification and the landed-fix verdict to **current** HEAD (prior waves were grounded at `dea65df8` / `bbddd23a` / `cc55a6fb`).

---

## 0. One-line verdicts

1. **D1, not duplication.** The doubled `overseer-obs:…|overseer-obs:…` and literal `|workstream-gap|workstream-gap|` in the raw cognitive-memory recall are the **positive fingerprint of D1 self-observation nesting**, and are **structurally impossible** to produce by per-token duplication inside a single signature. **Re-confirmed at HEAD.**
2. **Nothing landed.** No production `.rs` remediation has merged since `dea65df8`. The only `.rs` change to HEAD is `src/overseer/tests_root_cause.rs` (a **test**, +99 lines). Every load-bearing defect is **live at HEAD**. The `×2` is an **honest** re-observation tally, not a dedup/replay/collision bug.

---

## 1. Drift re-grounding (mandatory)

```
git diff --name-only dea65df8..HEAD -- '*.rs'   → src/overseer/tests_root_cause.rs   (ONLY)
git diff --name-only cc55a6fb..HEAD -- '*.rs'   → (empty)
git diff --stat  dea65df8..HEAD -- 'src/'       → tests_root_cause.rs | 99 ++++ (1 file)
```

Every load-bearing **source** line the prior waves cite is byte-identical at HEAD. Independently re-read below.

| Claim | Loc @ HEAD `d00e4c3f` | Re-verified |
|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`format!("overseer-obs:{}", keys.join("\|"))` | `overseer/mod.rs:1068-1073` | ✅ exact |
| `orient` merges same-`dedup_key` signals into ONE problem (`.find(\|p\| p.dedup_key == key)`) | `overseer/mod.rs:1211-1213` | ✅ exact |
| `write_back_observation(&cycle.problems)` — builds signature over **all** problems, **no** recall-derived exclusion | `overseer/mod.rs:534-546`; called `wiring.rs:301` | ✅ exact |
| `write_back_gate = WhisperGate::new(900, 5)` (in-memory 900 s / 5-per-hr) | `overseer/mod.rs:299`; peek `:548`, commit `:556` | ✅ exact |
| `RecurringSignature` dedup_key = `sanitize_recalled(signature)` (an `overseer-obs:…` string); summary = `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` | `overseer/mod.rs:1359-1362` | ✅ exact — **verbatim match to the investigation-question string** |
| `workstream-gap` dedup_key = fixed literal; `{gaps.len()}` only in summary | `overseer/mod.rs:1371` | ✅ exact |
| `resource:engineer_spawn` dedup_key = fixed literal | `overseer/mod.rs:1270` | ✅ exact |
| `record_occurrence` uses **non-idempotent** `store_fact` (ratchet) | `overseer/mod.rs:1034` | ✅ **NOT** switched |
| Escalation gate `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `overseer/signal.rs:362`, `:463` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` | ✅ exact |
| `root_cause_signature` helper exists | `overseer/root_cause.rs:53` | ✅ exact |

**No stale citations.** Prior-wave line numbers hold at HEAD (engineer_spawn moved `1268→1270`, RecurringSignature arm `1353→1353`, otherwise identical).

---

## 2. D1 nesting vs. per-token duplication — the decisive structural proof (re-verified)

Two invariants at HEAD jointly make per-token duplication **impossible** within a single composite signature:

- **Invariant A — full dedup in the signature.** `observation_signature` does `keys.sort_unstable(); keys.dedup();` (`mod.rs:1070-1071`). Because sort makes equal keys adjacent, `dedup()` removes **all** duplicates → **each unique `dedup_key` appears at most once** per composite.
- **Invariant B — merge before signature.** `orient` folds any two same-`dedup_key` signals into a single `Problem` (`mod.rs:1211-1213`), so the `problems` slice handed to `observation_signature` **never** carries two problems with equal `dedup_key` in the first place.

**Consequence:** a single-level `overseer-obs:` signature can never contain `X|X`. Therefore the observed `|workstream-gap|workstream-gap|` and `overseer-obs:…|overseer-obs:…` adjacency in the **raw recall dump** can *only* arise from **distinct nested `overseer-obs:…` strings**, each a different full string (each embedding its own `workstream-gap`) that is unequal and thus survives `dedup()`. That is the **D1 self-observation nesting fingerprint** — not a duplication bug. (The raw investigation-question string is a concatenation of many recalled memory keys/episodes, so the doubling reflects accrued nested recall, not a malformed single signature.)

**The self-feed loop is intact and unguarded at HEAD:**
1. `observation_signature(problems)` → `overseer-obs:<sorted|deduped keys>` (`mod.rs:1068-1073`).
2. `write_back_observation` persists it as an episode, gated only by in-memory `WhisperGate(900,5)` (`mod.rs:546-556`).
3. Later tick recalls it; `signal.rs:463` emits `RecurringSignature{signature,occurrences}` at `≥2`.
4. `classify_signal` mints a `Problem` whose **`dedup_key` is that `overseer-obs:…` string** (`mod.rs:1359`).
5. `wiring.rs:301` passes **all** `cycle.problems` (including step 4) back into `write_back_observation` with **no exclusion filter** (`mod.rs:534-546`) → the `overseer-obs:…` key nests into the next composite. Each recall level nests one deeper.

**D1 fix seam (unchanged from prior waves, still valid):** exclude recall-derived (`overseer-obs:` / `RecurringSignature`) dedup_keys from the set fed to `observation_signature` at `mod.rs:546`. Symptom-seam fix, orthogonal to the counter.

---

## 3. Landed-fix status (the reconciliation half of my mandate)

- **No production remediation merged.** Only `tests_root_cause.rs` changed since `dea65df8`. `record_occurrence` still calls `store_fact` at `mod.rs:1034` (ratchet unfixed); the write boundary at `mod.rs:534-546` still has no recall exclusion (D1 unfixed); escalation gate at `mod.rs:1613` unchanged. All prior D1/D2/D3 defects are **live at HEAD**.
- **The §6.2b remedy trap still stands as a warning.** The committed CONSOLIDATED_FINDINGS §6.2b one-liner — swap `store_fact` → `store_fact_with_caller_key(root_cause_signature(...))` — remains a **trap**: `DedupMode::CallerKey` keeps exactly one live fact per key, `recall_occurrences` reads only live facts, and `root_cause_signature` is stable for a repeating cause → recall collapses to **1 forever** → `recurrence` can never reach `3` → the `mod.rs:1613` escalation rung becomes **dead code**. Correct remedy = **count-in-content caller-key upsert** (`occurrence_count`/`first_seen`/`last_seen`, escalation reads the field, not `recall.len()`). Not my seam; I re-confirm it stands — do **not** adopt the literal one-liner.
- **`×2` is honest.** `RECURRING_SIGNATURE_THRESHOLD=2` (Lane A / observation episodes) vs. `RECURRENCE_ESCALATION_THRESHOLD=3` (Lane B / root-cause occurrences) → the visible `×2` lands in the `[2,3)` cross-lane dead zone. Audit the closing action, not the counter.

---

## 4. Token classification (benign-drift re-confirmation)

| Token | dedup_key @ HEAD | Volatile field | Classification |
|---|---|---|---|
| `goal:blocked:<goal_id>` | `format!("goal:blocked:{goal_id}")` | counters/priority only | **Load-bearing** — the persistent membership set that IS the problem |
| `overseer-obs:…` (nested) | `mod.rs:1359` `sanitize_recalled(signature)` | `occurrences` (summary only) | **Load-bearing / signature-inflating** — the D1 artifact that manufactures the doubling |
| `workstream-gap` | `mod.rs:1371` fixed literal | `gaps.len()` (summary only) | **Benign membership drift** |
| `resource:engineer_spawn` | `mod.rs:1270` fixed literal | `{live}` (summary only) | **Benign membership drift / telemetry** — NOT a new signal source, no ceiling error at the dedup_key |

Membership drift forks the Lane-A composite identity (set-hash over the whole tick) but is **immune** on Lane-B (escalation keys on per-problem dedup_key). Benign-but-latent Lane-A precision issue; never an escalation correctness defect.

---

## 5. Concerns / questions for verification phase

1. **D1 fix seam confirmation:** verify that excluding recall-derived (`overseer-obs:`/`RecurringSignature`) keys at `mod.rs:546` stops the nesting **without** suppressing legitimate first-order recurrence detection.
2. **Do not adopt the §6.2b one-liner** — require the count-in-content upsert variant; D2 (gate + counter) must ship atomically or nothing changes.
3. **Lane-A cross-restart:** the in-memory `WhisperGate(900,5)` gives no cross-restart dedup; confirm whether the two episodes came from two windows in one run or two restarts (a flapping daemon inflates Lane A).
4. **engineer_spawn:** only classify as a real cap breach if an explicit ceiling error exists in `launch.rs`/`guardrails.rs`/`config.rs`; none is present at the dedup_key mint — classify benign.
