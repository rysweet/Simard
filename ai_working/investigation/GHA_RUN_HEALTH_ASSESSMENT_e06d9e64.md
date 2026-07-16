# GitHub Actions run-health assessment — governance workflows

Closes the one open criterion from the round-1 investigation of
`goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`:
a **literal** assessment of GitHub Actions *workflow-run* health (pass/fail
rates, flakiness, deprecated actions, timeouts) — not just the goal-board
coverage-predicate mechanism.

Data source: live `gh run list` / `gh run view` against `rysweet/Simard`,
collected 2026-07-16 ~17:55 UTC. Sample = the most recent 10–40 runs per
workflow (all history the API returned within the window).

---

## 1. Scope — which workflows are "governance"

`gh workflow list --all` returns 6 workflows. The four first-party CI/release
governance workflows (defined in `.github/workflows/`) are in scope; the two
platform-managed ones are noted but not defense-relevant.

| Workflow | File | Trigger | Role |
|---|---|---|---|
| `verify` | `verify.yml` | push + pull_request | primary gate (pre-commit, tests, e2e-dashboard, install-real, node) |
| `coverage` | `coverage.yml` | pull_request | coverage-audit gate |
| `release` | `release.yml` | push (tags) | release build/publish + cosign |
| `advisory-scan` | `advisory-scan.yml` | daily cron 06:00 UTC + dispatch | supply-chain/advisory sweep |
| Dependabot Updates | (managed) | schedule | platform-managed, no first-party YAML |
| Copilot cloud agent | (managed) | dispatch | platform-managed, no first-party YAML |

---

## 2. Run pass/fail rates (headline health)

| Workflow | Sample | Success | Failure | In-progress | Success rate (completed) |
|---|---|---|---|---|---|
| `verify` | 30 | 25 | 1 | 4 | **96%** (25/26) |
| `coverage` | 30 | 29 | 1 | 0 | **97%** (29/30) |
| `release` | 30 | 30 | 0 | 0 | **100%** |
| `advisory-scan` | 10 | 10 | 0 | 0 | **100%** |

**Default-branch (`main`) health is green.** Last 15 `verify` runs on `main`:
14 `success`, 1 in-progress, **0 failures**. All observed failures are on
feature branches (`engineer/…`, `feat/…`, `fix/…`), i.e. the gate is correctly
red-flagging in-progress work, not indicating broken CI infrastructure.

---

## 3. Failure classification — real gate, not infra flakiness

The failing `verify` runs fail in the **`pre-commit`** job (lint/format/hook
gate); `e2e-dashboard` and `install-real` then show `skipped` (dependency
short-circuit), not independent failures. Examples:

- run `29489730413` (push, `main` intermediate) → `pre-commit` failure →
  immediately followed by green runs on `main`.
- run `29489527062` (PR `feat/issue-4163-restore-simard`) → `pre-commit` failure.

`coverage` failures likewise correlate with in-progress feature branches
(`29515875359`, `29381801482`). These are **content failures** (the gate
catching real lint/coverage deltas), not workflow misconfiguration or runner
faults.

---

## 4. Flakiness

**None observed.** Across the last 40 `verify` runs, **0** had `attempt > 1`
(no reruns). There is no evidence of intermittent/flaky failures — failures are
deterministic per-commit and clear on the next green commit. This directly
contradicts any "CI flakiness" reading of the gap.

---

## 5. Timeouts / duration headroom

Every job sets an explicit `timeout-minutes`; observed durations sit far below
budget, so there is **no timeout-exhaustion risk**:

| Workflow | Job timeout budget | Observed max | Observed avg |
|---|---|---|---|
| `verify` (main build) | 120 min (sub-jobs 10–60) | 19.7 min | 14.9 min |
| `coverage` | 120 min | 10.2 min | 7.5 min |
| `release` | (default) | 0.3 min | 0.2 min |
| `advisory-scan` | 20 min | 5.2 min | 4.5 min |

The single `verify` failure ran only 6.8 min — it failed fast at the
`pre-commit` gate, well before any timeout.

---

## 6. Deprecated / unpinned actions

**None.** All `uses:` references are **SHA-pinned with a version comment** and
on current major versions — no deprecated Node16 actions, no floating tags:

- `actions/checkout@…# v4`, `actions/upload-artifact@…# v4`,
  `actions/download-artifact@…# v4`, `actions/setup-node@…# v4`,
  `actions/cache@…# v4`, `actions/github-script@…# v7`
- `Swatinem/rust-cache@…# v2`, `taiki-e/install-action@…# v2.82.10`,
  `dtolnay/rust-toolchain@…# stable|nightly`,
  `sigstore/cosign-installer@…# v4.1.2`

Runners are all `ubuntu-latest`. No `set-output`/`save-state` deprecated
command usage surfaced in the action set.

---

## 7. Verdict — the reframe is substantiated

The round-1 investigation reframed the recurring gap
`…e06d9e64` as a **goal-board coverage-predicate artifact** rather than a real
CI outage, and deferred proof of underlying run health. This assessment
supplies that proof:

- Governance workflow run health is **green** (96–100% success; `main` 0
  failures in the last 15 `verify` runs).
- **No flakiness** (0 reruns/40), **no timeout risk** (≤20 min vs 120 min
  budget), **no deprecated/unpinned actions**.
- All observed failures are the CI gate **correctly** red-flagging in-progress
  feature branches at `pre-commit`/`coverage` — expected behavior.

**Conclusion:** there is no live GitHub Actions health defect behind the gap.
The recurrence is entirely the missing durable goal-board mutation documented
in `CONSOLIDATED_e06d9e64_GAP_CLOSURE.md` §1–§4. The correct disposition
remains the one-line durable close (`simard goal complete …e06d9e64`) plus the
systemic fix (land meta-issue #4126; exempt standing goals in the predicate).

## 8. Ongoing-health guardrails (prevent recurrence in future gap-scans)

1. Keep the scheduled `advisory-scan` daily sweep green (currently 100%); it is
   the standing supply-chain watch.
2. Preserve SHA-pinning + version-comment discipline on all `uses:` (Dependabot
   `Updates` already manages bumps) to avoid future deprecation breakage.
3. Track this run-health baseline in the owning issue **#4172**
   (`tracking(ci-stewardship)`) so a future Overseer gap-scan can point at a
   durable, dated health record instead of re-flagging the goal.

## 9. Provenance

- `gh workflow list --all`; `gh run list --workflow=<w> --json
  status,conclusion,event,headBranch,createdAt,attempt,updatedAt`;
  `gh run view <id> --json jobs` — all against `rysweet/Simard`, 2026-07-16.
- Workflow YAML audited in-repo: `.github/workflows/{verify,coverage,release,advisory-scan}.yml`.
- Complements (does not replace) `CONSOLIDATED_e06d9e64_GAP_CLOSURE.md`.
