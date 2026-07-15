# Secondary Investigation — Per-Token Provenance & Snapshot Membership Delta

**HEAD:** `388e6c29` (prior docs anchored at `85b9398a` / `dea65df8` — re-grounded)
**Focus:** Per-token provenance and membership deltas across the two overlapping
`overseer-obs:` snapshots, including the `workstream-gap` lane.

---

## Bottom line

The two occurrences are **overlapping-but-different snapshots** of a (nearly)
static blocked-work set, NOT byte-identical duplicates and NOT a hash/counter
artifact. **Every token in the aggregate is a stable literal or stable ID** —
including the NEW `resource:engineer_spawn` token, whose live `live` count is
confined to the summary and never reaches the `dedup_key`/signature. The `2×`
is a faithful re-observation of an unremediated problem set. The repeated
nested `overseer-obs:` fragments come from **self-referential write-back**, not
within-snapshot duplication.

---

## 1. Per-token provenance & stability (`classify_signal`, mod.rs:1204+)

`observation_signature` (mod.rs:1068-1073) = `overseer-obs:` + sorted, **deduped**
join of each `Problem.dedup_key`. So token stability = `dedup_key` stability.

| Token (dedup_key) | Signal source | Line | Volatile field | Where it lands | Stable? |
|---|---|---|---|---|---|
| `goal:blocked:<goal_id>` | `GoalBlocked` | 1336 | `consecutive_no_action`, `needs_review` | summary only | ✅ |
| `workstream-gap` (literal) | `WorkstreamGap{gaps}` | 1371 | `gaps.len()` | summary only | ✅ |
| `resource:engineer_spawn` (literal) | `EngineerSpawnRate{live}` | 1270 | `live` count | summary only | ✅ |
| `overseer-obs:...` (recalled) | `RecurringSignature{signature}` | 1359 | `occurrences` | summary only | ✅ (key = sanitized signature) |

**DRIFT check (mandatory) — `resource:engineer_spawn`:** RESOLVED. Despite being
a live-count signal, `dedup_key` is the fixed string `"resource:engineer_spawn"`
(mod.rs:1270); `{live}` appears only in `"elevated engineer spawn ({live} live)"`
(summary → `observation_content`, mod.rs:1079-1088), never in the signature.
Structurally identical to `goal:blocked` (count in summary) and `workstream-gap`
(`gaps.len()` in summary). **No volatile component leaks into the signature.**

**Verdict:** the signature is a deterministic function of the *membership set*.
Different membership ⇒ different signature ⇒ both legitimately recorded (this is
correct behavior, not a dedup miss).

---

## 2. `workstream-gap` — TWO INDEPENDENT LANES (confirmed)

1. **Notification lane** — `act_flag_workstream_gaps` (mod.rs:884-945): `gap_gate`
   keyed **per gap** on `format!("workstream-gap:{}", g.signature)` (mod.rs:901,932),
   15-min window. Dedups operator emails/Signal.
2. **Observation-signature lane** — `classify_signal` mints **ONE** consolidated
   `Problem` with `dedup_key = "workstream-gap"` (literal, mod.rs:1371); test
   pins this exact key (tests_gap_scan.rs:857). This is the token that enters
   `observation_signature`.

The lanes never cross. In a single Observe pass, `orient` merges all same-key
problems (mod.rs:1211), so `"workstream-gap"` collapses to **one** token, and
`observation_signature`'s `keys.dedup()` (mod.rs:1071) guarantees it appears
**at most once** per snapshot.

⇒ Any `workstream-gap|workstream-gap` seen *within one* aggregate can only arise
from **nested recalled `overseer-obs:...` tokens** (each carrying their own
`workstream-gap`), which are distinct strings and survive dedup — i.e. the
`workstream-gap` lane is independent of `goal:blocked` and does not itself
duplicate.

---

## 3. Self-referential feedback = source of repeated `overseer-obs:` fragments

Closed loop, all lines at HEAD `388e6c29`:

1. `write_back_observation` stores episode content `"... [sig:overseer-obs:<keys>]"`
   (wiring.rs:1084) + metadata copy.
2. Next Observe: `parse_failure_signature` extracts `overseer-obs:...` from the
   `[sig:...]` marker (wiring.rs:976-986, 1025).
3. `signals_from` counts identical `failure_signature`s; `≥ RECURRING_SIGNATURE_THRESHOLD (2)`
   ⇒ `RecurringSignature{ signature:"overseer-obs:...", occurrences }` (signal.rs:455-469, 362).
4. `classify_signal` ⇒ `Problem{ dedup_key = sanitize_recalled("overseer-obs:...") }`
   (mod.rs:1359) — a **distinct** key that does NOT same-key-merge with any
   `goal:blocked` problem (mod.rs:1211).
5. Next `write_back_observation` folds that whole prior `overseer-obs:...` in as
   **one sorted token** ⇒ nested `overseer-obs:...overseer-obs:...`.

