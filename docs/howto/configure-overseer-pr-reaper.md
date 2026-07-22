---
title: Configure the Overseer PR reaper
description: >
  Operator runbook for Simard's PR-reaper policy: the deterministic,
  fail-closed guard designed to sit between the merge-queue reasoner and the
  intervention gate (implemented and unit-tested; live wiring pending). Tune its conservative thresholds (stale 14d, CONFLICTING
  7d, title similarity >=0.85 + file overlap) via SIMARD_OVERSEER_REAPER_*,
  keep destructive duplicate-close in dry-run/notify-only by default, opt in to
  destructive close via allow_verify_merge, and verify decisions through the
  telemetry counters and the Overseer activity feed. No path uses --admin or
  --no-verify.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: partial
related:
  - ../concepts/overseer-pr-reaper-policy.md
  - ../reference/overseer-pr-reaper-policy-api.md
  - ../reference/agentic-merge-queue-reasoning-api.md
  - ../howto/configure-agentic-merge-queue-reasoning.md
  - ../howto/triage-stale-pull-requests.md
  - ../howto/watch-overseer-activity.md
  - ../reference/cross-repo-merge-authority.md
---

# Configure the Overseer PR reaper

> **Goal.** Understand how the PR-reaper policy is designed to guard every
> `Stale` / `Duplicate` disposition the merge-queue reasoner proposes, tune its
> conservative thresholds, keep destructive duplicate-close **dry-run by
> default**, and — only when you explicitly choose to — opt in to destructive
> close. **Never** `--admin` / `--no-verify`. (Live wiring is pending — see the
> [concept status note](../concepts/overseer-pr-reaper-policy.md).)

Background: [the concept](../concepts/overseer-pr-reaper-policy.md), the
[API reference](../reference/overseer-pr-reaper-policy-api.md), and the
[merge-queue reasoning howto](./configure-agentic-merge-queue-reasoning.md).

## Prerequisites

- The daemon binary includes the merge-queue reasoning pass and the
  `reaper_policy` layer (this feature). **Note:** the `reaper_policy` layer is
  implemented and unit-tested but is **not yet routed into the live disposition
  path** — see the [concept status note](../concepts/overseer-pr-reaper-policy.md).
  Until wiring lands, the thresholds below configure the policy for its unit
  tests and future integration; the live Overseer still hands `Stale`/`Duplicate`
  dispositions to the existing `MergeAuthority`-gated interventions directly.
- **`gh`** authenticated so the read-only reasoning step can list/view PRs.
- The Overseer is acting (`SIMARD_OVERSEER_ENABLED` truthy). See
  [Watch Overseer activity](./watch-overseer-activity.md).

## 1. Confirm the guard is available (dry-run by default once wired)

The reaper policy needs no enable flag — by design it only ever *tightens* what
the reasoner already proposes, so once wired it is **on** whenever the
merge-queue reasoner runs. It is **not yet wired into the live path** (see the
[concept status note](../concepts/overseer-pr-reaper-policy.md)); the posture
described here is what takes effect once integration lands. By design it is
**non-destructive**: `Stale` and long-`CONFLICTING` PRs get a review-comment
`FlagStalePr`, and proposed duplicate closes are **downgraded to a flag** because
the destructive gate is closed.

Verify the default posture:

```bash
# allow_verify_merge should be UNSET → notify-only / dry-run
systemctl --user show-environment | grep -i verify_merge \
  || echo "destructive close: OFF (dry-run default)"
```

Watch a cycle and confirm flags (not closes) are being proposed, using the same
Overseer log stream the merge-queue reasoner writes to:

```bash
journalctl --user -u simard-ooda -f | grep -E 'FlagStalePr|CloseDuplicatePr'
```

In dry-run you should see `FlagStalePr` entries and, at most, *notify-only*
`CloseDuplicatePr` proposals — never an actual `gh pr close`. The same activity
is visible in the dashboard **Overseer** tab, the `OVERSEER` section of
`simard status`, and `~/.simard/overseer/activity.json` (see
[Watch Overseer activity](./watch-overseer-activity.md)).

## 2. Tune the thresholds

