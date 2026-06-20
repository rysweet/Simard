# TDD Adoption Spec for Simard

## Status

- **Created**: 2026-06-14
- **State**: RATIFIED — PM-architect approved 2026-06-20
- **Consolidates goal slugs**: `adopt-tdd`, `adopt-tdd-for-new-modules`
- **Tracking issue**: #2291
- **Companion concept doc**: [`docs/concepts/prompt-driven-tdd-discipline.md`](../docs/concepts/prompt-driven-tdd-discipline.md)

### Ratified decisions (PM-architect, 2026-06-20)

The three policy items this spec opened for owner sign-off are hereby
ratified. They are consistent with the documented owner stance (no bash CI
gate per the #2150/#2151 closures, no coverage threshold per #1937, no
retroactive backfill):

1. **Pilot modules**: `src/engineer_loop/` and `src/goal_curation/` (§2.2).
   Both sit at the top of the measured 60-day churn distribution and are
   central to the loop that consumes the TDD instruction — a deliberate
   dogfood property.
2. **Four-layer enforcement model** (§3): prompt instruction → PR-description
   attestation → periodic audit → reviewer culture. No bash CI git-log
   parser, consistent with the #2150/#2151 rejections.
3. **Attestation syntax** (§2 / §3 Layer 2): the `tdd:` / `tdd-exempt:`
   PR-description rows are the ratified self-attestation format.

This ratification clears Phase 0 (§4). Phases 1–3 remain separate follow-up
cycles, each scoped to a single issue.

This file is the **canonical written charter** for Test-Driven Development
discipline in the Simard repository. It exists so that any future operator,
engineer agent, or meeting participant who asks "what is Simard's TDD policy?"
gets a single answer with explicit scope, exit criteria, and a record of
what was already tried and rejected — so the goal stops resurfacing without
a written deliverable.

## Why this spec exists now

The `adopt-tdd` goal has cycled through three full attempts:

1. **#1927** (May 2026, CLOSED) — original charter proposing a meeting-subsystem
   pilot plus a bash CI gate.
2. **PRs #2150 / #2151** (May 2026, CLOSED) — implementations of the bash CI
   gate (`scripts/check-tdd-ordering.sh`) explicitly rejected by the owner.
3. **PR #2276** (June 11, 2026, MERGED) — pivot to prompt-driven enforcement:
   strengthened the engineer system prompt with explicit `test:` → `feat:`
   commit-ordering instructions and added TDD as merge-ready criterion #7.

The audit cycle in #2160 measured **0% engineer-PR compliance** with the
prompt-driven instruction before PR #2276 landed. Goal-board tombstoning
in PR #2177 removed the slugs from the active goal board, but the slugs
keep re-appearing via meeting handoff input (the dispatch system that
generated *this* very PR cycle is an example). Each resurfacing produces
another planning cycle, but no durable artifact has consolidated the
decisions to date.

This spec is that durable artifact. Future cycles that try to "adopt TDD"
should be auto-resolved by pointing at this file and the exit criteria
below.

## 1. Policy

**Red → Green → Refactor is the default authoring order for new behavior**
in code under `src/` of this crate.

Concretely, a PR that introduces or changes externally-observable behavior
should land a failing test *before* the production change in the PR's
commit history (`git log --reverse origin/main..HEAD`). The failing-test
commit and the implementation commit are separate commits; the failing-then-
passing transition is the artifact that matters.

This rule is already encoded as merge-ready criterion #7 in the engineer
system prompt (`prompt_assets/simard/engineer_system.md`). This spec is the
policy text that prompt enforces.

### 1.1 Explicit exception list (no test-first ordering required)

- **One-line fixes** — typo, constant tweak, log-message text, obvious off-
  by-one with no regression surface area worth a separate test.
- **Doc-only PRs** — `docs/**`, `Specs/**`, `*.md`, code comments, rustdoc.
- **Generated code** — anything under a `// @generated` marker, build-script
  output, vendored snapshots.
- **Pure refactors with zero behavior change** — already covered by existing
  tests; reviewer judgment call, must be stated in PR description.
- **Prompt-asset edits** — `prompt_assets/**` text changes. Tests cannot
  meaningfully precede a prompt edit; the validation surface is the next
  engineer cycle's behavior.

