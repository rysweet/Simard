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

### 5. Modular, regeneratable components — bricks and studs

Each subsystem (memory, OODA daemon, base types, gym) is a **brick** with clear edges and a narrow public contract — its **studs**. A brick is rewritable in isolation without rewriting its neighbors, because the neighbors only see the studs. In practice this means:

- Public modules expose small, documented surfaces. Internal details stay `pub(crate)` or private.
- Cross-subsystem calls go through named types, not through shared mutable state.
- A brick can be regenerated (deleted and re-authored, possibly by an LLM) from its spec + its stud contract without breaking callers.

### 6. Ruthless simplicity is hierarchical

Simplicity is applied top-down, not uniformly:

1. First, simplify the **architecture** — fewer subsystems, narrower interfaces, less coupling.
2. Then simplify the **control flow** — fewer branches, explicit states, no silent fallbacks.
3. Then simplify the **data shapes** — fewer optional fields, fewer nullable indirections.
4. Only then simplify **syntax** — shorter names, fewer lines.

Optimizing step (4) while leaving (1)-(3) complex is anti-simplification.

### 7. Formal specification as a thinking tool

For any requirement that involves concurrency, multi-actor state, or multi-step invariants, write the invariant as a **formal predicate** (e.g. `failedAgents ≠ {} ⟹ phase ≠ complete`) or a **Gherkin scenario** before writing code. The predicate is the bar. The code either upholds it or does not. Prose requirements hide ambiguity that formal predicates expose.

### 6. Honesty about gaps

Simard is the successor to amplihack and is *on the path* to replacement — not finished with it. Docs, READMEs, and prompts say so plainly. See [amplihack-comparison.md](amplihack-comparison.md) for the honest gap ledger.

## What this looks like in practice

- Commit messages describe the behavior change, not the author's emotional journey.
- New modules come with tests before they come with docs.
- "Fallback" is a suspicious word — if a path is silently taken when the primary fails, something is wrong, and the user should know.
- When designing a new capability, ask first: does amplihack already solve this? Do we need it? Can it be a flag?

## Origins

Most of these principles come directly from amplihack's `PHILOSOPHY.md` and `TRUST.md`. Simard keeps them because they work. See also the `AGENTS.md` file at the repository root for role-specific guidance.

## Next

- [Patterns](patterns.md)
- [Workflows](workflows.md)