Sort order (`goal` < `overseer-obs` < `resource` < `workstream-gap`) exactly
matches the displayed aggregate: goal:blocked:* first, the embedded
`overseer-obs:...` recall token mid-string, then `resource:engineer_spawn`, then
`workstream-gap`. This is the **write-back-of-recall-derived-meta-problem**
pattern (PATTERNS.md), confirmed unchanged at HEAD.

---

## 4. Membership delta table (the two overlapping snapshots)

| Entry | Snapshot A (earlier) | Snapshot B (later) | Class |
|---|---|---|---|
| `goal:blocked:advance-...-kgpacks-...-parity` | ✅ | ✅ | PERSIST |
| `goal:blocked:audit-simard-...-coverage-...70` | ✅ | ✅ | PERSIST |
| `goal:blocked:build-a-local-coin-benchmark-...` | ✅ | ✅ | PERSIST |
| `goal:blocked:fix-agent-kgpacks-rs-issue-12/17/18/23/25` | ✅ | ✅ | PERSIST |
| `goal:blocked:simard-identity-atelier` | ✅ | ✖ | **DROP (unblocked)** |
| `goal:blocked:simard-identity-bursar` | ✅ | ✖ | **DROP** |
| `goal:blocked:simard-identity-cartographer` | ✅ | ✖ | **DROP** |
| `goal:blocked:simard-identity-concierge` | ✅ | ✖ | **DROP** |
| `goal:blocked:simard-identity-gastronome` | ✅ | ✖ | **DROP** |
| `resource:engineer_spawn` | ✖ | ✅ | **APPEAR (new)** |
| `workstream-gap` (+ nested recall copies) | ✅ | ✅✅ | PERSIST/GROW |
| embedded `overseer-obs:...issue-17|workstream-gap|workstream-gap` | — | ✅ | nested recall |

- **PERSIST:** the 8 kgpacks/core goals — unremediated across both passes.
- **DROP (A→B):** the 5 `simard-identity-*` goals resolved/unblocked between passes.
- **APPEAR (A→B):** `resource:engineer_spawn` + extra `workstream-gap` (via nested recall).

Because A ≠ B in membership, `observation_signature(A) ≠ observation_signature(B)`
by design — so both were stored, and the recall counter later saw the recurring
*family* prefix ≥2× ⇒ emitted `RecurringSignature`. **Loop, not artifact.**

---

## 5. Recurring pattern class uniting the tokens

All three token families are **observe-and-flag problems with no closing action**:
- `goal:blocked:*` (GoalHygiene) — goals blocked, awaiting resource/human review,
- `workstream-gap` (WorkstreamCoverage) — uncovered backlog work,
- `resource:engineer_spawn` (ResourcePressure, NEW) — elevated live engineer spawn.

Causally these are **one under-throughput/under-resourcing problem in three
views**: the system *is* spawning engineers (`engineer_spawn` up) yet goals stay
blocked and gaps stay uncovered. They recur because nothing remediates them and
they sit in the **"2× dead zone"**: deduped by the 15-min `write_back_gate`
(mod.rs:548) yet below `RECURRENCE_ESCALATION_THRESHOLD = 3` (root_cause.rs:33,
gate mod.rs:1613) — flagged forever, escalated never.

---

## 6. Integration points / concerns

- **Two counter lanes** (independent thresholds): emit at `RECURRING_SIGNATURE_THRESHOLD=2`
  (signal.rs:362) vs escalate at `RECURRENCE_ESCALATION_THRESHOLD=3` (root_cause.rs:33).
  The `2×` sits in the gap between them.
- **Signature fix seam** (symptom): exclude recall-derived `RecurringSignature`
  keys from `write_back_observation` (mod.rs:546 / the `RecurringSignature` arm at
  mod.rs:1359) OR carry count in content — cuts the nested `overseer-obs:` growth.
  This does NOT touch `stewardship/dedup.rs` (a different GitHub-issue path —
  confirmed dead end; the composite never flows through `find_existing`).
- **True lever** (root): a remediation/escalation rung at first proven recurrence
  (2×) for the `goal:blocked` + `workstream-gap` + `engineer_spawn` convergence.

---

## Questions for verification phase

1. Confirm `simard-identity-*` goals genuinely **transitioned to unblocked**
   (goal-board) between the two passes vs. merely dropping out of recall ranking.
2. Confirm `resource:engineer_spawn` fires from real elevated live-spawn telemetry
   at the time of snapshot B (not a one-off spike) — governs whether it belongs
   in the convergence class or is incidental.
3. Confirm the symptom fix (cut self-referential write-back) preserves the two
   idempotency tests: observer.rs dedup-signature test and tests_gap_scan.rs:857
   `workstream-gap` dedup_key. INVESTIGATION-ONLY — do not implement.
