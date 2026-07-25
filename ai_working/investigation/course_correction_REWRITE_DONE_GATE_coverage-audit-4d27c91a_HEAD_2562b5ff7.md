# Overseer — Course-correction for blocked goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`

HEAD: `2562b5ff7` · Role: escalation-triage brain · Recipe: `prompt_assets/simard/overseer/escalation_triage.md`.
Goal: `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` · Blocker outcome: `019f6c08-d053-7d93-89bf-f1f86aee408c` · Decision: **rewrite-done-gate** · Escalate: **one operator question**.

> This record closes the terminal-action gaps left after the investigation rounds:
> (1) the done-gate rewrite is bound to a durable, machine-checkable anchor (issue
> #4616); (2) ONE plain-English Signal message was actually sent to the operator
> (verified delivery); (3) the recurring blocker is now handled as a recorded
> escalation with a single operator question rather than a silent re-fail loop.

---

## 1. The block, restated in plain English (no raw markers)

Simard had this goal parked as *blocked*. Translated from its internal diagnostics:
Simard could not automatically tell when the goal — *audit Simard's test coverage and
raise it above 70% line coverage* — was finished, so it kept re-investigating every
cycle without ever shipping a completion. Separately, the goal had fallen off Simard's
authoritative active goal board (it survived only in the observation snapshot Simard
reads to decide what to work on), so it kept re-alerting while the daemon had nothing
on the board to attach a worker, PR, or finish-line to.

None of `OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` / `why=` /
`evidence=[` / 🔒 was surfaced to the operator — all translated to plain English.

## 2. Smallest concrete next step (plain English)

Give the goal a single, automatically-checkable finish line, then ask the operator the
one question only they can answer: put the goal back on the active list (tied to that
finish line), or retire it as already handled.

## 3. Root cause

The goal's finish condition was never expressed as something the daemon could observe
(no specific issue to see CLOSED, no specific PR to see MERGED). Compounding it, the
goal diverged between the two stores — present in the observation snapshot (so it keeps
re-alerting) but absent from the authoritative `goal_board.json` (so `advance-goal`
finds nothing to progress). That divergence, not the measurability gap alone, is why the
blocker never self-cleared.

## 4. Course-correction applied

- **Rewrote the done-gate to be machine-checkable.** Acceptance-anchor issue **#4616**
  now encodes `Specs/COVERAGE_AUDIT.md` §2/§3 (per-group ≥70% or justified exception,
  empty "Other groups" backlog, clean §3 high-risk scan, attached `cargo llvm-cov`
  table). It is CLOSED only by the final audit-complete PR (`Closes #4616`), so the
  completion gate certifies a merged PR **and** a closed issue on the same merge.
- **Binding tooling shipped.** `simard goal wip <goal-id> add issue 4616 …`
  (PR #4620) so the anchor can be attached to the goal the moment the operator says
  "resume" — using the anti-clobber board flock, safe against a concurrent OODA cycle.

## 5. Why not `complete-delivered-goal`

Every named per-group target has landed ≥70% (bin 76%, dashboard 70%, trace_collector
95%, gym 89%, cmd_cleanup 70%, status 91%, diagnosis 100%, git_guardrails 91%,
completion-gate 82%) and the backlog is empty — but **no single merged PR asserts the
whole-audit §2 verdict**, so there was nothing already-delivered to just mark complete.
The work is largely done; it simply could never self-certify.

## 6. Escalation — the one operator question

Because the goal has dropped off the authoritative board, resume-vs-retire is a genuine
human scope call. Exactly one plain-English question was asked:

> Should Simard put this coverage goal back on its active list (tied to the #4616
> checklist) so it finishes and certifies it — or retire it as already handled?

## 7. Signal — plain-English update actually sent (verified)

One consolidated jargon-free Signal message was sent to the operator's configured
rolling group via the live signal-cli JSON-RPC daemon. Delivery confirmed by the
daemon's accepted send timestamp `1784991219776` (empty per-recipient failure list =
successful group dispatch). The message states, in plain English: the work is
essentially done; the goal kept re-appearing because it had no automatic finish line;
a checklist (#4616) is now that finish line; and the one resume-vs-retire question.
No marker tokens were surfaced.

## 8. OUTPUT contract

```json
{
  "problem": "Simard couldn't automatically tell this coverage goal was finished, so it kept re-checking it every cycle without ever completing it; the goal had also dropped off Simard's active to-do list, so it kept re-alerting with nothing to progress.",
  "next_step": "Give the goal a single automatically-checkable finish line (issue #4616), then ask the operator whether to resume the goal tied to it or retire it as already handled.",
  "root_cause": "The goal's completion was never expressed as a daemon-observable condition, and the goal had diverged between the observation snapshot (still alerting) and the authoritative goal board (nothing to advance), so the block could not self-clear.",
  "decision": "rewrite-done-gate",
  "action_taken": "Bound the goal's finish line to machine-checkable acceptance-anchor issue #4616 (encoding COVERAGE_AUDIT.md §2/§3, closed only by the final audit-complete PR); shipped the simard goal wip binding CLI (PR #4620) to attach it; posted the decision + one question on #4616; and sent one plain-English Signal update to the operator (verified delivery, ts 1784991219776).",
  "escalate": "One operator scope call is genuinely required: resume the goal bound to #4616, or retire it as already handled — because the goal has fallen off the authoritative goal board and re-instating vs retiring is the operator's decision."
}
```
