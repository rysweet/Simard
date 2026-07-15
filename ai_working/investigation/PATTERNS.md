# Patterns — Observe/flag loops that never converge

Reusable patterns extracted from the recurring `goal:blocked` / `workstream-gap`
investigation (2026-07-15). See [`investigation_report.md`](./investigation_report.md).

## Anti-pattern: "Observe-and-flag without a closing action"

**Shape:** a control loop detects a condition, notifies/records it, dedups
re-notification within a window — but never takes an action that *removes* the
condition. The condition persists, is re-detected next window, and re-flagged.

**Where seen:** `overseer::act_flag_workstream_gaps` notifies the operator but
never launches a workstream; the gap survives and recurs.

**Fix:** every persistent-signal loop needs a **convergence rung** — an action
that resolves or escalates the condition on recurrence, so the signal trends to
zero instead of looping.

## Anti-pattern: "Recurrence dead zone"

**Shape:** a signal below the escalation threshold (e.g. `recurrence < 3`) and
above one-off noise gets *neither* auto-remediation *nor* escalation. It is real
but ignored.

**Fix:** track recurrence uniformly and place a remediation/escalation rung at
the *first proven* recurrence (2x) for signals that have no benign explanation
(coverage gaps, uncovered p1/p2 work).

## Pattern: "Classify-then-route the stall, don't park it"

**Shape (correct):** when work stalls, first determine *why*
(`NoProgressClass`), then route down a self-resolving ladder
(`resolution_for_why`): auto-complete / drop / self-heal / defer, and only reach
a human for genuinely-unclear or genuinely-stuck cases.

**Failure mode:** if the WHY reasoner is unwired or misclassifies, every stall
degrades to a bare "needs human review" park — turning a self-resolvable
condition into a permanent block.

**Guard:** periodically re-classify bare parks (`reinvestigate_bare_blocked_goals`)
and positively certify completion via the done-gate so "done" is never read as
"stuck" (the kgpacks incident).

## Pattern: "Two signatures, one root problem"

Under-resourced important work oscillates between a *coverage* signature
(uncovered, while active) and a *blocked* signature (parked, while idle). When
the same entities appear in both recurring families, treat them as one
resourcing/convergence problem, not two independent bugs.
