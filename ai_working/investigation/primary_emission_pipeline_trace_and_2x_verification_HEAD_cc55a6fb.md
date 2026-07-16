# Primary — Emission Pipeline Trace (`run_cycle → … → record_observation`) + `2×` Defect-Verification

**Role:** PRIMARY investigator (deep dive).
**Focus:** (a) end-to-end emission trace of the recurring
`overseer-obs:…|goal:blocked:…|workstream-gap|…` signature "seen 2×"; (b) verify what the `2×`
actually is — honest re-observation vs. storage/replay defect.
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ **`cc55a6fb`**.
**Doctrine:** validate-don't-re-derive / extend-don't-restart. Every citation below was re-read against
**live source at `cc55a6fb`**; the load-bearing suite was **re-run** (§4); the loop was **reproduced
from scratch** (§5).
**Relation to prior work:** re-grounds and confirms the `d187e414`/`b47b6413` primaries, the
`RECONCILIATION_LEDGER`, and `FINAL_SYNTHESIS` at a HEAD 4 commits newer. Adds: (1) a zero-drift proof
for the newer commits, (2) the exact source line that mints the investigation-question string.

---

## 0. Verdict (all re-confirmed at HEAD `cc55a6fb`)

1. **The signature is the Overseer's own observation write-back, not a raw memory key.**
   `observation_signature` (`mod.rs:1068-1073`) sorts + dedups the cycle's problem `dedup_key`s, joins
   with `|`, and prepends `overseer-obs:`. **Line 1072 is the sole producer** of the prefix and the join.
2. **The investigation-question string is minted verbatim at `mod.rs:1360-1362`.** The
   `RecurringSignature` arm of `classify_signal` builds the summary
   `sanitize_recalled("recurring signature seen {occurrences}× in cognitive memory ({signature})")`. The
   `2×` is the `occurrences` field, floored at `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`).
3. **"Seen 2×" is an honest re-observation, NOT a storage/replay/dedup bug.** It is the count of recalled
   episodes sharing a `failure_signature` (`signal.rs:455-469`, emit at `occurrences >= 2`). The counter
   is faithful; the *defect* is what it counts (see 4).
4. **The recurrence is a real self-ingestion feedback loop (defect D1).** The stored write-back is later
   recalled, and the **entire recalled composite is admitted as ONE new problem `dedup_key`**
   (`mod.rs:1359`, `sanitize_recalled(signature)`), then re-wrapped in a fresh `overseer-obs:` prefix on
   the next write-back (`wiring.rs:301 → mod.rs:1072`). Each generation adds **one nested prefix layer +
   one more copy of every frozen inner token**. Reproduced byte-shape in §5.
5. **The write-back dedup gate cannot stop the loop — it fuels it.** The gate keys on the full
   `observation_signature` (`mod.rs:546-548`), which **grows every generation**, so consecutive
   generations are never byte-identical → `peek` always returns `Deliver` → a fresh episode persists each
   window. Proven by the two window tests (§4).
6. **Zero source drift.** `git diff --stat d187e414..HEAD -- src/` and `b47b6413..HEAD -- src/` are both
   **empty**; intervening commits are docs-only. Every citation is live at `cc55a6fb`.

---

## 1. The emission pipeline — five nodes, `run_cycle → record_observation` (live @ `cc55a6fb`)

| # | Node | Code (file:line) | What it does |
|---|---|---|---|
| 1 | `run_cycle` — Observe | `mod.rs:384-438` | `snapshot` + `observe_board` (blocked_goals / in_flight) + gap-scan + `drain_recent` + best-effort recall snapshot. Read-only; every enrichment degrades to empty on failure, never aborts. |
| 2 | recall → signal | `signal.rs:455-469` | Tally recalled episodes by `failure_signature`; push `RecurringSignature{signature,occurrences}` at `occurrences >= 2`. **Additive only.** |
| 3 | Orient / `classify_signal` | `mod.rs:441-447`, `classify_signal` arms `1336` (GoalBlocked), `1353-1363` (RecurringSignature), `1368-1373` (WorkstreamGap) | Map each `Signal` → `(kind, priority, dedup_key, summary)`; dedup vs in-flight; the `RecurringSignature` dedup_key is `sanitize_recalled(signature)` — **the whole prior composite as one key**. |
| 4 | WHY + Decide/gate | `mod.rs:449-480` | Per-problem root-cause + decide/gate; builds `cycle.problems` (now carrying the composite-keyed problem). |
| 5 | `write_back_observation → record_observation` | `mod.rs:534-563`, called `wiring.rs:301` | Build `observation_signature(cycle.problems)` (`546`), gate `peek` (`548`); on `Deliver` → `caps.memory.record_observation` (`554`) then `commit` (`556`). Adapter embeds `[sig:…]` (`wiring.rs:1084`). |

**Loop closure (the seam):** stored `[sig:…]` (`wiring.rs:1084`) → recalled + parsed into
`RecalledEpisode.failure_signature` (`wiring.rs:976-986`, `capabilities.rs:611-614`) → counted
(`signal.rs:455-469`) → re-keyed as one `dedup_key` (`mod.rs:1359`) → re-emitted in
`observation_signature` (`mod.rs:1072`, via `wiring.rs:301`). One level deeper each pass.

