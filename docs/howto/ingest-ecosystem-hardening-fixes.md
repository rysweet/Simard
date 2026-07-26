---
title: How to ingest the ecosystem runner-hardening fixes (PR #131 batch)
description: >
  The downstream done-gate for the PR #131 runner-hardening batch: once the
  upstream P2 (#1018), P3 (#1025) and P5 (#1024) fixes land in amplihack-rs,
  bump Simard's amplihack-agent-eval git-rev pin to the audited SHA, refresh
  Cargo.lock, and re-verify — so the fixes Simard ships actually run in her own
  build. P1 (recipe-runner) and P4/P6 are ops/merge escalations, not rev-bumps.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: how-to
status: design — not yet implemented
related:
  - ../reference/ecosystem-hardening-pr131.md
  - ../howto/self-maintain-dependency-pins.md
  - ../reference/amplihack-pin-bump-2626.md
  - ../reference/dependency-trust-policy.md
  - ../reference/supply-chain-audit.md
  - ../howto/route-a-goal-to-its-target-repo.md
  - ../howto/triage-stale-pull-requests.md
  - ../safe-self-update.md
---

# How to ingest the ecosystem runner-hardening fixes (PR #131 batch)

> **Status: design — not yet implemented.** This guide specifies the **only**
> action the `rysweet/Simard` checkout will take for the
> [PR #131 runner-hardening batch](../reference/ecosystem-hardening-pr131.md):
> ingesting the upstream fixes by rev-bump **once they land**. As of this
> writing the P2/P3/P5 fixes are **not yet merged** upstream, so the rev-bump
> is not yet actionable. The code fixes themselves live upstream in
> `amplihack-recipe-runner` and `amplihack-rs` and are **not** editable here.

Use this when the upstream P2 (#1018), P3 (#1025), and/or P5 (#1024) fixes have
**merged** to `amplihack-rs` `main` and you need them running in Simard's own
build. It is a concrete instance of the reactive done-gate in
[How to keep Simard's dependency pins up to date](./self-maintain-dependency-pins.md),
worked exactly like [amplihack pin bump (#2626)](../reference/amplihack-pin-bump-2626.md).

## What is (and is not) a rev-bump

| Problem | Repo | Action here |
| --- | --- | --- |
| **P2** #1018 (version derivation) | `amplihack-rs` | ✅ ingest via `amplihack-agent-eval` rev-bump once landed |
| **P3** #1025 (graceful reflect stop) | `amplihack-rs` | ✅ ingest via `amplihack-agent-eval` rev-bump once landed |
| **P5** #1024 (subscriber lifecycle) | `amplihack-rs` | ✅ ingest via `amplihack-agent-eval` rev-bump once landed |
| **P1** PR #131 (Repo Guardian) | `amplihack-recipe-runner` | ❌ **not** a Simard rev-bump — ops secret rotation + upstream probe |
| **P4** #1015 / **P6** backlog | `amplihack-rs` / `Simard` | ❌ **escalations** — merge steward, not a rev-bump |

> **P1 is not ingested here.** The `Repo Guardian` blocker is an expired
> `ANTHROPIC_API_KEY` (infra 401). Its fix is a **secret rotation** (ops) plus a
> liveness probe in the recipe-runner workflow — neither is a Simard dependency.
> See the [batch reference, P1](../reference/ecosystem-hardening-pr131.md#p1--repo-guardian-credential-liveness--e2big-child-env-allow-list).

## Preconditions (gate the bump)

Do **not** bump until **all** of these hold:

1. The target fix (P2 / P3 / P5) has **merged** to `amplihack-rs` `main` with all
   required checks green.
2. You have the exact **40-char merged SHA** — an **audited** commit, not a
   moving branch ref. Bumping to a branch would ingest unrelated/unaudited
   commits (see the [supply-chain risk](../reference/ecosystem-hardening-pr131.md#security-considerations)).
3. For P2 specifically, its fix cannot land until the amplihack-rs build/Test
   checks are green — which is what P2 itself unblocks (#1022 / #1007 pattern).

## Steps

### 1. Confirm the fix is on `main` and get the audited SHA

```bash
# The merged SHA for the target fix (example: P3 / #1025).
gh pr view <UPSTREAM_PR> --repo rysweet/amplihack-rs \
  --json mergeCommit,mergedAt,state --jq '{state, mergedAt, sha: .mergeCommit.oid}'

# Cross-check it is reachable from main HEAD (audited, not a stray ref).
gh api repos/rysweet/amplihack-rs/compare/<SHA>...main --jq '{status, behind: .behind_by}'
# status: "identical" or "behind" (SHA is an ancestor of main) — never "diverged".
```

### 2. Bump the pin in `Cargo.toml`

`amplihack-agent-eval` is Simard's pin on `amplihack-rs`. Re-point its `rev` to
the audited SHA:

```bash
grep -nE 'amplihack-agent-eval.*rev = "[0-9a-f]{40}"' Cargo.toml
# Edit the rev = "…" value to the audited SHA from step 1.
```

### 3. Refresh only that crate in the lock

```bash
cargo update -p amplihack-agent-eval          # scoped: no unrelated churn
```

`Cargo.lock` must record the identical SHA. `version` fields are never
hand-edited — they update from the rev bump automatically.

### 4. Build and test against the ingested fix

```bash
cargo build --release                          # or: scripts/cargo-low-space build
cargo test
```

A bump that does not build/test clean is **rolled back, not shipped**. If a
call-site broke, fix it **forward** to the new API — never add a fallback shim
(a silent fallback is a silent failure this repo treats as a defect).

### 5. Re-verify the supply chain

```bash
cargo deny --locked check                      # advisories + licenses + bans + sources
cargo audit                                    # RUSTSEC
cargo vet --locked                             # transitive trust
```

The `[sources]` allowlist is unchanged (same `amplihack-rs.git` remote), so these
stay green.

### 6. Open the bump PR against `rysweet/Simard`

Follow the shared
[bump-PR convention](./self-maintain-dependency-pins.md#bump-pr-conventions-and-de-duplication)
— one PR per upstream repo, keyed on the repo not the crate:

| Field | Value |
| --- | --- |
| Branch | `chore/bump-amplihack-rs-pin` |
| Title | `chore(deps): bump amplihack-rs pin to <short-sha> (ingest #1018/#1025/#1024)` |
| Base | `rysweet/Simard` `main` |
| Body | Names each ingested upstream issue/PR and the audited SHA |

Check for an existing open bump PR for `amplihack-rs` first and **update** it
rather than opening a duplicate:

```bash
gh pr list --repo rysweet/Simard --state open --head chore/bump-amplihack-rs-pin \
  --json number,title,headRefName
```

The bump PR rides the normal
[PR-finalization pipeline](../reference/pr-finalization-pipeline.md): **all
required CI green before merge**, no `--no-verify`, no `--admin`.

## Done-gate

The ingestion is "done" **only** once:

1. `Cargo.toml` `amplihack-agent-eval` rev == the audited `amplihack-rs` SHA.
2. `Cargo.lock` records that same SHA.
3. `cargo build --release` and `cargo test` pass.
4. Supply-chain jobs are green.
5. The bump PR has **merged** to `rysweet/Simard` with all required CI green.

Rolling the merged bump into the **running daemon** is the operator's step, via
[Safe Self-Update](../safe-self-update.md) (`simard safe-update`) — not required
for this ingestion's goal to report done.

## Handle the escalations (P4 / P6)

These are **not** rev-bumps — route them, do not ingest:

- **P4 (`amplihack-rs` #1015):** a green, MERGEABLE PR → merge steward. See
  [Triage stale pull requests](./triage-stale-pull-requests.md).
- **P6 (Simard backlog):** 16 green PRs vs `per-cycle launch cap reached` →
  delivery steward and/or raise the per-cycle launch cap. Partially relieved once
  P3 lands (green runs stop reflecting, freeing launch slots). See
  [Review Overseer workstream gaps](./review-overseer-workstream-gaps.md).

## Verify end-to-end

```bash
# 1. Pin equals the audited upstream SHA and is not behind an unaudited main.
PINNED=$(grep 'amplihack-agent-eval' Cargo.toml | grep -oE '[0-9a-f]{40}')
gh api repos/rysweet/amplihack-rs/compare/$PINNED...main --jq '{status, behind: .behind_by}'

# 2. Lock agrees with the manifest.
grep -A3 'name = "amplihack-agent-eval"' Cargo.lock   # source …#<SHA>

# 3. Build, test, supply-chain all green.
cargo build --release && cargo test
cargo deny --locked check && cargo audit && cargo vet --locked

# 4. At most one open amplihack-rs bump PR.
gh pr list --repo rysweet/Simard --state open --search 'in:title "bump amplihack-rs pin"'
```

## Related reading

- [Batch reference: Ecosystem runner-hardening (PR #131)](../reference/ecosystem-hardening-pr131.md) —
  the finished-state spec for all six problems.
- [How to keep Simard's dependency pins up to date](./self-maintain-dependency-pins.md) —
  the reactive done-gate and proactive reconcile this ingestion instantiates.
- [amplihack pin bump to upstream main (#2626)](../reference/amplihack-pin-bump-2626.md) —
  a worked prior instance of the same rev-bump.
- [How to route a goal to its target repo](./route-a-goal-to-its-target-repo.md) —
  why the upstream code fixes belong upstream, and the bump belongs here.
- [Safe Self-Update](../safe-self-update.md) — the operator step that rolls a
  merged bump into the running daemon.
