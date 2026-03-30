---
title: Runtime contracts reference
description: Reference for the shipped Simard executable surfaces, the shared-state-root bridge between bounded terminal sessions and the repo-grounded engineer loop, the `engineer read` audit contract, compatibility binaries, and the in-process runtime contracts that back them.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./simard-cli.md
  - ../howto/inspect-meeting-records.md
  - ../howto/inspect-improvement-curation-state.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../howto/move-from-terminal-recipes-into-engineer-runs.md
  - ../tutorials/run-your-first-local-session.md
---

# Runtime contracts reference

This document covers:

- the canonical `simard` CLI surface
- the `engineer read` audit surface
- the compatibility binaries that still expose a few legacy entrypoints
- the in-process Rust runtime and bootstrap types in `src/bootstrap.rs`, `src/runtime.rs`, and related modules

Simard does **not** expose:

- an HTTP API
- a network service contract
- a database schema contract

## Executable surfaces

| Runtime behavior | Canonical surface | Compatibility surface |
| --- | --- | --- |
| explicit bootstrap | `simard bootstrap run ...` | `simard_operator_probe bootstrap-run ...` |
| bounded engineer loop | `simard engineer run ...` | `simard_operator_probe engineer-loop-run ...` |
| engineer state readback | `simard engineer read ...` | `simard_operator_probe engineer-read ...` |
| terminal-backed engineer substrate | `simard engineer terminal ...` | `simard_operator_probe terminal-run ...` |
| file-backed terminal engineer substrate | `simard engineer terminal-file ...` | `simard_operator_probe terminal-run-file ...` |
| named terminal recipe execution | `simard engineer terminal-recipe ...` | `simard_operator_probe terminal-recipe-run ...` |
| terminal session readback | `simard engineer terminal-read ...` | `simard_operator_probe terminal-read ...` |
| meeting mode | `simard meeting run ...` | `simard_operator_probe meeting-run ...` |
| meeting state readback | `simard meeting read ...` | `simard_operator_probe meeting-read ...` |
| goal-curation mode | `simard goal-curation run ...` | `simard_operator_probe goal-curation-run ...` |
| goal-curation state readback | `simard goal-curation read ...` | none |
| improvement-curation mode | `simard improvement-curation run ...` | `simard_operator_probe improvement-curation-run ...` |
| improvement-curation state readback | `simard improvement-curation read ...` | `simard_operator_probe improvement-curation-read ...` |
| review artifact persistence and readback | `simard review ...` | `simard_operator_probe review-run ...` and `review-read ...` |
| benchmark scenarios and suites | `simard gym ...` | `simard-gym ...` |

## Canonical CLI surface

The shipped operator-facing command tree is:

