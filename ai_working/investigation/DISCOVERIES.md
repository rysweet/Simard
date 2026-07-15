# Discoveries — Overseer recurring blocked goals & workstream-gaps

Investigation of why the same goals repeatedly enter `blocked` and why
`workstream-gap` signatures recur (2026-07-15). Full analysis in
[`investigation_report.md`](./investigation_report.md).

## Key discoveries

1. **`workstream-gap` is a backlog-coverage gap, not a decomposition failure.**
   It fires for a p1/p2 active goal with no assignee/PR/branch, a high-signal
   open issue with no PR, or a live anomaly with no fix in flight
   (`overseer/sensor.rs::detect_workstream_gaps`). Decomposition producing
   `<2` sub-goals is a *separate*, loud-failing path in `decompose.rs` that
   leaves the board untouched — it does not emit a `workstream-gap`.

2. **The gap loop observes but never closes.** `act_flag_workstream_gaps`
   (`overseer/mod.rs:884`) only sends an operator notification; it launches no
   workstream and files no issue. The gap therefore survives every cycle.

3. **"2x-seen" is a dead zone.** A signature that recurs is deduped by the
   `gap_gate` 15-minute window (`WhisperGate::new(900, 200)`), but root-cause
   escalation only triggers at `RECURRENCE_ESCALATION_THRESHOLD = 3`. A signal
   seen 2x is above noise but below escalation, and coverage gaps have *no*
   auto-remediation rung — so it recurs forever.

4. **Blocked goals recur because stalls degrade to bare "needs human review".**
   The no-progress breaker parks after 3 idle cycles. Self-resolvable causes
   (AlreadyComplete / MissingPrecondition / UpstreamDependency / UnclearCriteria)
   only route down `resolution_for_why` when the WHY reasoner is wired and
   classifies correctly; otherwise they become permanent bare parks.

5. **The two signatures are one problem in two views.** A persistently
   under-resourced goal oscillates: `workstream-gap` while active, `goal:blocked`
   once idle. That is why personas, the coverage audit, the coin harness, and
   kgpacks appear in both families together.

## Named-goal cause map

- kgpacks-rs parity / #12,#17,#18,#23,#25 → `AlreadyComplete` / `MissingPrecondition`
- Simard test-coverage audit to 70% → `UnclearCriteria` (uncheckable done-gate)
- coin benchmark harness → `MissingPrecondition` / `UpstreamDependency`
- simard-identity personas → `GoalUncovered` workstream-gaps / `UnclearCriteria`
