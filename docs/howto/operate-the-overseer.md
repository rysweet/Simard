# Operate the Overseer (M2–M4)

This guide shows how to enable and operate the Overseer's **acting** capabilities
— autonomous fix-launching + PR verify/merge (M2), guarded deploy + goal transfer
+ conflict resolution (M3), and audits + self-tuning (M4).

Everything is **additive** and **flag-gated** (`SIMARD_OVERSEER_*`, default
**OFF**). With the flags unset the daemon is unchanged. For the full API and
configuration table see [Overseer capabilities](../reference/overseer-capabilities.md).

> **Safety defaults you inherit for free**
> - Master gate is **OFF** unless `SIMARD_OVERSEER_ENABLED` is explicitly truthy.
> - HIGH-RISK actions (deploy, conflict-resolution) are **refused** unless you
>   opt in — they escalate to you instead.
> - Merges never use `--admin`; pushes never use `--no-verify`.
> - Every merge/deploy notifies you on **both** email and Signal.
> - The Overseer never touches `~/.simard/worktrees`.

## Prerequisites

- A Simard checkout that includes the Overseer M2–M4 capabilities.
- The `amplihack` CLI on `PATH` (the fix-launcher shells `amplihack recipe run`).
- The `gh` CLI authenticated (PR verify/merge and check-polling use it).

## 1. Enable the observer + acting loop

Start with the safest useful configuration: enabled, default budget, default
15-minute cadence, HIGH-RISK **off**.

```bash
export SIMARD_OVERSEER_ENABLED=1          # truthy: 1 | true | yes | on
export SIMARD_DAILY_BUDGET_USD=500        # single-sourced with the OODA ceiling
export SIMARD_OVERSEER_INTERVAL_SECS=900  # clamped to a 60s floor
```

Any non-truthy or unset `SIMARD_OVERSEER_ENABLED` keeps the Overseer OFF:

```bash
export SIMARD_OVERSEER_ENABLED=0     # OFF
export SIMARD_OVERSEER_ENABLED=off   # OFF
unset  SIMARD_OVERSEER_ENABLED       # OFF (daemon behaves exactly as before)
```

## 2. Receive the mandatory merge/deploy notification (M2)

Every merge and every deploy notifies you on **both** channels. Configure the
email channel; the Signal channel reuses the shipped conversation channel.

```bash
export SIMARD_OVERSEER_EMAIL_TO=operator@example.com
export SIMARD_OVERSEER_EMAIL_FROM=simard@example.com
export SMTP_HOST=smtp.example.com
export SMTP_PORT=587
export SMTP_USER=simard
export SMTP_PASS=…   # keep out of shell history / logs
```

If a channel is unconfigured the notification is **queued and logged**, never
dropped — you can confirm delivery from the `NotifyReport`:

```rust
// DualChannelNotifier::notify takes an OperatorNotification; a merge renders
// into one via MergeNotification::to_operator().
let report = notifier.notify(&merge.to_operator());
assert!(report.dispatched());     // at least attempted on every channel
if !report.all_sent() {
    // some channel was Queued/Failed — inspect and fix config
}
```

The notification body always names the PROBLEM and the PR that solves it
(rendered by `subject()` + `plain_text()`):

```text
Subject: [Overseer] merge: fix flaky launch-merge race

The Overseer autonomously performed a merge in rysweet/Simard.

Problem solved:
  launch→merge poll returned before required checks settled, merging early

Link:
  https://github.com/rysweet/Simard/pull/1234
```

## 3. Let the Overseer launch and merge a fix (M2)

When the observer spots a process problem it can (routine autonomy) launch a
`smart-orchestrator` workstream, poll it to a PR, verify that PR, poll checks to
green, and merge it — then notify you.

The invocation it runs is the same one engineers run by hand:

```bash
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="<the problem, in plain language>"
```

Before any merge, the PR must pass the **pr-verify checklist**. Items 3–6 are
pure diff-scans you can reason about directly:

| Check | Fails the merge when the diff… |
|-------|--------------------------------|
| no-`Bridge` | adds a line containing `Bridge` naming |
| no stray prints | adds `print!`/`println!`/`eprint!`/`eprintln!` under `src/**` |
| additive | removes a `pub` item (breaking change) |
| PRD preserved | removes a line from `Specs/ProductArchitecture.md` |

Items 1–2 (CI-green / mergeable / base-allowlist) and item 7 (review) reuse the
shipped objective gates. The merge itself is `gh pr merge --squash` — **never**
`--admin`, **never** `--no-verify`. If any required check is red, the Overseer
**escalates** instead of merging.