- `simard engineer run <topology> <workspace-root> <objective> [state-root]`
- `simard engineer read <topology> [state-root]`
- `simard engineer terminal <topology> <objective> [state-root]`
- `simard engineer terminal-file <topology> <objective-file> [state-root]`
- `simard engineer terminal-recipe-list`
- `simard engineer terminal-recipe-show <recipe-name>`
- `simard engineer terminal-recipe <topology> <recipe-name> [state-root]`
- `simard engineer terminal-read <topology> [state-root]`
- `simard meeting run <base-type> <topology> <structured-objective> [state-root]`
- `simard meeting read <base-type> <topology> [state-root]`
- `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard goal-curation read <base-type> <topology> [state-root]`
- `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard improvement-curation read <base-type> <topology> [state-root]`
- `simard gym list`
- `simard gym run <scenario-id>`
- `simard gym compare <scenario-id>`
- `simard gym run-suite <suite-id>`
- `simard review run <base-type> <topology> <objective> [state-root]`
- `simard review read <base-type> <topology> [state-root]`
- `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Bare `simard` prints help for that tree instead of attempting a hidden bootstrap fallback.

## Shared state-root contract

Whenever a command accepts `[state-root]`, Simard validates it before any persistence work tied to durable operator state.

Rejected inputs include:

- any path containing `..`
- an existing path that is not a directory
- a symlink root

Safe roots are canonicalized once and then reused throughout the command.

## Mode-scoped handoff artifacts

When terminal session surfaces and the repo-grounded engineer loop reuse one explicit `state-root`, Simard keeps readback truthful by writing mode-scoped handoff artifacts:

- `latest_terminal_handoff.json`
- `latest_engineer_handoff.json`
- `latest_handoff.json` as the compatibility fallback

Those files have different jobs:

- terminal session surfaces write the latest bounded terminal continuity summary
- engineer runs write the latest repo-grounded engineer summary
- compatibility readers can still inspect `latest_handoff.json` when a mode-scoped file has not been written yet

Fail-closed rules:

- `terminal-read` prefers `latest_terminal_handoff.json`
- `engineer read` prefers `latest_engineer_handoff.json`
- fallback to `latest_handoff.json` is allowed only when the mode-specific file is absent
- if a mode-specific handoff exists but is malformed, the command fails explicitly instead of silently replaying stale compatibility data

The bridge is descriptive continuity only. Terminal-derived data must never override `workspace-root`, the engineer objective, action selection, or verification.

## Mode contracts

### Engineer mode

Canonical entrypoint: `simard engineer run <topology> <workspace-root> <objective> [state-root]`

Compatibility surface: `simard_operator_probe engineer-loop-run <topology> <workspace-root> <objective> [state-root]`

The bounded engineer loop is intentionally narrow:

- it inspects the selected repo before acting
- it prints a short action plan and explicit verification steps
- it chooses one bounded local action
- it verifies the result explicitly
- it persists concise evidence and memory under the selected state root
- it surfaces active goals and up to the three most recent carried meeting records from the same state root
- it may surface a separate terminal continuity summary when the same state root already contains a valid terminal-scoped handoff

The bounded engineer loop supports two honest action shapes:

- a read-only repo-native scan such as `cargo-metadata-scan` or `git-tracked-file-scan`
- one explicit structured text replacement on a clean repo when the objective includes all of:
  - `edit-file: <repo-relative path>`
  - `replace: <existing text>`
  - `with: <replacement text>`
  - `verify-contains: <required post-edit text>`

That structured edit path is intentionally narrow:

- the target path must stay inside the selected repo
- the repo must start clean so Simard does not overwrite unrelated user changes
- only one expected changed file is allowed
- verification must confirm both file content and git-visible change state

#### Engineer state readback

Canonical entrypoint: `simard engineer read <topology> [state-root]`

Compatibility surface: `simard_operator_probe engineer-read <topology> [state-root]`

This is a read-only engineer audit surface, not a sixth operator mode. It exists to inspect the durable engineer artifacts that `engineer run` already writes.

The contract is intentionally explicit:

- `engineer run` remains the only mutation and execution path for engineer work
- `engineer read` reuses the same validated default state root as `engineer run` when `[state-root]` is omitted
- any explicit `state-root` must already exist as a directory before readback begins
- `engineer read` requires readable regular-file `memory_records.json` and `evidence_records.json`; symlinked artifacts are rejected
- `latest_engineer_handoff.json` is authoritative for identity, selected base type, topology, session phase, redacted objective metadata, and the exported memory/evidence snapshot tied to the latest engineer run
- `latest_handoff.json` is used only as a compatibility fallback when the engineer-scoped handoff is absent
- the rendered output includes which handoff file was used
- persisted handoff objective metadata must already be trusted `objective-metadata(chars=<n>, words=<n>, lines=<n>)`; malformed or tampered metadata fails instead of being replayed
- standalone `memory_records.json` and `evidence_records.json` files act as durability checks and supporting record-count sources; if they disagree with the handoff snapshot, handoff-derived values win
- only redacted objective metadata is printable; raw engineer objective text must never be rendered back to the terminal
- carried meeting state must remain valid persisted meeting records; malformed carried-meeting data fails explicitly instead of falling back to raw strings
- when the same state root contains a valid terminal-scoped handoff, `engineer read` renders a distinct terminal continuity section with sanitized fields such as recipe source, working directory, last output line, and transcript preview
- operator-visible strings are sanitized before printing so terminal control sequences and secret-shaped values are not replayed
- output order stays deterministic: runtime header, handoff session summary, repo grounding, carried context, terminal continuity, selected action summary, verification summary, durable record counts
- the command performs no mutation, repair, resume, or execution
- invalid state roots, missing files, unreadable storage, and malformed persisted engineer data fail explicitly

The default root remains the same engineer durable path already used by `engineer run`:

```text
target/operator-probe-state/engineer-loop-run/simard-engineer/terminal-shell/<topology>
```

### Terminal-backed engineer substrate

Canonical entrypoint: `simard engineer terminal <topology> <objective> [state-root]`

Compatibility surface: `simard_operator_probe terminal-run <topology> <objective> [state-root]`

This substrate exposes the real `terminal-shell` base type on the primary CLI:

- the selected base type remains `terminal-shell`
- reflection still reports `terminal-shell::local-pty` as the adapter implementation
- terminal objectives may include `command:` / `input:` lines and explicit `wait-for:` / `expect:` checkpoints so bounded interactive terminal turns can pause for expected output before sending the next line
- the run surface now renders the same structured terminal audit shape as `terminal-read`: shell details, command/wait counts, ordered steps, satisfied checkpoints, last meaningful output line, and sanitized transcript preview
- the run surface prints explicit boundary guidance showing that terminal mode is a bounded local session surface and that continuing into engineer mode still requires an explicit later `engineer run`
- the run persists `latest_terminal_handoff.json` plus compatibility `latest_handoff.json` under the shared root
- unsatisfied wait checkpoints fail explicitly instead of silently replaying the rest of the objective and claiming success
- unsupported topology and invalid state-root choices still fail explicitly

#### Terminal session readback

Canonical entrypoint: `simard engineer terminal-read <topology> [state-root]`

Compatibility surface: `simard_operator_probe terminal-read <topology> [state-root]`

This is the read-only audit companion to `simard engineer terminal`. It exists to inspect the durable terminal-session artifacts that `engineer terminal` already writes.

The contract is intentionally explicit:

- `engineer terminal` remains the only execution path for terminal-backed engineer work
- `terminal-read` reuses the same validated default state root as `engineer terminal` when `[state-root]` is omitted
- any explicit `state-root` must already exist as a directory before readback begins
- `terminal-read` requires readable regular-file `memory_records.json` and `evidence_records.json`; symlinked artifacts are rejected
- `latest_terminal_handoff.json` is authoritative for identity, selected base type, topology, session phase, redacted objective metadata, and the persisted terminal evidence summary tied to the latest terminal session
- `latest_handoff.json` is used only as a compatibility fallback when the terminal-scoped handoff is absent
- the rendered output includes which handoff file was used
- persisted terminal evidence may include ordered terminal step lines, satisfied wait checkpoints, and the last observed output line so operators can inspect how a bounded interactive session was driven instead of relying only on aggregate counters
- the rendered output also includes explicit engineer-next-step guidance so the bridge into repo-grounded engineer work stays operator-visible
- operator-visible strings are sanitized before printing so persisted terminal control sequences and secret-shaped values are not replayed
- output order stays deterministic: runtime header, handoff session summary, adapter details, shell details, step/checkpoint audit trail, transcript summary, continuation guidance, durable record counts
- the command performs no mutation, replay, resume, or execution
- invalid state roots, missing files, unreadable storage, and malformed persisted terminal data fail explicitly

#### File-backed terminal session execution

Canonical entrypoint: `simard engineer terminal-file <topology> <objective-file> [state-root]`

Compatibility surface: `simard_operator_probe terminal-run-file <topology> <objective-file> [state-root]`

This is the reusable authoring companion to `simard engineer terminal`.

- it executes the same bounded `terminal-shell` substrate as `engineer terminal`
- `<objective-file>` must exist as a readable UTF-8 regular file; symlinks and other file kinds are rejected explicitly
- the loaded file contents remain subject to the same shell validation, wait-checkpoint behavior, sanitization, and durable state contracts as inline terminal objectives
- the run surface and `terminal-read` continue to present the same structured terminal audit trail

#### Named terminal recipe execution

Canonical entrypoints:

- `simard engineer terminal-recipe-list`
- `simard engineer terminal-recipe-show <recipe-name>`
- `simard engineer terminal-recipe <topology> <recipe-name> [state-root]`

Compatibility surface:

- `simard_operator_probe terminal-recipe-list`
- `simard_operator_probe terminal-recipe-show <recipe-name>`
- `simard_operator_probe terminal-recipe-run <topology> <recipe-name> [state-root]`

This is the discoverable built-in recipe companion to inline and file-backed terminal execution.

- recipes live under `prompt_assets/simard/terminal_recipes/*.simard-terminal`
- recipe names are limited to lowercase ASCII letters, digits, and hyphens
- unknown or invalid recipe names fail explicitly
- `terminal-recipe-show` is read-only and prints the shipped recipe asset contents
- `terminal-recipe` executes the same bounded `terminal-shell` substrate as `engineer terminal` and `engineer terminal-file`
- the shipped built-ins currently include `foundation-check` and `copilot-status-check`
- `copilot-status-check` is a bounded local availability probe only: it runs `amplihack copilot -- --version`, requires the expected `GitHub Copilot CLI` version signal, and fails closed when that signal cannot be produced
- `copilot-status-check` must not claim interactive Copilot control, remote orchestration, or authenticated GitHub state inspection
- the selected recipe name is preserved into the terminal continuity summary that later engineer surfaces may render from the same state root

### Meeting mode

Canonical entrypoints:

- `simard meeting run <base-type> <topology> <structured-objective> [state-root]`
- `simard meeting read <base-type> <topology> [state-root]`

Compatibility surface: `simard_operator_probe meeting-run ...` and `simard_operator_probe meeting-read ...`

Meeting mode persists concise durable planning data without touching repository contents. Structured lines may include:

- `agenda: ...`
- `update: ...`
- `decision: ...`
- `risk: ...`
- `next-step: ...`
- `open-question: ...`
- `goal: title | priority=1 | status=active | rationale=...`

The shipped contract is intentionally explicit:

- `meeting run` remains the only mutation path for agenda, update, decision, risk, next-step, open-question, and goal-update capture
- `meeting read` is the read-only audit surface for the latest persisted meeting decision state
- both commands reuse the same validated state root, and both default to the same canonical durable root for `simard-meeting`
- `meeting read` loads the latest persisted meeting decision record, where "latest" means the last decision memory record whose value matches the shipped meeting record shape
- `meeting read` renders agenda, updates, decisions, risks, next steps, open questions, goal updates, and the latest raw meeting record in a stable operator-visible order
- operator-visible strings are sanitized before printing so persisted terminal control sequences are not replayed
- invalid state roots, missing memory state, unreadable storage, and malformed persisted meeting data fail explicitly

### Goal-curation mode

Canonical entrypoints:

- `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard goal-curation read <base-type> <topology> [state-root]`

Compatibility surface: `simard_operator_probe goal-curation-run <base-type> <topology> <structured-objective> [state-root]`

Goal-curation mode maintains durable backlog records and the active top five goals. The readback command exposes the stored goal register from the same validated state root without mutating it.

### Improvement-curation mode

Canonical entrypoints:

- `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard improvement-curation read <base-type> <topology> [state-root]`

Compatibility surface: `simard_operator_probe improvement-curation-run ...` and `simard_operator_probe improvement-curation-read ...`

Improvement-curation mode promotes approved review proposals into durable priorities and keeps deferred proposals inspectable instead of silently self-modifying.

The shipped contract is intentionally explicit:

- `improvement-curation run` remains the only mutation path for approving or deferring proposals
- `improvement-curation read` is the read-only audit surface for the latest review-to-priority promotion state
- both commands reuse the same validated state root, and both default to the same canonical durable root as `review run`
- `improvement-curation read` loads the latest persisted review artifact, where "latest" means the artifact with the highest `reviewed_at_unix_ms`
- `improvement-curation read` loads the latest persisted improvement decision record, where "latest" means the last decision memory record whose key ends with `improvement-curation-record`
- `improvement-curation read` renders approved proposals, deferred proposals, active goals, proposed goals, and the latest improvement decision record in a stable operator-visible order
- operator-visible strings are sanitized before printing so persisted terminal control sequences are not replayed
- invalid state roots, missing review artifacts, missing improvement records, unreadable storage, and malformed persisted decision data fail explicitly

```text
target/operator-probe-state/review-run/simard-engineer/<base-type>/<topology>
```

### Review mode

Canonical entrypoints:

- `simard review run <base-type> <topology> <objective> [state-root]`
- `simard review read <base-type> <topology> [state-root]`

Compatibility surface: `simard_operator_probe review-run ...` and `simard_operator_probe review-read ...`

Review mode persists a structured review artifact and makes the latest artifact readable from the same durable state root.

### Benchmark gym

Canonical entrypoints:

- `simard gym list`
- `simard gym run <scenario-id>`
- `simard gym compare <scenario-id>`
- `simard gym run-suite <suite-id>`

Compatibility surface: `simard-gym ...`

The gym currently benchmarks scenarios built around:

- repo exploration and truthful local inspection
- docs refresh flow
- safe code change flow through the `rusty-clawd` identity
- composite session review

Artifacts are written under `target/simard-gym/` as JSON and text reports plus a `review.json` artifact for each scenario run.

> [!IMPORTANT]
> Fresh benchmark runs now derive `unnecessary_action_count` and `retry_count` from benchmark-controlled attempt and action facts captured by the gym runner. Older artifacts, or any future report that lacks enough benchmark facts, should surface `unmeasured` instead of inventing `0`.

Fresh per-run reports expose these public scorecard fields under `scorecard`:

- `correctness_checks_passed`
- `correctness_checks_total`
- `evidence_quality`
- `unnecessary_action_count`
- `retry_count`
- `human_review_notes`
- `measurement_notes`

The current counting boundary is:

- `unnecessary_action_count`: benchmark-runner-observed benchmark-controlled action boundaries that do not advance the intended scenario execution or verification path
- `retry_count`: benchmark-runner-observed re-attempts of the same scenario work inside one benchmark run

Fresh benchmark runs persist those derived values in `report.json`, surface them on the CLI, and stop generating review proposals, `human_review_notes`, or `measurement_notes` that claim the metrics are "not measured". Older or incomplete artifacts should render `unmeasured` instead of a fabricated zero.

The gym also supports persisted run-to-run comparison for a single scenario:

- `simard gym compare <scenario-id>` compares the latest two completed runs
- comparison results are classified as `improved`, `unchanged`, or `regressed`
- comparison output includes current, previous, and delta values for `unnecessary_action_count` and `retry_count`
- if one side of the comparison comes from an older artifact that lacks either field, compare renders that value and its delta as `unmeasured` instead of inventing `0`
- comparison artifacts are written under `target/simard-gym/comparisons/<scenario-id>/`

### Bootstrap contract

Canonical entrypoint: `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Compatibility surface: `simard_operator_probe bootstrap-run <identity> <base-type> <topology> <objective> [state-root]`

The operator-facing bootstrap contract is now explicit:

- required values are passed positionally
- identity, base type, and topology mismatches fail explicitly
- there is no public zero-argument fallback path
- state-root validation runs before durable artifacts are read or written

## Durable carryover contract

Meeting, engineer, goal-curation, review, and improvement-curation commands can all share one explicit state root.

That shared state root is what allows:

- carried meeting decisions to appear in later engineer runs
- durable goals to stay visible across operator modes
- review artifacts to feed improvement-curation
- improvement-curation decisions to stay durable and remain auditable through the shipped `improvement-curation read` surface

The contract depends on passing the same validated state root across commands, not on hidden global state.

## Identity and backend contract

The builtin identities currently advertised by the loader are `simard-engineer`, `simard-meeting`, `simard-goal-curator`, `simard-improvement-curator`, `simard-gym`, and the composite `simard-composite-engineer`. All of them accept `local-harness`, `rusty-clawd`, and `copilot-sdk`; `simard-engineer` additionally accepts `terminal-shell` for the local terminal-backed path.

Reflection reports both the selected base type and the honest backend identity. For example:

- `copilot-sdk` currently resolves to the `local-harness` adapter implementation
- `terminal-shell` reports `terminal-shell::local-pty`
- `rusty-clawd` reports `rusty-clawd::session-backend`

## See also

- [Simard CLI reference](./simard-cli.md)
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)
