---
title: Drift-resilient self-deploy checkout reference
description: >
  Reference for the self-deploy source-prep hardening that stops a tracked-file
  drift in the managed clone (canonically `.github/hooks/amplihack-hooks.json`)
  from wedging every self-deploy with `git checkout` "your local changes would
  be overwritten". Covers the fail-closed, canonical-only `reset_source_tree`
  scrub that runs before every `checkout --detach`, the `remove_stale_checkout`
  clone-clean recovery, and the git-tracked / drift-free CI invariant.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-source-prep.md
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/self_deploy/source_prep.rs
  - ../../src/self_deploy/tests_source_prep.rs
  - ../../tests/self_deploy_hooks_tracked_invariant.rs
---

# Drift-resilient self-deploy checkout reference

> **Status: implemented.** The canonical-only `reset_source_tree` scrub, its
> `is_canonical_src_repo` fail-closed gate, and the `remove_stale_checkout`
> clone-clean recovery live in
> [`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs).
> The git-tracked / drift-free invariant is asserted by
> [`tests/self_deploy_hooks_tracked_invariant.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_deploy_hooks_tracked_invariant.rs),
> and the reset behaviour by
> [`src/self_deploy/tests_source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/tests_source_prep.rs).
> Everything here is **additive** and **non-breaking**: the clean-tree happy
> path (`git checkout --detach <sha>` succeeds first try) is unchanged, no error
> variant is renamed, and the [source-prep contract](./self-deploy-source-prep.md)
> keeps its existing signatures.

This reference extends the
[self-deploy source preparation reference](./self-deploy-source-prep.md). Read
that first: it defines the canonical repo-resolution precedence, the managed
disposable clone at `self_deploy_src_dir()`, and the warm
`self_deploy_target_dir()` this page hardens.

## Why this exists (#4914)

The self-deploy checkout failed **every** Overseer cycle for hours and the
running binary fell commits behind merged `main`, so `ProcessHealth` kept
firing `ooda.log tail contains recent ERROR line(s)`. The recurring error was:

```
error: Your local changes to the following files would be overwritten by checkout:
        .github/hooks/amplihack-hooks.json
Please commit your changes or stash them before you switch branches.
```

A prior self-deploy left the disposable canonical checkout at
`self_deploy_src_dir()` with locally-modified **tracked** files (canonically
`.github/hooks/amplihack-hooks.json`, rewritten out-of-band by the amplihack
framework each session) plus untracked cruft. That drift aborts the next
`git checkout --detach <merged_sha>`, so the running binary can never adopt the
shipped fix — a self-reinforcing wedge.

The repair has two independent halves:

1. **Recover at deploy time.** Reset + clean the disposable canonical checkout
   *before* `checkout --detach`, strictly gated so only the throwaway clone can
   ever be scrubbed (never a caller-supplied override or the operator cwd).
2. **Keep the drift source honest.** Assert in CI that `.github/hooks/` stays
   git-tracked and that a clean checkout carries **no** untracked drift, so a
   reappearing unconditional manifest rewrite reds `cargo test` instead of
   silently re-wedging self-deploy.

> **Escalation linkage.** This is the root-cause repair for the symptom that
> escalation PR #4914 tagged. Closing #4914 itself is an operational Overseer
> action, not part of this code change — see
> [operational autonomy model](../concepts/operational-autonomy-model.md).

## Contents

- [Design invariants](#design-invariants)
- [1. Reset-before-checkout on both prepare paths](#1-reset-before-checkout-on-both-prepare-paths)
- [2. Fail-closed canonical-only gate](#2-fail-closed-canonical-only-gate)
- [3. Clone-clean recovery for an absent/invalid checkout](#3-clone-clean-recovery-for-an-absentinvalid-checkout)
- [4. Git-tracked / drift-free CI invariant](#4-git-tracked--drift-free-ci-invariant)
- [Observability](#observability)
- [Error surface](#error-surface)
- [Security model](#security-model)
- [Testing](#testing)

## Design invariants

| Invariant | Guarantee |
| --- | --- |
| **Additive / non-breaking** | The clean-tree checkout path is unchanged; the scrub only discards drift a prior deploy left in the disposable clone. |
| **Fail-closed scrub** | `reset_source_tree` runs **only** when the double-canonicalized path is *exactly* `self_deploy_src_dir()`. Any canonicalize error or mismatch refuses the reset — it never guesses. |
| **`amplihack-hooks.json` stays tracked** | The manifest and hook scripts remain git-tracked for review and supply-chain integrity. The fix recovers from the drift; it does **not** untrack / gitignore the file. |
| **`clean -fd`, never `-x`** | Ignored files (`.env`, credential caches, warm build artifacts) are preserved. The warm `self_deploy_target_dir()` is a separate dir and is never scrubbed, so self-deploys stay incremental. |
| **Override / cwd never scrubbed** | A `SIMARD_SELF_DEPLOY_REPO` override resolves to a different canonical path, so a dirty override still fails loud at checkout — exactly as before. |
| **Structured observability only** | Detail is routed through `tracing` and `redact_credentials`. No `print!` / `println!` / `eprintln!` in the changed effectful paths. |

## 1. Reset-before-checkout on both prepare paths

Both source-prep entry points reset the disposable canonical checkout to a
pristine tree *after* fetch and *before* `checkout --detach`, so a wedged tree
left by a prior deploy cannot abort the checkout:

- `GitSourcePreparer::prepare` — the effectful deploy path. The reset runs on
  **both** the fetch branch and the skip-fetch (commit-already-present) branch,
  so a wedged tree whose target is already local cannot slip past.
- `SelfDeploySourcePreparer::prepare_existing_repo` — the autonomous pre-swap
  canary path that never clones from cwd.

`reset_source_tree` runs `git reset --hard` (discard tracked-file edits at
`HEAD`) then `git clean -fd` (remove untracked files/dirs). Both go through the
env-scrubbed `git_capture` (no shell, argv-array exec) so a hostile ambient env
cannot hijack them. `clean` is **`-fd`, never `-x`** — ignored secrets, caches,
and the separate warm `self_deploy_target_dir()` survive.

## 2. Fail-closed canonical-only gate

The scrub is destructive, so it is gated twice:

1. At each call site by `is_canonical_src_repo(&repo)`, which canonicalizes both
   `repo` and `self_deploy_src_dir()` and returns `true` only on an exact match.
   If either path cannot be canonicalized (e.g. the canonical checkout does not
   exist because this is a `SIMARD_SELF_DEPLOY_REPO` override run) the answer is
   `false` — **fail closed**: never reset a tree we cannot prove is the
   throwaway checkout.
2. Inside `reset_source_tree` as defense in depth: it re-canonicalizes `repo`
   and `self_deploy_src_dir()` and returns `CheckoutFailed { detail }` on any
   canonicalize error or mismatch **without** running reset/clean.

A dirty non-canonical override is therefore never scrubbed; it still fails
loudly at `checkout --detach`, preserving the pre-fix behaviour for overrides.

## 3. Clone-clean recovery for an absent/invalid checkout

When the canonical checkout is absent or is not a valid git work tree (a clone
killed mid-way, a leftover non-git directory, or a dangling symlink), the
preparer recovers to a **known-clean** tree by re-cloning from origin rather
than by resetting an unverified path:

- `resolve_repo` returns early when `self_deploy_src_dir()` is already a valid
  work tree; otherwise `clone_from_origin` is reached.
- `clone_from_origin` calls the idempotent `remove_stale_checkout` to tear down
  the stale path (a missing path is a no-op, never an error), then
  transport-validates the origin URL and `git clone`s a pristine tree.
- A freshly cloned tree is pristine by construction, so the fail-closed gate is
  never asked to reset an unverified path. The warm `self_deploy_target_dir()`
  is untouched, so builds stay incremental.

## 4. Git-tracked / drift-free CI invariant

Recovering at deploy time is necessary but not sufficient — the drift *source*
must stay honest. [`tests/self_deploy_hooks_tracked_invariant.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_deploy_hooks_tracked_invariant.rs)
asserts, in a git work tree (it skips cleanly in vendored/packaged builds):

