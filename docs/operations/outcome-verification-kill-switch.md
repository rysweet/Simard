---
title: "Operations: outcome-verification kill switch (SIMARD_OUTCOME_VERIFY)"
description: >
  The environment variable that disables the closed-loop outcome-verification
  step at daemon boot — what it does, when (and when not) to use it, how to set
  it via systemd, how to verify which mode the daemon is in, and how to remove
  it. Secure default is verification ON; unknown values fail safe to enabled.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/closed-loop-outcome-verification.md
  - ../reference/outcome-verification-api.md
  - ../howto/diagnose-a-reopened-goal.md
  - progress-evidence-kill-switch.md
---

# Outcome-verification kill switch (`SIMARD_OUTCOME_VERIFY`)

This page documents the environment variable that disables the closed-loop
outcome-verification step at daemon boot. The step itself is described
conceptually in
[Closed-loop outcome verification](../concepts/closed-loop-outcome-verification.md)
and the API is documented in
[Outcome-verification API](../reference/outcome-verification-api.md).

> The verifier is **enabled by default** on all production deployments. Secure
> default = verification ON. The kill switch exists for incident recovery and
> short-lived debugging sessions, not for steady-state operation.

---

## What this variable does

| Value | Behavior |
|---|---|
| Unset, or any value other than `off` (case-insensitive) | `OodaClients.outcome_verify_brain` and `OodaClients.live_signals` are wired to the production `RecipeBrain` + composed `LiveSignalSource`. Each completion-candidate goal is verified LIVE before archival: a goal is marked `achieved` only when the brain reasons its real success criteria are met **and** at least one adapter-verified live signal exists (Rail-3). |
| `off` (case-insensitive) | Both bridge fields are left `None`. The verifier is skipped and the legacy artifact-only curate path returns — a goal archives on the [deploy-aware done-gate](../concepts/deploy-aware-done-gate.md) alone (merged PR + closed issue + verified deploy), with **no** live-outcome check. **No `goal_live_outcome_verification` metric or `OutcomeVerify` cycle-report entry is emitted.** |

> **Unknown values fail safe to ENABLED.** Only the exact documented value `off`
> disables the step. A typo (`SIMARD_OUTCOME_VERIFY=false`, `0`, `no`) leaves the
> verifier **on** — the secure default is never silently disabled by a
> mis-spelled value.

The variable is read once, at daemon startup, in the client-construction path.
Changing it during a daemon run has no effect — restart the daemon to pick up a
new value.

---

## When to use `off`

There are exactly three legitimate uses. If you find yourself reaching for the
kill switch outside of these, file a bug instead.

1. **A defect in the verifier or a signal adapter is wedging archival.** The step
   is **NO-FALLBACK**: a signal-source or brain `Err` is a visible cycle failure
   that keeps the goal open. If a bug causes every verification to error (a hung
   `journalctl`, a broken adapter), completion-candidate goals stop archiving.
   Disable the step to restore archival while you fix the adapter, then
   re-enable.
2. **Investigating a regression in the step itself.** If you suspect the verifier
   is wrongly holding a genuinely achieved goal open, toggle the variable on a
   **non-production** daemon for a side-by-side comparison, then re-enable.
3. **Bisecting a daemon-level bug whose cause is unrelated to completion
   accounting.** Removing the step eliminates one variable from the
   investigation. Restore it as soon as bisection completes.

---

## When NOT to use `off`

- **"To unblock a goal that won't go achieved."** If the goal is held open with
  `reopen` / `keep_open_and_report`, the artifact landed but the **live effect is
  absent** — that is the correct result, not a bug. Turning the verifier off
  archives a goal whose real success criteria are unverified, re-introducing
  exactly the kgpacks silent-re-block this step prevents. Fix the live signal
  instead: [diagnose a re-opened goal](../howto/diagnose-a-reopened-goal.md).
