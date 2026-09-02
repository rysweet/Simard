---
title: Diagnose a no-progress breaker issue storm
description: >
  Runbook for confirming and clearing a storm of duplicate
  `OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)`
  tracking issues auto-filed by the OODA no-progress breaker. Explains the
  durable suppression marker that now caps filings at one per stuck goal, how to
  verify a goal is suppressed, how to bulk-close the duplicate `ooda-stuck`
  issues, and when a still-recurring filing means something else is wrong.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/no-progress-breaker-storm-suppression-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../reference/no-progress-breaker-api.md
  - ../concepts/no-progress-breaker-storm-suppression.md
  - ./unblock-stuck-ooda-goals.md
  - ./diagnose-a-no-progress-block.md
  - ./reinvestigate-bare-blocked-goals.md
  - ./run-ooda-daemon.md
---

# Diagnose a no-progress breaker issue storm

## Symptom

The repository fills with many **identical** auto-filed tracking issues, all
labelled `ooda-stuck` and all sharing one title:

```
OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)
```

You might see a dozen or more filed within a day or two (the motivating incident
was ~15 in ~2 days). This is **one** stuck goal (or a small population) being
re-filed every cycle — not that many independent goals stalled at once.

## What fixed it

As of the [issue-storm suppression change](../concepts/no-progress-breaker-storm-suppression.md),
the breaker writes a **durable suppression marker** to the goal board **before
and independent of** filing the `gh` issue. The dedup key is now durable goal
identity, so:

- a goal is suppressed after its **first** escalation even if the `gh` link
  failed to land, and
- the suppression **survives a daemon restart** (it lives on the goal board, not
  in the in-memory tracker).

The result is a hard cap of **one** filing attempt per stuck goal. If you are on
a build that predates this fix, upgrade the daemon (see
[run the OODA daemon](./run-ooda-daemon.md)); the storm stops on the next cycle
because every already-escalated goal is recognized as suppressed.

> **Root cause, briefly.** The old idempotence guard keyed on a **linked**
> tracking `WipRef`, which was only written when `gh issue create` succeeded
> **and** its URL parsed to a bare issue number. An `UNCLEAR-CRITERIA` goal has
> no tracked PR/issue by design, so a failed link left it "untracked" and the
> breaker re-filed the same issue every cycle. See the
> [concept](../concepts/no-progress-breaker-storm-suppression.md) for the full
> loop.

## Step 1 — Confirm the storm and its source goal

List the duplicate issues:

```bash
gh issue list --label ooda-stuck --state open \
  --search "OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)" \
  --limit 100
```

Then find which goal(s) they came from:

```bash
simard goal list
```

Look for a `Blocked` goal whose reason carries the sentinel and the
`why=UNCLEAR-CRITERIA` (or `why=GENUINELY-STUCK`) token:

```text
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 4 consecutive no-action cycles; why=UNCLEAR-CRITERIA evidence=[done-criteria <goal-id> (unmeasurable: no tracked PR/issue the done-gate can verify)]
```

## Step 2 — Verify the goal is now suppressed

Inspect the goal's `wip_refs` in the durable goal-board store. A suppressed goal
carries **exactly one** breaker artifact — either the bare suppression marker or
the linked tracking issue it was upgraded into:

```bash
# Bare suppression marker (gh link did not land) — still suppressed, no re-file:
#   kind  = "ooda-breaker-marker"
#   ref_id= "ooda-breaker"
#   label = "[no-progress-tracking] ooda-breaker (unlinked)"
#
# Upgraded linked tracking ref (gh link succeeded):
#   kind  = "issue"
#   ref_id= "<issue-number>"
#   label = "[no-progress-tracking] #<issue-number>"

jq '.active[] | select(.id=="<goal-id>") | .wip_refs' ~/.simard/goal_board.json
```

Either form proves the goal is suppressed: the breaker's idempotence guard
recognizes both (`is_breaker_tracking_ref`), so it will **not** file again. There
must be **at most one** such entry per goal — that is the enforced invariant.

## Step 3 — Watch one more cycle: no new filing

Confirm the storm has stopped:

```bash
# Count before, wait for at least one cycle, count after — the number must not grow.
gh issue list --label ooda-stuck --state open --limit 100 | wc -l
ls -t ~/.simard/cycle_reports/ | head -1   # wait for a fresh cycle report
gh issue list --label ooda-stuck --state open --limit 100 | wc -l
```

The count must be stable. A single suppressed goal never adds a new `ooda-stuck`
issue on subsequent cycles, even across a `systemctl --user restart
simard-ooda.service`.

## Step 4 — Close the duplicate issues

The suppression marker stops *new* filings; the already-filed duplicates are
yours to clean up. Keep the oldest (or the linked one) and close the rest:

```bash
# List them oldest-first, keep #<oldest>, close the duplicates.
gh issue list --label ooda-stuck --state open \
  --search "OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)" \
  --json number,createdAt --limit 100 \
  --jq 'sort_by(.createdAt) | .[1:] | .[].number' \
| while read -r n; do
    gh issue close "$n" --comment "Duplicate of the retained ooda-stuck tracking issue; storm suppressed by the durable breaker marker."
  done
```

If the retained issue's goal has since been resolved (criteria clarified, work
merged, or the goal completed/removed), close it too and clear the goal — see
[unblock stuck OODA goals](./unblock-stuck-ooda-goals.md).

## Step 5 — Decide the underlying goal's fate

The suppression stops the *noise*; the goal is still genuinely stuck. Pick one:

- **Make the criteria measurable.** Give the goal a tracked artifact or a
  concrete, checkable done-criterion so the done-gate can verify it. A goal whose
  criteria are derivable from its own description now proceeds as
  `GENUINELY-STUCK` and gets a real guided investigation rather than being flagged
  `UNCLEAR-CRITERIA` (see the
  [API reference](../reference/no-progress-breaker-storm-suppression-api.md#derive_criteria)).
- **Re-scope or replace it.** Demote/remove the open-ended goal and add a bounded
  replacement:

  ```bash
  simard goal demote <goal-id>
  simard goal remove <goal-id>
  simard goal add <priority> "module X line coverage >= 80%, PR merged"
  ```

- **Clear the block by hand** if you have resolved it out of band:

  ```bash
  simard goal unblock <goal-id>
  ```

- **Force a re-link if the marker is stuck bare.** If `file_issue()` failed on the
  first escalation, the goal keeps a *bare* suppression marker and the breaker will
  **never** retry the link on its own (storm suppression is prioritized over
  eventual linking — see the
  [API trade-off note](../reference/no-progress-breaker-storm-suppression-api.md#deliberate-trade-off-a-bare-marker-is-never-re-linked)).
  To get a tracked issue, remove the bare marker so the next cycle re-escalates
  cleanly (only do this once `gh` is healthy, or the storm can resume):

  ```bash
  # Drop the unlinked breaker marker; the next cycle re-files + links from scratch.
  jq '(.active[] | select(.id=="<goal-id>").wip_refs) |=
        map(select(.kind != "ooda-breaker-marker"))' \
    ~/.simard/goal_board.json > /tmp/goal_board.json \
    && mv /tmp/goal_board.json ~/.simard/goal_board.json
  ```

## When a filing still recurs

If new `ooda-stuck` issues keep appearing for the **same** goal after the fix,
that is not the storm this runbook covers — investigate one of:

- The daemon is on a **pre-fix build** — upgrade it.
- The goal board is not being persisted (the marker write is not surviving) —
  check for `goal_board.json` write errors and see
  [recover the goal board](./recover-goal-board.md).
- The issues are for **different goals** that each escalated once — that is
  correct behavior; each is one filing per goal. Resolve the goals themselves.

## Related

- [Issue-storm suppression API reference](../reference/no-progress-breaker-storm-suppression-api.md) — the marker, the storm-safe escalation, and `derive_criteria`.
- [The no-progress breaker suppresses its own issue storm](../concepts/no-progress-breaker-storm-suppression.md) — the incident and the fix rationale.
- [Diagnose a no-progress block and read its WHY](./diagnose-a-no-progress-block.md) — reading the block reason and its evidence.
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — clearing a blocked/suppressed goal by hand.
- [Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md) — the per-cycle pass that upgrades bare blocks to a concrete WHY.
