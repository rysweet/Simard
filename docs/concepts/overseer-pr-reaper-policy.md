---
title: "Overseer PR-Reaper Policy (deterministic, fail-closed thresholds that can only tighten agent PR dispositions)"
description: >
  How Simard adds a conservative, deterministic guard between the agentic
  merge-queue reasoner and the intervention gate so that stale, long-CONFLICTING,
  and near-duplicate PRs are handled under clearly-bounded policy — never on an
  agent's say-so alone. The reaper_policy layer is a pure, fail-closed
  post-parse validator: it takes the agent-proposed FlagStalePr / CloseDuplicatePr
  dispositions and applies numeric thresholds (stale 14d, CONFLICTING 7d, title
  similarity >=0.85 + changed-file overlap) that can only TIGHTEN a disposition
  (downgrade or drop it), never relax it. Destructive close stays behind the
  existing RiskClass::MergeAuthority notify-only gate (dry-run by default;
  opt-in via allow_verify_merge). No PR-controlled text ever reaches an argv;
  a survivor is chosen deterministically to defeat near-duplicate griefing.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./agentic-merge-queue-reasoning.md
  - ./stale-engineer-claim-reaper.md
  - ../operations/claim-reaper-kill-switch.md
  - ../reference/overseer-pr-reaper-policy-api.md
  - ../reference/agentic-merge-queue-reasoning-api.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/configure-overseer-pr-reaper.md
  - ../howto/triage-stale-pull-requests.md
  - ../design/agentic-observe-orient-merge-queue.md
---

# Overseer PR-Reaper Policy

> **Status: implemented.** This page describes shipped behaviour in the present
> tense. The policy layer lives in
> [`src/overseer/reaper_policy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/reaper_policy.rs)
> and is wired into the observe/orient dispatch in
> [`src/overseer/merge_queue_observe.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_queue_observe.rs),
> ahead of the existing
> [`Guardrails::admit`](../reference/agentic-merge-queue-reasoning-api.md) gate.
> Thresholds resolve from
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs).
> The change is purely **additive**: it *repairs and extends* the existing
> merge-queue reasoning surfaces rather than adding a new autonomous actor, and
> it changes **no** merge authorization. Documentation and implementation land
> in the **same pull request**.

## The problem it solves

The [agentic merge-queue reasoner](./agentic-merge-queue-reasoning.md) surveys
the whole open-PR queue each Overseer cycle and proposes `PrDisposition`s
(`ReadyForMerge`, `NeedsWork`, `Stale`, `Duplicate`). Those proposals become
`FlagStalePr` / `CloseDuplicatePr` interventions. Left unchecked, a large
stale/conflicting backlog accumulates that no janitor cleans up — for example a
queue of ~50 open PRs with many in `CONFLICTING` mergeable state, near-duplicate
pairs (two PRs with essentially the same title touching the same files), and
long-open conflicting PRs weeks past their last update.

The gap is **not** that the reasoner is silent — it is that acting on its
`Stale`/`Duplicate` calls *directly* would trust a model's judgement to close
someone's PR. That is unacceptable for a destructive action. What was missing is
a **deterministic, conservative policy** that decides when a proposed
disposition is actually eligible, and that can never be talked *up* by the agent
into closing more than the numbers justify.

## The reaper policy: a tighten-only guard

`reaper_policy` is a pure, fail-closed validation layer that mirrors the shape
of the existing [claim reaper](./stale-engineer-claim-reaper.md): agent proposes,
deterministic policy disposes. It sits **between** the parsed reasoner output and
`Guardrails::admit`:

```text
agent brief ──parse──▶ ReasonedPr (Stale | Duplicate | …)
                          │
                          ▼
              reaper_policy::evaluate(...)      ← deterministic thresholds
                          │   (can only TIGHTEN)
                          ▼
              FlagStalePr / CloseDuplicatePr ──▶ Guardrails::admit ──▶ (notify-only by default)
```

`evaluate` takes an agent-proposed disposition plus the objective PR facts
(`updatedAt`, `mergeable`, title, changed-file set) and returns a
`ReaperDecision` that is **never more aggressive** than what the agent proposed.
It can:

- **confirm** a proposal that also clears the numeric bar,
- **downgrade** a destructive `CloseDuplicatePr` to a non-destructive
  `FlagStalePr` (flagged with the `DuplicateNotClosable` reason, so the operator
  note reflects the *real* cause), or to nothing, or
- **drop** a proposal entirely when the facts do not meet the threshold.

It can **never** upgrade `FlagStalePr` into a close, invent a disposition the
agent did not propose, or lower the intervention's `RiskClass`.

### Conservative default thresholds