- **Every file under `.github/hooks/` — including `amplihack-hooks.json` — is
  git-tracked.** Untracking / gitignoring the manifest is explicitly **not** the
  fix (supply-chain integrity; the manifest and its hook scripts stay
  reviewable).
- **A fresh checkout leaves `.github/hooks/` pristine** (no untracked drift), so
  a reappearing unconditional manifest rewrite turns CI red instead of silently
  re-wedging self-deploy.

## Observability

The scrub logs a `tracing::debug!` on `self_deploy` before it runs
(`"resetting disposable self-deploy source checkout before checkout"`, with the
canonical path). Every failure — a refused gate, a failed reset/clean, or a
checkout that still fails — surfaces **loudly** as a `CheckoutFailed { detail }`
error up the deploy stack; the `detail` is routed through `redact_credentials`
so no tokens, env dumps, or credentialed URLs are emitted. There is no silent
degrade: a scrub the gate refuses aborts the deploy rather than resetting an
unverified tree.

## Error surface

No new `SafeUpdateError` variants. Failures reuse the existing surface from the
[source-prep reference](./self-deploy-source-prep.md):

| Variant | When |
| --- | --- |
| `CheckoutFailed { detail }` | SHA validation failed, the gated reset was refused (non-canonical / un-canonicalizable path) or its `reset`/`clean` failed, or `checkout --detach` failed. `detail` is redacted. |
| `SourceResolveFailed { detail }` | The canonical repo could not be resolved and the clone-clean re-clone also failed. |

