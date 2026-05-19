---
title: Operator read-subcommand state-root contract
description: Reference for the explicit-or-fail `<state-root>` contract on `simard meeting read`, `simard improvement-curation read`, and `simard review read` — including the unified error wording, the SIMARD_STATE_ROOT disclaimer, and the audit rationale tracked under issue #1909 / audit #1910.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./simard-cli.md
  - ./runtime-contracts.md
  - ./state-root-resolution.md
  - ../howto/inspect-meeting-records.md
  - ../howto/inspect-improvement-curation-state.md
---

# Operator read-subcommand state-root contract

This page documents the shipped, explicit-or-fail `<state-root>` contract for
three operator-probe read subcommands:

- `simard meeting read <base-type> <topology> <state-root>`
- `simard improvement-curation read <base-type> <topology> <state-root>`
- `simard review read <base-type> <topology> <state-root>`

These three reads **require** an explicit `<state-root>` positional argument.
There is no implicit fallback. The `SIMARD_STATE_ROOT` environment variable is
intentionally not honored for these reads.

> Tracked under audit issue
> [rysweet/Simard#1910](https://github.com/rysweet/Simard/issues/1910) and
> the targeted fix issue
> [rysweet/Simard#1909](https://github.com/rysweet/Simard/issues/1909).

## Why these reads are explicit-only

Operator-probe read paths are an audit surface. Their value is that the
operator sees the exact durable state that lives under the path they typed.
Two prior fallback shapes silently broke that guarantee:

1. **Synthesized default path.** Earlier builds would, on missing
   `<state-root>`, derive
   `target/operator-probe-state/<probe>/<identity>/<base>/<topology>` and
   read whatever happened to be there. That path can hold leftover state
   from an unrelated run, leftover state from a different working
   directory, or no state at all.
2. **`SIMARD_STATE_ROOT` environment fallback.** `goal-curation read`
   honors `SIMARD_STATE_ROOT` as a convenience for the same operator who
   set it for a daemon. Reusing that convenience for `meeting`,
   `improvement-curation`, and `review` reads meant an operator running
   `simard meeting read local-harness single-process` could be silently
   redirected to a goal-curation state root that contained no meeting
   records, producing either a misleading "empty" report or a confusing
   error far from its real cause.

Both shapes are now blocked at the same resolver layer. The three reads
hard-fail at state-root resolution time, before any storage I/O for the
durable record is attempted.

## Command surface

The synopsis line in `simard --help` and in the operator-facing reference
shows a required positional:

```text
simard meeting read <base-type> <topology> <state-root>
simard improvement-curation read <base-type> <topology> <state-root>
simard review read <base-type> <topology> <state-root>
```

The angle brackets matter. The single-line note below each synopsis in the
help text reads:

> `<state-root>` is required. `SIMARD_STATE_ROOT` is not honored for this
> command.

Clap still parses the positional as optional so that the resolver layer can
produce a richer error than clap would. Operator-visible behavior matches the
"required positional" framing.

## Exit code and error wording

When `<state-root>` is omitted, the command exits with a non-zero status and
prints exactly one structured error line to stderr, using the
`SimardError::MissingRequiredConfig` variant:

```text
error: missing required config 'state-root': state-root is required for `simard <subcommand> read <base-type>`: pass the positional <state-root> argument explicitly. The SIMARD_STATE_ROOT environment variable is not honored for this command.
```

Where:

- `<subcommand>` is one of `meeting`, `improvement-curation`, `review`.
- `<base-type>` is the literal base-type value the operator supplied
  (for example `local-harness`).

The message is stable. Tests under
`tests/issue_1909_state_root_required.rs` assert the substring
`SIMARD_STATE_ROOT environment variable is not honored` to keep the
disclaimer load-bearing.

### Worked failure example

```bash
$ simard meeting read local-harness single-process
error: missing required config 'state-root': state-root is required for `simard meeting read local-harness`: pass the positional <state-root> argument explicitly. The SIMARD_STATE_ROOT environment variable is not honored for this command.
```

The command exits non-zero. The same shape applies for
`improvement-curation read` and `review read`; only the `<subcommand>` token
changes.

### Worked success example

```bash
$ STATE_ROOT="$(mktemp -d /tmp/simard-meeting.XXXXXX)"
$ simard meeting run local-harness single-process \
    "agenda: align next workstream
decision: preserve meeting-to-engineer continuity" \
    "$STATE_ROOT"
…
$ simard meeting read local-harness single-process "$STATE_ROOT"
Probe mode: meeting-read
Identity: simard-meeting
…
```

## Environment-variable disclaimer

Setting `SIMARD_STATE_ROOT` has **no effect** on these three reads:

```bash
$ SIMARD_STATE_ROOT=/tmp/some-other-dir \
  simard improvement-curation read local-harness single-process
error: missing required config 'state-root': state-root is required for `simard improvement-curation read local-harness`: pass the positional <state-root> argument explicitly. The SIMARD_STATE_ROOT environment variable is not honored for this command.
```

This is asserted by
`tests/issue_1909_state_root_required.rs::read_subcommands_ignore_simard_state_root_env_var`.

The environment variable still affects:

- `goal-curation read` (kept for parity with goal-curation operator habits)
- `simard meeting run`, `simard improvement-curation run`,
  `simard review run`, `simard goal-curation run` (the run paths)
- General library callers that resolve state root through
  `state_root::simard_state_root()` (see
  [State-root resolution](./state-root-resolution.md))

Only the **three read paths** listed at the top of this page disable env
fallback.

## What is preserved

The change is surgically scoped. These behaviors are unchanged:

- `simard meeting run <base-type> <topology> <objective> [state-root]` —
  positional state-root remains optional; synthesized default path still
  resolves under `target/operator-probe-state/...` when omitted.
- `simard improvement-curation run <base-type> <topology> <objective> [state-root]`
  — same.
- `simard review run <base-type> <topology> <objective> [state-root]` —
  same.
- `simard goal-curation read <base-type> <topology> [state-root]` —
  positional state-root remains optional and `SIMARD_STATE_ROOT` is still
  honored (audit #1910 explicitly excludes this command from the change).
- `simard engineer run / read / terminal / terminal-read / copilot-submit`
  — unchanged. These read paths use `engineer run`'s canonical default
  and have their own contract documented in
  [Runtime contracts reference](./runtime-contracts.md).
- The `resolved_review_state_root` helper used by `review run` and by the
  `improvement-curation run` write path is untouched. A new sibling
  helper, `resolved_review_read_state_root`, owns the explicit-only
  contract for `review read`.

## Migration guidance

If you have a script that ran:

```bash
simard meeting read local-harness single-process
simard improvement-curation read local-harness single-process
simard review read local-harness single-process
```

…and relied on the synthesized default path, change each call to pass the
exact state root used during the corresponding `run`:

```bash
STATE_ROOT="/path/you/passed/to/run"
simard meeting read local-harness single-process "$STATE_ROOT"
simard improvement-curation read local-harness single-process "$STATE_ROOT"
simard review read local-harness single-process "$STATE_ROOT"
```

If you previously relied on `SIMARD_STATE_ROOT` to direct these three reads,
move the path onto the positional argument:

```bash
# Before
SIMARD_STATE_ROOT=/srv/simard-state simard meeting read local-harness single-process

# After
simard meeting read local-harness single-process /srv/simard-state
```

The diagnostic error from the new contract is intentionally explicit so the
needed fix is obvious from the failure.

## Library-callers

The guard lives at the resolver layer, not at the CLI parser. Any library
caller that invokes a read state-root through the public resolver surface
gets the same guarantee:

- `state_root::require_explicit_state_root_for_read(explicit, subcommand, base)`
  is the single source of truth.
- Calling it with `explicit = None` returns
  `Err(SimardError::MissingRequiredConfig { … })`.
- Calling it with `explicit = Some(path)` delegates to
  `bootstrap::validate_state_root(path)` and returns the canonicalized
  `PathBuf` on success.

The three resolver entrypoints downstream are:

- `state_root::resolved_meeting_read_state_root(explicit, base) -> Result<PathBuf, SimardError>`
- `state_root::resolved_improvement_curation_read_state_root(explicit, base) -> Result<PathBuf, SimardError>`
- `state_root::resolved_review_read_state_root(explicit, base) -> Result<PathBuf, SimardError>`

Each forwards `(explicit, "<subcommand>", base)` to the shared guard and
then runs the post-guard validators (`validate_meeting_read_state_root`,
`validate_improvement_curation_read_state_root`) where applicable. The
`base` argument is preserved so each helper can build fully-qualified
diagnostics if a post-guard validator fails on the caller-supplied path.
Topology is not consumed: explicit-only inputs never need to synthesize
a probe-relative default path, so the resolvers do not need to know
which runtime topology the read targets.

## Test coverage

The contract is asserted by integration tests in
`tests/issue_1909_state_root_required.rs`:

| Test name                                                       | Asserts                                                                  |
| --------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `meeting_read_hard_fails_without_state_root`                    | Non-zero exit and unified error wording for `meeting read`               |
| `meeting_read_succeeds_with_explicit_state_root`                | Read succeeds when a valid `<state-root>` is supplied                    |
| `improvement_curation_read_hard_fails_without_state_root`       | Non-zero exit and unified error wording for `improvement-curation read`  |
| `improvement_curation_read_succeeds_with_explicit_state_root`   | Read succeeds when a valid `<state-root>` is supplied                    |
| `review_read_hard_fails_without_state_root`                     | Non-zero exit and unified error wording for `review read`                |
| `review_read_succeeds_with_explicit_state_root`                 | Read succeeds when a valid `<state-root>` is supplied                    |
| `read_subcommands_ignore_simard_state_root_env_var`             | All three reads still hard-fail when `SIMARD_STATE_ROOT` is set          |

Two pre-existing `#[ignore]`d tests in `tests/simard_cli.rs` that encoded
the opposite (fallback-honored) contract for `meeting read` and
`improvement-curation read` have been replaced with hard-fail assertions
and un-ignored. They now run on every `cargo test` invocation to prevent
silent regression.

## Related reading

- [Simard CLI reference](./simard-cli.md) — full command tree, including
  the three reads with their required `<state-root>` positional.
- [Runtime contracts reference](./runtime-contracts.md) — executable
  contracts for the run and read paths the three read commands depend on.
- [State-root resolution](./state-root-resolution.md) — general resolver
  rules, including the resolver layer where this contract is enforced.
- [How to inspect meeting records](../howto/inspect-meeting-records.md) —
  walkthrough that uses the required `<state-root>` argument explicitly.
- [How to inspect improvement-curation state](../howto/inspect-improvement-curation-state.md)
  — walkthrough that uses the required `<state-root>` argument
  explicitly across `review run`, `improvement-curation run`, and the
  read companion.
- [Improvement context: denser execution evidence for the engineer loop](../concepts/improvement-context-execution-evidence-gap.md)
  — captured improvement-curation context adjacent to this contract;
  preserves the active "Capture denser execution evidence" goal and the
  observation that the legacy `simard_operator_probe` surface does not
  yet expose a terminal engineer-loop probe.