All three thresholds are conservative by default and config-overridable. See the
[API reference](../reference/overseer-pr-reaper-policy-api.md#configuration).

| Variable | Default | Clamp | Effect |
| --- | --- | --- | --- |
| `SIMARD_OVERSEER_REAPER_STALE_DAYS` | `14` | `>= 1` | Days without update before a PR may be flagged stale. |
| `SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS` | `7` | `>= 1` | Days a PR must be `CONFLICTING` before it may be flagged. |
| `SIMARD_OVERSEER_REAPER_SIMILARITY` | `0.85` | `[0.0, 1.0]` | Minimum normalized-title similarity for a near-duplicate (file overlap also required). |

Example — be a little more aggressive about flagging stale/conflicting PRs while
keeping duplicate detection strict. Set the values on the `simard-ooda` **user**
service (the same mechanism as the merge-queue reasoning knobs):

```bash
systemctl --user set-environment SIMARD_OVERSEER_REAPER_STALE_DAYS=10
systemctl --user set-environment SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS=5
systemctl --user set-environment SIMARD_OVERSEER_REAPER_SIMILARITY=0.9
systemctl --user restart simard-ooda
```

For a persistent system-wide install, drop `--user` and set the same
`Environment=` lines via `systemctl edit simard-ooda` (see the
[claim-reaper kill switch](../operations/claim-reaper-kill-switch.md) for the
drop-in pattern).

Bad or out-of-range values clamp to a safe bound with a `WARN`; they never widen
the policy beyond its floors, and no threshold env var can open a destructive
close.

> **Note.** A near-duplicate close **always** requires title similarity **and**
> overlapping changed files. Lowering `SIMARD_OVERSEER_REAPER_SIMILARITY` alone
> can never cause a close on title resemblance by itself.

## 3. Opt in to destructive duplicate-close (only when you mean it)

Destructive `CloseDuplicatePr` stays in the existing
[`RiskClass::MergeAuthority`](../reference/agentic-merge-queue-reasoning-api.md)
notify-only class. It is admitted **only** when the operator flips
`allow_verify_merge` on the Guardrails gate — the same opt-in that governs
verify-and-merge. Until then, proposed closes are downgraded to flags.

- To keep the safe default: **do nothing** — leave the destructive gate closed.
- To opt in: enable `allow_verify_merge` exactly as documented for
  [autonomous merge](./enable-autonomous-self-merge-canary.md) /
  [cross-repo merge authority](../reference/cross-repo-merge-authority.md). The
  reaper then admits `CloseDuplicatePr` for confirmed near-duplicates, closing
  the *higher-numbered (later)* PR and preserving the deterministically chosen
  lowest-numbered survivor.

Even with the gate open, the reaper never passes `--admin` or `--no-verify`, and
the close comment interpolates only the integer PR ids.

## 4. Verify decisions

Each decision emits a scalar/ID-only `tracing` span on the Overseer log stream,
and increments the OTel counters described in the
[API reference](../reference/overseer-pr-reaper-policy-api.md#telemetry). The
counters are exported over OTLP (internal names `simard.overseer.reaper_decision`
and `simard.overseer.reaper_downgraded`); the per-decision spans are the
easiest thing to read directly:

```bash
journalctl --user -u simard-ooda | grep 'overseer.*reaper' | tail
# ... reaper decision=flag repo=rysweet/Simard pr=4303 reason=duplicate_not_closable
# ... reaper decision=no_action repo=rysweet/Simard pr=4290
```

On a metrics backend the same data renders through the Prometheus exporter as
`simard_overseer_reaper_decision_total{decision="flag"}` and
`simard_overseer_reaper_downgraded_total`. A non-zero
`reaper_downgraded_total` with `decision="close_duplicate" == 0` confirms the
dry-run gate is doing its job: the reasoner proposed closes, and the policy
downgraded them to flags. (A downgraded close increments **both**
`decision="flag"` **and** `reaper_downgraded_total`.)

Per-decision context (repo, PR id, reason) is also in the Overseer activity
feed — the dashboard **Overseer** tab, the `OVERSEER` section of `simard status`,
or `~/.simard/overseer/activity.json`:

```bash
simard status | grep -iA3 overseer
```

## 5. Revert

```bash
systemctl --user unset-environment SIMARD_OVERSEER_REAPER_STALE_DAYS
systemctl --user unset-environment SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS
systemctl --user unset-environment SIMARD_OVERSEER_REAPER_SIMILARITY
systemctl --user restart simard-ooda
```

Thresholds return to their conservative defaults (14d / 7d / 0.85) and, if you
had opened it, close the destructive gate to return to dry-run.

## Worked example: clearing a duplicate pair

Given two near-duplicate PRs in `rysweet/Simard` — `#4269` and `#4303`, both
titled *"fix(cognition): drop stopwords in knowledge-pack relevance scoring"* and
touching the same files:

1. The reasoner proposes `Duplicate` for the newer one, `duplicate_of` the other.
2. `reaper_policy::evaluate` checks: normalized-title similarity `>= 0.85` ✔ and
   changed-file overlap non-empty ✔.
3. **Dry-run (default):** the destructive gate is closed ⇒ decision is
   `Flag(DuplicateNotClosable)`, and `reaper_downgraded_total` increments
   alongside `decision="flag"`. A review comment is posted on the
   higher-numbered PR pointing at the survivor. Nothing is closed.
4. **Opt-in:** with `allow_verify_merge` enabled, the decision is
   `CloseDuplicate { survivor: 4269 }` (the older PR survives), admitted through
   `MergeAuthority`, and the newer PR `#4303` is closed with a comment
   referencing `#4269` — using positional argv only.
