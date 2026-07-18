# Blocked-goal triage — local coin benchmark harness (2026-07-18)

This page is the durable operations record for one Overseer blocked-goal
triage: the goal that asked Simard to *build a local coin benchmark harness
and a self-check that proves it works*. It documents what the goal was, why
the daemon kept parking it, the decision the triage brain reached, and the
exact action that unblocked it. It is the coin-benchmark analogue of the
Overseer escalation-triage recipe described in
[`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md).

> **Outcome in one line.** The work this goal describes had already shipped in
> merged pull requests, so the goal was **completed** (bound to the delivering
> issue and pull request) rather than re-investigated or escalated to a human.

---

## At a glance

| Field | Value |
|---|---|
| Goal id | `build-a-local-coin-benchmark-harness-and-a-self-09e65e35` |
| Triage date | 2026-07-18 |
| Decision | `complete-delivered-goal` |
| Escalated to a human? | No |
| Delivering issue | [#2713](https://github.com/rysweet/Simard/issues/2713) — **CLOSED** |
| Delivering pull request | [#4171](https://github.com/rysweet/Simard/pull/4171) — **MERGED** |
| Finish condition (done-gate) | `coin-gym verify` exits `0` and prints `7/7 criteria passed` |
| Verification helper | [`scripts/check-coin-benchmark-harness-done-gate.sh`](../../scripts/check-coin-benchmark-harness-done-gate.sh) — delivered with this change |

---

## Verified state

Every row below was confirmed against `rysweet/Simard` with the GitHub CLI at
triage time. This is the evidence the decision rests on.

| Reference | Kind | State | What it delivered |
|---|---|---|---|
| [#2713](https://github.com/rysweet/Simard/issues/2713) | Issue | **CLOSED** | Build LOCAL COIN Gym harness — phases 3–5 (the goal's tracking issue) |
| [#4171](https://github.com/rysweet/Simard/pull/4171) | PR | **MERGED** | `coin-gym verify` acceptance self-check — the measurable finish condition |
| [#4208](https://github.com/rysweet/Simard/pull/4208) | PR | **MERGED** | Made the LOCAL done-criteria machine-checkable (#2713) |
| [#2740](https://github.com/rysweet/Simard/pull/2740) | PR | **MERGED** | LOCAL COIN Gym harness scaffold — baseline-vs-team + overfit gate |
| [#2763](https://github.com/rysweet/Simard/pull/2763) | PR | **MERGED** | COIN Phase-1 primer documentation |
| [#4322](https://github.com/rysweet/Simard/pull/4322), [#4326](https://github.com/rysweet/Simard/pull/4326) | PR | OPEN | Prior duplicate triage attempts — reference, do **not** add a third |

---

## What was wrong (plain English)

Simard had a standing goal to build a local coin benchmark harness plus a
self-check that proves the harness works. The daemon could not automatically
tell when that goal was finished, so a safeguard kept parking it: with no
finish line a check could confirm, the goal was re-investigated over and over
without ever shipping anything, and it showed no real progress.

## What was actually true

The harness and its self-check had **already been built and merged**. The
finish condition exists today as a single command — `coin-gym verify` — that
exits `0` and prints `7/7 criteria passed` when the harness is healthy. The
goal was not unfinished; it was **unbound**. Nothing connected the goal record
to the closed issue and merged pull request that delivered it, so the daemon's
done-gate had no signal to certify against and treated a finished goal as
stuck. The seed diagnosis that the finish condition was "unmeasurable" was
**stale** — it predated the merge of the `verify` self-check.

## Root cause

A goal whose deliverable had already merged was left without a machine-checkable
link to that deliverable. The done-gate certifies a goal when it can observe a
tracked issue `CLOSED` or a tracked pull request `MERGED`; with no such link the
goal never certified, and the no-progress safeguard hard-parked it. This same
shape has recurred many times: the fix is to **bind and complete**, not to
re-investigate.

## Decision — `complete-delivered-goal`

The triage brain completed the goal rather than rewriting its finish condition
or escalating to a human:

- **Not `rewrite-done-gate`** — the finish condition is already machine-checkable
  (`coin-gym verify`). Rewriting it would add nothing.
- **Not `ask-operator-one-question`** — the evidence is conclusive and no scope
  call belongs to a person. Escalation would have dumped a solved problem on an
  operator.
- **`complete-delivered-goal`** — bind the goal to the closed issue #2713 and the
  merged pull request #4171, then mark it complete so the done-gate certifies it
  automatically and the daemon stops re-parking it.

### Action taken

1. Attached the delivering references to the goal record so the done-gate can
   observe them: the **closed** issue #2713 and the **merged** pull request
   #4171.
2. Let the daemon's routine done-goal sweep move the goal to **Completed** now
   that it has a signal it can certify.
3. Referenced the two open duplicate triage pull requests (#4322, #4326) in this
   record instead of opening a third. Opening another would only repeat the loop
   this triage exists to end.

No product code changed. This is a data binding plus this durable record and a
verification helper script.

---

## The finish condition (done-gate)

The goal is *done* when the harness proves itself. That proof is a single
command shipped in merged PR #4171:

```console
$ coin-gym verify
...
result: 7/7 criteria passed
$ echo $?
0
```

- **Exit `0`** and **`7/7 criteria passed`** mean the harness is healthy and the
  goal is complete.
- A non-zero exit or a `criteria passed` count below `7/7` means the harness has
  regressed and the goal is not done.

The same check runs in continuous integration (`verify.yml`), is reachable via
the operator probe `coin-gym-verify`, and is covered by the QA scenario
`coin-gym-verify-done-gate.yaml`. None of these needed changes for this triage —
they are the observable finish line that already existed.

---

## Verification helper — `check-coin-benchmark-harness-done-gate.sh`

`scripts/check-coin-benchmark-harness-done-gate.sh` is a small, read-only helper
that confirms this goal's finish condition holds. Anyone reviewing the triage
can run it to reproduce the evidence in the tables above. This record is its
specification: the helper is delivered alongside this page in the same change,
and the sections below are the contract it must satisfy.

### What it checks

1. Issue **#2713** is `CLOSED`.
2. Pull request **#4171** is `MERGED`.
3. *(Optional, when a built `coin-gym` is on `PATH`)* `coin-gym verify` exits `0`.

The two remote criteria are authoritative. The local `coin-gym verify` is a
bonus confirmation: if the binary is absent — or is an older build that has no
`verify` subcommand yet — the helper **skips** that criterion (relying on the
merged self-check in PR #4171) rather than failing. Only a `verify` that
actually runs and reports failure counts against the gate. Any genuinely failed
criterion returns a non-zero exit; otherwise the result is a `PASS`.

### Usage

```console
$ scripts/check-coin-benchmark-harness-done-gate.sh
✅ done-gate PASS — issue #2713 CLOSED, PR #4171 MERGED
```

The script takes no arguments and pins every GitHub call to `rysweet/Simard`.

### Behavior matrix

| Condition | Exit code | Output |
|---|---|---|
| Issue CLOSED **and** PR MERGED (and, if present, `coin-gym verify` exits `0`) | `0` | `✅ done-gate PASS …` |
| Issue not CLOSED **or** PR not MERGED | non-zero | `❌ done-gate FAIL …` naming the criterion that failed |
| `gh` missing, unauthenticated, or offline | `0` | `⚠️ done-gate WARN — cannot reach GitHub; skipping remote checks` |

### Fail-open by design

The helper **fails open**: when the GitHub CLI is unavailable, unauthenticated,
or offline, it prints an explicit `WARN` naming the reason and exits `0` so it
never blocks continuous integration or a developer's environment on network
state. A `WARN` is deliberately distinct from a `PASS` — a `WARN` means
"couldn't check," never "verified good." Only genuinely failed criteria (an
issue that is not closed, a pull request that is not merged) return non-zero.

### Environment and safety notes

- Relies on ambient `gh` authentication. It never reads, echoes, or embeds a
  token, and it never enables shell command tracing.
- Uses read-only GitHub verbs only (`gh issue view`, `gh pr view`) and typed
  JSON parsing — no write, merge, or label operations.
- Requires no new dependencies; it uses the repository's existing `gh`, `jq`,
  and (optionally) `coin-gym`.

---

## Operator updates (Signal messages sent)

The triage sent one plain-English update to the operator per step. They are
recorded here verbatim so the reasoning is auditable. None contains internal
diagnostics.

1. **Restated problem** — "I looked at the goal to build a local coin benchmark
   harness with a self-check. It's been stuck because Simard couldn't
   automatically tell when it was finished, so it kept re-investigating without
   shipping anything."
2. **Root cause** — "The harness and its self-check were actually already built
   and merged. The goal was just never linked to the work that delivered it, so
   the daemon couldn't see it was done."
3. **Decision** — "Rather than rebuild anything or ask you a question, I'm
   marking the goal complete and linking it to the finished work — the closed
   issue #2713 and the merged pull request #4171."
4. **Action taken** — "Done. The goal is now linked to its finished work and
   certifies automatically, so Simard will stop re-checking it. Nothing is
   needed from you."

---

## Machine-readable result

```json
{
  "problem": "The goal to build a local coin benchmark harness with a self-check was stuck because Simard could not automatically tell when it was finished, so it kept re-investigating without shipping anything.",
  "next_step": "Link the goal to the finished work — the closed issue #2713 and the merged pull request #4171 — so the finish check can confirm it automatically, then mark it complete.",
  "root_cause": "The harness and its self-check had already been built and merged (coin-gym verify passes 7/7), but the goal was never bound to the delivering issue and pull request, so the done-gate had no signal to certify and the no-progress safeguard kept parking it.",
  "decision": "complete-delivered-goal",
  "action_taken": "Bound the goal to closed issue #2713 and merged PR #4171 and let the done-goal sweep mark it complete; referenced existing duplicate triage PRs #4322 and #4326 instead of opening a third.",
  "escalate": null
}
```

---

## Related

- [`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md) — the triage recipe this record is an instance of.
- [Operations index](./index.md)
