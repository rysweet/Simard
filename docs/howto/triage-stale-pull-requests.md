---
title: Triage stale open pull requests
description: Evergreen runbook for triaging stalled open PRs across rysweet/Simard (and rysweet/amplihack-rs), bringing the wanted ones to the six merge-ready criteria and merging them, and closing the obsolete/superseded ones — conservatively, quality over volume.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-cli.md
  - ../reference/pr-finalization-pipeline.md
  - ./edit-the-engineer-system-prompt.md
  - ./inspect-and-clean-engineer-worktrees.md
---

# Triage stale open pull requests

> **Goal.** Walk the **live** set of open pull requests, rank them by
> staleness, and give each one an explicit disposition: **land** it (drive it
> to the six merge-ready criteria and merge), **close** it (obsolete /
> superseded / abandoned), or **leave it open** with a concise triage note
> (intent unclear, risky, or needs a human). Be conservative — never
> force-merge past a real failing check, an unresolved conflict, or unclear
> intent.

This is an **operations + light-development** runbook. It is **evergreen**: it
always re-enumerates the live PR list at run time and never hard-codes a
point-in-time snapshot of PR numbers. Candidate numbers you may have seen in a
hand-off (for example a stale list like `#2406 / #2398 / #2394 / #2392`) are
**hints only** — they go stale within hours. The authoritative input is always
the output of `gh pr list` at the moment you run it.

## Scope

