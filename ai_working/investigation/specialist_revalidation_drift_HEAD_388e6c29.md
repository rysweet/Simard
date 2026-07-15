# Specialist Re-Validation & Drift Assessment — HEAD `388e6c29`

**Specialty:** Knowledge-archaeology — re-validate prior consolidated findings
against current HEAD, detect drift (notably the new `resource:engineer_spawn`
token). All citations independently re-grounded to live `src/` (no doc-to-doc
trust). Baseline tests re-run green.

---

## 0. Bottom line

Every load-bearing root-cause claim from the prior waves **still holds at HEAD
`388e6c29`**. There is **zero source drift**: `git diff --name-only
6e3113bc..HEAD -- '*.rs'` is **empty** — every investigation commit since the
original root-cause report is docs-only, so all `src/overseer/*` and
`src/stewardship/dedup.rs` line citations remain valid.

The one genuinely new element — the `resource:engineer_spawn` token in the later
snapshot — is **membership drift, not code drift**, and it does **not**
invalidate any prior finding.

---

## 1. Independent line-level re-verification (load-bearing citations)

| Claim | Location | Re-verified at HEAD |
|---|---|---|
| Composite = `overseer-obs:` + sorted/deduped `dedup_key`s | `mod.rs:1068-1073` | ✅ exact |
| `goal:blocked:<goal_id>` key; counts in summary only | `mod.rs:1336-1344` | ✅ |
| `workstream-gap` literal key; `gaps.len()` in summary | `mod.rs:1368-1372` | ✅ |
| `resource:engineer_spawn` literal key; `{live}` in summary | `mod.rs:1267-1272` | ✅ |
| Recalled `RecurringSignature` key = `sanitize_recalled(signature)`; `{occurrences}` in summary | `mod.rs:1353-1363` | ✅ |
| Same-key merge; RecurringSignature raises priority | `mod.rs:1211,1217-1218` | ✅ |
| Recall counts ≥ threshold ⇒ RecurringSignature | `signal.rs:455-469` | ✅ |
| `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ |
| Stable run-id `overseer-<sig>`; sig = `f(failure_kind,error_text)` | `observer.rs:76-86` | ✅ |
| `dedup_key` → `failure_kind`; `stable_error_text` → `error_text` | `observer.rs:128-135` | ✅ |
| `failure_signature` = 8-byte SHA256(kind\n normalized) | `dedup.rs:63-75` | ✅ |
| `normalize_for_signature` redacts PATH/RUNID/TS/HEX | `dedup.rs:19-59` | ✅ |
| Write-back path + `[sig:...]` marker parse | `wiring.rs:301,976,1025` | ✅ |

**No citation was found stale, moved, or misquoted.**

## 2. Baseline tests — green at HEAD

`cargo test -p simard --lib -- overseer::observer:: dedup_signature
brief_to_summary write_back_is_deduplicated` → **13 passed; 0 failed.**
Key idempotency guarantees confirmed live:
- `brief_to_summary_synthesises_stable_run_id_from_signature`
- `dedup_signature_ignores_recipe_and_step_differences`
- `issue_filer_is_idempotent_across_cycles_no_network`
- `same_process_problem_dedups_to_one_issue_across_cycles`
- `write_back_is_deduplicated_within_window`

## 3. Drift assessment (the deliverable)

### 3a. Still valid (no drift)
1. **Signature = deterministic membership fingerprint.** `observation_signature`
   is a pure function of the sorted/deduped `dedup_key` set. No count, timestamp,
   or live metric enters the key. (`mod.rs:1068-1073`.)
2. **`2×` is a real re-observation, not a dedup/storage/counter artifact.** The
   within-window write-back gate provably suppresses unchanged observations
   (`write_back_is_deduplicated_within_window` green). The count reflects two
   legitimate write-back passes of a near-static problem set.
3. **Two open observe-and-flag loops never close** → the problem set persists →
   the fingerprint recurs. Root cause is the un-remediated set, not the hashing.
4. **Self-referential write-back feedback** produces the nested `overseer-obs:`
   fragments: recalled `RecurringSignature` (key = sanitized prior signature,
   `mod.rs:1359`) does not merge with base `goal:blocked` keys, is written back
   (`wiring.rs:301`), and re-enters the next signature. Bounded by the 2× recall
   threshold + 15-min gate, but a real design smell — unchanged at HEAD.

### 3b. Stale / superseded
- **None analytically.** The only superseded item across all waves was a
  *remedy* (naïve one-line counter, §6.2b), already corrected to a
  count-in-content upsert. No prior *analysis* is stale.

### 3c. Newly introduced — `resource:engineer_spawn`
- **New to the observed membership set**, absent from the original consolidation
  (`6e3113bc`) and the `85b9398a` wave docs; first appears in the `388e6c29`
  docs.
- **NOT new code:** the `"resource:engineer_spawn"` literal key has existed since
  `add1708a` (#2419/#2533). Its appearance means an `EngineerSpawnRate{live}`
  signal crossed threshold **at observe time** in the later snapshot — a
  membership event, not a code change.
- **Drift is benign for dedup/idempotency.** Structurally identical to
  `goal:blocked` and `workstream-gap`: the volatile `{live}` count is confined to
  the summary (`mod.rs:1271` → `observation_content`), the `dedup_key` is a fixed
  literal. **No volatile component leaks into the signature.** The prior
  "deterministic membership fingerprint" verdict absorbs this token cleanly.
- **What it changes:** only the membership-delta narrative. The later snapshot
  **drops** the five `simard-identity` goals and **adds** `resource:engineer_spawn`
  + extra nested `workstream-gap` tokens — confirming the two occurrences are
  **overlapping-but-different** snapshots of a *near*-static (not byte-static)
  set. This corroborates, and does not contradict, the "real re-observation loop"
  verdict.

## 4. Reconciliation note (one nuance surfaced)

The newest secondary doc (§3 step 4) states the recalled `overseer-obs:` key
"does NOT same-key-merge with any `goal:blocked` problem." Re-grounded to
`mod.rs:1211` + the design comment at `mod.rs:1346-1352`: this is **correct** —
the merge is *intended* only when a recalled signature equals an in-cycle base
key; an `overseer-obs:…` recalled key never equals a base `goal:blocked` /
`workstream-gap` key, so it stands alone and is written back (feeding the
nesting). No contradiction; the nuance is that the merge path *exists* but is not
taken for the meta-signature. Confirmed consistent.

## 5. Fix-safety confirmation (advisory, investigation-only)

The proposed minimal fix (count-in-content upsert at the write-back seam, not a
naïve counter) does not regress the idempotency tests above: the composite
`dedup_key` set is unchanged, so `observation_signature` stability and the
within-window gate are untouched. Landing at the `write_back_observation` seam
(`wiring.rs:301`) rather than in `dedup.rs`/`observer.rs` keeps
`dedup_signature_ignores_recipe_and_step_differences` and
`tests_gap_scan.rs:857` green. No implementation performed.

---

**Verdict:** Prior findings **re-validated, no source drift**. The
`resource:engineer_spawn` addition is **benign membership drift** that the
existing root-cause model already explains. Confidence: **high** (every citation
re-verified exact; baseline tests green; `6e3113bc..HEAD` is docs-only).
