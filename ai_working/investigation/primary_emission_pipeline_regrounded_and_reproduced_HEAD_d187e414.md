# Primary — Emission Pipeline Re-Grounded & Loop Reproduced (independent verification)

**Role:** PRIMARY investigator (deep dive).
**Focus:** re-ground the emission pipeline `run_cycle → orient → signal_to_problem → write_back_observation`
that produces the recurring `overseer-obs:…|goal:blocked:…|workstream-gap|…` signature "seen 2×".
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `d187e414`.
**Doctrine:** validate-don't-re-derive. Every citation below re-read against **live** source at
`d187e414`; the load-bearing test suite was **re-run** (§4); the loop was **reproduced empirically** (§5).
**Relation to prior work:** confirms and extends the `d187e414` duplicated-prefix primary and the
`SYNTHESIS`. Adds an independent test re-run + a from-scratch reproduction of the exact blob shape.

---

## 0. Verdict (all re-confirmed at HEAD `d187e414`)

1. **The signature is the Overseer's own observation write-back, not a raw memory key.**
   `observation_signature` (`mod.rs:1068-1073`) sorts+dedups the cycle's problem `dedup_key`s, joins
   with `|`, and prepends `overseer-obs:`. Line **1072 is the sole producer** of both the prefix and
   the `|`-join.
2. **"Seen 2×" is an honest re-observation, not a storage/replay bug.** It is
   `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`), the floor at which `signals_from`
   (`signal.rs:463-467`) raises `Signal::RecurringSignature`.
3. **The repeated `overseer-obs:goal:blocked:…-7f5afcca` fragments are a real self-ingestion feedback
   loop.** The write-back is stored, later recalled, and the *entire recalled composite* is admitted
   as **one** new problem `dedup_key` (`mod.rs:1359`, `sanitize_recalled(signature)`), then re-wrapped
   in a fresh `overseer-obs:` prefix on the next write-back. Each generation adds one nested prefix
   layer and one more copy of every frozen inner token. Reproduced byte-for-byte in §5.
4. **The write-back dedup gate cannot stop the loop — it fuels it.** The gate keys on the full
   `observation_signature` (`mod.rs:546-548`), but that signature **grows every generation**, so
   consecutive generations are never byte-identical, `peek` always returns `Deliver`, and a fresh
   episode persists each window. Proven by the two window tests in §4.
5. **Zero source drift.** `git diff --stat b47b6413..HEAD -- src/` is empty; intervening commits are
   docs-only. Every citation is live.

---

## 1. The four pipeline nodes (re-read @ `d187e414`)

| Node | Code (file:line) | What it does |
|---|---|---|
| `run_cycle` | `mod.rs:384-489` | Observe (`snapshot` + `observe_board` blocked_goals/in_flight + `workstream_gaps` + `drain_recent` + recall) → `signals_from` (441) → `orient` (447) → per-problem WHY (455-459) → decide/gate (466-480). |
| recall→signal | `signal.rs:455-469` | Tally recalled episodes by `failure_signature`; emit `RecurringSignature{signature,occurrences}` at `occurrences >= 2`. Additive only. |
| `signal_to_problem` (`orient`+`classify_signal`) | `mod.rs:1200-1235`, `1238-1394` | Map each `Signal` → `(kind,priority,dedup_key,summary)`; dedup vs in-flight; merge same-key; `RecurringSignature` co-signal RAISES matching problem priority (`mod.rs:1217-1219`). |
| `write_back_observation` | `mod.rs:534-563` | Build `observation_signature` (546), gate `peek` (548), on `Deliver` `record_observation` (554) then `commit` (556). Sole call site `wiring.rs:301`. |