| Repo | Primary? | Merge mechanism |
|------|----------|-----------------|
| `rysweet/Simard` | **yes** | `simard merge-pr <N>` (deterministic gate + merge judge). A red `cargo-audit` / `install-real` can only be cleared by an explicit, recorded **operator override** (`gh pr merge`), never by the gate — see [Stage D](#stage-d-act). |
| `rysweet/amplihack-rs` | secondary (also check) | `gh pr merge --squash --delete-branch` |

Operate on **existing PR branches only**. You **never** open a new PR for work
that already has one — every "land" continues the PR's own branch.

## Prerequisites

- **`gh`** authenticated against both repos (`gh auth status`).
- **`simard`** CLI on `PATH` (the merge authority; see
  [Simard CLI reference](../reference/simard-cli.md)).
- A **clean place to work**. The top-level checkout and `worktrees/main` may be
  parked on unrelated feature branches; **never push from a dirty tree**.
  Always isolate each PR with `gh pr checkout <N>` (see
  [Working-tree safety](#working-tree-safety)).
- `NODE_OPTIONS=--max-old-space-size=32768` exported for any Node-backed
  tooling invoked during fixes (saved operator preference; raises the V8 heap
  so large lint/build steps do not OOM). Change it in
  `~/.amplihack/config` if needed.

## The procedure at a glance

```
A. ENUMERATE      gh pr list (both repos) → rank by staleness
        │
        ▼
B. FILTER         drop in-flight PRs (recent + owned) — leave untouched
        │
        ▼
C. TRIAGE         per PR: draft? base==main? mergeable/CONFLICTS?
                  checks: REAL vs ENVIRONMENTAL? superseded/obsolete?
        │
        ▼
D. ACT            LAND ─ checkout → rebase main → fix REAL failures →
                         write six-criteria evidence → push SAME branch →
                         merge (simard merge-pr / gh pr merge)
                  CLOSE ─ gh pr close + "why" comment
                  LEAVE ─ triage comment (blocker + what's needed)
        │
        ▼
E. REPORT         per-PR disposition table
```

## Stage A — enumerate and rank

List every open PR with the fields the triage needs, for **each** in-scope
repo:

```bash
gh pr list -R rysweet/Simard --state open \
  --json number,title,updatedAt,isDraft,mergeStateStatus,headRefName,baseRefName \
  --limit 100

gh pr list -R rysweet/amplihack-rs --state open \
  --json number,title,updatedAt,isDraft,mergeStateStatus,headRefName,baseRefName \
  --limit 100
```

Rank by **staleness** so the most-stalled work surfaces first. Sort key, in
order:

1. oldest `updatedAt` first,
2. then `mergeStateStatus` in the "stuck" states (`DIRTY`, `BLOCKED`, `BEHIND`,
   `UNKNOWN`) ahead of `CLEAN`,
3. then `isDraft == true` ahead of ready-for-review.

```bash
gh pr list -R rysweet/Simard --state open \
  --json number,title,updatedAt,isDraft,mergeStateStatus \
  --jq 'sort_by(.updatedAt) | .[] | "\(.updatedAt)  #\(.number)  draft=\(.isDraft)  \(.mergeStateStatus)  \(.title)"'
```

## Stage B — filter out in-flight PRs

A stale PR is one that has **stalled**. A PR that a live engineer is **actively
driving** is not stale — leave it untouched. Skip a PR (do **not** triage or
touch it) when **both** of these hold:

- `updatedAt` is **within the last 48 hours**, **and**
- it has an **active owner signal** — a non-empty `assignees`, or a live
  engineer claim (`.simard-engineer-claim` on its branch / an attached running
  engineer; see
  [inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)).

Everything else — old `updatedAt`, red checks, draft, or a stuck
`mergeStateStatus` — is **in scope** as a stale candidate.

## Stage C — triage each candidate

For each in-scope PR, gather the deterministic facts:

```bash
gh pr view <N> -R <owner/repo> \
  --json number,title,body,isDraft,mergeable,mergeStateStatus,baseRefName,statusCheckRollup,reviewDecision
gh pr checks <N> -R <owner/repo>
```

Classify along four axes:

1. **Draft?** A `isDraft == true` PR is **not** landable until the author marks
   it ready. Treat as *leave-open* unless it is clearly abandoned (then
   *close*).
2. **Base branch.** `baseRefName` **must** be `main` (or a value in
   `SIMARD_MERGE_BASE_ALLOWLIST`). A PR targeting a stale/wrong base is the
   PR-#1549 footgun; it is auto-refused by the gate — *leave-open* with a note
   unless the non-`main` base is clearly intentional and safe.
3. **Mergeability.** `mergeable == "CONFLICTING"` (or `mergeStateStatus ==
   DIRTY`) means conflicts must be resolved on the branch before anything else.
4. **Checks — REAL vs ENVIRONMENTAL.** This is the crux of the triage. See the
   next section.
5. **Superseded / obsolete?** If the change has already landed via another
   merged PR, or the surface it touches no longer exists, it is a *close*
   candidate.

### Real vs environmental checks

The triage classifies each `statusCheckRollup` entry by its **actual check
name**. The live Simard CI checks are `pre-commit`, `coverage`, `cargo-audit`,
`install-real`, `e2e-dashboard`, and `build` (the MkDocs `--strict` job from
`docs.yml`, present only when the PR touches `docs/**`, `mkdocs.yml`, or
`Specs/**`). Classify them like this:

| Class | Checks (actual rollup names) | Blocks merge? |
|-------|------------------------------|---------------|
| **Real** (hard gate) | `pre-commit`, `coverage` (present only when the PR touches `src/**`), `build` (present only when the PR touches `docs/**`, `mkdocs.yml`, or `Specs/**`), `e2e-dashboard`, and **every** PR-specific job | **Yes** — must be `SUCCESS` / `NEUTRAL` / `SKIPPED`. |
| **Environmental** (candidate non-blocking) | `cargo-audit`, `install-real` | **Only after a human confirms** the red is dependency/infra noise unrelated to this PR's diff — never automatically. |
| **Pending / unknown** | any check in `PENDING` / `QUEUED` / `IN_PROGRESS`, or an unrecognised name | **Yes** — blocks until it resolves green. |

Naming pitfalls that matter when you read a real rollup:

- **There is no separate Rust `build` or `fmt` check.** Formatting (`cargo fmt`),
  linting (`clippy`), and compilation (`cargo test` plus the binary build) all run
  **inside the single `pre-commit` job**. A formatting or Rust build break surfaces
  as a red **`pre-commit`**, which you fix locally (`cargo fmt --all`, then fix
  the lint/compile error) and push to the same branch. `pre-commit` is a **real
  hard gate** — `verify.yml` explicitly treats a `pre-commit` failure as a real
  regression — so it is **never** environmental.
- **A `build` check *does* appear on docs PRs.** When the PR touches `docs/**`,
  `mkdocs.yml`, or `Specs/**`, `docs.yml` runs a job named **`build`** that
  executes `mkdocs build --strict`. It is a **real hard gate** (a broken link or
  malformed Markdown reds it); reproduce it locally with `mkdocs build --strict`
  and push to the same branch. Don't mistake it for a Rust compile check.
- **`cargo-audit`** goes red on RUSTSEC advisories in transitive dependencies,
  which are frequently unrelated to the PR's diff.
- **`install-real`** is a ~25-minute from-scratch `cargo install` that can flake
  on runner infrastructure.

"Environmental" is **not** an automatic property: the merge tooling treats every
red as blocking. Only a **human**, after inspecting the failure and confirming it
is infra/dependency noise *not caused by this PR*, may classify `cargo-audit` or
`install-real` as environmental — and even then merging requires the explicit
[operator override](#the-deterministic-gate-what-simard-merge-pr-enforces) below.
An unknown red is **always** real and blocking; never assume a new red is
"probably environmental."

### The deterministic gate (what `simard merge-pr` enforces)

`simard merge-pr <N>` runs an **objective, never-agentic** gate before it will
merge a `rysweet/Simard` PR. It refuses (non-zero exit, reason on stderr) if
**any** of these fail:

1. `baseRefName` is **not** in the allow-list (default `["main"]`, override via
   `SIMARD_MERGE_BASE_ALLOWLIST`). Evaluated **first**.
2. `mergeable != "MERGEABLE"`.
3. **Any** `statusCheckRollup` entry is not `SUCCESS` / `NEUTRAL` / `SKIPPED`
   — i.e. any `FAILURE`, `CANCELLED`, `TIMED_OUT`, `STARTUP_FAILURE`,
   `ACTION_REQUIRED`, `PENDING`, `QUEUED`, `IN_PROGRESS`, or unknown state
   blocks.

Only after all three pass does the **agentic merge judge**
([`MergeJudge`](../reference/pr-finalization-pipeline.md)) read the PR body and
verdict the six merge-ready criteria. If the judge says `ready`, the gate runs
`gh pr merge <N> --squash --delete-branch`.

> **The gate never auto-excuses an environmental red.** `evaluate_objective_gates`
> refuses on **any** check that is not `SUCCESS` / `NEUTRAL` / `SKIPPED` —
> including `cargo-audit` and `install-real`. There is no flag, env var, or
> allow-list that makes `simard merge-pr` skip a red check, and the merge
> authority is deliberately built to **never silently fall back**. So a PR whose
> only reds are `cargo-audit` / `install-real` **will be refused** by
> `simard merge-pr`.
>
> The env-red bypass in [Stage D](#stage-d-act) is therefore **not**
> tooling-sanctioned behaviour — this runbook *introduces* it as an **operator
> policy** layered on top of the gate. A human, having confirmed the only reds
> are the named environmental checks **and** every real gate is green, makes a
> deliberate judgment call to merge via raw `gh pr merge` and records that
> decision in a PR comment. It is an **audited human override** of the gate, not
> something the gate does for you, and it is **never** a way past a real failure,
> an unresolved conflict, or a pending real check.

### The six MERGE-READY criteria

A PR may only be **landed** once its body carries all six evidence sections
(the merge judge's source of truth is
`prompt_assets/simard/merge_readiness_judge.md`; the skill template at
`~/.copilot/skills/merge-ready/pr-description-template.md` is authoritative for
wording). Each section must be **substantive** — concrete file paths, command
output, commit SHAs, links — not a heading with a placeholder.

1. **QA-team evidence** — scenarios + validate + run results.
2. **Documentation** — surfaces touched + doc updates (or an internal-only
   justification).
3. **Quality-audit** — at least **three** SEEK → VALIDATE → FIX cycles ending
   clean, each with a referenced commit SHA.
4. **CI** — a link to the green run for every required check.
5. **Scope** — a diff summary confirming **no unrelated edits**.
6. **Verdict** — an explicit `ready to merge` / `draft` / `blocked` call with
   rationale.

## Stage D — act

Choose **exactly one** disposition per PR.

### LAND — drive a wanted, fixable PR to merge

1. **Isolate the branch** (never the shared tree):

   ```bash
   gh pr checkout <N> -R <owner/repo>
   ```

   > **Security — a checked-out PR branch is untrusted code.** `gh pr checkout`
   > plus any local `cargo build` / `cargo test` / `cargo clippy` **executes the
   > author's code** (`build.rs`, proc-macros, test bodies, and dependency build
   > scripts) on your machine — where your `gh` merge credentials live. Prefer the
   > GitHub-side results (`gh pr checks <N>`) as the source of truth; only build
   > locally when you are authoring a real fix, and first review the diff —
   > especially new/changed `Cargo.toml` / `Cargo.lock` entries (`gh pr diff <N>`)
   > — for unexpected dependencies. **Never** run a PR's build just to LEAVE-OPEN
   > or CLOSE it.

2. **Resolve conflicts / staleness** by bringing `main` in on the **same**
   branch:

   ```bash
   git fetch origin main
   git rebase origin/main      # or: git merge origin/main
   # resolve, then:
   git push --force-with-lease  # rebase; plain push for merge
   ```

3. **Fix the REAL failures only.** Reproduce the real reds locally — `pre-commit`
   (run `cargo fmt --all`, `cargo clippy`, `cargo test`), `coverage`,
   `e2e-dashboard`, and any PR-specific job — fix them, and push to the **same**
   branch. Leave `cargo-audit` / `install-real` red only if you have confirmed
   they are infra/dependency noise (see
   [Real vs environmental](#real-vs-environmental-checks)).

4. **Write the six-criteria evidence into the PR body** (Stage C list). Use the
   merge-ready template; make each section substantive.

5. **Merge**, by repo:

   ```bash
   # rysweet/Simard — deterministic gate + merge judge:
   simard merge-pr <N>

   # Operator override (NOT a tooling fallback): use ONLY if simard merge-pr
   # refused SOLELY because of a human-confirmed environmental red
   # (cargo-audit / install-real) and every REAL gate is green. Record the
   # decision in a PR comment first.
   gh pr merge <N> -R rysweet/Simard --squash --delete-branch

   # rysweet/amplihack-rs and other cross-repo PRs:
   gh pr merge <N> -R rysweet/amplihack-rs --squash --delete-branch
   ```

6. **Close the linked issue** if the PR did not auto-close it (`Closes #<N>`
   for same-repo; explicit `gh issue close` cross-repo).

> Never use `--admin` / `-f`, never bypass a protected branch, never merge with
> a **real** required check failing. `main` is not branch-protected here, which
> is exactly why this runbook is conservative — the discipline is in the
> procedure, not the server.

### CLOSE — obsolete / superseded / abandoned

```bash
gh pr close <N> -R <owner/repo> \
  --comment "Closing as <obsolete|superseded by #<M>|abandoned>: <one-line why>. \
Reopen if this is still wanted."
```

Always leave the **reason** so the history is auditable. Deleting the stale
branch is optional (`--delete-branch`) and safe once closed.

### LEAVE OPEN — unclear, risky, or needs a human

Post a **concise triage comment** naming the blocker and the work needed, then
move on. Do **not** force-merge ambiguous work.

```bash
gh pr comment <N> -R <owner/repo> --body "$(cat <<'EOF'
**Triage:** left open.
- **Blocker:** <conflicts with main / base is not `main` / real `coverage` red / intent unclear>.
- **Needed to land:** <rebase + resolve X / repoint base to main / fix Y / author confirms intent>.
- **Not done here because:** <risky/ambiguous — needs a human decision>.
EOF
)"
```

## Stage E — report the dispositions

The runbook's deliverable is a **per-PR disposition table**. Produce it for the
PRs you handled this run (re-enumerated live, never a stored snapshot):

| PR | Triage finding | Action | Result |
|----|----------------|--------|--------|
| `<owner/repo>#<N>` | conflicts + stale base, real gates otherwise green | rebased main, cleared the `pre-commit` fmt red, wrote six-criteria evidence | **merged** (`simard merge-pr`) |
| `<owner/repo>#<N>` | superseded by `#<M>` (already merged) | closed with reason | **closed** |
| `<owner/repo>#<N>` | draft, intent unclear | triage comment posted | **left open** |
| `<owner/repo>#<N>` | only red was `cargo-audit` (confirmed infra noise); real CI green | recorded operator override after human review | **merged** (`gh pr merge`, env-red note) |

Every **merged** row must have passed the **real** CI gates
(`pre-commit` / `coverage` / `e2e-dashboard` + PR-specific) **and** the six
merge-ready criteria. Nothing is force-merged past a real failure or an
unresolved conflict.

## Configuration

| Variable | Default | Purpose |
|----------|---------|---------|
| `SIMARD_MERGE_BASE_ALLOWLIST` | `main` | Comma-separated list of base branches `simard merge-pr` will accept. The first gate; a PR targeting a base outside this list is refused before any other check. |
| `NODE_OPTIONS` | `--max-old-space-size=32768` (operator preference) | Raises the V8 heap for Node-backed lint/build steps run while fixing a PR, so large steps do not OOM. Set in `~/.amplihack/config`. |

There is deliberately **no** config knob for the "environmental" classification:
`simard merge-pr` blocks on `cargo-audit` / `install-real` like any other red.
Clearing one is a human [operator override](#stage-d-act), not a setting.

## Worked example — landing one stalled PR

```bash
# 1. Find the stalest Simard PR.
gh pr list -R rysweet/Simard --state open \
  --json number,updatedAt,mergeStateStatus,isDraft \
  --jq 'sort_by(.updatedAt) | .[0]'

# 2. Triage it.
gh pr view 1234 -R rysweet/Simard \
  --json mergeable,mergeStateStatus,baseRefName,statusCheckRollup,isDraft
gh pr checks 1234 -R rysweet/Simard
#   → base=main, mergeStateStatus=DIRTY (conflicts), reds: pre-commit (REAL —
#     a cargo fmt break inside the pre-commit job) + cargo-audit (candidate
#     environmental). Not a draft. Wanted work.

# 3. Isolate, rebase, fix the REAL red (the pre-commit fmt break), push SAME branch.
gh pr checkout 1234 -R rysweet/Simard
git fetch origin main && git rebase origin/main      # resolve conflicts
cargo fmt --all                                       # clears the red pre-commit
git commit -am "fix: rebase on main + cargo fmt"
git push --force-with-lease

# 4. Write the six merge-ready evidence sections into the PR body
#    (QA, Documentation, Quality-audit ×3, CI link, Scope, Verdict).
gh pr edit 1234 -R rysweet/Simard --body-file /tmp/pr-1234-body.md

# 5. Merge through the gate.
simard merge-pr 1234
#   merged: PR #1234 in rysweet/Simard (squash + delete-branch)
```

If step 5 instead prints
`refused: CI check 'cargo-audit' has state 'FAILURE' (expected SUCCESS/NEUTRAL/SKIPPED)`
**and** you have confirmed `cargo-audit` is the *only* red and is
dependency/infra noise unrelated to this PR (all real gates green), record the
decision and apply the operator override:

```bash
gh pr comment 1234 -R rysweet/Simard \
  --body "Real gates (pre-commit/coverage/e2e-dashboard + PR checks) all green; \
only red is cargo-audit (RUSTSEC advisory in a transitive dep, unrelated to this \
diff). Merging via gh as a recorded operator override per the stale-PR triage runbook."
gh pr merge 1234 -R rysweet/Simard --squash --delete-branch
```

## Worked example — closing an obsolete PR

```bash
gh pr view 5678 -R rysweet/amplihack-rs --json title,body,statusCheckRollup
#   → its change already landed in #5701 (merged). Superseded.

gh pr close 5678 -R rysweet/amplihack-rs --delete-branch \
  --comment "Superseded by #5701, which landed the same refactor. Closing; \
reopen if anything here is still wanted."
```

## Invariants and safety rules

- **Live enumeration is authoritative.** Always re-`gh pr list`; never trust a
  hand-off snapshot of PR numbers. This doc carries **no** point-in-time PR
  list.
- **Existing branches only.** Continue the PR's own branch; **never** open a
  duplicate PR for work that already has one.
- **No force-merge.** Never merge past a **real** failing check, an unresolved
  conflict, a non-`main` base, or unclear/risky intent. The only sanctioned
  bypass is the **recorded operator override** for a human-confirmed
  environmental red (`cargo-audit` / `install-real`) — and even that requires
  every real gate green and a PR comment documenting the decision.
- **Conservative on ambiguity.** Unclear intent / risky change → *leave open
  with a triage note*, never a forced merge.
- **Don't disturb in-flight work.** A PR updated within 48h **and** actively
  owned is skipped untouched.
- **Working-tree safety.** Push only from a `gh pr checkout` branch, never from
  the dirty top-level tree or `worktrees/main`.
- **Untrusted-code safety.** Treat every checked-out PR branch as untrusted:
  `cargo build` / `cargo test` runs the author's code (incl. `build.rs`,
  proc-macros, dependency build scripts) on your host alongside your merge
  credentials. Prefer `gh pr checks`; build locally only to author a fix, after
  reviewing the dependency diff.
- **Auditable closes.** Every `gh pr close` carries a one-line reason.

## Working-tree safety

The top-level checkout and `worktrees/main` are routinely parked on unrelated
feature branches with uncommitted changes. Pushing from there would corrupt an
unrelated branch. The rule is absolute: **one `gh pr checkout <N>` per PR**,
resolve and push there, then move to the next PR. See
[inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
for managing the per-PR checkouts and reclaiming them afterward.

## Related reading

- [Simard CLI reference](../reference/simard-cli.md) — the `merge-pr`
  subcommand and the rest of the operator surface.
- [PR-finalization review pipeline reference](../reference/pr-finalization-pipeline.md)
  — the per-engineer crusty→pr-guide→review→merge-ready→merge pipeline this
  runbook's "land" path mirrors.
- [How to edit the engineer system prompt](./edit-the-engineer-system-prompt.md)
  — where the Merge-Ready Contract and the six evidence headings live.
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
  — managing the per-PR checkouts you create while landing.
