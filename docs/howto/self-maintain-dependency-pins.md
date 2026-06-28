---
title: How to keep Simard's own dependency pins up to date
description: "A reactive done-gate and proactive reconcile that make Simard bump her own Cargo.toml git-rev pins after she lands a change upstream, so the fixes she ships actually run in her own daemon. Prompt-first, no Rust logic."
last_updated: 2026-06-26
review_schedule: as-needed
owner: simard
doc_type: howto
status: active
related:
  - ../safe-self-update.md
  - ../reference/pr-finalization-pipeline.md
  - ../howto/route-a-goal-to-its-target-repo.md
  - ../ecosystem-map.md
---

# How to keep Simard's own dependency pins up to date

> **Status: active.** This document describes shipped behaviour. The
> dependency-pin done-gate and proactive reconcile are implemented entirely in
> prompt assets (`goal_session_objective.md`, `engineer_system.md`,
> `progress_assessment_reviewer.md`, and the `progress-assessment.yaml` recipe
> mirror); this guide is both the operator reference and the spec the
> prompt-content pin tests enforce.

Simard's root `Cargo.toml` pins several of the tools she maintains by **exact
git rev**, not by branch:

| Crate (in `Cargo.toml`) | Upstream repo | Default branch |
| --- | --- | --- |
| `amplihack-agent-eval` | `rysweet/amplihack-rs` | `main` |
| `amplihack-memory` | `rysweet/amplihack-memory-lib` | `main` |
| `rustyclawd-core` | `rysweet/RustyClawd` | `main` |
| `rustyclawd-tools` | `rysweet/RustyClawd` | `main` |

