---
title: PR-finalization review pipeline reference
description: The bounded, ordered review pipeline every Simard engineer runs at the end of a PR — a high-end-model crusty review→fix loop, the pr-guide illustrated walkthrough, a final lightweight review, then the existing merge-ready gate → merge → close issue. Prompt-driven, hot-reloads, builds on #2404/#2405/#2410.
last_updated: 2026-06-26
owner: simard
doc_type: reference
status: reference
related:
  - ../howto/edit-the-engineer-system-prompt.md
  - ./concurrent-engineer-dispatch.md
  - ./maximum-safe-parallelism.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../concepts/prompt-driven-tdd-discipline.md
---

# PR-finalization review pipeline reference

> **Goal:** Before any PR an engineer opens is merged, run an **ordered,
> bounded** finalization pipeline — (1) a **crusty-old-engineer** review→fix
> loop on a **high-end reasoning model**, fixing every actionable finding and
> re-reviewing the *latest* PR state until crusty is satisfied or a cap is hit;
> (2) the **pr-guide** illustrated walkthrough (graceful-skip where unavailable);
> (3) a **final lightweight review** pass; then (4) the existing **merge-ready**
> gate → merge → close issue. The loop runs **inside one engineer's PR
> finalization** — it never spins the OODA goal-action brain.

