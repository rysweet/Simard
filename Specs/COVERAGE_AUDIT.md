# Coverage-Audit Charter for Simard

## Status

- **Created**: 2026-07-16
- **Updated**: 2026-07-26 — done-gate simplified to a single deterministic
  boolean (whole-repo line coverage ≥ 70%, via `scripts/coverage-gate.sh`),
  replacing the former per-module audit series. Ratified by the operator in
  the 2026-07-26 alignment meeting ("70% coverage *is* clear enough —
  `cargo llvm-cov`, compare to 70, done").
- **State**: ACTIVE. §1 (scope), §2 (the deterministic done-gate), and §3
  (how to raise the number when short) are in force. This charter changes no
  CI behavior; §4 still holds (no hard CI coverage gate).
- **Consolidates goal slugs**: `audit-simard-test-coverage`,
  `raise-coverage-to-70`, `improve-amplihack-test-coverage`.
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

> **2026-07-26 update.** The done-gate (question 2) is now a single
> deterministic boolean: whole-repo line coverage ≥ 70%, measured by
> `scripts/coverage-gate.sh`. The per-group ledger below is retained as a
> **map for choosing what to test next** (question 3), not as the done-gate.

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
> read it literally: *raise the repository's whole-crate aggregate line
> coverage to ≥ 70%*, measured by `cargo llvm-cov` (§2). This is a single
> deterministic number, not a per-module audit series. The per-group ledger
> in §5's companion file remains a **useful map for choosing what to test
> next**, but it is no longer the done-gate — the whole-repo total is.
>
> This is **not** a workspace-wide *hard coverage gate enforced by CI* (§4);
> it is the completion criterion for the recurring goal, evaluated on demand
> by running one command.

## 2. Measurable done-criteria — one deterministic boolean

The goal is DONE when the repository's **whole-crate aggregate line coverage
is ≥ 70%**. That is the entire criterion. It is measured by one command and
evaluated by a comparison — a boolean, not a judgement call:

```bash
scripts/coverage-gate.sh          # threshold defaults to 70
# which measures the same way CI does (.github/workflows/coverage.yml):
cargo llvm-cov --no-fail-fast --workspace --lib --bins \
  --ignore-filename-regex 'tests?/' --summary-only --json \
  | jq '.data[0].totals.lines.percent'
# → compare the printed total to 70
```

`scripts/coverage-gate.sh` runs the measurement, reads the `TOTAL` line-%,
and:

- prints the measured total and the verdict,
- exits **0** when coverage ≥ 70% (**DONE**),
- exits **1** when coverage < 70% (**NOT DONE**, and prints the exact gap),
- exits **2** only if the measurement itself could not run (could-not-verify).

**When the gate exits 0, the goal is DONE:** mark it complete and tombstone
its slugs via `simard goal remove`. A future resurfacing is resolved by
re-running the gate and re-tombstoning, not by opening another planning cycle.

There is deliberately **no** steward-identity gate, **no** recursion guard,
and **no** manual per-module audit charter standing between the measurement
and the verdict. Those layers exist to protect judgement calls; whether a
number clears 70 is not a judgement call. Running the command answers it.

> **Why this replaced the old per-module audit series.** This goal cycled
> repeatedly and self-diagnosed `GENUINELY-STUCK` not because 70% is
> ambiguous — it is trivially measurable — but because the done-gate had been
> reframed into an open-ended "attack each module to ≥ 70%, one PR at a time"
> series with no whole-goal terminator, then wrapped in scaffolding that
> checked the scaffolding instead of the number. The fix is to gate on the
> number.

## 3. If the gate says NOT DONE: how to make progress

The whole-repo total in §2 is the done-gate. It does **not** tell you *which*
tests to write to move the number, so use the per-group ledger as a map:

1. **Measure per file.** Run `cargo llvm-cov --no-fail-fast --summary-only`
   and capture the per-file line-% table.
2. **Filter.** Keep files under `src/` with **> 50 executable lines** and
   **< 70% line coverage**.
3. **Rank by risk, then by gap.** Prioritise the safety/critical path first —
   the loop and safety surfaces named in #1735 (`engineer_loop`, `ooda`/brain,
   merge/overseer authority, cognitive-memory, safe-update, `git_guardrails`) —
   then by absolute coverage gap (lowest line-% first).
4. **Pick the top candidate** and open a single bounded PR that adds
   **hermetic** tests (no network, no sleeps, no live runtime; use
   `InMemory*` stores and a `TempDir` `SIMARD_STATE_ROOT` per
   `docs/testing/hermetic-tests.md`).
5. **Re-run the gate.** `scripts/coverage-gate.sh`. If it now exits 0, the
   goal is DONE.

This is guidance for *raising* the number when it is short, not a second
done-gate. One PR still attacks one bounded area to stay reviewable.

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
