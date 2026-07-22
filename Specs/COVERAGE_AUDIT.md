# Coverage-Audit Charter for Simard

## Status

- **Created**: 2026-07-16
- **State**: RATIFIED — adopted as the goal's machine-checkable done-gate via
  escalation triage (`rewrite-done-gate`). The disambiguation (§1), the
  measurable done-criteria (§2), and the deterministic next-target procedure
  (§3) are in force and actionable immediately; they do not change any code or
  CI behavior.
- **Consolidates goal slugs**: `audit-simard-test-coverage`,
  `raise-coverage-to-70`, `improve-amplihack-test-coverage`,
  `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` (the recurring
  blocked goal this charter's `rewrite-done-gate` triage re-points here).
- **Companion ledger**: [`docs/testing/COVERAGE_BASELINE.md`](../docs/testing/COVERAGE_BASELINE.md)
- **Related epics**: [#1735](https://github.com/rysweet/Simard/issues/1735),
  [#1937](https://github.com/rysweet/Simard/issues/1937) (see the
  disambiguation in §1 — those epics describe a **different repository**).

This file is the **canonical written charter** for the recurring
"audit Simard's test coverage and raise it to 70%" goal. It exists so that
any future operator, engineer agent, or OODA cycle that picks up the goal
gets a single answer to three questions that were previously unanswered:

1. **What does the goal mean, precisely, and in which repository?** (§1)
2. **When is it DONE, measured how?** (§2)
3. **What is the concrete next step if it is not yet done?** (§3)

## Why this charter exists now

The goal has cycled repeatedly and, most recently, an OODA cycle diagnosed
itself as **`GENUINELY-STUCK` with `evidence=[(none)]`** — it could not
produce a shippable increment because it could not answer any of the three
questions above. Investigation found three root causes:

1. **Wrong-repository conflation.** The two epics the goal is nominally
   parented under — [#1735](https://github.com/rysweet/Simard/issues/1735)
   and [#1937](https://github.com/rysweet/Simard/issues/1937) — explicitly
   describe the **`rysweet/amplihack-rs`** Rust workspace, *not* this
   `Simard` repository. #1937's own body states: *"The amplihack Rust
   workspace lives in `rysweet/amplihack-rs` (not in this `Simard` repo,
   which is a single crate)."* Its baseline table (116,923 instrumented
   lines across 27 `amplihack-*` crates at 79.98% line coverage) is an
   amplihack-rs measurement. A cycle that reads the parent epic to find its
   target is pointed at a workspace that does not exist in this checkout.

2. **All named Simard targets are already done.** The Simard-scoped
   per-group targets that *were* filed against this repo are all CLOSED:
   `bin` (#1749), `operator_commands_dashboard` (#1750),
   `trace_collector` (#1751), `operator_commands_gym` (#1752), and
   `cmd_cleanup` (#1753). Their post-lift aggregates are recorded in the
   companion ledger and all clear the ≥ 70% target. A cycle that reads the
   ledger finds no open per-group target to attack.

3. **No deterministic next-target rule.** Sibling audit cycles have
   continued to raise coverage on individual modules opportunistically
   (`status` 29% → 91% via #2701, `overseer::diagnosis` 36% → 100% via
   #2844, `git_guardrails` 70.5% → 91.4% via #2729, `completion-gate`
   66.9% → 82.1% via #2958), but nothing defined *which* file to attack
   next or *when the audit as a whole is complete*. Without that rule, a
   cycle with no obvious low-coverage file in front of it has no evidence
   to act on and correctly reports `GENUINELY-STUCK`.

This charter fixes all three: it fixes the repository scope, records the
current ledger state, defines measurable done-criteria, and gives a
deterministic procedure that always yields a concrete next step (or a
defensible "already done" verdict).

## 1. Scope and repository disambiguation

- **In scope:** line coverage of the single `simard` crate and its sibling
  `simard-*` binary crates in **this** repository (`rysweet/Simard`), as
  measured by `cargo llvm-cov`.
- **Out of scope — different repository:** the `amplihack-rs` workspace
  coverage epic. Epics #1735 and #1937 are filed in `rysweet/Simard` only
  for coordination co-location; their *content* targets `rysweet/amplihack-rs`.
  A Simard coverage cycle **must not** try to satisfy #1937's per-crate
  `amplihack-*` targets from this checkout — those crates are not here.
  Work on amplihack-rs coverage belongs to a cycle scoped to that repo.
- **Out of scope — a workspace-wide hard coverage gate.** See §4.

> If the goal text says "raise it to 70%" with no further qualification,
> read it as: *raise each attacked Simard module to ≥ 70% aggregate line
> coverage, one bounded increment per PR, tracked in the companion ledger* —
> **not** as a single workspace-wide percentage enforced by CI.

## 2. Measurable done-criteria

The unit of measurement is **aggregate line coverage of a target group**
(a single file or a `src/<module>/` directory), produced by:

```bash
cargo llvm-cov --no-fail-fast --summary-only
# or, scoped to one library module:
cargo llvm-cov --lib --summary-only -- <module_path_fragment>
```

A target group **clears** the bar when its aggregate line coverage is
**≥ 70%**. Individual files inside a group may sit below 70% when the
uncovered paths are client-/runtime-dependent (require a live cognitive
memory or state-root), provided (a) the *group* aggregate clears 70% and
(b) the exception is recorded with a one-line justification in the ledger.
This mirrors the already-accepted `simard_ooda_step.rs` exception in the
companion ledger (group aggregate 76.07% with one 60.36% file).

The **audit goal as a whole is DONE** when *all* of the following hold:

- [ ] Every group listed in the companion ledger's tables shows a landed
      post-lift aggregate ≥ 70% (or a recorded, justified exception).
- [ ] The "Other groups" backlog table in the ledger is empty (every
      remaining tracked group has either landed or been explicitly deferred
      with justification).
- [ ] The deterministic scan in §3 finds no un-ledgered `src/` file that is
      both **high-risk** (per the §3 risk list) and below 70% with more than
      50 executable lines.

When those hold, the goal is marked DONE and its slugs are tombstoned via
`simard goal remove`; a future resurfacing is resolved by linking this
charter and re-tombstoning, not by opening another planning cycle.

## 3. Deterministic next-target procedure

Any cycle that picks up this goal runs this procedure and always ends with
either a concrete target file **or** a defensible DONE verdict — never
`GENUINELY-STUCK`:

1. **Measure.** Run `cargo llvm-cov --no-fail-fast --summary-only` (or the
   scoped `--lib` form for speed) and capture the per-file line-% table.
   The raw table *is* the evidence a cycle was previously missing.
2. **Filter.** Keep only files under `src/` with **> 50 executable lines**
   and **< 70% line coverage** that are not already recorded as a justified
   exception in the ledger.
3. **Rank by risk, then by gap.** Prioritise files on the safety/critical
   path first — the loop and safety surfaces named in #1735
   (`engineer_loop`, `ooda`/brain, merge/overseer authority,
   cognitive-memory, safe-update, `git_guardrails`) — then by absolute
   coverage gap (lowest line-% first).
4. **Pick the top candidate** and open a single bounded PR that raises that
   one group to ≥ 70% with **hermetic** tests (no network, no sleeps, no
   live runtime; use `InMemory*` stores and a `TempDir` `SIMARD_STATE_ROOT`
   per `docs/testing/hermetic-tests.md`).
5. **Record.** Add or update the group's row in the companion ledger with
   the before/after aggregate, the reproduce command, and any justified
   sub-70% file exception.
6. **If step 2 yields an empty set**, the audit is DONE per §2 — record that
   verdict (with the measured table as evidence) and tombstone the slug.

One PR attacks **one** group. Bounded increments keep each PR reviewable and
keep the merge-ready bar achievable, exactly as the landed per-group PRs did.

## 4. Explicitly NOT in this charter

- ❌ **A workspace-wide hard coverage threshold enforced in CI.** Consistent
  with the owner's rejection of bash CI gates in PRs #2150 / #2151 and with
  `Specs/TDD_ADOPTION.md` §2.4, this charter does not propose a CI job that
  fails a PR for dropping below a global percentage. The existing
  `.github/workflows/coverage.yml` is a **reporting** job (it posts a
  per-module table comment); it is not, and under this charter does not
  become, a blocking gate.
- ❌ **Retroactive backfill of modules that work fine and are already
  ≥ 70%.** The procedure only attacks files below the bar.
- ❌ **amplihack-rs coverage work.** Different repository (§1).

## 5. Prior art / cross-references

| Ref | Status | Relevance |
|---|---|---|
| #1735 | OPEN | Baseline epic — **describes `amplihack-rs`**, co-located here |
| #1937 | OPEN | Per-crate coverage epic — **describes `amplihack-rs`**, co-located here |
| #1749 | CLOSED | Simard `bin` group 1% → 76% (PR #1772) |
| #1750 | CLOSED | Simard `operator_commands_dashboard` 31% → 70% (PR #2257) |
| #1751 | CLOSED | Simard `trace_collector` 43% → 95% (PR #2338) |
| #1752 | CLOSED | Simard `operator_commands_gym` 43% → 89% (PR #2346) |
| #1753 | CLOSED | Simard `cmd_cleanup` 44% → 70% (PR #2353) |
| #2701 | MERGED | Ad-hoc Simard lift: `status` 29% → 91% |
| #2844 | MERGED | Ad-hoc Simard lift: `overseer::diagnosis` 36% → 100% |
| #2729 | MERGED | Ad-hoc Simard lift: `git_guardrails` 70.5% → 91.4% |
| #2958 | MERGED | Ad-hoc Simard lift: `completion-gate` 66.9% → 82.1% |
| #2150 / #2151 | CLOSED | Bash CI gate — rejected by owner (informs §4) |
| `docs/testing/COVERAGE_BASELINE.md` | landed | The companion per-group coverage ledger |
| `Specs/TDD_ADOPTION.md` | RATIFIED | Sibling charter for the recurring `adopt-tdd` goal; same "durable artifact stops the resurfacing" pattern |