## 4. Opt in to HIGH-RISK actions (M3)

Deploy and conflict-resolution are HIGH-RISK. They are **refused and escalated**
until you explicitly opt in via the autonomy gate (`allow_high_risk`, default
`false`). Enable this only when you want unattended deploy/conflict authority.

### Guarded deploy

An accepted deploy advances the deployed-commit marker **only** on a green
canary. The deploy gate refuses the dangerous shapes and escalates instead:

| Refusal | When |
|---------|------|
| no-op | target commit == running commit |
| rollback | target is an ancestor of the running commit |
| red canary | canary gates did not all pass |
| crash-loop | restart churn ≥ 3 |

The deployer operates only on the canary target dir and install path — it never
touches `~/.simard/worktrees` — and notifies you on both channels on success.
See also [Verify and roll back a self-deploy](verify-and-roll-back-a-self-deploy.md).

### Conflict resolution

Conflict-resolution does a conservative rebase-onto-base + push and **always
runs hooks**. `--no-verify` is refused at the git-runner boundary, and every git
command passes `git_guardrails::check_git_safety` first. If a hook fails, the fix
is the underlying cause (build/clippy/format) — the loop never bypasses the hook.

## 5. Transfer a goal to Simard (M3)

Hand a goal to Simard's OODA loop without opening the interactive REPL. The
transfer renders the same on-wire meeting record the REPL persists and writes it
to a durable handoff file under `<state_root>/meeting_handoffs/` (never
`~/.simard/worktrees`), which Simard's OODA reads to adopt the goal.

## 6. Run audits and let thresholds self-tune (M4)

### On-demand and recurring audits

The auditor drives the shipped `monthly-self-quality-audit.yaml` recipe
(SEEK → VALIDATE → FIX waves, each merge crusty-old-engineer-gated). A recurring
audit runs only when due per the durable cadence marker (default cadence:
`MONTHLY_SECS` = 30 days):

```rust
let auditor = SelfQualityAuditor::from_env(repo_root, state_root);
if let Some(result) = auditor.run_recurring(now_epoch) {
    let report = result?;                     // AuditReport { scope, passed, findings }
    if !report.passed {
        // audit did NOT fully pass — `report.findings` lists the crusty-unresolved
        // PRs that need human follow-up
    }
}
```

### Bounded self-tuning

The Overseer may adapt its own `SIMARD_OVERSEER_*` thresholds, but every knob is
**clamped**: `raise`/`lower` saturate at the floor/ceiling, so tuning can never
produce a hot loop (interval → 0) or blindness (threshold → ∞). The interval knob
is floored at `MIN_OVERSEER_INTERVAL_SECS` (60 s). Applying the tuned values as
env overrides is the operator's job.

```rust
let mut tuning = OverseerTuning::default();
tuning.apply(Feedback::TooNoisy);         // too many low-value signals → less sensitive, longer cadence
assert!(tuning.within_clamps());          // always holds, by construction
let secs = tuning.interval_secs_u64();    // >= MIN_OVERSEER_INTERVAL_SECS
```

## Dry-run / testing without side effects

Every acting capability is built behind an **injectable seam** so you can
exercise the full flow with fakes — no subprocess, no network, no filesystem:

| Capability | Inject a fake for… |
|------------|--------------------|
| fix-launch | `RecipeRunner` (no `amplihack` subprocess) |
| verify/merge | `PrSource`, `DiffReviewer`, `PollClock` (no `gh`, no sleeps) |
| notify | `NotifyChannel` (assert `NotifyReport` without sending) |
| deploy | `CanaryRunner`, `BinaryDeployer`, `AncestryOracle` |
| conflict | `GitRunner` (no real repo/remote) |
| goal transfer | `HandoffSink` (no filesystem) |
| audit | `AuditRunner` (no recipe subprocess) |
| tuning | pure — no seam needed |

This mirrors the shipped cognitive-thread discipline: pure + injected-clock unit
tests, zero side effects.

## Turn it off

Disable everything by unsetting the master gate — the daemon reverts to exactly
its prior behaviour:

```bash
unset SIMARD_OVERSEER_ENABLED
```

## Related reading

- [Overseer capabilities](../reference/overseer-capabilities.md) — full API and
  configuration reference.
- [Overseer design](../design/overseer.md) — architecture and roadmap.
- [Configure self-quality audit](configure-self-quality-audit.md) — the recipe
  the M4 auditor drives.
- [Verify and roll back a self-deploy](verify-and-roll-back-a-self-deploy.md) —
  the deploy action M3 reuses.
