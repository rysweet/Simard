---
title: Configure and verify the no-progress breaker open-issue backstop
description: >
  How-to for operators: confirm the OODA no-progress breaker's open-issue
  existence backstop is active, understand the `ooda-goal-key:<folded_id>` body
  marker and the `ooda-stuck` label it keys on, verify that goal-id churn and
  goal-board resets no longer produce duplicate `UNCLEAR-CRITERIA` tracking
  issues, tune / observe its (zero steady-state) API cost, and reconcile the
  branch-gated `recurring_goal_reblock` half. Prevention-only — this does not
  clean up issues already spammed before the fix landed.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/no-progress-breaker-goal-key-backstop.md
  - ../reference/no-progress-breaker-goal-key-backstop-api.md
  - ./diagnose-a-no-progress-breaker-issue-storm.md
  - ./configure-overseer-gap-scan-durable-dedup.md
  - ./unblock-stuck-ooda-goals.md
---

# Configure and verify the no-progress breaker open-issue backstop

> **Status: implemented.** The backstop is on by default and requires no
> configuration to function. This guide is for operators who want to *confirm*
> it is working, *observe* its behaviour, or *reconcile* the branch-gated
> reblock half. For the mechanism see
> [the concept doc](../concepts/no-progress-breaker-goal-key-backstop.md); for
> exact signatures see the
> [API reference](../reference/no-progress-breaker-goal-key-backstop-api.md).

## When you need this

Use this guide when you see (or want to prevent) **repeated identical**
`ooda-stuck` tracking issues for a single stuck goal — for example the observed
storm of `#4944, #4946, #4952, #4954, #4958`, all citing goal `a8f57a50`. If the
board-local suppression marker were doing its job you would see **one** issue;
several identical issues mean the marker was bypassed by **goal-id churn** or a
**goal-board reset**, which is exactly what the open-issue backstop covers.

## Prerequisites

- `gh` CLI installed and authenticated with **repo scope** on the target
  repository (the daemon already requires this to file issues at all).
