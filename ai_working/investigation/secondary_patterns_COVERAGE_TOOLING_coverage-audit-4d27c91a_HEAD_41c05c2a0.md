# Secondary (Patterns) — Coverage tooling capability + merged-PR delivery check

HEAD: `41c05c2a0` · Role: SECONDARY / patterns · Recipe: `prompt_assets/simard/overseer/escalation_triage.md`.
Goal: `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` · Typed blocker outcome: `019f6c08`.

**My scope only:** (a) confirm the repo's coverage tooling emits a machine-readable
percentage and give the exact daemon-queryable command a rewritten gate could invoke;
(b) test the `complete-delivered-goal` branch against real merged-PR history. I do **not**
write coverage tests, retune thresholds, or touch the Rust escalation seam.

---

## 1. Coverage tooling IS machine-checkable today (capability = YES)

The daemon can obtain a numeric line-coverage percentage without a human. Two ratified
command forms exist:

- **Canonical charter command** (`Specs/COVERAGE_AUDIT.md` §2, `docs/testing/COVERAGE_BASELINE.md`):
  ```bash
  cargo llvm-cov --no-fail-fast --summary-only            # human table
  cargo llvm-cov --lib --summary-only -- <module_fragment> # scoped, faster
  ```
- **Machine-readable form** (`.github/workflows/coverage.yml:85-89`), the one a gate/daemon
  should invoke:
  ```bash
  cargo +nightly-2026-07-01 llvm-cov --workspace --lib --bins \
    --ignore-filename-regex 'tests?/' --json --summary-only \
    --output-path target/ci-logs/coverage-summary.json
  ```

**JSON shape is confirmed** by `.github/scripts/coverage-comment.mjs:65-94`:
- overall %: `data.data[0].totals.lines.covered / data.data[0].totals.lines.count * 100`
- per-file %: `data.data[0].files[].summary.lines.{covered,count}` (grouped by `src/<module>`)

So a rewritten gate *could* mechanically read a number. **The subtlety (load-bearing):**
the charter (§1, §4) deliberately defines the target as **per-group aggregate ≥70%**, NOT a
single workspace-wide percentage, and explicitly rejects a workspace-wide CI threshold
(§4, echoing owner rejection of PRs #2150/#2151). Therefore a naive
`total.lines.percent >= 70` gate would **contradict the charter**. The correct
machine-checkable finish line is the **acceptance-anchor issue = CLOSED** predicate that
*encodes* §2/§3 (this is the tertiary dive's design and it is right). The `--json` command
above is what the closing engineer/CI runs to fill the anchor's evidence checkbox — not a
raw numeric daemon gate.

## 2. `complete-delivered-goal` branch — REFUTED (substantially delivered, not certifiable-as-whole)

Real merged-PR state (from `Specs/COVERAGE_AUDIT.md` §5 + `COVERAGE_BASELINE.md`, cross-checked
against git log), NOT from the raw markers:

| Landed work | PR | Result | State |
|---|---|---|---|
| `bin` (#1749) | #1772 | 1% → 76% | CLOSED |
| `operator_commands_dashboard` (#1750) | #2257 | 31% → 70% | CLOSED |
| `trace_collector` (#1751) | #2338 | 43% → 95% | CLOSED |
| `operator_commands_gym` (#1752) | #2346 | 43% → 89% | CLOSED |
| `cmd_cleanup` (#1753) | #2353 | 44% → 70% | CLOSED |
| ad-hoc `status` | #2701 | 29% → 91% | MERGED |
| ad-hoc `overseer::diagnosis` | #2844 | 36% → 100% | MERGED |
| ad-hoc `git_guardrails` | #2729 | 70.5% → 91.4% | MERGED |
| ad-hoc `completion-gate` | #2958 | 66.9% → 82.1% | MERGED |

- Every **named** per-group target has landed ≥70%; the ledger "Other groups" **backlog is empty**.
- **BUT no single merged PR asserts the whole-audit §2 three-checkbox DONE verdict**, and no
  closeable anchor encoded it until acceptance-anchor issue **#4616** was created by the prior
  triage run. There is therefore **nothing already-delivered to just mark complete**.

**Conclusion for this branch:** `complete-delivered-goal` is **not** the right decision. The
work is largely delivered, but "delivered" ≠ "certifiable by the daemon." This is the
*Verify-Real-State-Over-Narrative* pattern: measurable state proves most work is done, yet the
goal cannot self-certify because no daemon-observable finish line was ever **bound to it**.
That confirms **rewrite-done-gate** (bind the goal to anchor #4616), matching primary/tertiary.

## 3. The capability gap that blocked round 1 is now closed

Commit **`41c05c2a0`** — `feat(operator-cli): add simard goal wip add|remove|list to bind
done-gate anchors` — shipped the missing CLI. Round 1 could not bind the anchor because there
was **no way to attach a `wip_ref` to a goal**; now:
```bash
simard goal wip <goal-id> add issue 4616 "coverage-audit acceptance anchor" --url <issue-url>
```
uses the anti-clobber `with_board` flock + memory-cache refresh (mirrors `goal label`), so it
is safe against a concurrent OODA daemon cycle. Additive, non-breaking.

## 4. Sharp edge surfaced by real state — the phantom goal (feeds the ONE operator question)

The prior tertiary execution record (§9) and the store layering explain why binding still
can't be done silently: the goal is a **phantom** — present in the cognitive-memory
`goal-board:snapshot` (read by Observe/escalate, so it keeps re-escalating) but **absent from
the authoritative `goal_board.json`** (read by advance-goal via `load_or_migrate`, so
`simard goal wip <id> add …` returns "not found on active board" and there is nothing to
attach a worker/PR/WIP to). The two stores have **diverged** — that is the real mechanic
behind blocker `019f6c08` never clearing, independent of the done-gate measurability issue.

Whether to **re-instate** the coverage goal (bound to #4616) or **retire** it as already
handled is a genuine human scope call — the correct single plain-English operator question.

## 5. Findings summary (secondary)

- **Coverage measurable?** YES — `cargo llvm-cov … --json --summary-only`, percent at
  `data.data[0].totals.lines.{covered,count}`. Non-blocking *reporting* job only (§4).
- **Whole-audit gate should be a raw numeric %?** NO — charter mandates per-group ≥70%;
  bind the goal to the **anchor-issue-CLOSED** predicate (encodes §2/§3) instead.
- **Already delivered (complete-delivered-goal)?** NO — backlog empty and all named groups
  ≥70%, but no merged PR certifies the whole audit and no anchor existed until #4616.
- **Decision supported:** `rewrite-done-gate`. `escalate` = ONE question (re-instate vs
  retire the phantom goal), because the goal has fallen off the authoritative board.
- **Binding tool:** now exists (`simard goal wip add`, commit `41c05c2a0`).

## 6. Questions for the verification phase

1. Confirm the anchor issue **#4616** is OPEN and its checklist still matches
   `Specs/COVERAGE_AUDIT.md` §2 (per-group ≥70% or justified exception, empty backlog,
   clean §3 high-risk scan, attached `cargo llvm-cov` table).
2. Confirm the goal is truly a phantom (in `goal-board:snapshot` but not `goal_board.json`)
   at *current* HEAD — this gates whether the operator question is "re-instate vs retire"
   vs. a plain "bind #4616 and resume."
3. Confirm every operator-facing Signal string is plain English with **no** raw markers
   (`OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` / `why=` / `evidence=[` / 🔒).
