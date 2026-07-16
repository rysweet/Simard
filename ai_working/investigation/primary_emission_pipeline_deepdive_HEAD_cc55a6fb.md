# Primary — Signature Emission Pipeline Deep Dive (independent re-grounding @ `cc55a6fb`)

**Role:** PRIMARY investigator (deep dive).
**Investigation question:** the recurring signature "seen 2×" in cognitive memory —
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` fragments
repeated N× followed by a run of `workstream-gap`.
**Focus:** trace the full signature emission pipeline `signal.rs → sensor.rs → observer.rs → wiring.rs`
(plus the `mod.rs` orient/write-back seam) and prove where the blob is minted and why it recurs.
**HEAD:** `cc55a6fb` (branch tip). **Doctrine:** validate-don't-re-derive — every citation below was
re-read against **live** source at `cc55a6fb`, the load-bearing suite was **re-run** (§4), and the loop
was **reproduced from scratch** (§5).
**Relation to prior work:** confirms and re-grounds the `d187e414` primary and the `FINAL_SYNTHESIS`
at the current HEAD. No new hazards; the confirmed root cause and minimal fix are unchanged.

---

## 0. Verdict (all re-confirmed live @ `cc55a6fb`)

1. **The signature is the Overseer's own observation write-back — not a raw memory key.**
   `observation_signature` (`mod.rs:1068-1073`) collects each cycle problem's `dedup_key`,
   `sort_unstable` + `dedup`, joins with `|`, and prepends `overseer-obs:`. **Line 1072 is the sole
   producer** of both the `overseer-obs:` prefix and the `|`-join.
2. **"Seen 2×" is an honest re-observation, not a storage/replay bug.** It is the constant
   `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) — the floor at which `signals_from`
   (`signal.rs:462-468`) raises `Signal::RecurringSignature { signature, occurrences }`.
3. **The repeated `overseer-obs:goal:blocked:…-7f5afcca` fragments are a real self-ingestion feedback
   loop.** A write-back is stored, later recalled, and the **entire recalled composite** is admitted as
   **one** new problem `dedup_key` (`mod.rs:1359`, `sanitize_recalled(signature)`), then re-wrapped in a
   fresh `overseer-obs:` prefix on the next write-back (`mod.rs:1072`). Each generation adds exactly one
   nested `overseer-obs:` layer and one more copy of every frozen inner token. Reproduced byte-shape in §5.
4. **The write-back dedup gate fuels the loop, it cannot stop it.** The gate keys on the full
   `observation_signature` (`mod.rs:546-548`), but that signature **grows every generation**, so
   consecutive generations are never byte-identical → `peek` always returns `Deliver` → a fresh episode
   persists each window (proven by `write_back_persists_again_for_a_distinct_signature`, §4).
5. **`sanitize_recalled` does not break the recursion.** It only (1) replaces control chars with a space
   and (2) caps at `RECALLED_TEXT_MAX_LEN = 8192` bytes (`capabilities.rs:459-482`). It preserves `:`,
   `|`, and the `overseer-obs:` prefix, so the recalled composite re-nests intact until the 8 KB cap.
6. **Zero source drift.** `git diff --stat d187e414..HEAD -- src/` is empty; the two intervening commits
   are docs-only. Every citation is live.

---

## 1. The pipeline nodes (Observe → Signal → Orient → Write-back), re-read @ `cc55a6fb`

| Stage | Code (file:line) | What it does |
|---|---|---|
| **Sense** (Observe) | `sensor.rs` (board snapshot: blocked_goals / in_flight / workstream_gaps), `wiring.rs:301` orchestration | Gathers the raw board state + recall snapshot for the cycle. |
| **signal.rs — derive** | `signal.rs:366` `signals_from`; recall arm `signal.rs:455-469` | Pure Observe→Signal. The recall arm tallies recalled episodes by `failure_signature` and pushes `RecurringSignature` at `occurrences >= 2`. **Additive only** — a `None`/empty/error recall leaves the signal set untouched. |
| **observer.rs / mod.rs — orient** | `orient` `mod.rs:1200-1235`; `classify_signal` `mod.rs:1238-1394` | Maps each `Signal` → `(kind, priority, dedup_key, summary)`; dedups vs in-flight (`1207`); merges same-key problems (`1211`); a `RecurringSignature` co-signal **raises** the matching problem's priority (`1217-1219`). |
| **write-back** | `write_back_observation` `mod.rs:534-563`; sole call site `wiring.rs:301` | Builds `observation_signature` (`546`), gates via `write_back_gate.peek` (`548`), and on `Deliver` calls `record_observation` (`554`) then `commit` (`556`). |

Constants re-verified: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`);
`RECALLED_TEXT_MAX_LEN = 8192` (`capabilities.rs:455`).

---

## 2. Per-token provenance of the blob

| Token in the signature | Emitter (file:line) | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1072` | `format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `classify_signal` `GoalHygiene`/blocked arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; the `<slug>-7f5afcca` **is** the goal_id, minted upstream |
| `workstream-gap` (bare literal, repeated) | `classify_signal` `WorkstreamGap` arm — `mod.rs:1371` | `"workstream-gap".to_string()` — per-gap identity is erased into one constant key |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:464`, admitted `mod.rs:1359` | `sanitize_recalled(signature)` — the whole prior composite re-ingested as a single key |

The outer composite (`mod.rs:1072`) has **no length cap and no self-exclusion**: it will ingest any
`dedup_key`, including one already beginning with `overseer-obs:`. Nothing on the construction path
breaks the recursion.