- The daemon running a build that includes the backstop (see
  [Confirm it is active](#confirm-it-is-active)).

No environment variables, flags, or config-file keys are required — the backstop
is unconditional and additive.

## Confirm it is active

The backstop leaves two observable fingerprints.

### 1. The body marker on newly filed issues

Every issue the breaker files now ends with an `ooda-goal-key` line:

```bash
gh issue view <number> --json body -q .body | tail -1
# ooda-goal-key: 9f8c1a2b3c4d5e6f
```

A newly filed `ooda-stuck` issue **without** this line was filed by a
pre-backstop build.

### 2. The fast-path / backstop trace events

The gate emits structured `tracing` events (target `simard::ooda`). Follow the
daemon log:

```bash
journalctl -u simard --since "10 min ago" \
  | grep -E "no-progress breaker|ooda-goal-key"
```

You should see one of:

- **Fast path (steady state):** an "already suppressed (board marker)" event and
  **no** `gh issue list` call — the common case, zero API cost.
- **Backstop hit (churn / reset):** an "open duplicate found by ooda-goal-key,
  suppressing + re-seeding marker" event, following a single `gh issue list`
  search.
- **Filed:** the existing "tracking issue filed for stuck goal" warning, now
  with an `ooda-goal-key` marker in the body.

## How the labels and marker fit together

| Element | Value | Role |
|---------|-------|------|
| Label | `ooda-stuck` | Scopes the backstop search to breaker-filed issues only. |
| Body marker | `ooda-goal-key: <16-hex>` | Stable identity of the stuck goal, immune to id churn. |
| Search | `gh issue list --state open --label ooda-stuck --search "ooda-goal-key:<hex> in:body"` | The existence query. |

The 16-hex key is `sha256(goal_id)[..16]` (see
[`fold_goal_identity`](../reference/no-progress-breaker-goal-key-backstop-api.md#fold_goal_identity)).
You do **not** set it; it is derived. To compute the key for a given goal id for
your own verification:

```bash
printf '%s' '<goal_id>' | sha256sum | cut -c1-16
```

## Verify duplicate suppression end to end

Simulate the churn/reset case against a **test** repo (never production):

1. File a breaker-style issue carrying a known marker:
   ```bash
   KEY=$(printf '%s' 'demo-goal' | sha256sum | cut -c1-16)
   gh issue create -R <owner>/<test-repo> \
     --label ooda-stuck \
     --title "OODA no-progress breaker: goal stuck (UNCLEAR-CRITERIA)" \
     --body "verification fixture

   ooda-goal-key: $KEY"
   ```
2. Confirm the backstop query finds it:
   ```bash
   gh issue list -R <owner>/<test-repo> --state open --label ooda-stuck \
     --search "ooda-goal-key:$KEY in:body" --json number,title
   ```
   A non-empty result means a re-stall of `demo-goal` (even with a churned id or
   a wiped board) will be **suppressed**, not re-filed.
3. Close the issue and re-run the search — an empty result confirms **closed
   issues do not suppress** (OPEN-only scope), so a genuinely re-opened stall
   re-files as intended.

## Observe and reason about API cost

- **Steady state: zero extra calls.** The backstop query fires **only** when the
  board-local `WipRef` fast path misses. An intact board with stable goal ids
  never reaches the search.
- **Churn / reset: one call per re-escalation** of an affected goal, until the
  re-seeded `WipRef` marker restores the fast path.
- If you observe frequent backstop hits in steady state, that is a *signal of
  underlying id churn or repeated board resets* — investigate those causes; the
  backstop is correctly masking their symptom but they are worth fixing at the
  source.

## Fail-open behaviour (do not "fix" it)

If `gh issue list` fails (network, auth, rate-limit), the backstop returns "no
duplicate found" and filing proceeds. This is intentional: **a rare duplicate
issue is preferable to a lost stuck-goal signal.** Do not add a retry loop that
could block the OODA cycle, and do not change the direction to fail-closed —
that would let a transient `gh` outage silently swallow real stalls.

## The `recurring_goal_reblock` half

The overseer's `recurring_goal_reblock` filer
(`src/overseer/observer.rs`) is **branch-gated**: it is absent on some branches
(including the docs/verify branch where this feature was specified).

- **If your build includes the overseer reblock filer:** it uses the same
  pattern keyed on `stewardship::failure_signature` and deduped via
  `stewardship::find_existing` over open issues (matching
  `stewardship-signature: <sig>` in the body). Verify it the same way as above,
  searching for the `stewardship-signature:` marker instead of `ooda-goal-key:`.
- **If your build does not include it:** the reblock storm (e.g. `#4945, #4951,
  #4956`, signature `cfa5358a3b59894c`) is tracked as a documented follow-up.
  Do not attempt to configure a filer that is not compiled into your daemon;
  reconcile the branch first, then apply the identical backstop.

## Scope and limitations

- **Prevention only.** The backstop stops *new* duplicates. Issues already
  spammed before the fix landed are not auto-closed — triage them with
  [Diagnose a no-progress breaker issue storm](./diagnose-a-no-progress-breaker-issue-storm.md).
- **Thresholds unchanged.** This does not alter `NO_PROGRESS_BREAKER_THRESHOLD`
  or *when* the breaker fires — only whether the *file* step is suppressed.
- **OPEN issues only.** A closed duplicate does not suppress a genuine re-stall.

## See also

- [Concept: the breaker survives goal-id churn and board resets](../concepts/no-progress-breaker-goal-key-backstop.md)
- [Goal-key backstop API reference](../reference/no-progress-breaker-goal-key-backstop-api.md)
- [Diagnose a no-progress breaker issue storm](./diagnose-a-no-progress-breaker-issue-storm.md)
