# Specialist Re-Grounding & Source-Drift Verdict — HEAD `b9f99879`

**Specialty:** Knowledge-archaeology / source-drift verification.
**Focus:** Re-ground prior `ai_working/investigation/*` findings against the
*current* HEAD; run `git diff '*.rs'` since the baseline to decide **source-fix
present vs docs-only (zero source drift)**. Every citation re-verified against
live `src/` — no doc-to-doc trust.

---

## 0. Bottom line

- **DOCS-ONLY. Zero source drift.** No `.rs`, `.toml`, or `Cargo.lock` change
  has landed for this investigation — committed **or** in the working tree.
- **Baseline ref:** `dcf909c5` — `refactor(ooda): move goal-session decisions
  into an agentic recipe (#4058)`. This is the last real code commit on the
  branch and the **merge-base with `main`**. Everything after it
  (`6e3113bc..HEAD`, seven `docs(investigation)` commits) is documentation.
- **HEAD advanced, verdicts unchanged.** Prior specialist grounded at
  `0289572e`; HEAD is now **`b9f99879`** (two further docs-only commits:
  `5a85317b`, `b9f99879`). Nothing load-bearing moved. All prior root-cause
  claims **re-validated exact** at HEAD.
- **The overseer/stewardship pipeline is byte-identical to `6b2bf5e1`** — the
  last commit that touched `src/overseer/*` (a real code commit, PR #4063), not
  the docs series. Any proposed fix therefore remains *unimplemented*.

---

## 1. Source-drift evidence (reproducible)

| Command | Result |
|---|---|
| `git diff --stat dcf909c5..HEAD -- '*.rs'` | **empty** (0 lines) |
| `git diff --name-only 6e3113bc..HEAD \| grep -v '^ai_working/investigation/'` | **NONE** (0 non-docs files) |
| `git diff dcf909c5..HEAD -- '*.rs' '*.toml' 'Cargo.lock'` (working tree incl. uncommitted) | **0 lines** |
| `git diff HEAD -- '*.rs'` (staged + unstaged) | **0 lines** |
| `git diff --stat dcf909c5..HEAD` (all files) | **34 files, +6476, all `ai_working/investigation/*.md`** |
| `git log -1 --format=%h -- src/overseer/{mod,signal,observer}.rs` | **`6b2bf5e1`** (code commit, not docs) |

- **Baseline choice justified:** `git merge-base HEAD main` = `dcf909c5`. First-
  parent history shows `dcf909c5` immediately precedes the docs series
  `6e3113bc → … → b9f99879`. So `dcf909c5` is the correct "last consolidated
  code" baseline; `6e3113bc` (root-cause report) is the first docs commit.
- **Working-tree caveat handled:** several `src/overseer/*.rs` files carry
  Jul-15 mtimes (checkout/build touch), but `git diff HEAD -- '*.rs'` is **0
  lines** — content is identical; mtimes are not drift.
- **Uncommitted state:** staged/untracked changes are **exclusively**
  `ai_working/investigation/*.md`. No source, config, or lockfile is dirty.

## 2. Load-bearing citations — independently re-grounded at `b9f99879`

Re-verified by direct `grep`/`view` on live source (not the prior docs):

| Claim | Anchor (HEAD `b9f99879`) | Re-verified |
|---|---|---|
| Composite emitter `format!("overseer-obs:{}", keys.join("\|"))` after sort+dedup | `src/overseer/mod.rs:1068` (`fn observation_signature`), `:1072` | ✅ exact |
| `resource:engineer_spawn` literal `dedup_key` | `src/overseer/mod.rs:1270` | ✅ exact |
| `workstream-gap` literal `dedup_key` | `src/overseer/mod.rs:1371` | ✅ exact |
| Detection floor `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= …` | `src/overseer/signal.rs:362`, used `:463` | ✅ exact |
| Escalation floor `RECURRENCE_ESCALATION_THRESHOLD = 3` | `src/overseer/root_cause.rs:33` | ✅ exact |
| `failure_signature` = SHA256(kind\n text); issue dedup `find_existing` | `src/stewardship/dedup.rs:63-65`, `:78` | ✅ exact |
| Write-back seam passes **all** `cycle.problems` | `src/overseer/wiring.rs:301` | ✅ exact |
| Recall parses `failure_signature` from **any** episode — **no source-label self-exclusion** | `src/overseer/wiring.rs:1013` (`recall_episodic`), `:1025` (`parse_failure_signature(&e.content)`) | ✅ exact |
| Overseer self-authors episodes under `OVERSEER_SOURCE_LABEL = "overseer"` | `src/overseer/wiring.rs:952`, stored `:1088` | ✅ exact |

**No citation found stale, moved, or misquoted.** The exact "open-loop seam"
(recall counting the Overseer's own write-backs — `wiring.rs:1025` calls
`parse_failure_signature` on every episode with no `source_label` guard) is
present verbatim at HEAD.

## 3. The "2× dead zone" is a source fact, not a doc artifact

Both threshold constants coexist in live source: detection fires at **≥2**
(`signal.rs:362`) but escalation only at **≥3** (`root_cause.rs:33`). A signature
re-observed exactly `2×` therefore sits below escalation while the within-window
write-back gate suppresses same-window duplicates — structurally confirmed, and
**still unfixed** (zero source drift). This is a genuine design smell, not a
hashing/counter measurement error.

## 4. `resource:engineer_spawn` drift classification (re-grounded)

**Benign membership drift, not code drift.** `"resource:engineer_spawn"` is a
fixed literal `dedup_key` (`mod.rs:1270`); the volatile `{live}` count lives only
in the summary content, never in the fingerprint — structurally identical to
`goal:blocked:*` and `workstream-gap`. Its appearance in the later snapshot means
an `EngineerSpawnRate` signal crossed threshold at observe-time; it changes only
the membership-delta narrative, not any source line. Prior treatment stands.

## 5. Re-grounding verdict on prior waves

- **All prior analytical findings re-validated at `b9f99879`.** The seventh-wave
  `primary_signature_provenance_HEAD_b9f99879.md`, the prior specialist
  re-grounding at `0289572e`, and `CONSOLIDATED_FINDINGS.md` §0 are **consistent
  with live source**; the only HEAD delta since those notes is two docs commits.
- **Nothing superseded analytically.** The single ever-superseded item was a
  *remedy sketch* (naïve one-line counter bump), already corrected in the docs to
  a signature-keyed count-in-content upsert. No prior *analysis* is stale.

---

**VERDICT:** **Docs-only; zero source drift confirmed** against baseline
`dcf909c5` (merge-base with `main`; last code commit). All investigation output
since `6e3113bc` is `ai_working/investigation/*.md` (34 files, +6476, 0 source
lines). Every load-bearing citation resolves **exact** at HEAD `b9f99879`; the
overseer/stewardship pipeline is byte-identical to `6b2bf5e1`. The identified
root cause (self-observation write-back feedback + 2× dead zone) is real and
**still unremediated in code**. Confidence: **high** — reproducible git evidence
+ line-anchored source re-verification.
