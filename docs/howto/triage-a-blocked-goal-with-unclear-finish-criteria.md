---
title: Triage a blocked goal with unclear finish criteria
description: >
  Operator runbook for the Overseer's escalation-triage decision pipeline when a
  goal is re-blocked over and over because its finish line can't be checked
  automatically. Explains what happens automatically (RESTATE → verify-then-decide
  → act), how to read the three plain-English Signal messages, how to verify the
  machine-checkable finish line the pipeline writes into the tracking issue, and
  when (rarely) you'll be asked one question — using the "move the governed repo
  roster out of framework code" goal as the worked example.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/escalation-triage-decision-pipeline.md
  - ../atlas/escalation-flow/README.md
  - ./reinvestigate-bare-blocked-goals.md
  - ./unblock-stuck-ooda-goals.md
  - ./diagnose-a-no-progress-block.md
  - ./configure-overseer-signal-rpc-notifications.md
  - ../reference/ecosystem-roster-resolution.md
---

# Triage a blocked goal with unclear finish criteria

## Symptom

A goal keeps coming back as **blocked** with the same complaint: an engineer is
assigned but can't finish because it isn't clear what *done* looks like. In the
goal board and the daemon journal you'll see the goal marked blocked repeatedly
over a short window — each fresh attempt hits the identical wall, and no PR ships.

The **operator-facing** symptom is a run of Signal notices that all say, in plain
English, some version of "this goal is stuck because its finish line can't be
checked automatically." You should **not** see raw machinery
(`[OODA-SAFEGUARD]`, `why=UNCLEAR-CRITERIA`, `evidence=[…]`) in those notices — if
you do, that's a separate bug; see
[Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md).

## Automatic recovery (no operator action needed)

You do **not** need to hand-write a finish line. When the Overseer decides the
goal is genuinely blocked, it runs the
[escalation-triage decision pipeline](../reference/escalation-triage-decision-pipeline.md)
**before** paging you. The pipeline runs three stages and sends you exactly **one
plain-English Signal message per stage**:

1. **RESTATE** — it translates the internal diagnosis into plain English: *what*
   is wrong (the finish line isn't checkable), not the machinery that found it.
2. **VERIFY-THEN-DECIDE** — it runs a **read-only** GitHub check for a merged PR
   that already delivers the goal, then picks exactly one course-correction:

   | What the check finds | What the pipeline does |
   |---|---|
   | No merged PR delivers it yet (and the goal is clear enough to define) | **Writes a checkable finish line** into the tracking issue (the usual case) |
   | A merged PR already delivered *everything* the goal asked for | **Marks the goal done** — no rewrite needed |
   | The goal's *intent* is genuinely a judgment call only you can make | **Asks you one plain-English question** |

3. **ACT** — it performs that one action and tells you it's done.

The decision comes from the read-only check, **not** from how many times the goal
was re-blocked. A re-block count only decides *whether* triage runs, never *what*
it does.

So a goal that today reads "stuck — no clear finish line" will, after triage,
become one of:

| Outcome | What you'll see |
|---|---|
| **Checkable finish line written** | The tracking issue gains a "Finish line (machine-checkable)" checklist; a conforming merged PR (with its linked issue closed) can then trip the done-gate |
| **Completed** | The goal leaves the active board as done (the work had already shipped) |
| **One question for you** | A single, crisp Signal question — answer it and triage continues |

## The worked example: moving the governed repo roster out of framework code

The goal `move-the-governed-repo-roster-out-of-framework-a8f57a50` asks Simard to
move her list of stewarded repositories **out of framework code** and into her
own **durable identity**. It kept getting stuck: an engineer was assigned but
couldn't finish because "done" was undefined, and it was re-blocked several times
in a few hours for that same reason.

Here is what the pipeline does, and the three Signal messages you'd receive.

### Signal message #1 (RESTATE)

> "I looked at the goal to move Simard's list of stewarded repositories out of
> framework code and into her own durable identity. It keeps stalling because
> there's no clear, automatically-checkable definition of when it's finished — so
> each engineer who picks it up re-investigates the same wall and never ships."

### Signal message #2 (VERIFY-THEN-DECIDE)

> "I checked whether an existing change had already finished this — it hadn't — so
> I'm going to write down a clear, checkable finish line for it."

### Signal message #3 (ACT)

> "Done — I wrote a clear, checkable finish line into this goal's tracking issue,
> so it can now be certified automatically. Nothing needed from you."

### The finish line it writes

On the "write a checkable finish line" path, the pipeline edits the goal's
**GitHub tracking issue** (not source code, not the goal-board entry) and inserts
a delimited checklist. For this goal it is:

```markdown
<!-- SIMARD:done-criteria:begin -->
### Finish line (machine-checkable)

This goal is done when a single merged PR delivers all of the following, each of
which Simard can verify automatically:

- [ ] roster seeded from the identity file (not from framework code)
- [ ] roster stored as durable identity STATE that a self-deploy does NOT overwrite
- [ ] the old committed ecosystem_repos.toml wiring removed
- [ ] certified by exactly one merged PR (with its linked tracking issue closed)
<!-- SIMARD:done-criteria:end -->
```

In plain English: seed the roster from the identity file, keep it as identity
state a self-deploy won't wipe, delete the old committed `ecosystem_repos.toml`
wiring, and confirm the whole thing with **one** merged PR. Now an engineer has an
unambiguous target. The done-gate certifies on the merged PR + its closed linked
issue (the fourth item); the first three are the file/command checks that PR's
diff must satisfy.

> **What the pipeline does not do:** it does **not** perform the migration itself
> and does **not** touch the roster-resolution code
> ([`resolve_ecosystem_roster_path`](../reference/ecosystem-roster-resolution.md)).
> It only writes the finish line the migration must satisfy.

## Verify the finish line was written

### 1. Open the tracking issue

Find the goal's tracking issue (its URL is in the Signal notice's link and in the
goal-board entry) and confirm it now contains the delimited block:

