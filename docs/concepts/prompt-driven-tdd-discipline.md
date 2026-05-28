# Prompt-Driven TDD Discipline

Simard enforces Test-Driven Development commit ordering through **prompt
instructions** in the engineer system prompt, not through CI scripts or git
history parsing. The instruction is in
`prompt_assets/simard/engineer_system.md` under "Quality Standards."

## Why Prompt-Based, Not CI-Based

An earlier approach attempted to enforce TDD ordering with a bash CI gate
script (`check-tdd-ordering.sh`) that parsed `git log` output to verify test
commits appeared before implementation commits. This failed for several
reasons:

| Problem | Detail |
|---------|--------|
| **Brittleness** | Parsing `git log --name-only` for file-path patterns (`*test*`, `*spec*`) is fragile. Renamed files, non-standard test locations, and mixed commits all produce false positives/negatives. |
| **Squash commits** | Most PRs land as squash merges, collapsing the test-first → implementation-second commit sequence into a single commit. The script cannot verify ordering inside a squash. |
| **Scope creep** | The script must maintain an ever-growing list of path patterns, language-specific test conventions, and edge cases. Each false failure erodes trust in the gate. |
| **Wrong enforcement layer** | TDD is a *development practice* — it shapes how engineers write code in real time. A post-hoc CI check runs after the work is done and can only reject, not guide. |

The prompt-based approach solves these by instructing the engineer agent at
the point of work: before it writes any code, it knows to write the test
first and commit it separately.

## The Instruction

The exact text in `engineer_system.md` (Quality Standards section):

> **Test-Driven Development (commit ordering)**: Always write tests before
> implementation code. For every feature change, the test commit must come
> before the implementation commit. This means: (1) write a failing test that
> defines the expected behavior, (2) commit the test, (3) write the
> implementation that makes the test pass, (4) commit the implementation.
> This discipline is enforced through this prompt — not through CI scripts or
> git history parsing.

## How It Works in Practice

When the engineer agent receives a goal, the system prompt primes it with the
TDD discipline rule. The agent then follows this workflow for each feature:

```
1. Read the goal / issue requirements
2. Write a failing test that captures the expected behavior
3. git add && git commit  (test commit)
4. Write the implementation that makes the test pass
5. Run the test suite to confirm green
6. git add && git commit  (implementation commit)
7. Push both commits
```

The result is a commit history where test commits naturally precede
implementation commits — not because a script verified it, but because the
agent followed the correct development sequence.

## Design Principles

This approach aligns with Simard's core architecture principle: **iterate on
prompts, not code** (see [prompt-driven brain iteration](prompt-driven-brain-iteration.md)).

- **Prompt-first**: Behavioral rules live in prompt text, not in bash scripts
  or Rust code. Changing the TDD policy is a one-line edit to a markdown file.
- **Session-reloadable**: The engineer system prompt is loaded from disk at
  session start. Editing it changes behavior on the next engineer session —
  no rebuild required. (Unlike the OODA brain prompts, which hot-reload
  mid-cycle via `PromptStore`, the engineer prompt reloads per session.)
- **No false failures**: There is no CI gate to produce false positives.
  The discipline is intrinsic to how the agent works.
- **Auditable**: Commit history naturally shows the test-first pattern. Code
  reviewers can verify TDD ordering during PR review without tooling.

## What Was Removed

The following artifacts from the CI-based approach were removed:

| Artifact | Disposition |
|----------|-------------|
| `adopt-tdd` goal | Removed from goal board |
| `adopt-tdd-for-new-modules` goal | Removed from goal board |
| `scripts/check-tdd-ordering.sh` | Deleted (if present) |
| TDD CI workflow (`.github/workflows/`) | Deleted (if present) |
| PR #2150 (TDD ordering CI gate) | Closed |
| PR #2151 (TDD ordering CI gate) | Closed |
| Issue #1927 (adopt-tdd charter) | Closed |

## Related

- [Prompt-driven brain iteration](prompt-driven-brain-iteration.md) — how
  OODA brains use the same prompt-first pattern
- [Prompt-driven OODA brain](prompt-driven-ooda-brain.md) — the lifecycle
  decision brain that pioneered prompt-over-code in Simard
- [How-To: Edit the engineer system prompt](../howto/edit-the-engineer-system-prompt.md) —
  step-by-step guide for modifying engineer behavior
