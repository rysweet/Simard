# Specialist Re-Grounding & Drift Assessment — HEAD `0289572e`

**Specialty:** Knowledge-archaeology. Re-ground all prior `ai_working/investigation/`
verdicts to the *current* HEAD, reconfirm docs-only drift from `85b9398a`, and
assess the `resource:engineer_spawn` dedup-key interaction. Every citation
independently re-verified against live `src/` (no doc-to-doc trust). Baseline
tests re-run green.

---

## 0. Bottom line

- **HEAD correction:** the strategy names `388e6c29`, but the *current* HEAD is
  **`0289572e`** (fifth re-validation wave). One additional commit exists beyond
  the prior specialist's baseline. It changes nothing load-bearing.
- **Docs-only drift CONFIRMED, extended to current HEAD.** `git diff --name-only
  85b9398a..HEAD` touches **only** `ai_working/investigation/*.md`. `git diff
  --name-only 6e3113bc..HEAD -- '*.rs'` is **empty** — no Rust source has changed
  since the original root-cause report. Every `src/` line citation remains valid.
- **All prior root-cause claims re-validated at `0289572e`.** No analysis is
  stale. The `resource:engineer_spawn` token is **benign membership drift, not
  code drift**, and the existing "deterministic membership fingerprint" model
  absorbs it cleanly.

---

## 1. Commit-level drift ledger (85b9398a → HEAD)

| Commit | Subject | Scope |
|---|---|---|
| `85b9398a` | fold re-validation deep dives | docs-only (baseline) |
| `388e6c29` | consolidate fourth re-validation wave | **docs-only** (ai_working/ only) |
| `0289572e` | consolidate fifth re-validation wave (**HEAD**) | **docs-only** (ai_working/ only) |

`git diff --name-only 85b9398a..HEAD | grep -v '^ai_working/investigation/'` → **NONE**.
1803 insertions / 3 deletions, entirely under `ai_working/investigation/`.

## 2. Load-bearing citations — independently re-grounded at `0289572e`

| Claim | Location | Re-verified |
|---|---|---|
| Composite = `overseer-obs:` + sorted+deduped `dedup_key`s joined by `\|` | `mod.rs:1068-1073` | ✅ exact |
| `goal:blocked:<goal_id>` key; counts in summary only | `mod.rs:1336-1344` | ✅ |
| `workstream-gap` literal key; `gaps.len()` in summary | `mod.rs:1368-1371` | ✅ |
| `resource:engineer_spawn` literal key; `{live}` in summary only | `mod.rs:1267-1272` | ✅ |
| Recalled `RecurringSignature` key = `sanitize_recalled(signature)`; `{occurrences}` in summary | `mod.rs:1353-1363` | ✅ |
| Same-key merge; `RecurringSignature` only raises priority (`.min`) | `mod.rs:1211, 1217-1218` | ✅ |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (detection floor) | `signal.rs:362` | ✅ |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (escalation floor) | `root_cause.rs:33` | ✅ |
| `failure_signature` = SHA256(kind\n normalized) | `dedup.rs:63-75` | ✅ |
| Write-back passes **all** `cycle.problems` (incl. recall-derived) | `wiring.rs:301` | ✅ |
| Write-back gate keyed by `observation_signature`, commit only after store | `mod.rs:546-556` | ✅ |

No citation found stale, moved, or misquoted.

## 3. `resource:engineer_spawn` dedup-key interaction — SPECIALIST VERDICT

**Benign. No new dedup pathology. Prior treatment stands.**

1. **Fixed-literal key, volatile count quarantined.** `Signal::EngineerSpawnRate
   { live }` maps to the constant `dedup_key = "resource:engineer_spawn"`
   (`mod.rs:1270`); the volatile `{live}` appears **only** in the summary string
   (`mod.rs:1271` → `observation_content`, `mod.rs:1079-1088`), never in the
   signature. Structurally identical to `goal:blocked:*` and `workstream-gap`:
   **no volatile component leaks into the fingerprint.**
