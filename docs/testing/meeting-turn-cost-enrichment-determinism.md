---
title: Meeting-turn cost-enrichment test determinism
description: >
  How the copilot meeting-turn cost-accounting test
  (meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective) stays
  green and deterministic across CI, and how to remediate the stale-branch CI
  failures it produced on open PRs. Covers the #4164 enrichment invariant, the
  HOME/ledger isolation contract, and the rebase-first remediation.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ../../src/base_type_copilot/mod.rs
  - ../../src/base_type_copilot/tests.rs
---

# Meeting-turn cost-enrichment test determinism

> **Status: implemented.** The enrichment fix (`prompt_chars = formatted.len()`
> at [`src/base_type_copilot/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/base_type_copilot/mod.rs))
> and the isolation contract for
> `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` in
> [`src/base_type_copilot/tests.rs`](https://github.com/rysweet/Simard/blob/main/src/base_type_copilot/tests.rs)
> live on `main` today. `main` is green. This document specifies why the test is
> deterministic and how to clear the CI failures it surfaced on **stale** PR
> branches.

This guide is for anyone triaging a `pre-commit` (verify) or `coverage` CI
failure on an open PR where the failing test is:

```text
base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective ... FAILED
```

The headline: **`main` is green.** The failure is a stale-branch artifact, not a
regression on `main`. The primary remediation is a rebase, not a code change.

## The enrichment invariant (#4164)

A copilot **meeting** turn streams an *enriched* prompt to copilot on stdin —
the preamble + identity context + the objective wrapped in the
`## Objective` / `## Instructions` scaffold. The cost ledger must record the size
of that **enriched** prompt, not the bare objective:

```rust
// src/base_type_copilot/mod.rs — the enriched string is the true prompt size.
let prompt_chars = formatted.len();      // ✅ enriched preamble + scaffold
// NOT: let prompt_chars = input.objective.len();  // ✗ undercounts by tens of KB
```

Recording `input.objective.len()` undercounted meeting prompt tokens (often by
tens of KB), inverting the Cost tab's prompt/completion ratio and understating
spend. This is the invariant the test guards.

## The test and its isolation contract

`meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` asserts the
recorded `prompt_tokens_est` exceeds the token estimate of the **bare**
objective — proving the enriched length was recorded. It is deterministic
because it is fully hermetic and isolated:

- **`#[serial_test::serial(cognitive_memory)]`** — serialized against every other
  test that mutates the shared `cognitive_memory` env, so no concurrent test can
  tear the `HOME` write. See
  [cognitive-memory-serial-isolation](./cognitive-memory-serial-isolation.md).
- **Per-test `HOME`** — a fresh `tempfile::TempDir` is set as `HOME`, so the cost
  ledger is written to `$HOME/.simard/costs/ledger.jsonl` under the temp dir,
  never the developer's or CI runner's real ledger.
- **Guaranteed `HOME` restore** — the previous `HOME` is captured before the
  test and restored **before** any panic is re-propagated (`catch_unwind` +
  restore + `resume_unwind`), so a failing assertion can never leak a mutated
  `HOME` into the next serial test.
- **Fake copilot binary** — `fake_copilot(...)` provides a deterministic reply;
  the test never shells out to a real `copilot` on PATH.

These four properties are the determinism contract. Any change to this test must
preserve all of them: keep the `#[serial(cognitive_memory)]` key, keep the
temp-`HOME` isolation, and keep the guaranteed restore-before-resume ordering.

## Why open PRs failed while `main` is green

The `#4164` enrichment fix and the test landed together on `main`. PR branches
cut **before** that merge do not contain the fix, so the test — once the branch
picks it up in a merge, or once CI runs the merged view — fails on the stale
side. The signature is unmistakable: **PR #4359 showed both a `pre-commit`
FAILURE and a `pre-commit` SUCCESS in the same run set** — the classic
stale-branch (needs-rebase) pattern, not a flaky test.

## Remediation: rebase first, edit only if it reproduces

Apply the cheapest correct fix in order.

### Step 1 — Rebase the affected branch onto `main`

Pull the trusted, green `main` **into** the PR branch (fast-forward the trusted
base), then re-run CI:

```console
$ git fetch origin
$ git checkout <pr-branch>
$ git rebase origin/main        # or: git merge origin/main
$ git push --force-with-lease
```

Re-trigger the required checks. For the great majority of the affected PRs
(`4379, 4369, 4359, 4355, 4354, 4331, 4328, 4325, 4324, 4322` on `pre-commit`;
`4366, 4331, 4230` on `coverage`) this alone turns the checks green, because the
branch now contains the `#4164` fix.

> Rebase the **trusted `main` into the PR** — never merge PR content into `main`
> to "fix" validation. The direction matters: `main` is the trusted base.

### Step 2 — Reproduce on the rebased branch before touching code

Only if the test **still** fails on a freshly-rebased branch is a code change
warranted. Reproduce locally:

```console
$ cargo test -p simard --lib \
    base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective \
    -- --exact
```

(The test is a `#[cfg(unix)]` **lib** unit test in `src/base_type_copilot/tests.rs`,
so use `--lib` — `--test '*'` targets only integration tests under `tests/` and
will not run it.)

- **Passes on rebase** → confirmed stale-branch. Done. No production edit.
- **Fails on rebase** → a genuine regression or a determinism gap. Proceed to
  Step 3.

### Step 3 — Harden determinism (only if it reproduces)

If and only if the test reproduces on a rebased branch:

- **Do not** weaken or delete the assertion (`recorded > bare_objective_tokens`).
- **Do not** edit the already-correct `prompt_chars = formatted.len()` at
  `mod.rs`.
- **Do** harden the test's isolation in `tests.rs` — tighten the HOME/ledger
  isolation under `#[serial(cognitive_memory)]`, preferring an RAII guard that
  restores `HOME` on drop so the restore is unconditional even on early return
  or panic.

Fix the *determinism*, preserve the *invariant*.

## Guardrails

- Additive / non-breaking; the enrichment invariant and the assertion are
  preserved, never weakened to force a green check.
- No `print!`/`println!` in new code; use `tracing` for any new diagnostics. The
  pre-existing cost-write-failure `eprintln!` in `mod.rs` is intentionally left
  untouched (the rule applies to new code).
- Keep the `#[serial(cognitive_memory)]` key and the guaranteed HOME restore —
  a leaked mutated `HOME` is a data-integrity hazard for the shared ledger, not
  merely a flake.

## See also

- [Cognitive-memory serial isolation](./cognitive-memory-serial-isolation.md)
- [Deflaking known flaky tests](./deflaking-known-flaky-tests.md)
- [Hermetic tests](./hermetic-tests.md)
- [CI-resilient test patterns](./ci-resilient-test-patterns.md)
