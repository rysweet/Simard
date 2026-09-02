---
title: Completion-gate issue-fallback merged-PR recovery API reference
description: Reference for the issue-based merged-PR fallback in GhCliEvidenceSource::any_pr_merged. When reconcile_merged_prs has pruned a completed goal's `pr` wip_ref, the gate independently recovers merge evidence by asking gh whether a merged PR closes the goal's linked issue — cross-repo aware, fail-closed, and gated behind pr-ref absence so it fires only when needed. Fixes the systemic completed-vs-PrNotMerged re-block churn (issue #12).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./completion-evidence-gate-api.md
  - ./wip-ref-liveness-reconcile-api.md
  - ./wip-ref-liveness-reconcile-hardening-api.md
  - ./cross-repo-merge-authority.md
  - ../concepts/completion-gate-merged-pr-reconciliation.md
  - ../concepts/deploy-aware-done-gate.md
  - ../howto/diagnose-perpetual-completion-recuration.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/completion_gate/tests.rs
  - ../../src/ooda_loop/cycle.rs
---

# Completion-gate issue-fallback merged-PR recovery API reference

> **Status: implemented.** The issue-based merged-PR fallback lives in
> [`GhCliEvidenceSource::any_pr_merged`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> and its private helper `merged_pr_closes_issue`. It is an **additive**
> extension of the [Completion-Evidence Gate](./completion-evidence-gate-api.md):
> the `EvidenceSource` trait signature is **unchanged**, so the blanket `&T`
> impl and every `FakeEvidence` test double keep compiling. Tests live in
> [`src/goal_curation/completion_gate/tests.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate/tests.rs).

This reference specifies the merged-PR recovery path added in issue **#12**. For
the rationale and the defect it repairs, see
[Completion-gate merged-PR reconciliation](../concepts/completion-gate-merged-pr-reconciliation.md).
It builds directly on the
[wip-ref liveness reconcile](./wip-ref-liveness-reconcile-api.md) pruning prongs
— the fallback exists precisely because Prong 2 prunes a merged PR's `pr`
wip_ref before the gate reads it.

## Contents

- [The reconciliation defect](#the-reconciliation-defect)
- [`any_pr_merged` — resolution order](#any_pr_merged-resolution-order)
- [`merged_pr_closes_issue` helper](#merged_pr_closes_issue-helper)
- [Input validation](#input-validation)
- [Fail-closed mapping](#fail-closed-mapping)
- [Cross-repo behaviour](#cross-repo-behaviour)
- [Over-fetch guard](#over-fetch-guard)
- [Observability](#observability)
- [Test surface (`FakeEvidence`)](#test-surface-fakeevidence)
- [See also](#see-also)

## The reconciliation defect

Before this fix, a genuinely-completed goal could enter a **perpetual re-block
loop**. The observable signature from the OODA journal (issue #12): 9 goals —
`simard-example-identity-gastronome-culinary-men-84186abe` and
`rysweet/agent-kgpacks-rs` issues #12, #18, #19, #20, #21, #22, #23, #25 —
showed `STATUS=completed` in `simard goal list`, yet **every** OODA cycle
(17/17 cycles over a 6-hour window = 153 identical emissions) the completion
curate step re-blocked them with:

```text
[simard] OODA curate: completion BLOCKED for goal <id> — missing PR not merged
```

The root cause is a **per-cycle ordering interaction**, not false-completion,
not a `repo_slug` resolution bug, and not a stalled self-merge:

1. `reconcile_merged_prs` (wip-ref liveness Prong 2) prunes the merged PR's
   `pr` wip_ref each cycle — a merged PR is *not open*, so it is dropped from
   `wip_refs`.
2. The completion gate then calls `any_pr_merged`, finds **no** `pr` wip_ref,
   and — under the old `None => Ok(false)` fast path — reports
   `PrNotMerged` **without any recovery path**.

The pruning is correct (a merged PR should not count as live in-flight work).
The gate's old behaviour was also individually correct. Their *interaction*
was the defect: pruning stripped the only evidence the gate knew how to read.
This fix gives the gate a second, independent way to recover merge evidence.

## `any_pr_merged` — resolution order

`GhCliEvidenceSource::any_pr_merged` now resolves in three ordered steps:

```rust
fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool> {
    // 1. Fast path (unchanged): a tracked `pr` wip_ref is authoritative.
    if let Some(num) = first_ref_of_kind(goal, "pr") {
        let repo = self.repo_slug(goal);
        return Ok(self.gh_state("pr", &repo, num)?.eq_ignore_ascii_case("MERGED"));
    }

    // 2. Issue fallback (new): the `pr` ref was pruned or never existed.
    //    Attempt recovery ONLY when an `issue` ref is present.
    if let Some(issue_ref) = first_ref_of_kind(goal, "issue") {
        let repo = self.repo_slug(goal);
        return self.merged_pr_closes_issue(&repo, issue_ref);
    }

    // 3. No `pr` and no `issue` ⇒ no merge evidence, cheaply, no network.
    Ok(false)
}
```

| Goal state | Path taken | Result | Network call? |
| --- | --- | --- | --- |
| `pr` wip_ref present | Fast path | `gh pr view … .state == MERGED` | yes (unchanged) |
| No `pr`, `issue` present | Issue fallback | `merged_pr_closes_issue` | yes (new, guarded) |
| No `pr`, no `issue` | Cheap block | `Ok(false)` | **no** |

The fast path is byte-for-byte the previous behaviour: a live `pr` wip_ref is
still authoritative and short-circuits before any fallback. Only the former
`None => Ok(false)` branch is split into steps 2 and 3.

## `merged_pr_closes_issue` helper

A new private method on `GhCliEvidenceSource`:

```rust
/// Does a MERGED pull request close the given issue in `repo`?
///
/// Queries `gh api graphql` for
/// `repository.issue(number:).closedByPullRequestsReferences(includeClosedPrs:true){ nodes { merged } }`
/// and returns `Ok(true)` iff at least one referencing PR reports `merged: true`.
/// Cross-repo aware: `repo` is the goal's own `owner/repo` slug.
///
/// Fail-closed: any spawn failure, non-zero `gh` exit, malformed input, or
/// unparseable JSON surfaces as `Err(SimardError::VerificationFailed)` so the
/// gate blocks with `CouldNotVerify` rather than silently reporting `false`.
fn merged_pr_closes_issue(&self, repo: &str, issue_ref: &str) -> SimardResult<bool>;
```

Key properties:

- **Parameterized query.** `owner`, `name`, and `issue` are passed as typed
  `gh api graphql -F owner=… -F name=… -F number=<u64>` variables. The GraphQL
  document is a fixed string literal; goal-derived values are **never**
  `format!`-interpolated into the query body (GraphQL-injection guard).
- **Read-only.** The query reads `closedByPullRequestsReferences` only. No
  mutation (`mergePullRequest`, `closeIssue`, `addComment`) is ever issued.
- **`gh api graphql`, not `gh issue view`.** `gh issue view --json` does not
  expose `closedByPullRequestsReferences.merged`; the GraphQL surface is
  required to see the *merged* state of a closing PR.
- **Authoritative-true only.** `Ok(true)` requires at least one node with
  `merged == true`. An issue closed with no merged closing PR (e.g. closed
  manually, or closed by an unmerged/`draft` PR) yields `Ok(false)`, keeping
  the goal blocked — completion still requires a real merge.
- **No internal retries.** A single subprocess call per invocation, reusing the
  same `std::process::Command` + `.output()` spawn shape as the existing
  [`gh_state`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
  fast path. Like `gh_state`, the call inherits `gh`'s own network timeouts
  rather than imposing a separate Rust-side subprocess deadline — no such
  bounded-timeout pattern exists for `gh` spawns elsewhere in the crate, so the
  fallback deliberately does **not** invent one. Rate-limit and DoS safety comes
  from the [over-fetch guard](#over-fetch-guard) (the call fires only when the
  `pr` ref is absent), not from backoff or a wall-clock deadline inside the
  helper.

## Input validation

Validation is applied **on the fallback path only**. The unchanged fast path
retains its pre-existing behaviour (it forwards the tracked `pr` `ref_id` and the
resolved `repo` slug to `gh_state` without added validation — an accepted,
pre-existing asymmetry, since those values originate from the same trusted
wip_ref/`repo_slug` sources). The new fallback validates both goal-derived inputs
before the `gh` process is spawned. Validation failures map to `Err`
(→ `CouldNotVerify`), never to a silent `Ok(false)`:

| Input | Rule | On failure |
| --- | --- | --- |
| `issue_ref` | Parsed to `u64` (`str::parse::<u64>()`). | `Err(VerificationFailed)` |
| `repo` slug | Matches `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`. | `Err(VerificationFailed)` |

This rejects any value that could be interpreted as a flag or shell/argument
metacharacter — a leading `-`, embedded spaces, `..`, `;`, `$`, backticks, etc.
The parsed `u64` is passed as an integer GraphQL variable and the validated slug
is split into `owner`/`name` variables. Crucially, **every goal-derived value
reaches `gh` only as a `-F <key>=<value>` GraphQL field variable** (`owner`,
`name`, `number`) — never as a bare positional argument — so `gh`'s flag parser
never encounters it in a flag position. That structural property, not a `--`
option terminator, is what closes the argument-injection surface; the validation
above is defence-in-depth on top of it. (The subprocess is invoked directly via
`std::process::Command` args, so there is no shell to interpret metacharacters
either.)

## Fail-closed mapping

The fallback preserves the gate's central invariant: **never archive on
unverifiable evidence.** Every non-authoritative outcome maps as follows:

| Situation | `any_pr_merged` returns | Gate verdict |
| --- | --- | --- |
| A closing PR reports `merged: true` | `Ok(true)` | clause satisfied |
| Issue exists, no merged closing PR | `Ok(false)` | `Blocked { PrNotMerged }` |
| No `pr` and no `issue` wip_ref | `Ok(false)` | `Blocked { PrNotMerged }` |
| `gh` spawn/exit/JSON error | `Err(VerificationFailed)` | `Blocked { CouldNotVerify }` |
| Malformed `issue_ref` / `repo` slug | `Err(VerificationFailed)` | `Blocked { CouldNotVerify }` |

There is no path from a `gh` error to `Ok(false)`: a transient GitHub outage
blocks the goal for that cycle (`CouldNotVerify`) rather than misreporting it as
unmerged and archiving it incorrectly. `Ok(true)` is reachable **only** from an
authoritative `merged: true` node.

## Cross-repo behaviour

The fallback queries the goal's **own** repository, resolved through the
existing [`repo_slug`](./completion-evidence-gate-api.md#evidence-sources)
helper — the same resolution the fast path and `issue_closed` use:

- A goal scoped to `rysweet/agent-kgpacks-rs` (issue #12, #18–#23, #25) queries
  `owner: "rysweet", name: "agent-kgpacks-rs"`, **not** Simard's default owner.
- An unscoped goal (or an explicit `Simard` slug) queries the default
  `rysweet/Simard`.

This is why the 8 cross-repo `agent-kgpacks-rs` goals recover correctly: their
merged PRs live in a different repo than the daemon's own checkout, and the
GraphQL query is scoped there.

## Over-fetch guard

The GraphQL call fires **only** when the `pr` wip_ref is absent. A goal that
still carries a live `pr` ref takes the fast path and never reaches the
fallback. This mirrors the reconcile fast-path and bounds network traffic:

- For an in-flight goal (has `pr` ref): 0 extra calls — unchanged.
- For a completed goal whose `pr` ref was pruned: exactly 1 GraphQL call per
  cycle, and once evidence is recovered the goal **archives**, so the call
  stops recurring. This is what kills the 153-emission churn — the goal leaves
  the active board instead of being re-blocked forever.
- For a goal with neither `pr` nor `issue`: 0 calls (cheap `Ok(false)`).

## Observability

New code emits **structured tracing + OTel only** — no `print!`/`println!`.
A single span wraps the fallback with non-sensitive fields:

| Field | Example | Notes |
| --- | --- | --- |
| `goal_id` | `simard-example-…-84186abe` | goal identity |
| `repo_slug` | `rysweet/agent-kgpacks-rs` | queried repo |
| `issue_num` | `18` | parsed issue number |
| `merged` | `true` | authoritative result |
| `outcome` | `recovered` / `not_merged` / `could_not_verify` | terminal branch |

Tokens, `GH_TOKEN`, and raw auth-bearing `gh` stderr are **never** logged; on a
`gh` error only the sanitized `VerificationFailed { reason }` string is
recorded. The pre-existing `eprintln!` that emits the BLOCKED curate line
(`src/ooda_loop/cycle.rs`) is **out of scope** and unchanged — it simply stops
firing for these goals once they archive.

## Test surface (`FakeEvidence`)

`FakeEvidence` in
[`completion_gate/tests.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate/tests.rs)
is extended with an injectable issue-fallback outcome so the new path is
exercised hermetically (no network, no live `gh`). The added coverage:

| Test | Scenario | Expected |
| --- | --- | --- |
| pruned-`pr` recovers via issue fallback | completed goal, `pr` ref pruned, `issue` ref present, closing PR merged | `Complete` |
| cross-repo recovery | goal scoped to `rysweet/agent-kgpacks-rs`, merged closing PR | `Complete`, queried against that repo |
| fail-closed on verify error | fallback source returns `Err` | `Blocked { CouldNotVerify }` |
| no-`pr`/no-`issue` cheap block | goal with neither ref | `Blocked { PrNotMerged }`, no fallback call |
| malformed input rejected | `issue_ref` with leading `-` / metachar `repo` slug | `CouldNotVerify`, value never reaches `gh` as a flag |

Because the fallback lives in the production impl (no trait-signature change),
existing `FakeEvidence` doubles and the blanket `&T` impl compile unchanged; the
new field carries a conservative default so untouched tests keep their prior
behaviour.

## See also

- [Completion-gate merged-PR reconciliation concept](../concepts/completion-gate-merged-pr-reconciliation.md) — the rationale.
- [Completion-Evidence Gate API](./completion-evidence-gate-api.md) — the gate this extends.
- [wip-ref Liveness Reconcile API](./wip-ref-liveness-reconcile-api.md) — the pruning prong whose interaction this repairs.
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — how goals route to non-Simard repos.
- [How to diagnose perpetual completion re-curation](../howto/diagnose-perpetual-completion-recuration.md) — operator playbook for the churn signature.