2. **Deterministic sort placement verified empirically.** ASCII prefix order
   `g < o < r < w` ⇒ the composite orders as `goal:blocked:*` …, embedded
   `overseer-obs:…`, then `resource:engineer_spawn`, then `workstream-gap` —
   exactly the observed blob layout. Confirmed via sort of the literal keys.
3. **It is a membership event, not a code change.** The `"resource:engineer_spawn"`
   literal predates the investigation; its appearance in the later snapshot means
   an `EngineerSpawnRate{live}` signal crossed threshold *at observe time*. This
   only alters the **membership-delta narrative** (later snapshot drops the five
   `simard-identity-*` goals, adds `resource:engineer_spawn` + extra nested
   `workstream-gap`), corroborating "overlapping-but-different near-static
   snapshots" — it does **not** contradict the real-re-observation verdict.
4. **Convergence-class semantics (unchanged).** `goal:blocked` +
   `workstream-gap` + `resource:engineer_spawn` are one under-throughput problem
   in three views (spawning engineers, yet goals stay blocked and gaps uncovered).
   They sit in the **"2× dead zone"**: recurrence detection fires at ≥2
   (`signal.rs:362`) but escalation only at ≥3 (`root_cause.rs:33`), while the
   15-min write-back gate deduplicates re-observation — so the set neither
   remediates nor escalates. This is a real design smell, structurally confirmed,
   not a hashing/counter artifact.

## 4. Still-valid vs superseded

**Still valid (no drift):**
- Signature is a pure deterministic membership fingerprint (`mod.rs:1068-1073`).
- `2×` is honest re-observation, not a dedup/storage/counter artifact — proven by
  `write_back_is_deduplicated_within_window` (green) + within-window gate.
- Self-observation write-back feedback produces the nested `overseer-obs:`
  fragments: recall-derived `RecurringSignature` (key = sanitized prior signature,
  `mod.rs:1359`) stands alone (never equals a base `goal:blocked`/`workstream-gap`
  key, so `mod.rs:1211` merge is not taken), is written back (`wiring.rs:301`),
  and re-enters the next signature. Bounded by the 2× threshold + 15-min gate.
  Real design smell, unchanged.

**Superseded:** None analytically. The only ever-superseded item was a *remedy*
(the naïve one-line counter), already corrected to a count-in-content upsert. No
prior *analysis* is stale at `0289572e`.

**Newly assessed at this HEAD:** `resource:engineer_spawn` — benign membership
drift (see §3). No new dedup-key interaction beyond what the fingerprint model
already covers.

## 5. Baseline tests — GREEN at HEAD `0289572e`

`cargo test -p simard --lib -- overseer::observer:: dedup_signature
brief_to_summary write_back_is_deduplicated` → **13 passed; 0 failed.** Notably:
- `write_back_is_deduplicated_within_window` — dedup gate suppresses unchanged obs
- `dedup_signature_ignores_recipe_and_step_differences`
- `issue_filer_is_idempotent_across_cycles_no_network`
- `same_process_problem_dedups_to_one_issue_across_cycles`
- `brief_to_summary_synthesises_stable_run_id_from_signature`

## 6. Fix-safety (advisory only — no implementation)

The proposed minimal remedy (idempotent signature-keyed count-in-content upsert
at the `write_back_observation` seam, `wiring.rs:301`; optionally exclude
recall-derived meta-problems from write-back; add a first-recurrence remediation
rung to close the 2× dead zone) does not touch the composite `dedup_key` set, so
`observation_signature` stability and the within-window gate are untouched — the
idempotency tests above stay green. Landing at the write-back seam (not
`dedup.rs`/`observer.rs`) preserves `dedup_signature_ignores_recipe_and_step_differences`.

---

**Verdict:** Prior findings **re-validated at current HEAD `0289572e`, zero
source drift** since `6e3113bc`; docs-only drift from `85b9398a` **confirmed**.
`resource:engineer_spawn` is **benign membership drift** already explained by the
existing root-cause model — no new dedup-key interaction. Confidence: **high**
(every citation re-verified exact against live source; baseline tests green;
`6e3113bc..HEAD -- '*.rs'` empty).