```bash
# Replace <n> with the goal's tracking-issue number.
gh issue view <n> --json body --jq '.body' | sed -n '/SIMARD:done-criteria:begin/,/SIMARD:done-criteria:end/p'
```

You should see the four-item "Finish line (machine-checkable)" checklist between
the `SIMARD:done-criteria:begin` / `…:end` markers. The rest of the issue body is
unchanged — the pipeline only rewrites the delimited span, so re-running triage
never duplicates or clobbers other content.

### 2. Confirm the goal is no longer stuck on "unclear done"

```bash
simard goal list | grep move-the-governed-repo-roster-out-of-framework
```

The goal should no longer be cycling as "blocked — unclear finish line." Once an
engineer ships a PR meeting all four criteria, the done-gate certifies it
automatically.

### 3. (Optional) Watch the triage in the journal

```bash
journalctl --user -u simard -f | grep -iE "escalation|triage|move-the-governed-repo-roster"
```

You'll see the thin trigger launch triage and the per-stage activity. The raw
markers stay in the internal journal; your **Signal** notices remain plain
English.

## When the pipeline finds the work already shipped

If the read-only check finds a merged PR that already delivered **all four**
criteria (roster seeded from identity, stored as self-deploy-safe state,
`ecosystem_repos.toml` wiring removed, its linked issue closed), the pipeline
**skips the rewrite** and marks the goal done. Signal message #3 then reads
something like:

> "Good news — this goal was already finished by a change that's since merged, so
> I've marked it complete. Nothing needed from you."

Confirm with `simard goal list` (the goal is gone from the active board) and by
checking the merged PR and its closed linked issue.

## When you'll be asked one question

Only if the goal's **intent** is genuinely a judgment call — for example, whether
the roster should live *entirely* in identity state or stay dual-sourced during a
transition — will the pipeline ask you. It asks **exactly one** plain-English
question, never a wall of jargon. Answer it (reply on the Signal channel) and
triage continues. For the roster goal this is unlikely: its four intents are
clear, so the normal outcome is a written finish line.

## If triage doesn't run (daemon offline)

The pipeline only runs while the Overseer is live. If the daemon is offline the
goal will simply stay blocked. Bring the daemon back up (see
[Run the OODA daemon](./run-ooda-daemon.md)) and the next tick that re-detects the
block will launch triage. For an immediate manual override you can still
[unblock the goal by hand](./unblock-stuck-ooda-goals.md), but prefer letting
triage write the checkable finish line so the goal doesn't just re-block.

## Related

- [Escalation-triage decision pipeline (reference)](../reference/escalation-triage-decision-pipeline.md) — the reference specification for the three stages, the read-only probe, and the criteria block.
- [Escalation Triage & Course-Correction atlas](../atlas/escalation-flow/README.md) — the end-to-end data-flow.
- [Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md) — the sibling pass for goals stranded with a *bare* marker.
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — manual override for the offline / immediate case.
- [Diagnose a no-progress block](./diagnose-a-no-progress-block.md) — reading the underlying block markers (internal).
- [Configure Overseer Signal notifications](./configure-overseer-signal-rpc-notifications.md) — set up the channel that carries the three plain-English messages.
- [Ecosystem-roster path resolution](../reference/ecosystem-roster-resolution.md) — the roster subsystem triage must not modify.
