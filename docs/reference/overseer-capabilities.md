# Overseer capabilities (M2–M4) — reference

The Overseer is Simard's autonomous operator/observer co-process. **M1** shipped
the read-only observer (observe → orient → report → file deduped issues). This
reference documents the **acting** capabilities that landed in milestones
**M2**, **M3**, and **M4**, all of which are **additive** and **flag-gated**
(`SIMARD_OVERSEER_*`, default **OFF**). With the flags unset the daemon behaves
exactly as before — nothing here is constructed or scheduled.

For the architecture, the co-process-vs-`CognitiveThread` decision, and the
grounding ledger see [Overseer design](../design/overseer.md).

- [Configuration](#configuration)
- [M2 — fix-launching + PR verify/merge + mandatory NotifyOperator](#m2-fix-launching-pr-verifymerge-mandatory-notifyoperator)
- [M3 — guarded deploy + goal transfer + conflict resolution](#m3-guarded-deploy-goal-transfer-conflict-resolution)
- [M4 — audits + bounded self-tuning](#m4-audits-bounded-self-tuning)
- [Guardrails shared by all acting capabilities](#guardrails-shared-by-all-acting-capabilities)

To turn any of this on and operate it, see
[Operate the Overseer](../howto/operate-the-overseer.md).

---

## Configuration

Every knob is read through an **injectable resolver** (`impl Fn(&str) ->
Option<String>`), so gating and budget/cadence resolution are unit-tested with
zero process-environment mutation. The `*_env` entry points are the only
functions that read the real `std::env`. All defaults are fail-safe.

| Environment variable | Default | Effect |
|----------------------|---------|--------|
| `SIMARD_OVERSEER_ENABLED` | **off** | Master gate. Only an explicit truthy value (`1`, `true`, `yes`, `on`; case-insensitive, trimmed) enables the Overseer. Unset / empty / unrecognised → **OFF**, daemon unchanged. |
| `SIMARD_DAILY_BUDGET_USD` | `500.0` | LLM-spend ceiling, **single-sourced** with the OODA loop so the Overseer's [`BudgetGate`] can never drift from the daemon's ceiling. Unset / empty / unparseable / non-positive → default. |
| `SIMARD_OVERSEER_INTERVAL_SECS` | `900` (15 min) | Observer cadence. **Clamped** to a hard floor of `60` s (`MIN_OVERSEER_INTERVAL_SECS`) so self-tuning (M4) can never drive a hot loop. Unset / empty / unparseable → default. |
| `SIMARD_OVERSEER_EMAIL_TO` | — | Recipient for the mandatory merge/deploy notification email (M2). Unset → the email channel is **queued and logged**, never silently dropped. |
| `SIMARD_OVERSEER_EMAIL_FROM` | — | Sender address for notification email. |
| `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS` | — | SMTP transport for the email channel. Unset → email is queued. |

Relevant constants (module `overseer::config`):

```text
OVERSEER_ENABLED_ENV            = "SIMARD_OVERSEER_ENABLED"
DAILY_BUDGET_ENV                = "SIMARD_DAILY_BUDGET_USD"
OVERSEER_INTERVAL_ENV           = "SIMARD_OVERSEER_INTERVAL_SECS"
DEFAULT_OVERSEER_INTERVAL_SECS  = 900
MIN_OVERSEER_INTERVAL_SECS      = 60
DEFAULT_DAILY_BUDGET_USD        = 500.0
```

Resolver API (all pure; each has an `_env` production entry point):

```rust
overseer_enabled_from(lookup) -> bool          // + overseer_enabled()
resolve_daily_budget_usd(lookup) -> f64        // + daily_budget_usd()
resolve_interval_secs(lookup) -> u64           // + overseer_interval_secs()
```

---

## M2 — fix-launching + PR verify/merge + mandatory NotifyOperator

M2 gives the Overseer its core "drive a fix **outside** Simard's OODA loop"
action: launch a `smart-orchestrator` workstream, poll it to a PR, verify that
PR against the full pr-verify checklist, poll its required checks to green
(**never** `--admin`), merge it through the shipped gated authority, and fire a
**mandatory** operator notification on both channels.

### Fix-launching — `overseer::launch`

Runs the exact `amplihack recipe run` invocation engineers use, with
`AMPLIHACK_AGENT_BINARY` preserved. Subprocess mechanics sit behind an
injectable `RecipeRunner` seam so the whole launch → PR flow is unit-testable
with a fake (no subprocess, no network).

| Item | Kind | Purpose |
|------|------|---------|
| `SMART_ORCHESTRATOR_RECIPE` | `const &str` | `"amplifier-bundle/recipes/smart-orchestrator.yaml"` — the recipe every fix-launch runs. |
| `smart_orchestrator_args(brief: &RecipeBrief) -> Vec<String>` | fn (pure) | Builds the `recipe run … -c task_description=…` argument vector. Unit-tested so the invocation contract is pinned without spawning. |
| `extract_pr_ref(output: &str) -> Option<(String, u32)>` | fn (pure) | Parses the `(repo, pr_number)` out of recipe output using the shipped noise-stripping. |
| `RecipeRunner` | trait | Injectable subprocess seam; the fake drives tests, the real runner spawns. |
| `SmartOrchestratorLauncher` | struct | `RecipeLauncher` adapter. `new(runner)` for tests, `from_env()` for production. |
| `AmplihackRecipeRunner` | struct | Production `RecipeRunner` that shells the real `amplihack` CLI. |

Concurrency is bounded by the Overseer's own per-cycle launch cap and
[`BudgetGate`]; the launcher never raises real parallelism beyond those ceilings.

### pr-verify diff-scans — `overseer::pr_verify`

The **new, additive** merge-safety checks (checklist items 3–6). These are a
**hard gate**: no merge capability ships before they exist and are unit-tested.
Every scan is a **pure** function over a unified diff (`gh pr diff` output), so
the whole merge-safety surface is tested on fixture diffs with zero network.

| # | Function | Passes when… |
|---|----------|--------------|
| 3 | `scan_no_bridge_naming(diff) -> Vec<DiffFinding>` | No `Bridge` naming appears in **added** lines. |
| 4 | `scan_no_stray_prints(diff) -> Vec<DiffFinding>` | No stray `print!`/`println!`/`eprint!`/`eprintln!` in added `src/**` lines. |
| 5 | `scan_additive_no_removed_pub(diff) -> Vec<DiffFinding>` | No **removed** `pub` items (change is additive / non-breaking). |
| 6 | `scan_prd_preserved(diff) -> Vec<DiffFinding>` | No removed lines in the PRD (`PRD_PATH = "Specs/ProductArchitecture.md"`). |
| — | `run_diff_scans(diff) -> Vec<CheckItem>` | Runs all four and projects each into a pass/fail `CheckItem`. |

Each hit is a `DiffFinding { file, line: Option<usize>, text }`. Checklist items
1–2 (CI-green / mergeable / base-allowlist) reuse
`stewardship::merge_authority::evaluate_objective_gates`; item 7 reuses
`review_pipeline::should_commit`. Those are wired in `merge_ops`, not here.

> The scanner's own source contains the literal strings `"Bridge"`, `print!`,
> etc. as **detection constants and test fixtures**. These are intentional and
> must not be "sanitised" — doing so would blind the scanner.

### Verify + merge — `overseer::merge_ops`

`MergePrOps` verifies a PR against the full checklist, **polls required checks
until green**, merges through the shipped gated authority, and fires the
mandatory notification.

Operator hard-gates encoded here:

- **Poll until green; never `--admin`.** `MergePrOps::poll_until_green` refuses
  (escalates) on any failed/red check and only proceeds when every check is
  `SUCCESS`/`NEUTRAL`/`SKIPPED` **and** the PR is `MERGEABLE`.
- **The merge itself** reuses `merge_pr_if_merge_ready_with_judge` →
  `gh pr merge --squash` — **no `--admin`, no `--no-verify`**.
- **Every merge notifies via both channels.** `MergePrOps::merge` fires the
  `DualChannelNotifier` after a successful merge; the merge is not complete
  without a dispatched `NotifyReport`.

Injected seams (all fakeable, no network): `PrSource` (diff + title),
`DiffReviewer`, `PollClock` (default `ThreadSleepClock`), and `PollConfig`.
`MergePrOps::new(...)` for tests; `MergePrOps::from_env()` for production.

### Mandatory operator notification — `overseer::notify`

**Every** PR merge — autonomous or human — notifies the operator over **both**
email and Signal with a concise, plain-language description of the PROBLEM being
solved and the PR that solves it.

| Item | Kind | Purpose |
|------|------|---------|
| `MergeNotification { problem, pr_title, pr_url, repo, autonomous }` | struct | The summary sent on every merge. `subject()` / `plain_text()` render it. |
| `DualChannelNotifier` | struct | Fires **every** channel on **every** notification and records each outcome. `from_env()` builds the email + Signal channels from config. |
| `NotifyChannel` | trait | One delivery channel. |
| `EmailNotifyChannel` / `EmailConfig` / `TcpSmtpSender` | struct | Email channel over SMTP; `EmailConfig::from_env()` reads the `SIMARD_OVERSEER_EMAIL_*` / `SMTP_*` vars; `is_configured()` gates real send. |
| `SignalNotifyChannel` / `ConversationSignalSender` | struct | Signal channel reusing the shipped `ConversationChannel` abstraction (PR #2529); adapts any channel (incl. the mock) into an object-safe `SignalSender`. |
| `ChannelDelivery` | enum | `Sent` / `Queued` / `Failed` — **there is no code path that drops a notification on the floor.** An unconfigured channel returns `Queued` (logged); an error returns `Failed` (logged). |
| `NotifyReport` | struct | Per-channel outcomes; `all_sent()` and `dispatched()`. |

---

## M3 — guarded deploy + goal transfer + conflict resolution

M3 adds three **HIGH-RISK, opt-in** actions. All three are classed HIGH-RISK by
the [autonomy gate](#guardrails-shared-by-all-acting-capabilities) and are
**refused unless `allow_high_risk` is explicitly enabled**; a refused
intervention becomes an `Escalate` in the plan.

### Guarded deploy — `overseer::deploy`

`GuardedDeployer` advances the deployed-commit marker **only** on a green
canary, refusing every dangerous shape. It reuses `self_relaunch::{build_canary,
verify_canary, all_gates_passed, default_gates}` for the canary and
`self_deploy::orchestrator::SelfDeployOrchestrator::run` for the deploy, and
compares against the running commit marker (`env!("SIMARD_GIT_HASH")`).

`evaluate_deploy_gate(ctx: &DeployContext) -> Result<(), DeployRefusal>` refuses:

| Refusal | Condition |
|---------|-----------|
| no-op | target commit == running commit |
| rollback | target is an ancestor of the running commit |
| red canary | canary gates did not all pass |
| crash-loop | restart churn ≥ `CRASH_LOOP_CHURN_THRESHOLD` (`3`) — deploying into an unstable process makes it worse |

A refused deploy **escalates** — it never mutates the binary. Every accepted
deploy fires the operator notification on both channels. The deployer operates
only on the canary target dir and the install path — it **never** touches
`~/.simard/worktrees`. Injected seams: `CanaryRunner`, `BinaryDeployer`,
`AncestryOracle` (real `GitAncestryOracle`).

### Conflict resolution — `overseer::conflict`

`GitConflictResolver` performs a conservative rebase-onto-base + push and
**never bypasses hooks** (operator hard-gate #8). Two always-on floors run
before any git command through `RealGitRunner`:

1. **Refuse `--no-verify`** at the git-runner boundary — pre-commit/pre-push
   hooks MUST run; no code path can smuggle the flag in.
2. **`git_guardrails::check_git_safety`** — the shipped destructive-command
   guardrail.

The `GitRunner` trait is injectable so the resolver is unit-tested without a
real repository or remote. `GitConflictResolver::from_env(repo_dir)` for
production; `new(git, repo_dir, base_ref)` for tests.

### Goal transfer — `overseer::meeting_ops`

`MeetingGoalTransfer` hands a goal to Simard via the meeting handoff surface
**without** running the interactive REPL. It reuses
`meetings::build_persisted_meeting_record_value` to render the same on-wire
meeting record the REPL persists, then writes it to a durable handoff file that
Simard's OODA reads to adopt the goal. The `HandoffSink` is injectable; the real
`FileHandoffSink` writes timestamped files under
`<state_root>/meeting_handoffs/` — **never** `~/.simard/worktrees`.
`render_goal_record(goal: &GoalBrief) -> String` produces the record;
`from_env()` builds the production sink.

---

## M4 — audits + bounded self-tuning

### Quality audits — `overseer::audit`

`SelfQualityAuditor` runs **crusty-old-engineer-gated** quality audits on demand
and on a recurring cadence, driving the shipped `monthly-self-quality-audit.yaml`
recipe (SEEK → VALIDATE → FIX waves, each merge crusty-gated) via
`self_quality_audit::run_self_quality_audit`. The durable cadence marker reuses
`self_quality_audit::{read_last_run, write_last_run}` — no new schema. The recipe
subprocess sits behind an injectable `AuditRunner` seam.

| Item | Kind | Purpose |
|------|------|---------|
| `SelfQualityAuditor` | struct | `new(runner, marker_path, interval_secs)` / `from_env(repo_root, state_root)`. |
| `recurring_due(now_epoch: u64) -> bool` | method | Is a recurring audit due per the cadence marker? |
| `run_recurring(now_epoch: u64) -> Option<Result<AuditReport, OverseerError>>` | method | Runs the audit iff due; `None` when not due. |
| `AuditRunner::run(scope) -> Result<QualityAuditOutcome, OverseerError>` / `SelfQualityAuditRunner` | trait / struct | Injectable audit executor; production runs the recipe. |
| `AuditReport { scope, passed, findings }` | struct | What `run_recurring` returns. `SelfQualityAuditor` projects the runner's `QualityAuditOutcome` into it: `passed = crusty_unresolved.is_empty()`, and every unresolved PR is listed in `findings`. |
| `QualityAuditOutcome { scope, waves_completed, prs_opened, prs_merged, crusty_unresolved, summary }` | struct | The `AuditRunner::run` output. A non-empty `crusty_unresolved` means the audit did **not** fully pass — it surfaces as `passed = false` with the PRs in `AuditReport::findings` for human follow-up. |
| `MONTHLY_SECS` | `const u64` | `30 * 24 * 60 * 60` — the default recurring cadence. |

### Bounded self-tuning — `overseer::tuning`

`OverseerTuning` lets the Overseer adapt its own `SIMARD_OVERSEER_*` thresholds
**within clamped floors/ceilings** — never off a cliff. The logic is
deliberately **pure** (no I/O), so "stays within clamps" and "no unbounded
growth" are exhaustively unit-tested. Applying the tuned values (as env
overrides) is the caller's job.

| Item | Kind | Purpose |
|------|------|---------|
| `ClampedKnob` | struct | One tunable clamped to `[floor, ceil]`. `new(value, floor, ceil, step)` clamps on construction; a degenerate range collapses to `floor`. |
| `ClampedKnob::{value, floor, ceil}()` | methods | Read the current value / bounds. |
| `ClampedKnob::raise()` / `lower()` | methods | Step by `step`, **saturating** at the bounds — never beyond. |
| `ClampedKnob::tune(feedback)` | method | Raise/lower per feedback. |
| `ClampedKnob::within_clamps()` | method | Invariant check used by tests. |
| `Feedback` | enum | Tuning signal — `TooNoisy` (too many low-value signals → less sensitive, longer cadence), `TooQuiet` (real problems slipped through → more sensitive), or `Stable` (hold). |
| `OverseerTuning` | struct | Bundles the knobs; `apply(feedback)`, `within_clamps()`, `interval_secs_u64()` (whole-second cadence, always ≥ `MIN_OVERSEER_INTERVAL_SECS` because the interval knob is floored there). |

Self-tuning can never drive a threshold to zero (a hot loop) or to infinity
(blindness): the interval knob is floored at `MIN_OVERSEER_INTERVAL_SECS` and
every knob saturates at its clamp.

---

## Guardrails shared by all acting capabilities

Every acting capability (M2–M4) is admitted through the first-class guardrails
in `overseer::guardrails` (landed with M1):

| Guardrail | Role |
|-----------|------|
| `classify(iv) -> RiskClass` + `AutonomyGate { allow_high_risk }` | Classifies each `Intervention` as `Routine` or `HighRisk`. `Deploy`, `ResolveConflict`, and `Escalate` are **HighRisk**; admitted only when `allow_high_risk` is explicitly `true` (default `false`), otherwise turned into an `Escalate`. |
| `RecursionGuard` | Refuses the Overseer's own PRs/commits so it never loops on itself. Fails **closed**: when identity is unconfigured it **refuses**, and the Overseer runs under a distinct identity from the human operator. |
| `BudgetGate` | Single-sourced from `SIMARD_DAILY_BUDGET_USD`; refuses launches once the daily ceiling is reached. |
| `ConflictSequencer` | Avoids two acting paths colliding on the same group; `admit(group)` / `release(group)`. |

## Related reading

- [Overseer design](../design/overseer.md) — architecture, capability→module
  table, grounding ledger, phased roadmap (M1–M4), and the crusty-review risks.
- [Operate the Overseer](../howto/operate-the-overseer.md) — enable, dry-run,
  and operate the acting capabilities.
- [Self-Deploy API](self-deploy-api.md) and
  [Cross-repo merge authority](cross-repo-merge-authority.md) — the guarded
  deploy and merge actions M2/M3 reuse.
- [Stewardship API](stewardship-api.md) — the deduped issue filing and objective
  merge gates.
