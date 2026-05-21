---
title: "Improvement context: denser execution evidence for the engineer loop"
description: Captured improvement-curation context — preserves the active "Capture denser execution evidence" goal and the architecture observation that the legacy `simard_operator_probe` surface does not yet expose a terminal engineer loop, so a future improvement-curation cycle can act on it without re-deriving the framing.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../reference/operator-read-state-root-contract.md
  - ../howto/inspect-improvement-curation-state.md
  - ../tutorials/run-your-first-local-session.md
---

# Improvement context: denser execution evidence for the engineer loop

This page is an **explicit improvement-context capture**. It is not a shipped
feature, not a contract, and not a runtime surface. It exists so an active
improvement priority and its architectural framing live as a versioned,
inspectable doc in the repo — the same operator-visible discipline the rest
of Simard already applies to state, prompts, and runtime contracts.

> **Status:** captured improvement context only.
> No code under `src/` or `tests/` changes because of this page.
> The next `improvement-curation run` / `improvement-curation read` cycle
> (see [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md))
> is where this priority is meant to be promoted to a tracked active goal
> with full denser execution evidence.

## Why this is preserved as a doc

`Specs/ProductArchitecture.md` is explicit about how improvement context
should travel between sessions and modes:

- Prompt files and other shaping context should live as explicit assets
  so they can be inspected, versioned, composed, replaced, and benchmarked
  independently of runtime logic
  (`Specs/ProductArchitecture.md`, the prompt-assets discipline section).
- Persisted state used to bridge bounded terminal sessions into engineer
  mode must be operator-visible local artifacts under the same explicit
  `state-root`, with mode-scoped handoff records and readback that shows
  what was reused
  (`Specs/ProductArchitecture.md` § *Interactive Terminal-Driven Engineer
  Behavior* and adjacent paragraphs).

A captured improvement context is shaping context. Storing it only inside
an operator's local `target/operator-probe-state/...` would mean it lives
on one machine, in one ad-hoc state root, with no audit trail and no way
for a future engineer session on a clean checkout to see it. A focused
docs entry is the explicit, versioned, repo-grounded surface.

## Captured active goal

The active improvement-curation goal captured by this session is:

> **`Capture denser execution evidence`**

This is the same goal text the canonical improvement-curation walkthrough
uses as its worked example
(see [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md),
step 3: `approve: Capture denser execution evidence | priority=1 | status=active | …`).

When this priority is promoted into the runtime improvement-curation
pipeline, the canonical invocation is:

```bash
cargo run --quiet -- \
  improvement-curation run local-harness single-process \
  "approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now" \
  "$STATE_ROOT"
```

The expected operator-visible confirmation lines from
`improvement-curation read` are documented at the top of the
[improvement-curation read walkthrough](../howto/inspect-improvement-curation-state.md#4-read-the-durable-improvement-curation-state).

## Captured architecture observation

The shaping observation that motivates this priority — preserved verbatim
so a later improvement-curation pass does not have to re-derive it — is:

1. **Engineer mode requires `inspect → act → verify → persist`.**
   `Specs/ProductArchitecture.md` § *Modes* says engineer mode
   "accepts a concrete task, inspects the local repo, forms a bounded plan
   with explicit verification steps, executes through terminal actions, and
   reports outcomes with evidence." § *Interactive Terminal-Driven Engineer
   Behavior* further requires the engineer to "Inspect before editing",
   "Verify results with commands or artifact inspection when verification
   is possible", and to leave behind durable session records so a future
   developer can explain why Simard took an action by inspecting them.
   The compressed phrasing for that contract is
   **inspect → act → verify → persist**.

2. **Operator probe exists but does not yet expose a terminal engineer
   loop.** The legacy `simard_operator_probe` compatibility binary, and
   the corresponding probe-mode subcommands (`simard meeting`,
   `simard improvement-curation`, `simard review`, `simard goal-curation`,
   `simard bootstrap`), all expose read/run audit surfaces under
   `target/operator-probe-state/<probe>/...`. None of them currently
   surface a probe-shaped readback of an end-to-end terminal engineer
   loop — i.e., the same `inspect → act → verify → persist` cycle, with
   denser execution evidence and an explicit-or-fail read companion,
   rendered through the same operator-probe vocabulary. The terminal
   engineer surfaces (`simard engineer terminal`, `engineer terminal-read`,
   `engineer run`, `engineer read`) exist as first-class commands but are
   not represented as a probe.

3. **Runtime contracts already document the operator/runtime public
   surfaces and prior spec reconciliation.** See:
   - [Runtime contracts reference](../reference/runtime-contracts.md) — the
     executable contracts and state-root guarantees for the shipped run
     and read paths.
   - [Operator read-subcommand state-root contract](../reference/operator-read-state-root-contract.md)
     — the explicit-or-fail `<state-root>` reconciliation for
     `meeting read`, `improvement-curation read`, and `review read`
     under audit issue #1910 / fix issue #1909.

   A future "denser execution evidence" improvement is expected to extend
   these contracts rather than invent a parallel surface.

## Intentional non-goals

This page deliberately does **not**:

- claim that a terminal-engineer-loop probe has been designed, scheduled,
  or shipped;
- propose any new CLI subcommand, struct, trait, or persisted file format;
- modify any existing operator contract, runtime contract, or test;
- treat the captured goal as approved — promotion is the explicit job of
  `improvement-curation run` against a durable `state-root`.

It only preserves the goal text and the framing, so the next
improvement-curation cycle does not have to re-derive them from scratch.

## How a future cycle is expected to act on this

A later session that picks this up is expected to:

1. Read this page to recover the goal text and the framing.
2. Run `improvement-curation run` with the canonical approval string
   shown above against an explicit `state-root`, then verify with
   `improvement-curation read` per
   [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md).
3. If, and only if, that cycle also produces a concrete spec for denser
   execution evidence — for example, a probe-shaped readback of a
   terminal engineer loop — extend
   `Specs/ProductArchitecture.md` and
   [Runtime contracts reference](../reference/runtime-contracts.md) in
   the same change, and remove or rewrite this captured-context page so
   it no longer claims to be the source of truth.

Until then, this page is the durable handoff for that improvement
context.

## Related reading

- [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md)
  — the canonical runtime path for promoting this captured goal into
  active improvement-curation state.
- [Operator read-subcommand state-root contract](../reference/operator-read-state-root-contract.md)
  — the prior spec reconciliation under issue #1909 / audit #1910 that
  this captured context is adjacent to.
- [Runtime contracts reference](../reference/runtime-contracts.md) —
  the executable contracts for the existing operator/runtime public
  surfaces.
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)
  — the broader operator walkthrough that exercises engineer, meeting,
  goal-curation, improvement-curation, and bootstrap modes.
