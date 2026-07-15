# PRIMARY Investigation — Signature Emission Provenance & Idempotency Verdict

**Question:** A recurring `overseer-obs:...|goal:blocked:<slug>-<hash>|...|workstream-gap`
signature was "seen 2×" in cognitive memory. What emits each token, and is 2× two
real cycles or a duplicated write of one cycle?

**Scope read:** `src/overseer/{mod,observer,signal,sensor,root_cause,guardrails,wiring}.rs`,
`src/overseer/tests_gap_scan.rs`, `src/stewardship/dedup.rs`,
`src/cognitive_memory/library_adapter.rs`.

---

## 1. Per-token provenance map

The observed string is a **single composite episodic record** whose payload is the
sorted, deduped set of the cycle's problem `dedup_key`s, pipe-joined and prefixed.

| Token | Emitting code | How it is built |
|-------|---------------|-----------------|
| `overseer-obs:` prefix + `\|`-join of the whole snapshot | `observation_signature()` — `mod.rs:1068-1073` | `keys = problems.map(dedup_key); keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<goal_id>` (e.g. `advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`) | `signal_to_problem`, `Signal::GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`. The `<slug>-<8hex>` shape **is the goal_id itself** (slugified goal title + id suffix), minted upstream at goal creation — the Overseer does not construct the slug/hash. |
| `workstream-gap` (constant) | `signal_to_problem`, `Signal::WorkstreamGap` arm — `mod.rs:1371` | Literal `"workstream-gap".to_string()` — one consolidated, evidence-independent key per Observe pass (per-gap sub-signatures `workstream-gap:{g.signature}` are used only for the gap-notification gate at `mod.rs:901/932`, not for this composite). |

**Assembly pipeline (one record per surviving tick):**
`run_cycle` → `orient` → `signal_to_problem` stamps each `Problem.dedup_key`
→ `write_back_observation(problems)` (`mod.rs:534`) → `observation_signature(problems)`
builds the composite → `record_observation` (`wiring.rs:1076`) writes
`"{content} [sig:{signature}]"` via `store_episode(content, "overseer", {signature})`.
Called **exactly once per tick** at `wiring.rs:301`.

So the pipe-joined tokens in the question ARE the sorted/deduped `dedup_key`s of one
cycle's problem set; the aggregate is written as ONE episode carrying that signature.

---

## 2. Dedup / recurrence / idempotency machinery

**Persistence layer is append-only — no content dedup.**
`record_observation` (`wiring.rs:1084-1090`) calls `store_episode(content, LABEL, Some(metadata))`;
the adapter (`library_adapter.rs:625-627`) forwards with `caller_key = None`. Every call
mints a **new episode node**. Two calls with an identical signature ⇒ two distinct nodes.

**Sole dedup is an in-process, non-durable gate.**
`write_back_gate: WhisperGate::new(900, 5)` (`mod.rs:299`) — a 900 s dedup window + 5/hr
cap. `write_back_observation` does `peek → store → commit` (`mod.rs:548-557`): the slot is
consumed **only after a successful store**. The gate is a plain in-memory `HashMap`
(`guardrails.rs:291-333`) built fresh per Overseer construction — it **does not survive a
process restart**.

### Verdict: 2× = two REAL observation events, NOT a duplicated write of one cycle.

`write_back_observation` runs once per tick and is gated. A second identical
`overseer-obs:` record can only appear when the gate did **not** suppress it, i.e.:

1. **> 900 s after the first** — a persistent condition legitimately re-recorded in a new
   window (the documented "at most once per window", `mod.rs:295-298`); or
2. **After a process restart** — the non-durable `write_back_gate` starts empty, so the
   first post-restart tick re-records the still-true condition. Given frequent ticks, this
   is the most probable source of exactly-2×.

There is **no per-cycle double-write path** (single call site, gated, commit-after-store).
2× is therefore expected windowed/idempotent behavior of an append-only episodic store, not
a duplicate-write defect within a window.

### Recurrence machinery is a SEPARATE, deliberately append-only path.

The composite `overseer-obs:` episode is a stewardship **activity log**; it does **not**
feed `RECURRENCE_ESCALATION_THRESHOLD`. Recurrence is measured from per-problem occurrence
**facts** written by `record_occurrence` → `store_fact(occurrence_concept(dedup_key), …)`
(`mod.rs:1004-1043`), recalled by `recall_occurrences` filtering `o.signature == dedup_key`
(`mod.rs:972-997`), and counted in `root_cause::analyze` (`root_cause.rs:79-82`). At
`recurrence ≥ 3` (`root_cause.rs:33`), `decide_blocked_goal` flips `UnblockGoal →
EscalateBlockedGoal` (`mod.rs:1613`). Those facts are also append-only **by design** — the
append is precisely how recurrence becomes measurable. So "seen 2×" is the recurrence
mechanism functioning, not a dedup bug.

---

## 3. Observations worth flagging (not defects)

- **High-cardinality composite key.** `observation_signature` concatenates *all* current
  `dedup_key`s, so any change in the problem set (a goal unblocks, a new gap appears)
  yields a different signature that bypasses the gate. It only dedups an **identical**
  problem set — brittle as a dedup key but harmless as an activity log. The observed 2× has
  an *identical* signature, confirming the same problem set observed in two windows/lifetimes.
- **Non-durable gate ↔ append-only store** is the real mechanism behind exactly-2× across a
  restart. If cross-restart dedup of the activity log were ever desired, it would require a
  durable `caller_key`/dedup_key on `store_episode` (the adapter supports one — currently
  `None`). Not required for correctness; the recurrence path *wants* the appends.

## Bottom line

Every token is Overseer-emitted except the `goal_id` slug/hash (minted upstream). The 2× is
**two genuine observation events** (two 900 s windows or a process restart), consistent with
an append-only episodic store guarded by a non-durable in-process window gate. It is the
intended recurrence-tracking behavior, **not** a duplicate-write defect.