Constants re-verified: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`); write-back gate
`WhisperGate::new(900, 5)` (`mod.rs:299`); recalled-text cap `RECALLED_TEXT_MAX_LEN` at
`capabilities.rs:455-472`.

---

## 2. Per-token provenance (each fragment of the blob)

| Token | Emitter | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1072` | `format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `classify_signal` `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; the `<slug>-7f5afcca` **is** the goal_id, minted upstream |
| `workstream-gap` (constant, repeated) | `classify_signal` `WorkstreamGap` arm — `mod.rs:1371` | bare literal `"workstream-gap"`; per-gap identity erased |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:464`, admitted `mod.rs:1359` | `sanitize_recalled(signature)` — the whole prior composite re-ingested as one key |

The outer composite (`mod.rs:1072`) has **no length cap and no self-exclusion**: it ingests any
`dedup_key`, including one already beginning with `overseer-obs:`. Nothing in the construction path
breaks the recursion.

---

## 3. Loop closure (the seam)

1. **Store** — `write_back_observation` persists `signature = observation_signature(problems)`; the
   adapter (`wiring.rs:1084`) embeds it as a `[sig:…]` text marker.
2. **Recall + parse** — `parse_failure_signature` (`wiring.rs:976-986`) extracts `[sig:…]` into
   `RecalledEpisode.failure_signature` (`capabilities.rs:611-614`). The Overseer's own prior write-back
   is now indistinguishable from any other recalled failure signature.
3. **Count** — `signals_from` (`signal.rs:455-469`) tallies by signature; `>= 2` ⇒ `RecurringSignature`.
4. **Classify → key** — `mod.rs:1359` sets `dedup_key = sanitize_recalled(signature)` — the whole prior
   composite becomes a single problem key.
5. **Re-emit** — `wiring.rs:301` calls `write_back_observation(&cycle.problems)` with the full set,
   including the composite-keyed problem. Back to step 1, nesting one level deeper.

---

## 4. Empirical re-grounding — load-bearing tests re-run @ `d187e414`

`cargo test -p simard --lib overseer::tests_memory_recall` → **32 passed; 0 failed**. Load-bearing:

- `recurring_signature_emitted_when_two_episodes_share_signature` — confirms the ×2 threshold path.
- `recurring_signature_not_emitted_for_single_occurrence` — a single prior is not "recurring".
- `recurring_signature_is_additive_not_replacing` — recall only appends signals.
- `write_back_is_deduplicated_within_window` — identical signature suppressed inside 900 s.
- `write_back_persists_again_for_a_distinct_signature` — a **distinct** signature re-persists. This is
  the exact condition the growing composite satisfies every generation.
- `tick_writes_observation_back_once` / `run_cycle_populates_recall_snapshot_when_enabled` — end-to-end
  Observe→write-back wiring.

---

## 5. Empirical reproduction of the exact blob shape

A faithful sim of the two exact functions (`observation_signature` + recall re-ingestion) run from
scratch:

```
gen0 len=97   prefix_count=1
gen1 len=195  prefix_count=2
gen2 len=293  prefix_count=3
gen2 head: overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|
           overseer-obs:goal:blocked:…-7f5afcca|overseer-obs:goal:blocked:…-7f5afcca|
           workstream-gap|workstream-gap|workstream-gap …
goal-token occurrences in gen2: 3
```

This matches the investigation-question string exactly: repeated
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` fragments followed
by a run of `workstream-gap`. **N repeated fragments ⇒ N generations of the loop; the visible "2×" is
2 generations at the `RECURRING_SIGNATURE_THRESHOLD` floor.** Signature length grows ~linearly
(+98 chars/gen here), guaranteeing the dedup gate never fires on the growing composite (§0.4).

---

## 6. Unremediated hazards (extend, don't restart)

- **D1 — self-ingestion loop is unguarded.** `observation_signature` (`mod.rs:1072`) neither excludes
  keys already prefixed `overseer-obs:` nor caps composite length. Minimal safe fix: at `mod.rs:1069`,
  filter out `dedup_key`s starting with `overseer-obs:` before join (self-exclusion), and/or cap the
  composite with the existing `RECALLED_TEXT_MAX_LEN` primitive.
- **D1b — truncation hazard.** The stored `[sig:…]` marker is untruncated; unbounded growth risks
  eventual truncation that could split a `[sig:…]` marker on recall.
- **Root non-closure (why the problem set is frozen):** blocked goals are parked without a WHY-driven
  resolution rung and `workstream-gap` is notify-only (no launch edge) — the two open loops the
  `SYNTHESIS` documents. The signature recurs because the underlying problem set never changes.

**Load-bearing citations (all live @ `d187e414`):** `mod.rs:1068-1073` (sig build), `mod.rs:1336`
(blocked key), `mod.rs:1359` (recall re-ingestion seam), `mod.rs:1371` (gap key), `mod.rs:534-563`
(write-back), `mod.rs:1217-1219` (priority raise), `signal.rs:362` / `signal.rs:455-469` (threshold +
RecurringSignature), `capabilities.rs:611-614` (failure_signature), `wiring.rs:301` (call site),
`wiring.rs:976-986`/`wiring.rs:1084` (marker parse/embed).