- **"Because the dashboard shows many re-opened goals."** Each re-open is
  evidence that an artifact shipped without producing its effect — silencing the
  signal does not make the effect appear.
- **"To make a demo look complete."** The demo will lie. Use the auditable
  dashboard operator override for a specific goal instead of disabling the whole
  gate.

---

## How to set it

### One-shot for an interactive run

```bash
SIMARD_OUTCOME_VERIFY=off simard daemon
```

### Persistent across daemon restarts (systemd unit)

The Simard daemon ships with a reference unit file at
[`scripts/simard-ooda.service`](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service)
and is typically installed as a **user-level** unit at
`~/.config/systemd/user/simard-ooda.service`. Operators who install it
system-wide (`/etc/systemd/system/`) should drop the `--user` flag from every
command below.

Add the override to the unit's `[Service]` section:

```ini
[Service]
Environment="SIMARD_OUTCOME_VERIFY=off"
```

Then reload and restart:

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

To remove the override, delete the `Environment=` line, `daemon-reload`, and
restart.

For system-level installs, prefer `systemctl edit simard-ooda` (with `sudo`) so
the override lands in
`/etc/systemd/system/simard-ooda.service.d/override.conf` rather than being
merged into the upstream unit file:

```bash
sudo systemctl edit simard-ooda
# add the same [Service] / Environment= snippet
sudo systemctl daemon-reload
sudo systemctl restart simard-ooda
```

---

## Verifying which mode the daemon is running in

The daemon logs the active mode at boot, and audits every degradation to
artifact-only. The `outcome-verify:` substring and the `enabled` / `DISABLED`
words are stable; the parenthetical detail may evolve.

```
[simard] outcome-verify: enabled (RecipeBrain + LiveSignalSource)
```

Or:

```
[simard] outcome-verify: DISABLED (artifact-only curate — SIMARD_OUTCOME_VERIFY=off) [AUDIT: degraded to artifact-only completion]
```

Grep the daemon log to confirm (drop `--user` for a system-level install):

```bash
journalctl --user -u simard-ooda -n 200 | grep 'outcome-verify:'
```

You can also probe live behavior via the metrics stream: when the verifier is
enabled there is a `goal_live_outcome_verification` metric entry per
completion-candidate goal per cycle. When disabled, zero such entries are
emitted.

```bash
simard metrics query --name goal_live_outcome_verification | tail -n 5
```

---

## Failure modes the kill switch addresses

Unlike the fail-open progress-evidence reviewer, the outcome verifier is
**fail-closed and NO-FALLBACK**: a signal-source or brain error keeps the goal
open and surfaces the error (it never accepts a completion on failure). This is
the correct safety posture, but it means a **persistent** adapter defect can
stall archival for every completion-candidate goal. The kill switch is the escape
hatch for that specific class of incident:

| Scenario | When to use kill switch |
|---|---|
| A signal adapter errors/hangs on every call, wedging archival | Disable while you fix the adapter; restore when fixed. |
| The verifier wrongly holds a proven-achieved goal open | Disable on a non-prod daemon to confirm, fix the adapter/recipe, restore. |
| Bisecting a daemon bug unrelated to completion | Remove one variable from the investigation. |

---

## Removing the kill switch

When the underlying issue is resolved, remove the environment variable and
restart the daemon. Confirm via the boot log line above that the verifier is
`enabled`. The next cycle with a completion-candidate goal should emit a
`goal_live_outcome_verification` metric and an `OutcomeVerify` cycle-report
entry.

---

## Related

- [Closed-loop outcome verification (concept)](../concepts/closed-loop-outcome-verification.md)
- [Outcome-verification API (reference)](../reference/outcome-verification-api.md)
- [Diagnose a re-opened goal (how-to)](../howto/diagnose-a-reopened-goal.md)
- [Progress-evidence kill switch](progress-evidence-kill-switch.md) — the sibling fail-open mid-flight gate.