## Security model

| Control | Enforcement |
| --- | --- |
| **Reset only the disposable managed clone** | Double-canonicalized equality with `self_deploy_src_dir()` at the call site (`is_canonical_src_repo`) and again inside `reset_source_tree`; any error/mismatch refuses the reset (fail-closed). A `SIMARD_SELF_DEPLOY_REPO` override resolves to a different canonical path, so its tree is **never** reset — a dirty override still fails loud on checkout. |
| **Keep `.github/hooks/` tracked** | The git-tracked / no-drift invariant test makes untracking or a reappearing unconditional rewrite red. |
| **`clean -fd`, never `-x`** | Ignored secrets/caches and the warm target dir survive the scrub. |
| **No shell, no injection** | Both git invocations use the env-scrubbed argv-array `git_capture`; the validated full SHA is pinned to `checkout --detach` so a skipped fetch can never check out a different tree (SEC-I2). |
| **Forward-only swap intact** | The recovery paths do not bypass the ancestry oracle or the `self_deploy_canary` forward-only swap gates. |

## Testing

Covered by
[`src/self_deploy/tests_source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/tests_source_prep.rs)
and
[`tests/self_deploy_hooks_tracked_invariant.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_deploy_hooks_tracked_invariant.rs):

| Test | Asserts |
| --- | --- |
| `prepare_resets_dirty_canonical_checkout_before_checking_out_merged_head` | A managed clone with a modified tracked `amplihack-hooks.json` + an untracked stray checks out the merged head cleanly (reset + `clean -fd`). |
| `prepare_resets_before_checkout_even_on_the_skip_fetch_present_commit_branch` | The reset guards the skip-fetch (commit-already-present) branch too — proven with origin destroyed so no fetch is possible. |
| `resetting_the_source_checkout_never_touches_the_warm_target_dir` | The warm `self_deploy_target_dir()` sentinel survives the source reset. |
| `dirty_non_canonical_override_is_not_reset_and_still_fails_loud_at_checkout` | A dirty `SIMARD_SELF_DEPLOY_REPO` override is **not** scrubbed and fails loud at checkout; its local edit + untracked stray survive. |
| `hooks_manifest_and_scripts_are_git_tracked` | Every file under `.github/hooks/` — including `amplihack-hooks.json` — is git-tracked. |
| `hooks_dir_has_no_untracked_drift_in_a_clean_checkout` | A fresh checkout leaves `.github/hooks/` pristine (CI drift ⇒ red). |

Run:

```bash
cargo test -p simard self_deploy::tests_source_prep
cargo test --test self_deploy_hooks_tracked_invariant
# canary gate must stay green:
cargo test --test self_deploy_canary --features canary-tests
```