All thresholds are config-overridable (see
[configure the reaper](../howto/configure-overseer-pr-reaper.md)) and chosen to
match the observed backlog evidence without risking false positives:

| Signal | Default | Rationale |
| --- | --- | --- |
| **Stale** | No update in **14 days** | Comfortably older than a normal review cycle. |
| **Long-CONFLICTING** | `mergeable == CONFLICTING` for **7 days** | A week of unresolved conflicts is a strong staleness signal. |
| **Near-duplicate** | Normalized title similarity **≥ 0.85** *and* overlapping changed-file set | Requires *both* textual and structural overlap — never title similarity alone. |

Title normalization drops stopwords (reusing the existing cognition
relevance-scoring precedent) before similarity scoring, so cosmetic wording
differences do not defeat detection while unrelated PRs stay well below the bar.

## Fail-closed and safe by construction

The policy is designed so that ambiguity always resolves toward **doing less**:

- **Fail-closed parsing.** An unknown `mergeable` value or an unparseable
  timestamp makes a PR **ineligible** for auto-close — the policy declines rather
  than guesses.
- **Dry-run by default.** Destructive `CloseDuplicatePr` stays in the existing
  [`RiskClass::MergeAuthority`](../reference/agentic-merge-queue-reasoning-api.md)
  notify-only class. Nothing is actually closed until an operator opts in by
  flipping `allow_verify_merge`. `FlagStalePr` (a non-destructive review comment)
  remains the default visible action.
- **Never lowers risk.** `evaluate` can only tighten; it can never move
  `CloseDuplicatePr` below `MergeAuthority` down to `Routine`.
- **Never closes on similarity alone.** A near-duplicate close requires title
  similarity **and** file overlap, and even then defaults to flag-not-close
  unless the destructive gate is open.
- **Deterministic survivor selection.** When a duplicate pair is confirmed, the
  survivor is chosen deterministically as the **lowest-numbered (earliest)** PR —
  `PrFacts` carries the PR `number` (a monotonic creation-order proxy) rather
  than a `created_at`, so the lower number is always the earlier PR. An attacker
  who opens a near-duplicate *after* a legitimate PR therefore cannot cause the
  earlier PR to be closed (griefing resistance, review risk **R3**).

## Security posture

> **No off switch for flagging.** Like the non-destructive side of the
> [claim reaper](./stale-engineer-claim-reaper.md), the reaper's *flagging* has
> no kill switch — a review comment is safe by construction, so there is nothing
> to disable. Only the **destructive** duplicate-close is gated (dry-run by
> default via `allow_verify_merge`). The tunable surface is the thresholds; see
> the [claim-reaper kill switch](../operations/claim-reaper-kill-switch.md) for
> the analogous operations posture.

Untrusted PR data (title, body, ref, changed files, `mergeable`) flows only as
**positional subprocess argv** through the existing intervention builders and
`run_gh` — never as a shell string, and never interpolated into a command line.
The close-comment template interpolates **only integer PR ids**; no
PR-controlled free text reaches destructive argv, and the reaper path asserts
that argv can never contain `--admin` or `--no-verify`. `reaper_policy` itself
issues **no** `gh pr close` — it only *decides*; the guarded intervention layer
acts. Telemetry logs scalars and PR ids only, truncating any echoed PR text to
avoid log-flood DoS, and never reads or logs the `gh` token. Per the project
rule, it uses structured `tracing` + OTel only — **no `print!` / `println!` /
`eprintln!`**.

## Relationship to existing surfaces (repair/extend, not rebuild)

The reaper policy deliberately **reuses** what already exists:

- the reasoner and its `PrDisposition` parse in
  [`merge_queue_observe.rs`](../reference/agentic-merge-queue-reasoning-api.md);
- the `StalePrDetected` / `DuplicatePrDetected` signals and the
  `FlagStalePr` / `CloseDuplicatePr` interventions;
- the [`Guardrails`](../reference/agentic-merge-queue-reasoning-api.md)
  `MergeAuthority` notify-only gate and the `allow_verify_merge` opt-in.

The only *new* surface is the pure `reaper_policy` module; everything else is a
wiring change that routes proposals through it before admission.

## Related

- [Overseer PR-Reaper Policy API reference](../reference/overseer-pr-reaper-policy-api.md)
- [Configure the Overseer PR reaper](../howto/configure-overseer-pr-reaper.md)
- [Agentic Merge-Queue + Issue Reasoning](./agentic-merge-queue-reasoning.md)
- [Triage Stale Pull Requests](../howto/triage-stale-pull-requests.md)
- [Design — Agentic observe/orient merge-queue](../design/agentic-observe-orient-merge-queue.md)
