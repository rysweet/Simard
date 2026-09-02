---
title: Outcome-verification API reference
description: >
  Reference for the closed-loop outcome-verification step — the GoalOutcomeCtx /
  GoalOutcomeDecision / LiveSignal types, the LiveSignalSource trait, the
  OodaBrain::decide_goal_outcome_verification method, the gather→reason→apply
  seam and its three thin rails, the OodaClients bridge fields, the reasoning
  recipe, the sanitization boundary, the observability record and metric, and
  the SIMARD_OUTCOME_VERIFY kill-switch.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/closed-loop-outcome-verification.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/self-deploy-api.md
  - ../reference/telemetry-metrics.md
  - ../howto/diagnose-a-reopened-goal.md
  - ../operations/outcome-verification-kill-switch.md
  - ../../src/goal_curation/outcome_verify.rs
  - ../../src/goal_curation/live_signal.rs
  - ../../src/ooda_brain/mod.rs
  - ../../src/ooda_loop/types.rs
---

# Outcome-verification API reference

> **Status: implemented.** The types, trait, seam, rails, and kill-switch below
> live in
> [`src/goal_curation/outcome_verify.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/outcome_verify.rs)
> and
> [`src/goal_curation/live_signal.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/live_signal.rs);
> `GoalOutcomeCtx` / `GoalOutcomeDecision` and the
> `OodaBrain::decide_goal_outcome_verification` method live in
> [`src/ooda_brain/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/mod.rs);
> the bridge fields on `OodaClients` live in
> [`src/ooda_loop/types.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/types.rs);
> and the reasoning asset is
> [`prompt_assets/simard/recipes/ooda-goal-outcome-verification.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/ooda-goal-outcome-verification.yaml).
> The production daemon installs the bridge pair on the OODA curate seam unless
> `SIMARD_OUTCOME_VERIFY=off`.

This reference specifies the API for the closed-loop outcome-verification step.
For the rationale, see
[closed-loop-outcome-verification](../concepts/closed-loop-outcome-verification.md).
The step lives alongside the [completion-evidence
gate](completion-evidence-gate-api.md) in `src/goal_curation/` and shares the
goal types in `src/goal_curation/types.rs`.

## Contents

- [`LiveSignal`](#livesignal)
- [`LiveSignalSource`](#livesignalsource)
- [`GoalOutcomeCtx`](#goaloutcomectx)
- [`GoalOutcomeDecision`](#goaloutcomedecision)
- [`OodaBrain::decide_goal_outcome_verification`](#oodabraindecide_goal_outcome_verification)
- [The seam and the three rails](#the-seam-and-the-three-rails)
- [`OodaClients` bridge fields](#oodaclients-bridge-fields)
- [The reasoning recipe](#the-reasoning-recipe)
- [Sanitization boundary](#sanitization-boundary)
- [Observability](#observability)
- [Kill-switch](#kill-switch)
- [Test matrix](#test-matrix)

## `LiveSignal`

One authenticated observation of the goal's real-world effect. Ephemeral —
gathered fresh each cycle, never persisted raw.

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSignal {
    /// Trusted origin of the observation, e.g. "self_metrics", "journald",
    /// "reconcile_detector", "behavior_probe".
    pub source: String,
    /// What was observed, e.g. "e2big_absent", "threshold_crossed",
    /// "drift_cleared".
    pub kind: String,
    /// The load-bearing flag. `true` ONLY when the adapter corroborated the
    /// effect from an authenticated source. NEVER set from LLM output or
    /// unsanitized text. Rail-2 reads this and nothing else.
    pub verified: bool,
    /// Short, sanitized human-readable detail (capped, control/ANSI-stripped).
    pub detail: String,
    /// When the observation was made.
    pub observed_at: chrono::DateTime<chrono::Utc>,
}
```

> **Invariant.** `verified` is set exclusively by a `LiveSignalSource` adapter
> that authenticated the observation (a crossed metric threshold, a matched
> journald line, `!DeployDrift::needs_deploy`, a successful behavior re-probe).
> It is never derived from model output. This is what makes Rail-2 (below)
> non-bypassable by prompt injection or recipe tampering.

## `LiveSignalSource`

The signal-acquisition trait. Mirrors
[`EvidenceSource`](completion-evidence-gate-api.md#evidence-sources): lookups are
injected so tests run hermetically with no network, no live `gh`, and no
`journalctl`.

```rust
pub trait LiveSignalSource: Send + Sync {
    /// Gather every live signal relevant to this goal's real success criteria.
    /// Each adapter sets `verified` only from an authenticated positive
    /// corroboration. On a hard error, returns `Err` — the seam surfaces it as
    /// a NO-FALLBACK cycle failure (never an empty "no signals" success).
    fn gather(&self, goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>>;
}

/// Blanket impl so `&T` and `Arc<T>` are also sources (mirrors EvidenceSource).
impl<T: LiveSignalSource + ?Sized> LiveSignalSource for &T { /* … */ }
```

The production source composes thin adapters over signals Simard already emits:
the [`self_metrics`](telemetry-metrics.md) reader, a read-only argv-only
`journalctl` reader, and the
[`ReconcileDetector`](self-deploy-api.md#reconciledetector). Tests inject a
`FakeLiveSignals` double that returns canned signals (or an error).

> **Security — argv-only shellouts.** Adapters that shell out (journald,
> behavior re-probe) use `Command::args([...])` (never `sh -c`), a `--`
> terminator, per-call timeouts, read-only credentials, and validate every
> goal-derived token (numeric PR/issue ids, `owner/name` repo slugs,
> allow-listed service names). Signal provenance is authenticated from a trusted
> origin (metrics stream, deploy-reconcile state, the daemon's own structured
> markers) and `verified` is set from positive corroboration — never from string
> presence in untrusted text.

## `GoalOutcomeCtx`

The structured context handed to the brain. Assembled by the gather step.

```rust
pub struct GoalOutcomeCtx {
    /// Goal identity.
    pub goal_id: String,
    pub goal_title: String,
    /// The goal's REAL success criteria (what "achieved" actually means).
    pub success_criteria: String,
    /// Artifact-level signals from the completion-evidence gate (merged PR,
    /// closed issue, deployed). Fed as INPUT, not as the decider.
    pub artifact_signals: CompletionEvidence,
    /// Live signals gathered this cycle. Rail-2 checks `.iter().any(verified)`.
    pub live_signals: Vec<LiveSignal>,
    /// How many times this goal has already been re-verified (bumped on each
    /// `reopen` / `replan`). Lets the brain notice a goal that keeps landing
    /// artifacts without ever producing the live effect.
    pub reverify_count: u32,
}
```

## `GoalOutcomeDecision`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum GoalOutcomeDecision {
    /// Real success criteria observed live. Archive ONLY if Rail-2 passes.
    MarkAchieved { rationale: String },
    /// Artifact landed, live effect absent — keep the goal active.
    Reopen { rationale: String },
    /// Live effect absent AND the current plan won't produce it.
    Replan { rationale: String, replan_hint: String },
    /// Ambiguous / absent / unverifiable — no archive, surface a report.
    /// The fail-closed default (also what `Default` returns).
    KeepOpenAndReport { rationale: String },
}
```

The serde tag/rename convention (`choice`, snake_case) matches
`EngineerLifecycleDecision`. **As of Group D (epic #4719, issue #4967) the
outcome seam no longer scrapes JSON from the recipe's stdout.** The reasoner
RECORDS its verdict by calling the gated `simard ooda record-outcome` tool,
which validates the closed enum through
[`GoalOutcomeDecision::from_choice_fields`](#goaloutcomedecision) and atomically
writes one typed `OutcomeDecisionRecord` (schema `simard.ooda.outcome.v1`).
`RecipeBrain` then reads that record back via `read_verified_outcome` — it never
parses the agent's prose. The former stdout-scraping mapper
(`outcome_decision_from_variant`) and its `OutcomeEnvelope` were deleted.

> **`replan_hint` ownership.** `replan_hint` is a load-bearing, `replan`-only
> field. It is threaded end-to-end through the typed record: the tool accepts an
> optional `--replan-hint` (owned by `replan`; rejected on any other choice by
> the shared chokepoint), and the flattened `OutcomeDecisionRecord` serializes it
> alongside the `choice` tag. Because the record round-trips through
> `GoalOutcomeDecision`'s own serde representation (not the lossy shared
> `DecisionEnvelope` shim), the hint can never be silently defaulted away.

## `OodaBrain::decide_goal_outcome_verification`

Added as a **defaulted** trait method so the three existing `OodaBrain` impls and
every test double compile unchanged; the default is the fail-closed
`KeepOpenAndReport`.

```rust
pub trait OodaBrain: Send + Sync {
    // … existing methods …

    /// Reason about whether the goal's real success criteria are met LIVE.
    /// Default is conservative (never `MarkAchieved`) so unmigrated impls
    /// cannot accidentally complete a goal.
    fn decide_goal_outcome_verification(
        &self,
        ctx: &GoalOutcomeCtx,
    ) -> SimardResult<GoalOutcomeDecision> {
        Ok(GoalOutcomeDecision::KeepOpenAndReport {
            rationale: "outcome-verification not implemented by this brain".into(),
        })
    }
}
```

| Impl | Behavior |
| --- | --- |
| `RecipeBrain` | Loads `ooda-goal-outcome-verification.yaml` via `RecipeBrain::new(repo_root, "ooda-goal-outcome-verification.yaml", "recipe-outcome-verify-brain")`, renders the ctx over a fresh per-call temp dir, runs the recipe (`run_outcome_verify_recipe`), then reads the typed record via `read_verified_outcome` — stdout is ignored. Every read-verification failure is a fail-closed `Err`. |
| `DeterministicLifecycleBrain` (floor) | Conservative: always `KeepOpenAndReport`, never `MarkAchieved`. |
| `RustyClawdBrain<S>` | Not migrated for the outcome seam — inherits the defaulted method (`KeepOpenAndReport`) so it compiles and never accidentally completes a goal. (Its separate per-goal-cycle seam WAS migrated to the typed `record-decision` record in Group D.) |
| Test doubles (`StubOutcomeBrain`) | Return the injected decision (or `Err`) for hermetic tests. |

The three existing `OodaBrain` impls (`RecipeBrain`, `DeterministicLifecycleBrain`,
`RustyClawdBrain`) plus every test double compile unchanged because the method is
defaulted — verify this at implementation step 2.

## The seam and the three rails

The step is `gather → reason → apply` in
[`src/goal_curation/outcome_verify.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/outcome_verify.rs).
The rails guard only the one irreversible action (archival):

```rust
/// Verify a completion-candidate goal's live outcome. Returns the applied
/// decision. NEVER archives without ≥1 verified live signal.
pub fn verify_goal_outcome(
    goal: &ActiveGoal,
    artifact_signals: &CompletionEvidence,
    brain: &dyn OodaBrain,
    signals: &dyn LiveSignalSource,
) -> SimardResult<GoalOutcomeDecision> {
    // Rail 1 — skip perpetual goals (they never archive). Non-candidates are
    // filtered by the caller before this function is reached.
    if goal.is_perpetual {
        return Ok(GoalOutcomeDecision::KeepOpenAndReport {
            rationale: "perpetual goal — verification skipped".into(),
        });
    }

    // Gather — a source Err is a visible failure, never an empty success.
    let live_signals = signals.gather(goal)?; // Rail 2 (NO-FALLBACK)

    let ctx = GoalOutcomeCtx { /* … sanitized … */ };

    // Reason — a brain Err is a visible failure (matches spawn.rs #1711).
    let decision = brain.decide_goal_outcome_verification(&ctx)?; // Rail 2

    // Rail 3 — MarkAchieved requires ≥1 adapter-verified live signal, else
    // the rail overrides to KeepOpenAndReport (fail-closed).
    let has_live_proof = ctx.live_signals.iter().any(|s| s.verified);
    let applied = match decision {
        GoalOutcomeDecision::MarkAchieved { rationale } if !has_live_proof => {
            GoalOutcomeDecision::KeepOpenAndReport {
                rationale: format!("rail override (0 verified signals): {rationale}"),
            }
        }
        other => other,
    };
    Ok(applied)
}
```

| Rail | Guard | On failure |
| --- | --- | --- |
| **1 — Skip** | Perpetual / non-candidate goals | No brain call, no metric. Skipped. |
| **2 — NO-FALLBACK** | Signal-source `Err` or brain `Err` | Visible cycle failure (`tracing::error!` + `eprintln!` + `success=false`); goal stays open; no archive. Mirrors [`spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs) (#1711/#1748). |
| **3 — Verified signal** | `MarkAchieved` with 0 verified live signals | Rail **overrides** to `KeepOpenAndReport`. Never archives on the LLM's word alone. |

Only `MarkAchieved` **that survives Rail-3** archives the goal
(`GoalStatus::Completed`). Every other outcome reuses the existing goal-board
update APIs (`reopen`, `replan` marker, report) — no new archival machinery.

## `OodaClients` bridge fields

Two optional fields are added to
[`OodaClients`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/types.rs),
mirroring `decide_brain` / `completion_evidence`:

```rust
pub struct OodaClients {
    // … existing fields (brain, decide_brain, completion_evidence, …) …

    /// Optional structured-reasoning brain for live outcome verification
    /// (#2751). When `Some`, a completion-candidate goal is verified LIVE
    /// before archival; when `None`, the legacy curate path is unchanged.
    pub outcome_verify_brain: Option<std::sync::Arc<dyn OodaBrain>>,

    /// Optional live-signal source paired with `outcome_verify_brain`.
    /// Production boot wires the composed adapter set unless
    /// `SIMARD_OUTCOME_VERIFY=off`; tests inject a stub. `None` => legacy path.
    pub live_signals:
        Option<std::sync::Arc<dyn crate::goal_curation::live_signal::LiveSignalSource>>,
}
```

The curate seam in
[`cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
gates completion-candidate archival through `verify_goal_outcome` when **both**
fields are `Some`; otherwise it takes the existing
`archive_completed_evidence_aware` / `archive_completed` path. New goal-board
fields (`reverify_count`, re-plan marker) are `#[serde(default)]` so
`goal_board_store` tolerates old records.

## The reasoning recipe

`prompt_assets/simard/recipes/ooda-goal-outcome-verification.yaml` follows the
[`ooda-per-goal-cycle.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/ooda-per-goal-cycle.yaml)
typed-record shape: role → live-context vars → snake_case `choice` options →
**record-the-verdict-via-tool** instructions → few-shot examples. It is installed
to the hot-reload path `~/.simard/prompt_assets/...` for production; tests use a
`home_override` tempdir.

Context vars (passed via `-c`; trusted daemon-minted paths verbatim, each
model-facing string rendered through
[`sanitize_context_var`](#sanitization-boundary)):

| Var | Meaning |
| --- | --- |
| `record_path` | Owner-only per-call temp path the tool writes the typed record to (trusted). |
| `simard_bin` | Resolved `current_exe()` the agent invokes for `record-outcome` (trusted). |
| `cycle_number` | Sentinel `REASONER_RECORD_CYCLE` (0) — the outcome ctx carries no cycle number; `goal_id` (R6) binds identity. |
| `goal_id`, `goal_title` | Goal identity. |
| `success_criteria` | What "achieved" actually means for this goal. |
| `artifact_signals` | Merged/closed/deployed summary from the done-gate. |
| `live_signals` | Rendered list of `{source, kind, verified, detail, observed_at}`. |
| `reverify_count` | Times this goal has already been re-verified. |

Output: **NONE scraped from stdout.** The agent RECORDS its verdict by calling
the `simard ooda record-outcome` tool exactly once:

```bash
"{{simard_bin}}" ooda record-outcome --choice reopen \
  --reason "PR #4821 merged and deployed, but journald still shows E2BIG on the next real spawn; live effect absent" \
  --record-path "{{record_path}}" --goal-id "{{goal_id}}" --cycle-number {{cycle_number}}
```

A `replan` decision additionally passes `--replan-hint` (owned by `replan`;
rejected on any other choice — see [`GoalOutcomeDecision`](#goaloutcomedecision)):

```bash
"{{simard_bin}}" ooda record-outcome --choice replan \
  --reason "artifact shipped but at the wrong layer; success criteria untouched" \
  --replan-hint "target the spawn arg-length path, not the packer" \
  --record-path "{{record_path}}" --goal-id "{{goal_id}}" --cycle-number {{cycle_number}}
```

where `--choice` is exactly one of `mark_achieved`, `reopen`, `replan`,
`keep_open_and_report`. The recipe's few-shot set **includes the kgpacks /
E2BIG case** (artifact present, outcome absent → `reopen`) so the reasoning is
anchored on the exact failure this step exists to catch. A genuine "it's really
achieved, live" answer is a real decision — record `mark_achieved` explicitly; a
record that is absent, malformed, out-of-enum, or goal/cycle-mismatched fails
CLOSED via `read_verified_outcome` and surfaces (never a silent
`keep_open_and_report`).

> **The recipe never carries the safety invariant.** The recipe is
> hot-reloadable and user-writable. The load-bearing control (`any(verified)`)
> lives in the Rust Rail-3, so editing the prompt can change *reasoning quality*
> but can never make the daemon archive a goal with zero verified live signals.

## Sanitization boundary

Every `LiveSignal` field is rendered to a string and routed through the existing
sanitizer before it reaches the recipe — `source`, `kind`, and `detail`
directly; `observed_at` as an RFC3339 timestamp (already injection-free, capped
for consistency). Each passes through
[`sanitize_context_var`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/sanitize.rs)
before it becomes a recipe `-c` arg:

- **Caps:** `detail ≤ 2000` chars; `source` / `kind` / other fields ≤ 500.
- **Strip:** control characters and ANSI escape sequences.
- **Bound:** `live_signals.len() ≤ 32` (prompt-cost DoS guard).

The recipe delimits live-signal detail as **untrusted** input. Because Rail-3 is
the decider, injection in `detail` cannot forge a completion.

## Observability

| Surface | What it records |
| --- | --- |
| `BrainJudgmentRecord` (phase `BrainPhase::OutcomeVerify`) | Built by `BrainJudgmentRecord::from_goal_outcome(...)`, pushed via `push_brain_judgment`; carries the decision label, verified-signal count, and scrubbed rationale. Serialises as `"outcome_verify"`. |
| `goal_live_outcome_verification` metric | Appended to `metrics.jsonl` via [`self_metrics::record_metric`](telemetry-metrics.md); context carries the reasoning string, the outcome, and the verified-signal count. |

Both persist **bounded, sanitized** summaries only — never raw journald or log
payloads (secret/PII scrubbing).

## Kill-switch

```text
SIMARD_OUTCOME_VERIFY=off
```

Secure default is **verification ON**. Only the explicit documented value `off`
(case-insensitive) disables the step by leaving the bridge pair `None` (legacy
curate path). Any unknown value **fails safe to enabled**. Every degradation to
artifact-only is audited. See the
[outcome-verification kill-switch operations page](../operations/outcome-verification-kill-switch.md).

## Test matrix

All hermetic — stub brain + injected `LiveSignal`s, no network.

| # | Scenario | Expected |
| --- | --- | --- |
| T1 | Rail-3 override: brain `mark_achieved`, 0 verified signals | NOT archived; goal stays open. |
| T2 | Ambiguity: absent/ambiguous signals | Open + report; no archive. |
| T3 | NO-FALLBACK (brain `Err`) | `success=false` + loud log; no archive. |
| T3b | NO-FALLBACK (signal source `Err`) | `success=false` + loud log; no archive. |
| T4 | Observability | `push_brain_judgment` records `OutcomeVerify`; `goal_live_outcome_verification` metric carries the reasoning string. |
| T5 | E2BIG / kgpacks repro: PR merged + deployed, live effect absent | `reopen` / `keep_open_and_report`; never `mark_achieved`. |
| T6 | Happy path: `mark_achieved` + ≥1 verified signal | Archived `Completed`. |
| T7 | Perpetual skip | Verifier not invoked; no metric. |
| T8 | Backward-compat: bridge `None` | Legacy path; no behavior change. |
| T-sec1 | Injection/newline/ANSI in `LiveSignal.detail` | Neutralized by `sanitize_context_var`. |
| T-sec2 | Spoofed unverified signal + LLM `mark_achieved` | Rail-3 blocks archive. |
| T-sec3 | Adapter `Err` / timeout | NO-FALLBACK; no archive. |

## See also

- [Closed-loop outcome verification (concept)](../concepts/closed-loop-outcome-verification.md)
- [How to diagnose a re-opened goal](../howto/diagnose-a-reopened-goal.md)
- [Outcome-verification kill-switch](../operations/outcome-verification-kill-switch.md)
- [Completion-evidence gate API](completion-evidence-gate-api.md) — the artifact gate whose verdict is an input signal.
- [Self-deploy API](self-deploy-api.md) — the `ReconcileDetector` behind the deploy live-signal.