This pipeline is **prompt-driven**. This change **adds** it as a new ordered
section in `prompt_assets/simard/engineer_system.md` (the engineer's
PR-finalization instructions) and **adds a short cross-reference note** in
`prompt_assets/simard/goal_session_objective.md` (so the OODA brain knows
finalization runs *inside* the engineer and does not spin the goal-action
cycle); the gate is enforced **before** merge. Both prompt edits are net-new
content this change introduces — they do not exist today. The engineer-lifecycle
recipe (`recipes/ooda-engineer-lifecycle.yaml`) is **unchanged** — it dispatches
the engineer that reads `engineer_system.md`, so no recipe edit is required. The
change introduces **no new Rust decision logic** and **hot-reloads** from
`~/.simard/prompt_assets/simard/` — see [Deployment](#deployment).

It builds **on top of** three earlier behaviors and must not regress them:

- **Loop-awareness (#2404)** — the engineer does the review loop *internally*;
  the goal-action brain only dispatches and checks, so the OODA cycle does not
  re-loop on a goal whose engineer is mid-finalization.
- **Parallel fan-out (#2405)** — finalization is per-PR and per-engineer, so
  multiple engineers can finalize their own PRs concurrently up to the AIMD cap
  (see [concurrent engineer dispatch](./concurrent-engineer-dispatch.md)).
- **Own-PRs-to-landing (#2410)** — finalization is the work that *precedes* the
  existing "drive to merge" landing behavior; it does not replace it.

## Where it sits in the engineer flow

The pipeline slots into the engineer's **Definition of Done** between "PR opened
with evidence headings" and "Drive to merge". The merge step is now **gated on
the pipeline having run**:

```
commit → push → open/update PR (evidence headings)
        │
        ▼
┌─────────────────────────────────────────────────────────────┐
│  PR-FINALIZATION PIPELINE  (this reference)                  │
│                                                             │
│  1. CRUSTY REVIEW→FIX LOOP   (high-end model, bounded cap)  │
│       review → fix every finding → push → re-review LATEST  │
│       … repeat until satisfied OR cap reached               │
│  2. PR-GUIDE                 (illustrated guide; skip-OK)   │
│  3. FINAL REVIEW             (one lightweight pass)         │
└─────────────────────────────────────────────────────────────┘
        │
        ▼
  4. MERGE-READY skill → simard merge-pr / gh pr merge → close issue   (#2410)
```

Stages 1–3 run **only after** the PR is open and the six merge-ready evidence
headings are present; stage 4 is the pre-existing
[Merge-Ready Contract](../howto/edit-the-engineer-system-prompt.md) path and is
unchanged except that it now runs **last**.

## Stage 1 — crusty review→fix loop (high-end model)

The engineer invokes the **crusty-old-engineer** skill to review the PR's
diff/changes, then fixes **every actionable finding** and re-reviews, looping
until crusty is satisfied or the cap is reached.

| Property | Behavior |
|----------|----------|
| **Reviewer** | `crusty-old-engineer` skill — a curmudgeonly senior-engineer reviewer that surfaces correctness, maintainability, and long-term-consequence findings. |
| **Model** | A **high-end reasoning model**, pinned explicitly (the engineer itself runs the Copilot CLI default/auto model, so crusty is invoked through a `copilot --model "$SIMARD_REVIEW_MODEL" --reasoning-effort high --context long_context` subprocess to force the high-end model at **high** reasoning effort over the **1M-token** context tier). Default `gpt-5.5`; see [Configuration](#configuration). |
| **Per-iteration input** | The **latest** PR state — the engineer re-fetches the current diff each iteration (`gh pr diff <PR>`); it never re-reviews a stale diff. |
| **Fix discipline** | Every **actionable / blocking** finding is fixed in code and pushed to the **same PR branch** before the next review. |
| **Termination** | Crusty reports **no blocking/actionable findings** (satisfied), OR the iteration cap is reached. See [Bounded loop & blocker semantics](#bounded-loop-blocker-semantics). |
| **Satisfied signal** | A **structural sentinel** verdict (the review's first output line is exactly `NO BLOCKING FINDINGS`) gates "satisfied" — not free-text — so a chatty review cannot accidentally pass the gate. |

Each iteration is, in order: **re-fetch latest diff → crusty review on the
high-end model → if blocking findings remain, fix + push → loop**. The loop
operates on the freshly-pushed state every time (no TOCTOU on a stale diff).

### Trivial-PR filtering (cost awareness)

The full loop is expensive (a high-end reasoning model, multiple passes), so it
runs **only on non-trivial PRs**. A **trivial** PR — docs/comments-only, or a
small change (roughly **< 3 files / < ~30 changed lines**) — gets a **single
lightweight pass**, not the loop. This mirrors pr-guide's own trivial filter and
keeps the high-end loop's spend within the daemon's pre-existing daily budget
(`SIMARD_DAILY_BUDGET_USD`, default `500`, tracked by the OODA loop — **not**
read or enforced by this pipeline; see [Configuration](#configuration)).

## Stage 2 — pr-guide (illustrated walkthrough)

The engineer runs the **pr-guide** skill to generate or update the PR's
illustrated guide — an end-of-workflow walkthrough of the change.

> **Graceful degradation (the only sanctioned skip).** `pr-guide` ships in the
> amplihack-rs amplifier bundle and is available to engineers working in repos
> that ship that bundle. In a repo where the `pr-guide` skill is **not
> available**, the engineer **logs a note** ("pr-guide unavailable in
> `<owner/repo>`, skipping illustrated guide") and **continues** the pipeline.
> It does **not** hard-fail. This is the *only* place in the pipeline where a
> missing step is tolerated; crusty and merge-ready failures always surface as
> blockers.

pr-guide applies its own trivial/small-PR filter, so a one-line or doc-only PR
may legitimately produce no guide.

## Stage 3 — final review (one lightweight pass)

After the guide is generated, the engineer reviews the PR **once more** — a
single, lightweight correctness/consistency pass to catch anything the guide
generation or the last fix introduced. This is **one pass, no loop**, on the
engineer's default model (it may be a single crusty pass or the existing
`review_pipeline`). It is a final sanity check, not a second review→fix loop.

## Stage 4 — merge-ready → merge → close issue

Only after stages 1–3 does the engineer run the final **merge-ready** gate and
land the PR through the **gated merge authority** described in the
[Merge-Ready Contract](../howto/edit-the-engineer-system-prompt.md) and #2410:

> **Layering note.** The six merge-ready *criteria and evidence headings* are
> **not** new work introduced at stage 4 — they are gathered when the PR is first
> opened (the engineer's Definition-of-Done step 3, *"PR opened with evidence
> headings"*, in `engineer_system.md`). What stage 4 adds is the final
> **gate + merge**: re-verify that the evidence headings are present and CI is
> green, then merge. Stages 1–3 run *between* "PR opened with evidence" and this
> gate; the gate is the step they precede, not a re-collection of evidence.

- `rysweet/Simard` PR → `simard merge-pr <PR>` (re-checks the objective gates and
  the merge-readiness judge before invoking `gh pr merge --squash --delete-branch`).
- Cross-repo PR in a governed repo → `simard merge-pr <PR> --repo <owner/repo>`,
  which routes the cross-repo merge through the **same** gated objective-gates +
  merge-judge authority (see the
  [cross-repo merge authority reference](./cross-repo-merge-authority.md)). A bare
  `gh pr merge --squash --delete-branch <PR> --repo <owner/repo>` is the fallback
  only where `simard merge-pr` is genuinely unavailable.

> **Autonomy note.** For a governed repo with **no required human reviewer**,
> Simard does not wait on an external approver — the `merge-ready` skill's
> **required-reviews/approvals criterion** is satisfied once the objective gates
> and the merge-judge verdict pass, because no *required* approval is
> outstanding. This does **not** change the six evidence headings above and
> renumbers nothing; it only resolves the skill's separate reviews/approvals
> criterion for a repo that has no required reviewer. A *genuinely required*
> human review (a branch-protection-mandated approval) is still a real blocker.
> See the
> [operational autonomy model](../concepts/operational-autonomy-model.md).

Then **close the linked issue** (`Closes #<N>` auto-close for same-repo; explicit
`gh issue close <N> --repo <owner/repo>` for cross-repo). A fix/implement goal is
**done only when its PR is merged and the issue is closed** — unchanged from
#2410.

## Configuration

This change introduces **two net-new** knobs (`SIMARD_REVIEW_MODEL`,
`SIMARD_REVIEW_MAX_ITERS`) that **do not exist today** — they are read by the
new PR-finalization section of the engineer prompt; **no Rust threading is
required** — the engineer subprocess inherits the daemon's environment.
`SIMARD_DAILY_BUDGET_USD` is a **pre-existing** daemon knob (already read by the
OODA loop) shown here for context only; this pipeline does not read it.

| Variable | New? | Default | Range / Allowlist | Purpose |
|----------|------|---------|-------------------|---------|
| `SIMARD_REVIEW_MODEL` | **net-new** | `gpt-5.5` | Validated against an allowlist (`gpt-5.5`, `claude-opus-4.8`) before being passed to `copilot --model … --reasoning-effort high --context long_context`; an unrecognized value falls back to the default. | The high-end reasoning model the crusty review loop runs on. Defined and read by the new engineer-prompt section. |
| `SIMARD_REVIEW_MAX_ITERS` | **net-new** | `3` | Integer, bounded to `[1, 5]`. | Hard cap on crusty review→fix iterations before the loop must terminate. Defined and read by the new engineer-prompt section. |
| `SIMARD_DAILY_BUDGET_USD` | pre-existing | `500` | — | **Pre-existing** Simard knob, read by the OODA-loop budget tracker (`src/ooda_loop/types.rs`, default `500.0`) — **not** introduced or consumed by this pipeline. It is the daemon-wide spend ceiling, not a per-pipeline gate; the trivial-PR filter and the iteration cap are what actually bound *this* loop's spend. |

> **Model verification.** `gpt-5.5` is the verified high-end default and
> `claude-opus-4.8` is the verified premium alternative — both are accepted by
> `copilot --model <m> --reasoning-effort high --context long_context` on the
> enterprise Copilot endpoint. If you change `SIMARD_REVIEW_MODEL`, confirm the new
> string is accepted by the CLI (`copilot --model <X> --reasoning-effort high
> --context long_context -p "reply OK"`) before deploying; an unaccepted value
> falls back to the default rather than failing the pipeline.

### Examples

Run the default pipeline (high-end crusty review on `gpt-5.5` at high reasoning
effort + 1M context, cap 3) — no configuration needed; this is the shipped default.

Pin the premium model and widen the cap to 5 for a session:

```bash
export SIMARD_REVIEW_MODEL=claude-opus-4.8
export SIMARD_REVIEW_MAX_ITERS=5
```

Confirm a candidate model string before adopting it:

```bash
copilot --model gpt-5.5 --reasoning-effort high --context long_context -p "Reply with exactly: OK" --allow-all-tools
# expect: OK
```

## Bounded loop & blocker semantics

The crusty loop **must terminate**. It ends when either:

1. **Crusty is satisfied** — the sentinel verdict `NO BLOCKING FINDINGS` is
   emitted on the latest PR state. The engineer proceeds to stage 2.
2. **The cap is reached** (`SIMARD_REVIEW_MAX_ITERS`, default 3) **with findings
   still open.** The engineer then, in order:
   - posts the **remaining findings as a PR comment** (so they are visible on the
     PR), and
   - surfaces a **goal blocker** in `cycle_summary.engineer_summary`
     (e.g. "PR #819 blocked: crusty review not satisfied after 3 iterations —
     remaining findings posted on the PR"),
   - and **does NOT merge.** Silent merge-past-unsatisfied-findings is forbidden.

This honesty rule mirrors the existing #2410 blocker contract: an un-satisfiable
review is an **external/quality blocker to surface**, never a reason to silently
land the PR.

## Output-contract preservation

The pipeline is **additive prose only**. It preserves every output contract the
Rust parsers depend on:

| Surface | Contract | Effect of this pipeline |
|---------|----------|-------------------------|
| Engineer system prompt | Free prose (no parser) | New ordered section added; no parsed shape. |
| `goal_session_objective.md` | **Prose-only** goal-action output | A short cross-reference note added; still prose-only, parsed shape unchanged. |
| `ooda_decide` | `DECISION:` keyword | Untouched. |
| Progress-assessment reviewer | JSON | Untouched. |

Prompt-content tests in `src/ooda_brain/prompt_store_tests.rs` assert the new
pipeline anchors are baked into the embedded prompts (content assertions only —
no parser or logic changes).

## Security model

The review→merge automation is guarded by a small set of controls. The
**genuinely enforced** controls are GitHub branch protection and the
repo-scoped token; the prompt-level guardrails are defense-in-depth.

> **Operational prerequisite — required.** On **any repo where this pipeline can
> merge**, server-side **GitHub branch protection** (required status checks +
> required reviews) and a **least-privilege, repo-scoped token** MUST be
> configured. These are the only controls that are actually enforced; the
> prompt guardrails below assume they are in place.

- **Command injection** — every shell expansion is quoted (`"$VAR"`); the review
  prompt is built via heredoc; PR title/body/diff are passed as **data**, never
  interpolated into the command string.
- **Model allowlist** — `SIMARD_REVIEW_MODEL` is validated against an allowlist
  before reaching `--model`; `SIMARD_REVIEW_MAX_ITERS` is integer-bounds-checked
  to `[1, 5]`.
- **Prompt-injection / TOCTOU** — merge is gated on the **structural sentinel**
  verdict, not free text; the diff is **re-fetched** each loop iteration and
  again before merge.
- **Authorization** — self-PR-only (the PR the engineer created/owns); `--admin`
  / `-f` and protected-branch bypass are forbidden; never merge with required
  checks failing.
- **No leakage** — PR comments contain only findings text; env vars, tokens, and
  full prompts are never echoed; the subprocess command is scrubbed of env before
  any logging.
- **No silent degradation** — only `pr-guide` unavailability may be skipped (and
  it is logged). Crusty and merge failures surface as blockers and never
  silent-pass to merge.

## Deployment

This is a **prompt-only change** to `prompt_assets/simard/*.md`, so it
**hot-reloads** from `~/.simard/prompt_assets/simard/` — **no binary rebuild or
daemon redeploy** is required for the runtime behavior. (The accompanying
prompt-content assertions in `src/ooda_brain/prompt_store_tests.rs` are compiled
into the test binary via `include_str!`, so they re-bake on `cargo test`; that is
a test-only build, not a runtime redeploy.)

Contrast [concurrent engineer dispatch](./concurrent-engineer-dispatch.md), which
*is* a Rust core change requiring a rebuild + redeploy. For this pipeline, the
operator deploys by syncing the prompt assets; the live daemon picks them up on
the next engineer cycle.

## Invariants

- **Ordered.** For every non-trivial PR: crusty loop → pr-guide → final review →
  merge-ready → merge → close — in that order, with merge gated on the prior
  stages.
- **Bounded.** The crusty loop terminates within `SIMARD_REVIEW_MAX_ITERS`
  iterations; cap-with-findings posts a PR comment + goal blocker and does **not**
  merge.
- **Latest-state.** Each loop iteration reviews the freshly-pushed PR state, never
  a stale diff.
- **High-end.** The crusty pass runs on the pinned `SIMARD_REVIEW_MODEL`
  (default `gpt-5.5`, at `--reasoning-effort high --context long_context`),
  independent of the engineer's default model.
- **Cost-aware.** Trivial PRs get a single pass; the trivial filter and the
  iteration cap keep the high-end model's spend bounded, well inside the
  daemon-wide `SIMARD_DAILY_BUDGET_USD` (which this pipeline does not itself
  read).
- **Per-engineer.** The loop runs inside one engineer's finalization; the OODA
  brain only dispatches/checks — #2404 loop-awareness and #2405 fan-out are
  preserved.
- **Graceful-skip scope.** Only `pr-guide` unavailability is skippable; all other
  failures surface explicitly.

## Related reading

- [How to edit the engineer system prompt](../howto/edit-the-engineer-system-prompt.md)
  — where the PR-finalization section and the Merge-Ready Contract live.
- [Concurrent engineer dispatch](./concurrent-engineer-dispatch.md) — how multiple
  engineers (each finalizing their own PR) start concurrently in one OODA round.
- [Maximum safe parallelism](./maximum-safe-parallelism.md) — the AIMD cap that
  bounds how many engineers (and therefore finalization loops) run at once.
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
  — the dispatch path that hands a goal to the engineer that then runs this
  pipeline.
- [Concept: prompt-driven TDD discipline](../concepts/prompt-driven-tdd-discipline.md)
  — the same prompt-first philosophy this pipeline follows (quality enforced in
  the prompt, not in Rust).
