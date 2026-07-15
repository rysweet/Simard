# Tertiary Investigation (7th wave) — Minimal Signature-Path Fix & Prior-Findings Reconciliation

**Role:** Tertiary investigator (architect).
**HEAD:** `5a85317b` — verified.
**Date:** 2026-07-15.
**Mandate:** (a) define the *minimal contained* signature-path fix — exclude
recall-derived problems from the observation write-back so the self-referential
`overseer-obs:…|overseer-obs:…` nesting cannot form; (b) reconcile the prior
`ai_working/investigation/` artifacts (pinned to `85b9398a` / `388e6c29` /
`0289572e` / `dea65df8`) against current HEAD. **Investigation-only — no code changed.**

---

## 0. Reconciliation verdict (mandate half b)

**The prior investigation is sound and re-grounds exactly at HEAD. Do not restart.**

The four commits between the last cited HEAD and now are **documentation-only**:

```
5a85317b 0289572e 388e6c29 85b9398a   (docs(investigation): …)
git diff --stat dea65df8..HEAD -- src/   →  (empty)
```

Because `src/` was **not touched** across the entire docs-commit chain, every
load-bearing line number the older-pinned docs cite is **still exact at HEAD**.
Re-read directly from source at `5a85317b`:

| Claim | Cited loc (docs) | Re-read @ HEAD | Status |
|---|---|---|:--:|
| `observation_signature` = `sort_unstable`→`dedup`→`format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1068-1073` | `mod.rs:1068-1073` | ✅ exact |
| `write_back_observation` records **all** problems through one gate | `mod.rs:534-563`; `wiring.rs:301` | same | ✅ exact |
| Recall-derived `RecurringSignature` → `ProcessHealth` problem, `dedup_key = sanitize_recalled(signature)` | `mod.rs:1353-1363` | `mod.rs:1353-1363` | ✅ exact (self-nesting live) |
| `WorkstreamGap` → `WorkstreamCoverage`, fixed key `"workstream-gap"` | `mod.rs:1368-1373` | `mod.rs:1371` literal | ✅ exact |
| `resource:engineer_spawn` fixed key | `mod.rs:1270` | `mod.rs:1270` literal | ✅ exact |
| `goal:blocked:{goal_id}` key | `mod.rs:1336` | `mod.rs:1336` | ✅ exact |
| `RecurringSignature` emits at `occurrences >= 2` | `signal.rs:362,463` | `signal.rs:362,463-464` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | `root_cause.rs:33` | ✅ exact |
| `decide_blocked_goal` escalates only at `recurrence >= 3` | `mod.rs:1613` | `mod.rs:1613` | ✅ exact |
| `WorkstreamCoverage` Decide arm → notify-only `FlagWorkstreamGaps` | `mod.rs:1534-1543` | `mod.rs:1534-1543` | ✅ exact |
| `record_occurrence` → append-only `store_fact` (ratchet) | `mod.rs:1034` | `mod.rs:1034` | ✅ exact |

**Net drift: zero.** The single documentation-level correction already captured
in `RECONCILIATION_LEDGER.md §2` (the §6.2b `store_fact_with_caller_key`
one-liner is a *never-escalate* trap, superseded by the count-in-content upsert)
still stands and needs no revision. The `×2` remains a **faithful cross-window
recurrence count of a static, unresolved problem set — a real re-observation
loop, not a storage/dedup/replay artifact.**

---

## 1. D1 self-referential nesting — full assembly trace @ HEAD

The literal `overseer-obs:goal:blocked:…|goal:blocked:…|workstream-gap` shape
(with `overseer-obs:`-prefixed tokens sitting *inside* a fresh `overseer-obs:`
signature) is produced by a closed 5-hop loop, every hop verified in source:

