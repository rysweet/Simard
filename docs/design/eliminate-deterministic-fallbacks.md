---
title: Eliminate Deterministic Fallbacks
description: Target architecture for agent-owned reasoning with typed, fail-loud deterministic safety rails.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: design
---

# Design: Eliminate Deterministic Fallbacks

**Status:** Design consolidation (Step 5e) — grounded against source at commit on
branch `fix/brain-eliminate-deterministic-fallbacks`.
**Scope:** The reasoning path (Decide / Orient / engineer-lifecycle / merge-judge /
progress-checker / distillation) and the engineer-count telemetry path.
**Issue family:** #2432 (confidence-gated escalation ladder), #2419 / #2421 /
#2429 / #2430 (parse-failure visibility & classification), #2484 (shared
sanitizer), #1678 (workboard "No spawned engineers" contradiction).

This document consolidates four investigation threads into one target
architecture. Every claim is anchored to `file:line` so implementation steps can
verify against the code rather than against prose.

---

## 1. Problem statement (grounded)

A *deterministic fallback* is a fixed default that a phase emits when it cannot
parse a real answer out of an LLM/recipe reply. The failure mode is not that
defaults exist — a floor is legitimate — but that historically the default was
**silent, unbounded in coverage, and indistinguishable from a real decision**, so
goals stalled for days at 0.00% while cycle reports still showed "decisions."

Live evidence from `~/.simard/metrics/metrics.jsonl` at consolidation time:

| Signal | Value | Meaning |
|--------|-------|---------|
| `recipe_parse_failure_total` vs `recipe_parse_success_total` | 3500 / 2952 (~54% fail) | Over half of recipe reads still miss. |
| `brain_parse_failure` events | 27,430 | Brain-phase parse misses are the dominant metric by volume. |
| `brain_verdict_parsed_total` | 2,672 | Real parsed verdicts. |
| `brain_lifecycle_decision` | 1,450 | Lifecycle decisions recorded. |
| Distill (historical, #2517 family) | ~90% batch-parse failure | Envelope-behind-banner misparse. |
| `count_live_engineers()` vs ground truth | **1** matched / **17** real | Dashboard engineer count is wrong by 16. |

The last row is a telemetry (not reasoning) fallback: the surface silently
reports a plausible-but-false number instead of the truth.

---

## 2. What already exists (do not rebuild)

Substantial machinery from the #2419/#2432/#2484 work is already in tree. The
consolidation must **build on** it, not duplicate it.

### 2.1 Shared sanitizer chokepoint — `strip_recipe_noise`
`src/recipe_output/extract.rs:219`. Strips ANSI SGR/CSI/OSC, timestamped tracing
lines, the runner's human summary banner, and the Copilot CLI launch preamble
(the `ℹ … NODE_OPTIONS … (saved preference)` marker et al., #2496). Returns
`Cow::Borrowed` on clean input (zero-copy, byte-identical), so adoption never
changes behaviour on clean output (extract.rs:33–39).

**Adopted by** (verified call sites):
- Decide / Orient / lifecycle: `recipe_brain.rs:1194, 1310, 1405, 4322`.
- Merge-judge: `stewardship/recipe_merge_judge.rs:319, 1030`.
- Progress-checker: `goal_curation/recipe_progress_checker.rs:200, 485`.
- Distillation: `memory_consolidation/distillation.rs:1373, 1437, 2685`.

**Coverage verdict:** the sanitizer chokepoint is fully adopted across every
reasoner capture path. This thread is closed.

### 2.2 Confidence-gated escalation ladder — `run_brain_ladder`
`src/ooda_brain/recipe_brain.rs:502` (generic backbone; its doc at :479–480
states it is "shared by every recipe-backed brain phase") with the bounded rung
sequence `Base → SchemaRepair → Escalate` (rung enum at recipe_brain.rs:207):
- **SchemaRepair** feeds the malformed prior output back with the closed variant
  list, asking for a valid answer (`build_phase_escalation_note`, recipe_brain.rs:282).
- **Escalate** = schema-repair + a higher-effort/step-by-step reasoning tier.
- Bounded by `EscalationConfig` (recipe_brain.rs:343): default **2** rungs,
  **HARD_CAP = 3**, env override `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS`, clamped so
  no configuration can produce an unbounded retry (recipe_brain.rs:370–374).
- Outcomes classified as `Parsed | DefaultEmpty | DefaultMalformed | Error |
  Repaired | Escalated` (recipe_brain.rs:169–181) and termination as
  `Recovered | Exhausted | InvokeError | Disabled` (recipe_brain.rs:395–409), so a
  recovered-by-repair decision is measurably distinct from a floor.

**Wired in production for four phases:**
- engineer-lifecycle: `recipe_brain.rs:461` (via `run_escalation_ladder`).
- Decide: `recipe_brain.rs:733`.
- Orient: `recipe_brain.rs:792`.
- Merge-judge: `stewardship/recipe_merge_judge.rs:145`.

### 2.3 Parse-failure visibility — `parse_failure.rs`
`src/ooda_brain/parse_failure.rs`. Every brain parse-failure fires structured
`tracing::error!`, a `record_metric("brain_parse_failure", ...)` line, and an
on-disk `ParseFailureRecord` embedded in `cycle_*.json`. At
`ISSUE_ESCALATION_THRESHOLD = 3`, eligible evidence may produce a typed issue
proposal; all autonomous mutation uses the durable stewardship mutation guard.

### 2.4 Confidence primitive — `confidence.rs`
`src/ooda_brain/confidence.rs`. Provides verbalized-confidence wrappers
(`JudgedDecision` / `JudgedLifecycle`, flat wire shape), a self-consistency
majority vote (`self_consistency_vote`, K = 3, gated to high-stakes/irreversible),
a rolling ECE `CalibrationWindow`, and the **fail-closed trust policy**:
`LOW_TRUST_CONFIDENCE = 0.0` for any *solicited-but-absent/malformed* confidence
(confidence.rs:83–116). Pure and unit-tested.

### 2.5 Deterministic floors (the legitimate defaults)
- Lifecycle: `fallback.rs:14` `DeterministicLifecycleBrain` → always
  `ContinueSkipping`; ladder-exhaustion default `default_continue_skipping`
  (recipe_brain.rs:1451).
- Decide: `decide.rs:120` `DeterministicDecideBrain` (prefix routing) and
  `default_advance_goal` with a rationale that *names the parse-miss*
  (recipe_brain.rs:1253) so a defaulted row is never mistaken for a real decision.

---

## 3. Consolidated gaps (the actual work)

The investigation shows the #2432 ladder is real and wired for the four core
brain phases, and the sanitizer is universal. What remains are **coverage
asymmetries and telemetry divergence** — the residual sources of silent defaults.

### G1 — Progress-checker is off the ladder
`goal_curation/recipe_progress_checker.rs` adopts `strip_recipe_noise` but has
**zero** references to `run_brain_ladder` / `run_escalation_ladder` (verified:
`grep -c` = 0). On a parse-miss it falls straight to its permissive default. With
progress-checks contributing heavily to the 3,500 recipe parse failures, this is
the highest-volume remaining silent default.

**Design:** route the progress-checker's recipe read through `run_brain_ladder`
using its own `(invoke, parse, default, choice-label)` closures — the backbone is
already generic (recipe_brain.rs:502, doc at :479–480: "shared by every
recipe-backed brain phase"). No new mechanism; one more adopter.

### G2 — Distillation runs a *separate* retry mechanism
`memory_consolidation/distillation.rs` uses `DISTILL_RETRY = 1` scoped to the
`CopilotTerminalFailure` class and fails **closed** on surviving parse failures
(returns `Err` **without** marking the batch, so it retries next pass — no silent
deferral; distillation.rs:71–81, 310–318). This is correct behaviour but is a
*parallel* ladder with different vocabulary and bounds.

**Design decision (recommended):** keep distillation's failure-class-aware retry
(structural failures *should* escalate immediately rather than re-prompt), but
(a) emit the same outcome vocabulary (`Repaired`/`Escalated`/`Exhausted`) so
dashboards can aggregate one story, and (b) document it here as an *intentional*
second ladder rather than an omission. Do **not** force it onto `run_brain_ladder`
— its retry predicate is genuinely different.

### G3 — `confidence.rs` is built but unwired
`JudgedDecision` / `JudgedLifecycle` / `self_consistency_vote` /
`CalibrationWindow` have **no production call sites** (verified: grep for
`confidence::Judged*` / `self_consistency_vote(` outside tests returns nothing).
The wired ladder recovers via schema-repair/tier-bump; it does **not** consume a
verbalized confidence or run a self-consistency vote.

**Design decision (recommended, phased):**
1. **Now:** treat the escalation ladder (§2.2) as the canonical #2432 path.
   Explicitly mark `confidence.rs`'s self-consistency + ECE as *reserved for
   high-stakes/irreversible lifecycle decisions* (the module's own gate:
   `is_irreversible_lifecycle`, `should_self_consistency_sample`).
2. **Next:** wire the fail-closed policy `confidence_or_low_trust`
   (confidence.rs:111) into the ladder's parse step so a *solicited* confidence
   that comes back absent/garbage forces at least one escalation rung instead of
   trusting a `1.0` default. This is the smallest change that makes the confidence
   primitive load-bearing.
3. **Later (optional):** enable the K× self-consistency vote only for irreversible
   lifecycle actions (`OpenTrackingIssue`, `ReclaimAndRedispatch`,
   `MarkGoalBlocked`) where the K× cost is justified.

Avoid the trap of two competing confidence stories: the ladder is primary; the
confidence module supplies the *trust floor* and the *high-stakes vote*, not a
second escalation path.

### G4 — Three divergent engineer-count sources (telemetry fallback)
There is no single source of truth for "how many engineers are live":

| Source | Mechanism | Used by |
|--------|-----------|---------|
| `status/provider.rs:203` `count_live_engineers()` | `pgrep -f -c "simard-engineer\|RustyClawd\|copilot.*--auto"` | `StatusSnapshot.resources.live_engineers`, dashboard Overview, overseer sensor |
| `ooda_brain/context.rs:111` `count_live_engineer_claims()` | `.simard-engineer-claim` heartbeat files with a live PID | brain lifecycle context (`in_flight_engineer_count`) |
| `operator_commands_dashboard/workboard.rs:202` | `subagent_sessions::load()` filtered to `ended_at.is_none()` | dashboard Workboard "Active Engineers" |

These disagree with each other and with reality. The claim-file count
(context.rs) is the most truthful (it verifies a live PID per real engineer
worktree), yet it is **not** what either dashboard surface renders.

**Design:** promote `count_live_engineer_claims()` to the single source of truth
for "live engineers," and have `count_live_engineers()` (status) and the workboard
panel both derive from it (or from one shared function). The subagent-session
registry and pgrep become *diagnostic cross-checks*, not the authoritative number.
Rationale: the claim file is written by the spawn path itself and gated on a live
sentinel PID, so it cannot silently drift the way a name-pattern or a
registration side-effect can.

### G5 — The `count_live_engineers()` pattern bug (root cause of "0/undercount")
`status/provider.rs:205` greps for `simard-engineer` (**hyphen**), but engineers
are actually spawned as `simard engineer run single-process …` (**space** — the
subcommand form, confirmed from live `ps` argv). The hyphenated pattern therefore
never matches a real engineer; the current live reading is **1 matched vs 17
real**. `RustyClawd` and `copilot.*--auto` are also stale/rare argv shapes.

**Design:** this bug is *subsumed by G4* — once the surface derives from
`count_live_engineer_claims()`, the fragile argv pattern is retired. If a
process-liveness cross-check is still wanted, fix the pattern to `simard engineer`
(space) and match the real `single-process` argv, but do not let it be the
authoritative count.

---

## 4. Target architecture & invariants

The unifying principle: **a deterministic default is a floor reached only after a
bounded, visible escalation — never a silent first response, and never a
plausible-but-false number.**

Four invariants the implementation must hold:

1. **Sanitize once, everywhere.** Every recipe/LLM reader passes through
   `strip_recipe_noise` before extraction. (Already true — §2.1.)
2. **Escalate before you floor.** Every reasoning phase that can parse-miss runs a
   *bounded* escalation before emitting its deterministic default: the shared
   `run_brain_ladder` for Decide/Orient/lifecycle/merge-judge/**progress-checker
   (G1)**, and distillation's failure-class-aware retry for distillation (G2).
   Bounds come from `EscalationConfig` (default 2, hard cap 3).
3. **Every floor is loud and labelled.** A defaulted outcome always carries a
   distinct `LifecycleParseOutcome` and a rationale that names the miss, and fires
   the four `parse_failure.rs` visibility channels. No outcome may be
   indistinguishable from a real decision. (Already true — §2.3, §2.5.)
4. **One truth per number.** Any operator-facing count derives from a single
   authoritative function; alternate computations are cross-checks, not
   substitutes. (New — G4/G5 for engineer counts.)

Trust boundary for solicited confidence: absent/malformed → `LOW_TRUST_CONFIDENCE`
(0.0), never the cheerful `default_confidence()` (1.0). Wiring this into the
ladder parse step is G3-step-2.

---

## 5. Acceptance criteria

- **A1 (G1):** progress-checker parse-misses run the bounded ladder; its
  parse-failure rate drops and `Repaired`/`Escalated` outcomes appear in metrics.
- **A2 (G4/G5):** `resources.live_engineers` and the Workboard "Active Engineers"
  panel both equal `count_live_engineer_claims()` and match the true live count
  (17 vs 17, not 1/0 vs 17) in a live check.
- **A3 (G3-step-2):** a solicited-but-malformed confidence forces ≥1 escalation
  rung rather than defaulting to trust; covered by a unit test over the ladder
  parse step using `confidence_or_low_trust`.
- **A4 (invariant 3):** no code path emits a deterministic default without a
  distinct outcome label + a parse-failure visibility fire; enforced by existing
  outcome-classification tests plus a new grep-gate in CI if practical.
- **A5 (G2):** distillation emits the shared outcome vocabulary so one dashboard
  aggregates brain + distill parse health.
- **A6 (bounded):** no escalation path can exceed `EscalationConfig::HARD_CAP`;
  already unit-tested (recipe_brain.rs:3550–3569), extend to G1's adopter.

---

## 6. Non-goals

- Rewriting the deterministic floors themselves. They are the correct safety net
  when no LLM is configured (fallback.rs:1–2, decide.rs:116–119) and must remain
  bit-for-bit.
- Removing the pgrep / subagent-session computations — they stay as diagnostics.
- Enabling the K× self-consistency vote broadly; it stays confined to
  high-stakes/irreversible decisions to bound cost (G3-step-3, optional).
- Changing the sanitizer (§2.1 is complete).

---

## 7. Thread → gap traceability

| Investigation thread | Verdict | Resulting gap(s) |
|----------------------|---------|------------------|
| parse-fail → deterministic-default flow | Ladder wired for 4 phases; floors are loud/labelled | G1 (progress-checker), G3 (confidence trust floor) |
| extract.rs chokepoint coverage | Complete — universal adoption | none (closed) |
| distillation parser failure | Fails closed with own bounded retry | G2 (vocabulary alignment only) |
| active-engineers telemetry | 3 divergent sources; pattern bug undercounts 17→1 | G4 (single source), G5 (pattern bug, subsumed by G4) |

---

## 8. Recommended sequencing

1. **G4 + G5** (single engineer-count source) — highest operator-visible impact,
   smallest blast radius, no reasoning-path risk.
2. **G1** (progress-checker onto the shared ladder) — highest parse-failure-volume
   reduction; reuses the generic backbone.
3. **G3-step-2** (fail-closed confidence into the ladder parse step) — makes the
   confidence primitive load-bearing with one focused change.
4. **G2** (distillation outcome-vocabulary alignment) — reporting-only.
5. **G3-step-3** (optional high-stakes self-consistency vote) — deferred.