Constants re-verified live: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`); write-back gate
`WhisperGate::new(900, 5)` (`mod.rs:299`).

---

## 2. Per-token provenance of the blob

| Token | Emitter (file:line) | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1072` | `format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `classify_signal` GoalBlocked — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; the `…-7f5afcca` **is** the upstream goal_id |
| `workstream-gap` (repeated literal) | `classify_signal` WorkstreamGap — `mod.rs:1371` | bare `"workstream-gap"`; per-gap identity erased (INV-GAP-KEY) |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — admitted `mod.rs:1359` | `sanitize_recalled(signature)`: prior composite re-ingested as ONE key |

The outer composite (`mod.rs:1072`) has **no length cap and no self-exclusion**: it ingests any
`dedup_key`, including one already prefixed `overseer-obs:`. Nothing in the construction path breaks
the recursion.

---

## 3. What the `2×` is (defect-verification)

- **Lane A (visible `2×`)** — `occurrences` in the summary (`mod.rs:1361`) = count of recalled episodes
  sharing a signature (`signal.rs:459-467`), floored at `2` (`signal.rs:362`). **This count is honest.**
  A re-observation loop legitimately produces ≥2 matching episodes.
- **The defect is not the count — it is the self-fed signature it counts.** Because the write-back is
  recalled and re-ingested (D1), the same growing composite reappears every window, so the `RecurringSignature`
  signal fires perpetually and the `2×` never clears. The `N` repeated `overseer-obs:goal:blocked:…-7f5afcca`
  fragments in the question = `N` generations of the loop; the visible `2×` is 2 generations at the threshold floor.
- **Not a storage/replay artifact.** `write_back_is_deduplicated_within_window` (identical suppressed) and
  `write_back_persists_again_for_a_distinct_signature` (distinct re-persists) — §4 — prove the gate is
  working *correctly*; the growing composite is always *distinct*, which is exactly the condition that keeps
  a fresh episode landing each window. Honest counter, defective input.

---

## 4. Empirical re-grounding — load-bearing suite re-run @ `cc55a6fb`

`cargo test -p simard --lib overseer::tests_memory_recall` → **32 passed; 0 failed**. Load-bearing:

- `recurring_signature_emitted_when_two_episodes_share_signature` — the `×2` threshold path.
- `recurring_signature_not_emitted_for_single_occurrence` — a single prior is not "recurring".
- `recurring_signature_is_additive_not_replacing` — recall only appends.
- `recurring_signature_problem_summary_is_sanitized` — the `2×` summary passes the sanitize boundary.
- `write_back_is_deduplicated_within_window` — identical signature suppressed inside 900 s.
- `write_back_persists_again_for_a_distinct_signature` — a **distinct** signature re-persists (the exact
  condition the growing composite meets every generation).
- `tick_writes_observation_back_once` / `run_cycle_populates_recall_snapshot_when_enabled` — end-to-end
  Observe → write-back wiring.

---

## 5. Empirical reproduction of the exact blob shape (from scratch)

Faithful sim of `observation_signature` (`mod.rs:1072`) + the recall re-ingestion seam (`mod.rs:1359`):

```
gen0 len= 97  prefix_count=1  goaltok_occurs=1  distinct_stored=1
gen1 len=195  prefix_count=2  goaltok_occurs=2  distinct_stored=2
gen2 len=293  prefix_count=3  goaltok_occurs=3  distinct_stored=3
gen3 len=391  prefix_count=4  goaltok_occurs=4  distinct_stored=4
gen3 head: overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|overseer-obs:goal:blocked:…
```

Matches the investigation string exactly: repeated
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` fragments followed by
a run of `workstream-gap`. Length grows ~linearly (+98 chars/gen), and **`distinct_stored` grows 1:1 with
generations** — the gate never sees a repeat, confirming §0.5.

---

## 6. Unremediated hazards (extend, don't restart — unchanged, re-confirmed live)

- **D1 — self-ingestion loop unguarded.** `observation_signature` (`mod.rs:1069-1072`) neither excludes
  `dedup_key`s already prefixed `overseer-obs:` nor caps composite length. Minimal safe fix: filter out
  `overseer-obs:`-prefixed keys before the join (self-exclusion) and/or cap with the existing
  `RECALLED_TEXT_MAX_LEN` primitive. Unbounded growth also risks a `[sig:…]` marker being split on
  eventual truncation (D1b).
- **Root non-closure (why the problem set is frozen):** blocked goals are parked without a WHY-driven
  resolution rung, and `workstream-gap` is notify-only (no launch edge) — the two open loops the
  `SYNTHESIS`/`RECONCILIATION_LEDGER` document (D2 counter dead-zone+ratchet, D3 coverage routing hole).
  The signature recurs because the underlying problem set never changes. Ship order (per ledger):
  D2 gate+counter atomically → D3 closing rung → D1 write-back filter → convergence gauges.

**Load-bearing citations (all live @ `cc55a6fb`):** `mod.rs:1068-1073` (sig build), `mod.rs:1336`
(blocked key), `mod.rs:1353-1363` (RecurringSignature arm — mints the `2×` summary at `1360-1362`,
re-ingestion key at `1359`), `mod.rs:1371` (gap key), `mod.rs:534-563` (write-back), `mod.rs:384-489`
(run_cycle), `signal.rs:362` / `signal.rs:455-469` (threshold + emission), `capabilities.rs:611-614`
(failure_signature), `wiring.rs:301` (call site), `wiring.rs:976-986` / `wiring.rs:1084` (marker
parse/embed).