```
 (1) prior tick stores an episode whose failure_signature is an
     observation_signature  →  "overseer-obs:goal:blocked:X"
        mod.rs:1072   format!("overseer-obs:{}", keys.join("|"))
        mod.rs:546-556  write_back_observation → record_observation(episode{signature})
                                    │  (recalled next tick)
                                    ▼
 (2) signals_from() counts recalled failure_signatures; ≥2 identical →
     Signal::RecurringSignature{ signature:"overseer-obs:goal:blocked:X", occurrences }
        signal.rs:455-469  (RECURRING_SIGNATURE_THRESHOLD = 2, signal.rs:362)
                                    │
                                    ▼
 (3) classify_signal maps it to a Problem with
     dedup_key = sanitize_recalled("overseer-obs:goal:blocked:X")   ── still "overseer-obs:…"
        mod.rs:1353-1363
                                    │
                                    ▼
 (4) orient() puts it in cycle.problems as a STANDALONE High ProcessHealth
     problem (no fresh problem shares an "overseer-obs:" key, so it never merges)
        mod.rs:1200-1235
                                    │
                                    ▼
 (5) write_back_observation(&cycle.problems) joins ALL dedup_keys, including the
     "overseer-obs:…" one, into a NEW signature → nesting deepens by one level
        wiring.rs:301 → mod.rs:534,546 → observation_signature(mod.rs:1068)
        └── loops back to (1) with a longer composite
```

**Per-token provenance of the reported string (all fixed constants, not hashes):**

- `overseer-obs:…` prefixed tokens → **recall echoes** from hop (3), `mod.rs:1359`.
- `goal:blocked:{id}` → fresh `Signal::GoalBlocked`, `mod.rs:1336`.
- `workstream-gap` → fixed literal, `mod.rs:1371` (one consolidated coverage problem).
- `resource:engineer_spawn` → fixed literal, `mod.rs:1270`.

Confirmed: `workstream-gap` and `resource:engineer_spawn` are **stable dedup-key
constants**, not per-item hashed entries — so their repetition in the corpus is
re-observation of the *same* unremediated condition, not key churn.

---

## 2. Minimal contained fix — exclude recall-derived problems from write-back

### 2.1 The discriminator, and why it is exact

A recall-derived self-observation echo is **uniquely identifiable** by its
`dedup_key` prefix `"overseer-obs:"`. Proof of exactness at HEAD:

1. **Only one producer** of the `overseer-obs:` prefix exists in the whole tree —
   `observation_signature` at `mod.rs:1072` (`grep "overseer-obs:" src/overseer/*.rs`
   returns exactly that line). No fresh `classify_signal` arm emits that prefix;
   fresh keys are `process:…`, `resource:…`, `goal:…`, `workstream-gap`,
   `quality:…`, `delivery:…`, `loop:…`, `drift:…`, `anomaly:…`.
2. The prefix reaches a Problem's `dedup_key` **only** via the RecurringSignature
   recall arm (`mod.rs:1359`), i.e. only when the recalled `failure_signature`
   was itself a prior observation signature.
3. `sanitize_recalled` (`capabilities.rs:468-482`) **preserves the prefix**: it
   only blanks control chars and truncates from the *end* on length cap
   (`RECALLED_TEXT_MAX_LEN`). Position-0 `overseer-obs:` always survives — so the
   filter is robust even against very long nested composites.

### 2.2 Why the prefix filter beats an "evidence == RecurringSignature" filter

Filtering on *"evidence contains `Signal::RecurringSignature`"* would be **wrong**:
a RecurringSignature whose recalled signature is a **domain** key (e.g.
`goal:blocked:X`, not an `overseer-obs:` one) has `dedup_key = "goal:blocked:X"`
and therefore **merges** into the genuine fresh blocked-goal problem in
`orient` (`mod.rs:1211-1219`), raising its priority. That merged problem is a
**real** current observation and must stay in the write-back. The
`starts_with("overseer-obs:")` filter excludes **only** the truly self-referential
standalone echoes and leaves every genuine (possibly recall-*boosted*) problem
intact. This is the tighter, safer boundary.

### 2.3 The change (single seam, `write_back_observation`, `mod.rs:534-563`)

Filter before signature/content assembly; if nothing survives, treat as a clean
tick (write nothing — consistent with the existing empty-set guard at `mod.rs:543`):