---

## 3. Loop closure (the seam that makes it recur)

1. **Store** — `write_back_observation` (`mod.rs:546`) computes `signature = observation_signature(problems)`;
   `record_observation` (`wiring.rs:1084`) embeds it as a `[sig:…]` text marker in the episode content.
2. **Recall + parse** — `parse_failure_signature` (`wiring.rs:976-986`) extracts `[sig:…]` into
   `RecalledEpisode.failure_signature` (`capabilities.rs:611-614`). The Overseer's own prior write-back is
   now indistinguishable from any other recalled failure signature.
3. **Count** — `signals_from` (`signal.rs:455-469`) tallies by signature; `>= 2` ⇒ `RecurringSignature`.
4. **Classify → key** — `classify_signal` (`mod.rs:1359`) sets `dedup_key = sanitize_recalled(signature)` —
   the whole prior composite becomes **one** problem key (control-stripped, ≤8 KB, otherwise verbatim).
5. **Re-emit** — `wiring.rs:301` calls `write_back_observation(&cycle.problems)` with the full problem set,
   including the composite-keyed problem → back to step 1, nested one level deeper.

---

## 4. Empirical re-grounding — load-bearing tests re-run @ `cc55a6fb`

`cargo test -p simard --lib overseer::tests_memory_recall` → **32 passed; 0 failed** (this run). Load-bearing:

- `recurring_signature_emitted_when_two_episodes_share_signature` — confirms the ×2 threshold path.
- `recurring_signature_not_emitted_for_single_occurrence` — a single prior is not "recurring".
- `recurring_signature_is_additive_not_replacing` — recall only appends signals.
- `recurring_signature_problem_summary_is_sanitized` — the untrusted signature is cleaned at admission.
- `write_back_is_deduplicated_within_window` — an **identical** signature is suppressed inside the window.
- `write_back_persists_again_for_a_distinct_signature` — a **distinct** signature re-persists. This is the
  exact condition the growing composite satisfies every generation (§0.4).
- `tick_writes_observation_back_once` / `run_cycle_populates_recall_snapshot_when_enabled` — end-to-end
  Observe→write-back wiring.

---

## 5. From-scratch reproduction of the exact blob shape

A faithful sim of the two real functions (`observation_signature` + the recall re-ingestion of the whole
prior composite as one key) produces, with the frozen problem set from the question:

```
gen0 len=97   overseer-obs_prefixes=1 blocked-token_copies=1
gen1 len=195  overseer-obs_prefixes=2 blocked-token_copies=2
gen2 len=293  overseer-obs_prefixes=3 blocked-token_copies=3
head: overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|
      overseer-obs:goal:blocked:…-7f5afcca|overseer-obs:goal:blocked:…-7f5afcca|workstream-gap…
```

This matches the investigation-question string exactly: repeated
`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` fragments followed by
a run of `workstream-gap`. **N repeated fragments ⇒ N generations of the loop; the visible "2×" is 2
generations at the `RECURRING_SIGNATURE_THRESHOLD` floor.** Growth is ~linear (+98 chars/gen here), so the
dedup gate never fires on the growing composite and growth only halts when the 8 KB `sanitize_recalled`
cap is reached — a truncation ceiling, not a loop breaker.

---

## 6. Unremediated hazards (extend, don't restart)

- **D1 — self-ingestion loop is unguarded.** `observation_signature` (`mod.rs:1069-1072`) neither excludes
  keys already prefixed `overseer-obs:` nor caps composite length. **Minimal safe fix:** at `mod.rs:1069`,
  filter out `dedup_key`s starting with `overseer-obs:` before the join (self-exclusion), and/or cap the
  composite with the existing `RECALLED_TEXT_MAX_LEN` primitive. Self-exclusion alone breaks recursion:
  a self-derived recalled key can never re-enter the next composite, so the signature stops growing and
  the write-back gate begins de-duping identical observations as designed.
- **D1b — truncation hazard.** The stored `[sig:…]` marker is untruncated at the embed site
  (`wiring.rs:1084`); unbounded growth risks a later truncation splitting a `[sig:…]` marker on recall.
  Bounding the composite (D1) also removes this.
- **Root non-closure (why the problem set is frozen):** blocked goals are parked without a WHY-driven
  resolution rung, and `workstream-gap` is notify-only (no launch edge). Because the underlying problem
  set never changes tick-to-tick, the base signature recurs even independent of the nesting loop — the
  self-ingestion loop then amplifies each stable observation into a growing composite.

**Load-bearing citations (all live @ `cc55a6fb`):** `mod.rs:1068-1073` (sig build / sole `overseer-obs:`
producer), `mod.rs:1336` (blocked key), `mod.rs:1359` (recall re-ingestion seam), `mod.rs:1371` (gap key),
`mod.rs:534-563` (write-back + gate), `mod.rs:1200-1235` (orient/merge), `mod.rs:1217-1219` (priority
raise), `signal.rs:362` (threshold=2), `signal.rs:455-469` (RecurringSignature emit), `capabilities.rs:459-482`
(`sanitize_recalled` — control-strip + 8 KB cap, prefix preserved), `capabilities.rs:611-614`
(`failure_signature`), `wiring.rs:301` (write-back call site), `wiring.rs:976-986` (`[sig:…]` parse),
`wiring.rs:1084` (`[sig:…]` embed).
