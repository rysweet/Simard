---
title: Argv-free Copilot/OODA invocation reference
description: >
  Reference for the argv-free prompt transport at Simard's three copilot launch
  sites — the meeting turn, the builder PTY turn, and the OODA decision-cycle
  launch. Specifies the exact per-site invocation grammar (stdin pipe / direct
  Command + prompt_delivery), the removal of the `-p "$(cat …)"` antipattern that
  caused the live exit-126 / E2BIG "Argument list too long" defect, the preserved
  flags, the temp-file lifetime, and the argv-free invariant its tests enforce
  (#2640).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/self-diagnose-on-step-error.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../prompt-delivery.md
  - ../reference/engineer-loop-argv-sanitization.md
  - ../reference/base-type-adapters.md
  - ../../src/base_type_copilot/mod.rs
  - ../../src/ooda_actions/session.rs
  - ../../src/prompt_delivery/mod.rs
---

# Argv-free Copilot/OODA invocation reference

> **Status: implemented.** The meeting and builder sites live in
> [`src/base_type_copilot/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/base_type_copilot/mod.rs);
> the OODA decision-cycle launch lives in
> [`src/ooda_actions/session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/session.rs).
> The shared prompt chokepoint they reuse is
> [`simard::prompt_delivery`](../prompt-delivery.md). Closes
> [#2640](https://github.com/rysweet/Simard/issues/2640).

## Contents

- [The invariant](#the-invariant)
- [Why the old invocation overflowed argv](#why-the-old-invocation-overflowed-argv)
- [The three sites](#the-three-sites)
  - [Site A — meeting turn](#site-a-meeting-turn)
  - [Site B — builder PTY turn](#site-b-builder-pty-turn)
  - [Site C — OODA decision-cycle launch](#site-c-ooda-decision-cycle-launch)
  - [Related — meeting/Signal agent proxy (documented separately)](#related-meetingsignal-agent-proxy-documented-separately)
- [Preserved flags and behaviour](#preserved-flags-and-behaviour)
- [Temp-file lifetime and permissions](#temp-file-lifetime-and-permissions)
- [No silent fallback](#no-silent-fallback)
- [Tests](#tests)

## The invariant

> **Prompt bytes never appear in `argv` or in the PTY `command:` string.**

At all three copilot launch sites, the prompt/objective is delivered to the tool
on **stdin** (or via `prompt_delivery` for the non-PTY site). The only
prompt-related token that may appear in the command string is the **path** to a
temp file that the shell `cat`s into the pipe — never the prompt content itself.
Because the prompt no longer contributes to the process's argument vector, prompt
size can no longer exceed `ARG_MAX`.

This generalises the existing [Subprocess Prompt Delivery](../prompt-delivery.md)
contract to the three copilot sites that historically built their `argv` by hand.

## Why the old invocation overflowed argv

The old invocation inlined the prompt into `argv`:

```text
amplihack copilot --subprocess-safe -p "$(cat '/tmp/prompt')" --allow-all-tools
```

`$(cat '/tmp/prompt')` expands at **shell-parse time**, so the whole prompt became
one giant `argv` element. On Linux, `argv` + `envp` must fit inside `ARG_MAX`
(~2 MiB total; ~128 KiB per single argument). A large-context OODA goal produced a
prompt that overflowed that budget, so `execve` returned **`E2BIG`**. The shell
prints `Argument list too long` and exits **126**. See
[Self-diagnose on step error](../concepts/self-diagnose-on-step-error.md#the-incident)
for the full incident.

The fix removes `-p` entirely (the tool reads a non-TTY stdin as the prompt when
`-p` is absent) and pipes the prompt in:

```text
cat '/tmp/prompt' | amplihack copilot --subprocess-safe --allow-all-tools ; exit
```

The prompt now travels through a pipe; only the short, fixed-alphabet **path**
`'/tmp/prompt'` is in `argv`.

## The three sites

### Site A — meeting turn

`base_type_copilot/mod.rs`, `run_meeting_turn`.

- **Before:** `sh -c '{binary} --no-custom-instructions --silent --allow-all-tools --session-id '…' -p "$(cat '…')"'`.
- **After:** a **direct** `std::process::Command` (no `sh -c` wrapper) whose prompt
  is delivered through `prompt_delivery::apply_std(&mut cmd, prompt_bytes, PromptDelivery::Stdin)`;
  the returned guard writes the prompt to the child's stdin and owns cleanup.

Rationale: the meeting site does not need a PTY, so it drops the shell wrapper
altogether and reuses the sanctioned chokepoint. The `--session-id`,
`--no-custom-instructions`, `--silent`, and `--allow-all-tools` flags are passed as
ordinary `Command::arg(...)` tokens — never string-formatted with the prompt.

### Site B — builder PTY turn

`base_type_copilot/mod.rs`, builder invocation (the `command:` line written into
the terminal recipe).

- **Before:** `command: {binary} --subprocess-safe -p "$(cat '{path}')" --allow-all-tools ; exit`.
- **After:** `command: cat '{path}' | {binary} --subprocess-safe --allow-all-tools ; exit`.

The `; exit` terminator, the wait-for semantics, and the `--subprocess-safe` /
`--allow-all-tools` flags are unchanged. Only `-p "$(cat …)"` is replaced by the
leading `cat '{path}' |` pipe.

Both PTY sites emit the identical `cat '{path}' | … ; exit` grammar. Site C's
builder (`ooda_actions::build_ooda_launch_command`) is **fail-closed**: it returns
an error if the temp path contains a single quote — which would break out of the
single-quoted `cat '…'` context — rather than emitting a mis-quoted command (no
silent fallback). Site B (`build_copilot_terminal_objective`) interpolates only a
Rust `NamedTempFile` path, drawn from a restricted safe alphabet
(`/`, `.`, alnum, `_`), and additionally `debug_assert!`s the path carries no
single quote — so neither PTY site can emit an injectable `cat '…'`.

### Site C — OODA decision-cycle launch

`ooda_actions/session.rs` (the `decision cycle-copilot` base type).

- **Before:** a hand-escaped `printf '%s' '{task}' > "$SIMARD_PROMPT_FILE"; amplihack copilot -p "$(cat \"$SIMARD_PROMPT_FILE\")" ; … rm -f …`.
- **After:** the objective is written to a Rust `NamedTempFile` (see
  [Temp-file lifetime](#temp-file-lifetime-and-permissions)), and the PTY command
  is `cat '{path}' | amplihack copilot --subprocess-safe --allow-all-tools ; exit`.

This site had **two** overflow bugs — the argv `E2BIG` *and* a PTY `command:`
string that could itself grow unbounded from the inlined `printf '%s' '{task}'`.
Both are removed: the task text is written to disk by Rust (never interpolated into
the command string), and only the generated temp-file path appears in the PTY
command.

### Related — meeting/Signal agent proxy (documented separately)

The meeting backend's `PersistentAgentProxy`
(`src/meeting_backend/agent_proxy.rs`) is a **fourth, distinct** copilot launch
site — the one the **Signal channel** reaches through a meeting session. It is
not a variant of Sites A–C (it shares no code with `base_type_copilot` or
`ooda_actions`), so it had its own argv-inlining `E2BIG` and its own fix, covered
in the
[argv-free meeting/Signal agent-proxy reference](./argv-free-meeting-agent-proxy.md).
Like Site A it delivers the prompt on **stdin** (via
`spawn_payload::attach_prompt_std`) rather than the PTY `cat '…' |` grammar, and
it observes the same argv-free invariant.

## Preserved flags and behaviour

The change is strictly a **transport** change. All of the following are preserved
byte-for-byte:

- `--subprocess-safe`, `--allow-all-tools`, `--no-custom-instructions`,
  `--silent`, and `--session-id '<id>'` where each was already present.
- The PTY `; exit` terminator and the terminal-session wait-for / idle-detection
  semantics.
- `validate_command()` still rejects operator-supplied terminal commands
  containing `;`, `|`, `&`, `$`, or backticks. The internal `cat '…' | … ; exit`
  string is generated by Simard from a safe-alphabet temp path, not accepted from
  an operator.

## Temp-file lifetime and permissions

Site C writes the objective to a `tempfile::NamedTempFile` created with mode
`0o600` and `O_EXCL`, replacing the previous shell `mktemp`/`printf` sequence.

- **Permissions:** `0o600` from creation closes the brief world-readable window
  the old `mktemp` default left open.
- **Cleanup:** the file's `Drop` impl unlinks it on **every** exit path — success,
  early-return error, or panic. The old explicit `rm -f` in the shell script is
  removed; cleanup no longer depends on the shell reaching its last line.
- **Fail-closed path check:** the Site C builder
  (`ooda_actions::build_ooda_launch_command`) runs a **runtime** check that rejects
  any temp path containing a single quote and returns
  `SimardError::InvalidConfigValue` rather than emitting a mis-quoted command. Site
  B (`build_copilot_terminal_objective`) interpolates only a `NamedTempFile` path
  from a safe alphabet (`/`, `.`, alnum, `_`) and `debug_assert!`s it carries no
  single quote. In practice the path is always safe, so neither ever fires; when
  Site C's guard does, the launch fails loudly.

## No silent fallback

If the prompt cannot be delivered argv-free — e.g. `prompt_delivery::apply_std`
returns an error, or the temp file cannot be created — the launch returns
`SimardError::AdapterInvocationFailed` with the reason. It **never** falls back to
inlining the prompt in `argv`. A future regression that reintroduced argv delivery
would be caught by the tests below, not silently tolerated.

## Tests

The following hermetic tests encode the invariant (see
`src/tests_base_type_copilot.rs` and `tests/ooda_transport_e2e.rs`):

- **Argv-free construction.** Building the invocation for a large prompt asserts
  the prompt content appears **nowhere** in the constructed `argv` / PTY
  `command:` string — only the temp-file path does. The stale assertions that
  matched the old `-p "$(cat '` substring are rewritten to assert the new
  `cat '…' | … ; exit` grammar (rewritten to preserve intent, not deleted).
- **`>ARG_MAX` prompt succeeds.** A prompt larger than `ARG_MAX` (and larger than
  256 KiB) is delivered and executed without an `E2BIG` / exit-126 failure,
  because it travels on stdin.
- **Flags preserved.** `--subprocess-safe` and `--allow-all-tools` remain present
  after the rewrite at each site.