Anything outside the exception list and inside the in-scope path set (see §2)
ships test-first or carries an explicit `tdd-exempt:<reason>` note in the
PR description naming which exception applies.

## 2. Scope

### 2.1 In scope — new modules (test-first required)

- **All new Rust source files** under `src/` introduced by any PR.
- **All new public functions, structs, traits, or behaviors** added to
  existing modules under `src/`, when the addition is externally observable
  (changes a CLI surface, a public API, persisted state, network protocol,
  or operator-visible behavior).

### 2.2 In scope — pilot modules (test-first required for *changes*, not just additions)

Two pilot modules are selected based on **measured 60-day churn** on
`origin/main` (top of the active-churn distribution, both central to the
engineer loop that consumes TDD instructions, giving the policy a
"dogfood" property):

| Pilot module | 60-day file-touches | Rationale |
|---|---:|---|
| `src/engineer_loop/` | 45 | The loop that *executes* every engineer cycle. Bugs here invalidate every downstream PR. |
| `src/goal_curation/` | 45 | Owns the goal lifecycle that surfaces *this very goal*. Recent regressions (#2177 tombstone fix) are the kind of thing a test-first reflex would have caught. |

For the pilot modules, the rule applies to *changes* (not just additions):
any PR touching a `.rs` file under either pilot path must satisfy the policy
or carry the `tdd-exempt:<reason>` note.

The two pilots are deliberately small. The original #1927 charter chose
`src/meeting_*` for similar reasons; that pilot did not complete because the
enforcement substrate (bash CI gate) was rejected before any pilot PR shipped.
This spec keeps the "one to two small pilots" principle but picks modules
that are *currently* high-churn rather than historically.

### 2.3 Out of scope — legacy code (test-when-touched)

- **All existing modules NOT named as pilots above.** For these, test-first
  is *encouraged* but not required. The rule kicks in only when a PR
  introduces a new public surface (per §2.1).
- **No retroactive refactor.** This spec does not authorize spending a cycle
  to backfill tests into modules that work fine. Coverage-driven test
  backfill is a separate workstream tracked under #1937.

### 2.4 Out of scope — explicitly NOT in this spec

- ❌ **A bash CI gate that parses `git log` for test-commit ordering.**
  This was tried in PRs #2150 and #2151 and rejected by the owner. See
  `docs/concepts/prompt-driven-tdd-discipline.md` §"Why Prompt-Based, Not
  CI-Based" for the full rationale (brittleness, squash-commit blindness,
  scope creep, wrong enforcement layer). This spec does not re-propose it
  in any form.
- ❌ **A per-file or per-crate coverage threshold.** TDD is about authoring
  order, not coverage percentages. The coverage measurement and target-
  setting work lives in #1937 and is intentionally a separate epic.
- ❌ **Rewriting historical commits or amending merged PRs.** The policy
  applies only to PRs opened *after* this spec is merged.

## 3. Enforcement layers

Enforcement is layered to compensate for the audit-measured 0% compliance
of any single layer. No layer is a CI bash script.

### Layer 1 — Prompt instruction (already shipped, PR #2276)

The engineer system prompt at
`prompt_assets/simard/engineer_system.md` lines 172, 193 already contains:

- A merge-ready contract criterion (#7): *"TDD commit ordering verified: run
  `git log --oneline` on your branch and confirm that every `test:` commit
  appears before its corresponding `feat:` commit."*
- A PR-evidence heading requirement: *"`git log --oneline` excerpt showing
  `test:` commits before corresponding `feat:` commits (or justification if
  no feature code was added)."*

This is the primary enforcement surface. No additional prompt edits are
proposed in this spec.

### Layer 2 — PR description self-attestation (template-driven)

This spec proposes adding a `tdd:` attestation row to the engineer-PR
evidence block already required by the prompt's merge-ready contract.
The attestation is one of:

- `tdd: test-first ordering verified — <link to commit>` (default for in-
  scope PRs)
- `tdd-exempt: <reason from §1.1>` (for exception cases)
- `tdd: not applicable — PR touches no in-scope paths` (for the bulk of
  ops/docs/prompt PRs)

The attestation is a PR-description field, not a checkbox enforced by a
bot. Reviewers verify it visually during PR review (which they already do
for the six other merge-ready evidence headings).

### Layer 3 — Periodic audit cadence (re-run of the #2160 model)

A lightweight quarterly audit cycle, modeled on the #2160 audit. The audit:

1. Lists the last N engineer-authored PRs (`gh pr list --author <engineer-
   bot> --limit N --search "is:pr is:merged"`).
2. For each, checks whether the PR description contains a `tdd:`/`tdd-
   exempt:` attestation row.
3. For pilot-module PRs, additionally inspects `git log --reverse
   <base>..<head>` for `test:` → `feat:` ordering.
4. Produces a compliance percentage and files a follow-up issue if
   compliance is below 80%.

The audit is a manual or operator-dispatched cycle, not a recurring CI job.
Cadence: quarterly, or after any 10 merged engineer PRs touching pilot
paths — whichever comes first.

### Layer 4 — Reviewer culture (no automated layer)

Code reviewers (human or `code-review` agent) are expected to flag
in-scope PRs that lack a `tdd:`/`tdd-exempt:` attestation. This is the
fail-safe layer when the prompt instruction is ignored and the audit
cadence has not yet caught up.

## 4. Phasing

| Phase | Deliverable | Owner | Exit signal |
|---:|---|---|---|
| 0 | This spec merged (no behavior change) | Engineer cycle that generated this PR | This PR merges to `main` |
| 1 | Engineer-PR template updated to include `tdd:` attestation row | Follow-up PR | Template merged; next engineer PR includes the row |
| 2 | First quarterly audit run | Audit cycle (manual or dispatched) | Audit issue filed reporting compliance % across last 10 engineer PRs |
| 3 | If Phase 2 reports <80% compliance, file a single prompt-tuning issue (modeled on #2166) | Operator | Prompt-tuning PR merged; mark this spec's tracking issue DONE |

Phase 0 ships in this PR. Phases 1–3 are separate cycles, each scoped to a
single follow-up issue.

## 5. Exit criteria for the goal

The `adopt-tdd` (and synonym `adopt-tdd-for-new-modules`) goal can be marked
**DONE** when *all* of the following hold:

- [ ] This spec (`Specs/TDD_ADOPTION.md`) has merged to `main`.
- [ ] The tracking issue filed alongside this PR is CLOSED.
- [ ] Both goal slugs (`adopt-tdd`, `adopt-tdd-for-new-modules`) are
      tombstoned via `simard goal remove` (which already invokes the
      tombstone fix from PR #2177).
- [ ] A reference to this spec is added to `prompt_assets/simard/
      meeting_system.md` near the example line `simard goal set-priority
      adopt-tdd 1` so the meeting facilitator stops re-creating the goal
      from boilerplate. (Out of scope for *this* PR; tracked as a Phase-1
      deliverable.)

When a future meeting handoff re-surfaces the goal, the operator should
respond by linking this file and re-tombstoning the slug. No further
planning cycles are needed unless this spec itself is reopened.

## 6. Prior art / cross-references

| Ref | Status | Relevance |
|---|---|---|
| #1927 | CLOSED 2026-05-28 | Original adopt-tdd charter; identified pilot pattern; rejected CI-bash enforcement |
| #2150 | CLOSED | Bash CI gate implementation #1 — rejected |
| #2151 | CLOSED | Bash CI gate implementation #2 — rejected |
| #2160 | CLOSED | Audit that measured 0% compliance with the prompt-driven instruction |
| #2161 | MERGED | Linked the prompt-driven TDD concept doc from the main docs index |
| #2166 | CLOSED | Prompt-tuning issue that motivated PR #2276 |
| #2177 | MERGED | Goal tombstoning fix — stops `adopt-tdd` from re-appearing after `simard goal remove` |
| #2276 | MERGED 2026-06-11 | Strengthened engineer prompt with explicit `test:` → `feat:` ordering + merge-ready criterion #7 |
| #2288 | MERGED 2026-06-13 | Made merge-ready criteria practical with fallback evidence paths |
| #1937 | OPEN | Coverage-baseline epic (orthogonal: measures coverage, does not enforce authoring order) |
| `docs/concepts/prompt-driven-tdd-discipline.md` | merged | Concept doc explaining *why* prompt-driven, not CI-driven |