Two of those crates — `rustyclawd-core` and `rustyclawd-tools` — pin the **same
upstream repo at the same rev**, so they must always be bumped together (see
[Bump-PR conventions](#bump-pr-conventions-and-de-duplication)).

A git-rev pin is reproducible, but it is also **frozen**. When Simard lands a
fix in one of those upstream repos, her own pin keeps pointing at the *old*
commit, so the fix she just merged is **not in her own build**. The motivating
case: `amplihack-agent-eval` was pinned ~22 commits behind `amplihack-rs`
`main`, which meant roughly nine merged PRs were absent from the very daemon
that wrote them.

This guide describes the two behaviours that close that gap. Both are
expressed **entirely in prompt assets** — there is no new Rust subsystem, no new
CLI command, and no change to any parser contract.

> **The finish line moves.** Under this feature, opening (or even merging) the
> *upstream* PR is **not** "done". A goal that changes a Simard
> build-dependency is done only once
> the fix is **running in Simard's own build** — i.e. her own pin has been
> bumped, `cargo build` is green, and the bump PR has landed on
> `rysweet/Simard`. This is the source-side complement of
> [Safe Self-Update](../safe-self-update.md), which swaps the *running binary*.

---

## The two triggers at a glance

```mermaid
flowchart TD
    subgraph A[Reactive done-gate]
      A1([engineer lands upstream change]) --> A2[bump own Cargo.toml rev<br/>to merged main commit]
      A2 --> A3[cargo build verifies]
      A3 --> A4[open / update bump PR<br/>vs rysweet/Simard]
      A4 --> A5[land bump PR]
      A5 --> A6([goal DONE])
    end
    subgraph B[Proactive reconcile]
      B1([idle / research time]) --> B2[compare each pinned rev<br/>to upstream default HEAD]
      B2 -->|behind| B3[open / update bump follow-up]
      B2 -->|current| B4([nothing to do])
    end
```

| | Trigger A — Reactive | Trigger B — Proactive |
| --- | --- | --- |
| **Fires when** | An engineer merges a change to a repo Simard depends on | Simard has spare, low-priority "ok to be idle" research time |
| **Priority** | Blocks the originating goal from being "done" | Low — a follow-up, never preempts real work |
| **Output** | Same goal stays open until the own-pin bump lands | A new bump follow-up goal/PR |
| **Lives in** | `goal_session_objective.md` + `engineer_system.md` + `progress_assessment_reviewer.md` | `engineer_system.md` (dependency-drift) + a note in `goal_session_objective.md` |

---

## Trigger A — the reactive done-gate

When an engineer **lands** (merges to the upstream default branch) a change to
one of the three repos Simard pins by git rev — `amplihack-rs`,
`amplihack-memory-lib`, or `RustyClawd` — the *same goal* is not complete until
Simard has also:

1. **Bumped her own pin.** Edit the matching `rev = "…"` in the root
   `Cargo.toml` to the merged `main` commit SHA.
2. **Verified the build.** `cargo build` (low-space variant when disk is tight —
   `scripts/cargo-low-space build`) must succeed against the new rev. A bump
   that does not build is rolled back, not shipped.
3. **Opened (or updated) the bump PR** against `rysweet/Simard` — see
   [Bump-PR conventions](#bump-pr-conventions-and-de-duplication).
4. **Landed the bump PR.** It rides the same
   [PR-finalization review pipeline](../reference/pr-finalization-pipeline.md)
   and merge-ready gate as any other Simard change (CI-green → merge), exactly
   like [making Simard own each PR to landing (#2410)](../reference/pr-finalization-pipeline.md).

Only after step 4 is the originating goal allowed to report `done`.

The actual **redeploy** of the running daemon (swapping the binary, hot-reloading
prompts) remains **operator-gated** — it is the operator's step, performed via
[Safe Self-Update](../safe-self-update.md), and is *not* required for the
automated goal to be marked done. The done-gate guarantees the fix is *in the
shipped source build*; the operator decides when to roll it out.

### Where this gate lives

| Prompt asset | Responsibility |
| --- | --- |
| `prompt_assets/simard/goal_session_objective.md` | **Encodes** the done-gate: a goal that lands an upstream change to a build-dependency repo is not done until the own-pin bump lands and `cargo build` passes. Stays **prose only** (preserves the `NO ACTION` marker and `PROGRESS: NN` line the goal-session parser reads). |
| `prompt_assets/simard/engineer_system.md` | **Tells** the engineer that landed the upstream change to follow through with the own-`Cargo.toml` bump + build-verify + bump PR. |
| `prompt_assets/simard/progress_assessment_reviewer.md` | **Rejects** a premature `done` — see [Reviewer enforcement](#reviewer-enforcement). |

This is a **new done-gate that runs *after* landing**, alongside #2410. It
composes additively with the already-shipped behaviours:

- **#2404 loop-awareness** — the bump is concrete forward progress, not a loop.
- **#2405 per-issue fan-out** — a bump follow-up is just another work item.
- **#2410 own-PRs-to-landing** — the bump PR is owned to landing like any PR.
- **#2413 crusty / pr-guide finalization** — the bump PR runs the same
  finalization pipeline before merge.

### Does the done-gate hold the goal open, or spawn a follow-up?

**Trigger A holds the *originating* goal open** — that is what makes "landing
upstream is not done" meaningful. It deliberately does *not* spawn a separate
goal; coupling the bump to the goal that caused it is the point.

This does **not** trip loop-awareness (#2404). Loop/stall detection flags
*repetition without progress*. The done-gate instead appends a **bounded,
monotonic sequence of new artifacts** — one rev-bump commit, one `cargo build`,
one bump PR — each concrete forward progress on a *different* file
(`Cargo.toml`) and a *different* PR than the upstream change. If that bounded
sequence itself gets stuck (e.g. the bump PR cannot land), it surfaces through
the **normal stall path** and is escalated like any other blocked PR — it is
never retried indefinitely. Contrast with Trigger B, which *is* a separate
low-priority follow-up precisely because nothing is holding a goal open.

---

## Trigger B — the proactive reconcile

When Simard has low-priority idle/research time (the same budget she uses for
self-maintenance), she periodically checks each rev-pinned git dependency for
**drift**: is the pinned rev behind the upstream default branch?

For each dependency she compares the pinned rev to the upstream `HEAD`:

```bash
# Current pin (from Cargo.toml)
PINNED=59548a96049ab8d558110bcaf9c82a4316f1bbf0

# Upstream default-branch HEAD
git ls-remote https://github.com/rysweet/amplihack-rs.git main

# How far behind is the pin? (GitHub compare API: base=pin, head=main)
gh api repos/rysweet/amplihack-rs/compare/$PINNED...main \
  --jq '{status, behind: .behind_by, ahead: .ahead_by}'
```

`status: "behind"` (or `ahead > 0`) means the pin is stale. Simard then opens —
or updates — a **bump follow-up** that does exactly what Trigger A does:
re-point the rev to the new `main`, verify `cargo build`, and ship it through
the normal landing pipeline.

This trigger is **low priority by construction**. It never preempts an active
engineering goal; it only fills spare capacity. It is described in
`engineer_system.md` (the "dependency-drift" self-maintenance directive) and
**noted** as acceptable idle-time work in `goal_session_objective.md`.

This is the **upstream-repo analog of the self-update Simard already has.**
`goal_session_objective.md` already carries a "Self-update awareness" section in
which the OODA brain detects *Simard-repo* drift via `compute_commits_behind()`
and triggers `simard safe-update` when no engineers are in flight. Trigger B
points that same "detect drift, reconcile when idle" posture at the **repos
Simard pins** instead of the Simard repo itself — it reuses the existing idle
self-maintenance idea rather than inventing a new scheduler.

---

## Bump-PR conventions and de-duplication

Both triggers can want to bump the *same* dependency, and the daemon runs
multiple engineers concurrently (#2405). To avoid a pile of duplicate bump PRs,
the bump uses a **deterministic naming convention keyed on the upstream repo**
— *not* on the crate — so that crates sharing a repo collapse into one PR:

| Field | Value |
| --- | --- |
| Branch | `chore/bump-<upstream-repo>-pin` (e.g. `chore/bump-rustyclawd-pin`) |
| PR title | `chore(deps): bump <upstream-repo> pin to <short-sha>` |
| Base | `rysweet/Simard` `main` |

`<upstream-repo>` is the source repository, lower-cased (`amplihack-rs`,
`amplihack-memory-lib`, `rustyclawd`). Each bump PR updates **every** crate that
pins that repo:

| `<upstream-repo>` | Crates bumped together in one PR |
| --- | --- |
| `amplihack-rs` | `amplihack-agent-eval` |
| `amplihack-memory-lib` | `amplihack-memory` |
| `rustyclawd` | `rustyclawd-core` **and** `rustyclawd-tools` |

> **One PR per upstream repo — bump shared crates atomically.** When several
> crates pin the same repo at the same rev (today `rustyclawd-core` and
> `rustyclawd-tools` both pin `RustyClawd` at `43ebaa1`), the bump PR re-points
> **all** of them to the new rev in the **same commit**. Never open a branch per
> crate: that would split one upstream commit across two PRs and let the build
> see two different `RustyClawd` revs at once.

**Before opening a bump PR, check for an existing open one:**

```bash
# Is a bump PR already open for this upstream repo?
gh pr list --repo rysweet/Simard --state open \
  --head "chore/bump-rustyclawd-pin" \
  --json number,title,headRefName
```

- **Found** → **update it**: re-point every crate from that repo to the latest
  `main`, re-run `cargo build`, force-update the branch, and refresh the PR
  body. Do **not** open a second PR.
- **Not found** → open a fresh PR with the convention above.

> **Cross-repo safety.** The upstream change lives in another repo
> (`amplihack-rs`, `RustyClawd`, …); the **bump PR is always opened against
> `rysweet/Simard`**, because `Cargo.toml` lives in Simard. This is the same
> separation as [routing a goal to its target repo](./route-a-goal-to-its-target-repo.md):
> work happens where the file being changed lives.

---

## Reviewer enforcement

`progress_assessment_reviewer.md` **is** the gate that stops a premature
`done`. Its output contract is **unchanged** — a single-line JSON object
`{"verdict": "accept" | "reject", "rationale": "…"}` — and the `verdict` token
stays exactly `accept` / `reject` (lowercase) so the Rust reviewer parser keeps
working.

The reviewer cannot diff git revs itself (it sees only the problem, plan,
prior/claimed percent, and a WIP summary). So the rule **is** phrased as
**evidence-absence**, not rev-comparison:

> Reject a `done` / 100% claim when the problem or plan describes **landing an
> upstream change to a Simard build-dependency repo**, but the plan / WIP
> summary shows **no evidence** of *both* (a) the own `Cargo.toml` rev bump to
> the merged commit and (b) a verified `cargo build`.

Example rejection the reviewer **emits** (matching the spaced JSON form the
existing reviewer prompt already uses):

```json
{"verdict": "reject", "rationale": "Upstream PR merged to amplihack-rs main, but no evidence the own Cargo.toml amplihack-agent-eval rev was bumped and cargo build re-verified; landing upstream is not done until the fix ships in Simard's own build."}
```

If the WIP summary *does* show the bump landed and the build passed, the
reviewer accepts as normal.

---

## Operator: rolling a bump into the running daemon

Landing the bump PR puts the fix in the **source build**. To get it into the
**running daemon**, an operator:

1. Pulls the merged bump on the host and rebuilds, or
2. Runs [Safe Self-Update](../safe-self-update.md)
   (`simard safe-update`) which downloads, self-tests, swaps, and validates the
   new binary.

Prompt-asset changes (this feature is all prompts) are **hot-reloaded** by
syncing the assets into the live state root:

```bash
rsync -a prompt_assets/simard/ ~/.simard/prompt_assets/simard/
```

No daemon restart is required for the prompt changes to take effect on the next
OODA cycle. The binary-level dependency bump still requires a rebuild/redeploy
as above.

---

## Verify end-to-end

1. **Confirm the pins and their upstreams:**

   ```bash
   grep -nE 'git = .*(amplihack-rs|amplihack-memory-lib|RustyClawd)' Cargo.toml
   ```

2. **Check one dependency for drift:**

   ```bash
   PINNED=$(grep 'amplihack-agent-eval' Cargo.toml | grep -oE '[0-9a-f]{40}')
   gh api repos/rysweet/amplihack-rs/compare/$PINNED...main --jq '.behind_by'
   ```

   A non-zero result means the pin is stale and Trigger B should produce a bump
   follow-up.

3. **After a bump lands, confirm the rev moved and the build is green:**

   ```bash
   git -C ~/src/Simard pull
   grep -n 'amplihack-agent-eval' ~/src/Simard/Cargo.toml   # rev now == merged main
   cargo build --quiet                                       # succeeds
   ```

4. **Confirm no duplicate bump PRs are open:**

   ```bash
   gh pr list --repo rysweet/Simard --state open \
     --search 'in:title "chore(deps): bump"'
   ```

   At most one open PR per **upstream repo** (so `rustyclawd-core` and
   `rustyclawd-tools` never appear in two separate bump PRs).

---

## Related reading

- [Safe Self-Update](../safe-self-update.md) — the binary-swap half of
  "ship it into her own running build"; this guide is the source-pin half.
- [PR-finalization review pipeline](../reference/pr-finalization-pipeline.md) —
  the gate every bump PR passes before merge.
- [How to route a goal to its target ecosystem repo](./route-a-goal-to-its-target-repo.md) —
  the same "work where the file lives" separation, applied to upstream goals.
- [Ecosystem map](../ecosystem-map.md) — the repos Simard stewards and depends on.
