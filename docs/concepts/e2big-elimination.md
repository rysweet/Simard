---
title: "Comprehensive E2BIG elimination — one payload policy, one spawn facade"
description: >
  Why the "Argument list too long" (E2BIG, os error 7) failure kept recurring
  across Simard's agent/recipe launch sites even after two piecemeal fixes
  (#2660 decision-cycle, #2700 journal), and how it is eliminated as a CLASS: a
  single payload invariant ("a value that can grow large never rides argv or
  envp"), a single spawn facade (`simard::spawn_payload`) that copilot prompt
  sites route through on stdin (with a recipe-context file transport ready for
  recipe sites), a whole-repo launch-site audit with per-site dispositions, an
  errno-keyed pre-exec failure classifier that surfaces any residual spawn error
  instead of swallowing it, and a grep-shaped anti-regression guard that fails CI
  the moment a new argv-inlined payload is introduced (#2640).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/large-payload-spawn-api.md
  - ../howto/add-a-safe-agent-spawn-site.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/recipe-context-file-transport.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../prompt-delivery.md
  - ./self-diagnose-on-step-error.md
  - ./journal-recipe-spawn-e2big.md
  - ../../src/spawn_payload/mod.rs
  - ../../src/prompt_delivery/mod.rs
  - ../../src/recipe_context_file.rs
  - ../../src/overseer/diagnosis.rs
---

# Comprehensive E2BIG elimination

> **Status: implemented.** This document is the narrative for closing E2BIG as a
> class. The two byte-transports it unifies **already exist and are in use**:
> [`simard::prompt_delivery`](../prompt-delivery.md) (copilot prompts on stdin,
> #2660) and
> [`simard::recipe_context_file`](../reference/recipe-context-file-transport.md)
> (recipe context on a file path, #2692), as does the errno-keyed
> [`classify_spawn_failure`](../reference/terminal-failure-diagnosis-api.md).
> The **net-new** pieces now ship: the single `simard::spawn_payload` facade
> (`src/spawn_payload/mod.rs`), the copilot prompt sites (meeting, signal,
> engineer subprocess) routed through its **stdin** transport, the whole-repo
> launch-site audit dispositions, and the grep-shaped anti-regression guard
> (`tests/e2big_argv_guard.rs`). The recipe `-c` sites whose assets are external
> (Overseer launch, self-improve, engineer-loop) stay bounded-inline and
> E2BIG-safe until those assets gain a `_path` read. Tracks
> [#2640](https://github.com/rysweet/Simard/issues/2640); builds on
> [#2660](https://github.com/rysweet/Simard/pull/2660) and
> [#2700](https://github.com/rysweet/Simard/pull/2700) without regressing them.

## The symptom the operator kept hitting

Simard launches agent and recipe subprocesses from many places: the meeting
turn, the Signal channel, the OODA decision cycle, the engineer loop, the
Overseer fix-launch, the self-improve recipe, the stewardship merge judge, the
daily journal, episode distillation. Periodically one of these died on the
operator with:

```text
execve: Argument list too long (os error 7)   # E2BIG, exit 126 in a shell wrapper
```

It was fixed twice — once for the decision cycle (#2660) and once for the
journal's `recipe-runner-rs` spawn (#2700) — and **still recurred**, most
recently on the SIGNAL channel. Fixing one call site at a time never worked,
because this is not one bug. It is a **class** of bug.

## The root pattern (one class, many sites)

Every occurrence is the same shape: **a value that can grow large is placed on
the child process's argument vector (`argv`) or environment (`envp`).** The
kernel caps the total size of `argv + envp` at `ARG_MAX`, and caps a single
argument at `MAX_ARG_STRLEN` (128 KiB on Linux). When the payload — a prompt, a
day of context, a batch of episodes, a PR body — pushes past that cap, `execve`
fails with `E2BIG` **before the child ever runs**.

Two concrete sub-patterns produced it:

1. **Shell command-substitution into `argv`** — the copilot family:

   ```sh
   sh -c 'amplihack copilot -p "$(cat PROMPT_FILE)"'
   ```

   `$(cat PROMPT_FILE)` expands the file's *contents* into `copilot`'s `argv`.
   A large prompt overflows `ARG_MAX`.

2. **Large context inlined as a recipe flag** — the `recipe-runner-rs` family:

   ```text
   recipe-runner-rs RECIPE -c day_context=<...hundreds of KiB of JSON...>
   ```

   `recipe-runner-rs` only accepts context as `-c KEY=VALUE` on `argv`, so an
   unbounded value overflows `ARG_MAX`.

Because the offending code lived in **shared** base types, a single unfixed
pattern surfaced through every mode that used it.

## The fix: eliminate the class, not the instances

The comprehensive fix has four parts, each specified in detail in the
[large-payload spawn API reference](../reference/large-payload-spawn-api.md).
Parts 1–2 build directly on transports that already ship; parts 3–4 (the
unifying facade, the audit, and the guard) are the net-new work this design
introduces.

### 1. One invariant

> **A dynamic value whose length can exceed `ARGV_PAYLOAD_MAX_BYTES` (8 KiB) is
> delivered out-of-band and never appears in `argv` or `envp`.**

"Out-of-band" means one of exactly two byte-transports, chosen by the target
binary — never the caller:

| Target binary family | Out-of-band transport | Path on argv? | Payload on argv? |
| --- | --- | :---: | :---: |
| `copilot` / `amplihack copilot` (prompts) | **stdin** pipe (or a `0600` temp file for ≥ 100 KiB, still fed on stdin) | no | no |
| `recipe-runner-rs` / `amplihack recipe run` (context) | **file**, referenced by `-c <key>_path=<abs>` | yes (short path) | no |

The payload reaches the model **byte-for-byte, untruncated** (guideline **G3** —
agentic over brittle: we change *how* the bytes are delivered, never *whether*
the model sees them). Only the delivery channel changes.

### 2. One facade

Agent/recipe launch sites route their large payloads through a single module,
[`simard::spawn_payload`](../reference/large-payload-spawn-api.md) — the copilot
prompt sites on its stdin transport today, with the recipe-context file
transport ready for recipe sites whose assets support a `_path` read. It is the
sanctioned chokepoint that:

- decides, from the payload size and the target family, which transport applies;
- delegates to `prompt_delivery` (stdin) or `recipe_context_file::ContextFile`
  (file) — it does **not** re-implement either;
- refuses to build an invocation that would inline a large value into `argv`
  or `envp`.

"Single facade" means a **single call-through point and a single policy**, not a
single byte channel: copilot genuinely needs stdin and `recipe-runner-rs`
genuinely needs a file path, but both are dispatched from one place under one
rule. This is what makes the elimination *comprehensive* rather than piecemeal —
a new launch site cannot re-introduce the bug without deliberately bypassing the
facade.

### 3. One audit

Because the defect is a class, the fix includes a **whole-repo audit of every
launch site** — copilot, `recipe-runner-rs`, `amplihack recipe run`, and any
`sh -c` wrapper — with an explicit, durable disposition for each: *file channel*,
*bounded guard*, or *safe (fixed-size)*. The audit table lives in the
[reference](../reference/large-payload-spawn-api.md#whole-repo-launch-site-audit)
and is mirrored in the PR description. The copilot prompt sites (meeting, signal,
engineer subprocess) are routed through the facade's **stdin** transport by this
work; the in-repo recipe sites (journal, distillation, merge-judge) already use
the **file** channel (#2692/#2700). The recipe `-c` sites whose assets are
**external** (Overseer launch, self-improve, engineer-loop) stay **bounded-inline**
(`sanitize_context_var(…, 8000)`) — already E2BIG-safe (8000 chars ≪ ARG_MAX) —
because their recipes read `{{key}}` inline with no `{{key_path}}` support; the
facade's `recipe_context` file channel is ready for them once those external
assets gain a `_path` read.

### 4. One anti-regression guard

A grep-shaped, CI-visible test — `tests/e2big_argv_guard.rs` —
**fails the build** if anyone reintroduces the pattern: a new `$(cat`
argv-expansion, a new inline `-c <unbounded-key>=<contents>`, or removal of the
`spawn_payload` facade chokepoint. This is the mechanism that guarantees the bug
"can never come back" rather than merely being absent today. It mirrors the
shape of the existing `tests/no_bridge_naming.rs` linter.

## No silent fallback

E2BIG is a **pre-exec** failure: `Command::output()` / `spawn()` returns an
`io::Error` (errno 7) and there is no child, no exit status, and no transcript.
The old code `warn!`-and-dropped it, degrading to a jargon-filled raw dump. The
errno-keyed
[`classify_spawn_failure`](../reference/terminal-failure-diagnosis-api.md)
classifier already exists and is used by the journal path; the comprehensive fix
will route **every** launch site's spawn error through it into
`overseer::failure_sink` at `error`, so a residual spawn failure (a full disk on
the temp-file write, an OOM fork) is diagnosed and surfaced — never swallowed.
This is the operator principle "ask WHY it happened, don't just log it" applied
to the transport itself. See
[Self-diagnose on step error](./self-diagnose-on-step-error.md).

## Relationship to the piecemeal fixes

This work **builds on** the earlier fixes and must never regress them:

| Prior fix | What it covered | How this work relates |
| --- | --- | --- |
| **#2660** | Decision-cycle / OODA copilot launch → stdin | Kept; the OODA site becomes one of the facade's copilot callers, and `tests/ooda_e2big_transport.rs` / `tests/ooda_argv_free_invocation.rs` already pin it argv-free. |
| **#2700 / #2692** | Journal `recipe-runner-rs` spawn → `ContextFile` file channel; errno classifier | Kept; `ContextFile` is the facade's recipe transport. The facade generalises the same file channel to *every* recipe site, not just the journal. |
| **#2640 (copilot)** | Meeting / builder-PTY / OODA copilot sites → stdin | In progress; to be unified under the facade's prompt transport. |

The net-new contribution of #2640's comprehensive step is the **unifying facade**,
the **whole-repo audit**, and the **anti-regression guard** — the three things
that turn three separate point-fixes into one closed class.

## What "done" looks like

- No live `-p "$(cat` argv-expansion anywhere in `src/` (only doc-comments and
  the guard's own fixtures may mention it).
- Every large-payload launch site routes through `spawn_payload`.
- A > 256 KiB payload spawns successfully on every path (decision-cycle, meeting,
  Signal, engineer, Overseer-launch, self-improve, stewardship-judge) because the
  payload goes via stdin/file, not `argv`.
- `tests/e2big_argv_guard.rs` is green and will go red on any new argv-inlined
  payload.

## See also

- [Large-payload spawn API reference](../reference/large-payload-spawn-api.md) —
  the facade API, policy constants, audit table, and test matrix.
- [How to add a safe agent/recipe spawn site](../howto/add-a-safe-agent-spawn-site.md)
  — the tutorial for wiring a new launch site through the facade.
- [Argv-free Copilot/OODA invocation reference](../reference/argv-free-copilot-invocation.md)
  — the copilot-side detail (#2640).
- [Recipe context-file transport reference](../reference/recipe-context-file-transport.md)
  — the recipe-side detail (#2692).
- [Subprocess prompt delivery](../prompt-delivery.md) — the stdin/tempfile
  transport the facade delegates to for prompts.
