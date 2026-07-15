# Secondary Investigation — Token Idempotency / Dedup Namespacing & the
# workstream-gap → resource:engineer_spawn "orchestration cycle" (defect vs steady-state)

**Role:** SECONDARY (pattern / idempotency / orchestration-cycle classification)
**HEAD:** `b9f99879` — `git diff --name-only <prior-bases>..HEAD -- '*.rs'` empty; every
cited line is byte-identical to all prior waves. Re-verified live below.
**Verdict:** Prior secondary conclusions HOLD and are EXTENDED with a sharper
classification of the "cycle." The framing "workstream-gap → engineer_spawn
orchestration cycle" is a **misnomer**: there is **no causal code edge** between the two.

---

## S1 — Two DISTINCT dedup namespaces (they never mix)

| Namespace | Minted by | Shape | Consumed by |
|---|---|---|---|
| **Overseer Problem `dedup_key`** | `signal_to_problem` (mod.rs:1262-1371) | colon-namespaced literal / stable ID (`goal:blocked:<id>`, `resource:*`, bare `workstream-gap`) | `observation_signature` (mod.rs:1068-1073) → cognitive-memory episode |
| **Stewardship `failure_signature`** | `stewardship/dedup.rs:63` | `sha256(kind + ANSI/whitespace/token-redacted text)` | GitHub **issue** dedup via `find_existing` (dedup.rs:78) / routing |

The investigation signature (`overseer-obs:…|goal:blocked:…|workstream-gap|resource:engineer_spawn`)
is composed **entirely from overseer `dedup_key`s** — it never touches the sha256
`failure_signature` namespace. The two dedup systems are orthogonal (issue-filing dedup
vs. observation-write-back dedup). Answer to "do these tokens share a dedup namespace?":
**Yes — they all share the single `overseer-obs:` composite namespace, and NO — that
namespace is distinct from the stewardship issue-dedup namespace.**

## S2 — Every token is idempotently keyed; signature is a stable membership fingerprint, NOT an inflating join

- `goal:blocked:<goal_id>` (mod.rs:1336) — stable goal ID; volatile `consecutive_no_action`
  / `needs_review` confined to summary + priority.
- `workstream-gap` (mod.rs:1371) — fixed literal; `gaps.len()` in summary only.
- `resource:engineer_spawn` (mod.rs:1270) — fixed literal; `{live}` in summary only.
- Orient merges same-`dedup_key` problems (mod.rs:1211) and `keys.dedup()` collapses
  adjacent equals (mod.rs:1071) ⇒ **each family key appears at most once per snapshot**.

**Conclusion (rejects the "vacuous aggregate-join that inflates indefinitely" hypothesis):**
the signature is a **deterministic function of the open-problem membership SET**. A *fixed*
set of stuck goals yields a **stable** signature; adding/removing a member changes it
(benign §11 membership drift). The `×2` is **cross-window re-observation** of an unchanged
set, not per-goal inflation. It is a genuine **failure fingerprint of the current
open-problem set**, not a vacuous join. Nested `overseer-obs:…|…|overseer-obs:…` fragments
that survive `dedup()` are distinct recall strings (self-write-back), not a counting bug.

## S3 — HEADLINE: workstream-gap → resource:engineer_spawn is NOT a causal orchestration cycle

| Token | Emission | Overseer's action |
|---|---|---|
| `workstream-gap` | `detect_workstream_gaps` (sensor.rs:288) — uncovered p1/p2 goals/issues with no executor | `FlagWorkstreamGaps` → `act_flag_workstream_gaps` (mod.rs:884): **operator notification ONLY** (email+Signal), gated by per-gap `gap_gate` keyed `workstream-gap:{g.signature}`. Launches no workstream, files no issue, **spawns no engineer.** |
| `resource:engineer_spawn` | overseer merely **observes** telemetry: `state.live_engineers >= ENGINEER_SPAWN_THRESHOLD(8)` (signal.rs:351,393-396) | passive resource-pressure metric; `ResourcePressure → Escalate{reason}` (mod.rs:1444) |

The **actual** engineer spawning lives in a **different subsystem** — the OODA loop:
`dispatch_spawn_engineer` (cycle.rs:665) and `no_progress.rs` `SpawnEngineer` arm
(no_progress.rs:712-713), driven by no-progress resolution and **bounded to one guided
retry** per threshold (`mark_guided_retry`, no_progress.rs:716).

⇒ **No code edge connects workstream-gap to engineer_spawn.** They co-occur in the
composite signature because both conditions were true in the same observation window:
engineers maxed out (≥8 live) **and** backlog coverage incomplete. That is a real
**under-resourced system STATE**, not an orchestration loop. This confirms the prior
pattern *"two signatures, one root problem"* (under-resourced work oscillates
gap(active/uncovered) ↔ blocked(idle)).

## S4 — Defect vs steady-state (split verdict)

- **`resource:engineer_spawn` side = STEADY-STATE / benign.** Passive telemetry; count in
  summary only; real spawn path (OODA) is bounded (one guided retry). No unfulfilled-spawn
  defect at the overseer boundary. Matches prior "benign membership drift."
- **`workstream-gap` side = DEFECT**, but the **"observe-and-flag-without-closing"** defect
  (missing convergence rung), NOT an orchestration cycle. `act_flag_workstream_gaps`
  notifies but never closes → the gap recurs. **Same shared root cause** as the
  `goal:blocked` cluster: one resourcing/convergence problem surfaced through multiple
  lenses. WorkstreamCoverage is the only High-priority Decide arm with no `launch.rs` edge.

## S5 — Reconciliation with the RECONCILIATION_LEDGER traps

- **§2 CallerKey trap (Lane B) is orthogonal to my focus and CONFIRMED.** The TOKENS are
  already idempotent (S2); the storage-idempotency gap is on the *occurrence counter*
  (`record_occurrence` → non-idempotent `store_fact`, mod.rs:1034). The correct fix carries
  the count IN content (occurrence_count + first_seen/last_seen); `store_fact_with_caller_key
  (root_cause_signature)` would collapse recall to 1 forever and make escalation
  (mod.rs, `recurrence >= 3`) dead code. Do NOT conflate token idempotency with this fix.
- **INV-GAP-KEY trap CONFIRMED and directly relevant to my cycle:** any remediation rung for
  workstream-gap must key on `GapItem.signature` (per-gap), NOT the bare `"workstream-gap"`
  dedup_key — else all gaps fold into one issue. The bare literal is *correct* for the
  observation SIGNATURE (idempotent membership) but *wrong* as a remediation/issue key.

## Questions for verification phase

- **Q1:** Confirm `ProblemKind::ResourcePressure → Escalate` (mod.rs:1444) for the
  Priority::Normal `engineer_spawn` problem is gated (priority filter / escalation dedup)
  before firing — so elevated-but-normal spawn rate cannot escalate spuriously. Prior waves
  say benign; a targeted check on the Normal-priority Decide/Act filter would close it.
- **Q2:** Confirm the OODA guided-retry bound (`mark_guided_retry`, no_progress.rs:716)
  cannot be re-armed every cycle under sustained gaps (no unbounded-spawn path).