```rust
pub fn write_back_observation(&mut self, problems: &[Problem])
    -> Result<Option<RecordOutcome>, OverseerError>
{
    if !self.memory_recall_enabled { return Ok(None); }

    // #2628 hygiene: never re-record the Overseer's own recalled observation
    // echoes. A RecurringSignature recalled from a prior write-back carries an
    // `overseer-obs:` dedup_key (the ONLY producer of that prefix is
    // observation_signature, mod.rs:1072). Re-recording it nests overseer-obs:
    // inside the next signature — the self-referential loop. Genuine problems
    // that a recall co-signal merely BOOSTED keep their domain key and survive.
    let fresh: Vec<Problem> = problems.iter()
        .filter(|p| !p.dedup_key.starts_with("overseer-obs:"))
        .cloned()
        .collect();
    if fresh.is_empty() { return Ok(None); }

    let signature = observation_signature(&fresh);
    // …unchanged: gate.peek → record_observation(observation_content(&fresh)) → gate.commit
}
```

Cost: ~4 lines + a bounded clone of a tiny per-tick `Vec`. No signature/content
helper signature change required (`observation_signature`/`observation_content`
already take `&[Problem]`).

### 2.4 Blast radius (what this fix deliberately does NOT touch)

- It does **not** remove the RecurringSignature problem from `cycle.problems`;
  Decide/Act still see it (it still drives its `ProcessHealth → LaunchRecipe`
  intervention at `mod.rs:1429`). Scope is **write-back only**, exactly as the
  mandate frames "exclude recall-derived problems from write-back".
- It does **not** alter the `WhisperGate` window, `dedup()`, or the two counter
  lanes. It removes the **nested shape**; it does not by itself make the
  underlying static problem set converge.

---

## 3. Where D1 sits relative to the systemic defect set (boundaries)

D1 is one of three independent defects on three seams (confirmed unchanged from
`tertiary_architecture_VALIDATION_HEAD.md §1`, re-grounded here):

| # | Seam | Live @ HEAD | Fix independence |
|---|---|---|---|
| **D1** | Emission hygiene — write-back nests recall echoes | `mod.rs:534-563` + `mod.rs:1353-1363` | **Independently shippable.** Removes only the `overseer-obs:…\|overseer-obs:…` shape. |
| **D2** | Escalation counter — WHY double-gate starves accrual (`cycle.rs:582-702`) **and** append-only ratchet (`mod.rs:1034`) | live | Gate + counter must ship **atomically**; not D1's concern. |
| **D3** | Closing edge — `WorkstreamCoverage` Decide arm notify-only, no `launch.rs` edge, no cross-window ledger | `mod.rs:1534-1543` | Independent; converges the `workstream-gap` family. |

**Structural note / honest scope limit:** D1 alone makes the *fingerprint* stop
nesting, but the **count still recurs at ~2** because the problem set stays
static (that is D2+D3). D1 is the *cheapest, orthogonal, lowest-risk* fix and the
correct answer to *this* mandate ("minimal contained signature-path fix"); it is
**not** a substitute for closing the D2 latch or the D3 rung. Recommended landing
order is unchanged: **D2 (atomic gate+counter) → D3 (closing rung) → D1
(this filter) → convergence gauges.**

---

## 4. Residual uncertainties (explicitly marked)

- **Window-vs-restart origin of the `×2`** — `write_back_gate`/`gap_gate` are
  in-process `WhisperGate` `HashMap`s (per-process, no cross-restart memory), so
  whether the two observations came from two 900 s windows or two daemon restarts
  is **not decidable from static source** — log-only residual, does not change the
  fix (real re-observation either way).
- **Recipe-launch echo** — the recall-derived ProcessHealth problem also triggers
  `LaunchRecipe` (`mod.rs:1429`) with the recalled summary as task_description.
  Whether the Overseer should *act* on its own bookkeeping echo (not just avoid
  re-recording it) is a **broader emission-hygiene question** outside this
  minimal write-back fix; flagged for the D-set owner, not fixed here.

## 5. Answer to the mandate

1. **Minimal contained fix:** a ~4-line `dedup_key.starts_with("overseer-obs:")`
   filter inside `write_back_observation` (`mod.rs:534-563`), proven exact by the
   single-producer + prefix-preserving-`sanitize_recalled` argument (§2.1), and
   safer than an evidence-based filter (§2.2). Breaks the self-referential
   nesting without perturbing genuine `goal:blocked` / `workstream-gap`
   recurrence signalling.
2. **Reconciliation:** prior artifacts re-ground with **zero line drift** at HEAD
   `5a85317b` (docs-only commit chain); analysis is sound; the one previously
   flagged remedy trap (§6.2b) remains correctly superseded. Extend, do not restart.
