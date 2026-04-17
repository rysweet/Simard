# Philosophy

Simard inherits amplihack's engineering philosophy and adapts it for a Rust-native, terminal-first agent. The short version: **ruthless simplicity, evidence over narrative, honesty over spin.**

## Core principles

### 1. Ruthless simplicity

Prefer the smallest change that solves the problem. Avoid speculative abstractions. When in doubt, delete code. If a feature can be expressed as a CLI flag on an existing command instead of a new subsystem, use the flag.

### 2. Evidence over narrative

Claims about behavior require artifacts: tests, logs, file paths, or commit hashes. A PR description that says "this makes X faster" without a benchmark, or "this fixes the bug" without a failing test and a passing test, is incomplete.

### 3. Working code only — no stubs, no placeholders

Everything committed to `main` must actually do its job. If a function's body is `todo!()`, the feature is not done. If a "migration" leaves the old code in place behind a feature flag forever, it is not a migration.

### 4. Zero-BS error reporting

When something fails, report the specific failure: the error, the input that caused it, the file/line. Never say "something went wrong." Never silently degrade. If Simard can't do the job, she says so, with the reason.

### 5. Modular, regeneratable components

Each subsystem (memory, OODA daemon, base types, gym) is a brick with clear edges. Any brick should be rewritable in isolation without rewriting its neighbors.

### 6. Honesty about gaps

Simard is the successor to amplihack and is *on the path* to replacement — not finished with it. Docs, READMEs, and prompts say so plainly. See [amplihack-comparison.md](amplihack-comparison.md) for the honest gap ledger.

## What this looks like in practice

- Commit messages describe the behavior change, not the author's emotional journey.
- New modules come with tests before they come with docs.
- "Fallback" is a suspicious word — if a path is silently taken when the primary fails, something is wrong, and the user should know.
- When designing a new capability, ask first: does amplihack already solve this? Do we need it? Can it be a flag?

## Origins

Most of these principles come directly from amplihack's `PHILOSOPHY.md` and `TRUST.md`. Simard keeps them because they work. See also [AGENTS.md](../AGENTS.md) for role-specific guidance.
