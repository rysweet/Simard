---
title: "The journal E2BIG recipe-spawn incident — file the context, don't argv it"
description: >
  Why Simard's daily journal silently produced raw-error-dump reports every hour
  even after the copilot argv-free fix (#2640): the journal spawns recipe-runner-rs
  and passed a full day of context as `-c day_context=<...>` on argv, overflowing
  ARG_MAX (E2BIG, os error 7) BEFORE any exit status, then swallowed the failure
  into a warn! and fell back to a jargon-filled deterministic raw dump. Covers the
  root cause, why #2640 did not cover this spawn path, the file-channel fix, the
  no-silent-fallback rule, and the boundary against reintroducing argv inlining
  (#2692).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/recipe-context-file-transport.md
  - ../howto/diagnose-journal-e2big-spawn-failures.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ./self-diagnose-on-step-error.md
  - ../reference/distill-recipe-output-capture.md
  - ../reference/journal-api.md
  - ./simard-journal.md
  - ../../src/journal/recipe.rs
  - ../../src/recipe_context_file.rs
  - ../../src/overseer/diagnosis.rs
---

# The journal E2BIG recipe-spawn incident — file the context, don't argv it

> **Status: implemented.** The fix lives in
> [`src/recipe_context_file.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_context_file.rs)
> and its journal caller
> [`src/journal/recipe.rs`](https://github.com/rysweet/Simard/blob/main/src/journal/recipe.rs);
> the pre-exec classifier is
> [`classify_spawn_failure`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs).
> Closes [#2692](https://github.com/rysweet/Simard/issues/2692). For the wire-level
> contract read the
> [recipe context-file transport reference](../reference/recipe-context-file-transport.md);
> for a live-occurrence runbook read
> [Diagnose journal E2BIG spawn failures](../howto/diagnose-journal-e2big-spawn-failures.md).

This is the narrative for a **live, recurring** production failure on Simard's
journal path and the fix that resolved it. It is a sibling of, but distinct from,
the [copilot/OODA argv-free incident (#2640)](./self-diagnose-on-step-error.md).

## The incident

Every hour, on the deployed daemon that **already had** the copilot argv-free fix
(#2640), the journal thread logged:

```
WARN simard::journal: journal draft recipe failed; using the deterministic
report drafter error=base type 'journal' failed during invocation:
recipe-runner-rs spawn failed: Argument list too long (os error 7)
```

`os error 7` is **`E2BIG`**. The journal's two agentic passes — the
`journal-narrative` draft and the `journal-plain-language` de-jargon rewrite —
failed at the `recipe-runner-rs` spawn, every tick. The loop caught the error,
emitted a `warn!`, and fell back to the deterministic `TemplateDrafter` +
glossary `scrub_jargon`. Those assemblers dump raw episodic-memory `content`
verbatim — including *historical, pre-fix* decision-cycle E2BIG error messages —
with engineering jargon intact.

So the operator saw a journal full of **raw error dumps**: the narrative
improvements ([#2654](../reference/journal-api.md)) and the de-jargon pass never
actually ran, because the healthy path never started.

## Root cause: the day's context travelled through `argv`

`JournalRecipe::run` built the runner command by inlining **every** context value
as a command-line argument (`src/journal/recipe.rs`, old):

```rust
for (key, value) in ctx {
    cmd.arg("-c").arg(format!("{key}={value}"));   // value = the whole day_context
}
let output = cmd.output()
    .map_err(|e| invocation_failed(format!("recipe-runner-rs spawn failed: {e}")))?;
```

The draft pass passes `("day_context", day_context_json(day))` — a JSON blob with
every episode's `content`, every PR summary, and all facts/triggers/procedures for
the day. It is **unbounded** in the number and size of episodes. On a busy day the
serialized context is tens to hundreds of KiB, and the concatenated `argv` +
environment exceeds the kernel's fixed `ARG_MAX` budget (~2 MiB total, ~128 KiB
per single argument on Linux). `execve` then fails with `E2BIG` **before the
runner binary ever runs**.

Crucially, this surfaces as an `io::Error` from `cmd.output()` with
`raw_os_error() == Some(7)` — there is **no** `ExitStatus`, because no process
was created.

## Why the copilot fix (#2640) did not cover it

[#2640](./self-diagnose-on-step-error.md) fixed the E2BIG at the **three copilot
launch sites** by piping the prompt on **stdin** (`cat 'PATH' | amplihack copilot …`)
and by classifying the resulting exit-126 with
[`classify_terminal_failure`](../reference/terminal-failure-diagnosis-api.md#classify_terminal_failure).
Two reasons that fix left the journal exposed:

1. **Different binary, different channel.** The journal does not spawn `copilot`;
   it spawns `recipe-runner-rs`, which has **no** stdin/`--context-file` mechanism.
   `recipe-runner-rs` accepts context **only** as `-c KEY=VALUE` on `argv`. The
   stdin trick simply does not apply.
2. **Different failure shape.** #2640's classifier reads an `ExitStatus` +
   transcript. The journal E2BIG has **no** exit status — it is a pre-exec
   `io::Error`. Even the self-diagnose seam could not see it.

E2BIG in `argv` is a **class** of bug, and the copilot fix closed only one member
of it.

## The fix: a file channel, mirroring distillation

`recipe-runner-rs` has no stdin, but it **does** substitute any `-c` variable into
the recipe prompt — including one that holds a **path**. Distillation already uses
exactly this shape for its *output*
([`facts_output_path`, #2622/#2619](../reference/distill-recipe-output-capture.md)):
the agent writes a file whose path rides on `argv`.

The journal fix is the **input mirror**. The caller writes the large value to a
private temp file and passes only the path:

```text
# before  (✗ E2BIG on a busy day)
recipe-runner-rs journal-narrative.yaml --output-format json
    -c day_context=<...hundreds of KiB...>

# after  (✓ argv is a few hundred bytes, constant)
recipe-runner-rs journal-narrative.yaml --output-format json
    -c day_context_path=/tmp/simard-journal-ctx-XXXX/day_context.ctx
```

A single shared brick, [`ContextFile`](../reference/recipe-context-file-transport.md#contextfile-api)
(`src/recipe_context_file.rs`), owns the temp file and its cleanup; the recipe
prompt is updated to **read** `{{day_context_path}}` instead of interpolating
`{{day_context}}`. The de-jargon pass gets the same treatment (`draft_path`).

Because the payload is off `argv`, its size can no longer approach `ARG_MAX` —
`E2BIG` is impossible by construction, with **no truncation** (guideline **G3**:
the model still sees the whole day). The audit extended the same brick to the
other unbounded spawn sites (distillation `episodes`, merge-judge `pr_body`); see
the [spawn-site audit](../reference/recipe-context-file-transport.md#spawn-site-audit-and-dispositions).

## No silent fallback — fallback was the failure

The operator rule is blunt: **a fallback is a silent failure**. The old journal
embodied the antipattern — it `warn!`-swallowed the E2BIG and quietly emitted a
jargon-filled raw dump, so the failure reached no loop and no human, and the
"fixed" journal (#2654) never actually rendered. Three changes make the failure
impossible to hide:

- **The healthy path works.** The file channel makes the draft + de-jargon
  recipes succeed on realistic 24 h volumes, so the agentic narrative is the
  normal hourly outcome — not a fallback.
- **Genuine failures are diagnosed, not logged-and-dropped.** A residual spawn
  `io::Error` is classified by
  [`classify_spawn_failure`](../reference/recipe-context-file-transport.md#classify_spawn_failure)
  (errno `7` ⇒ `FailureCause::ArgListTooLong`, `28` ⇒ `DiskFull`, `12` ⇒
  `OutOfMemory`) and **recorded** to `overseer::failure_sink`, so the Overseer
  lifts it to a `Signal::StepFailureDiagnosed` and can act — the same corrective
  loop #2640 built, reached from a pre-exec error.
- **The last resort is loud and readable.** The deterministic drafter is kept
  **only** as a genuine last resort. When it runs it is loudly surfaced (recorded
  diagnosis + explicit error telemetry) and it no longer dumps raw error-log
  episodes or jargon into `## Remembered moments`.

## Why not just retry, or bound the context?

- **Blind retry reproduces E2BIG.** Re-spawning the same oversized `argv` fails
  identically. The fix must change the *cause* — where the payload travels — not
  re-run it.
- **Truncating the context corrupts it.** `day_context` is structured JSON the
  agent must read in full; lopping bytes off the end yields invalid JSON and drops
  real events (a fidelity loss that violates G3). The file channel keeps the whole
  payload; bounding is reserved for small free-text scalars where a documented cap
  is safe (the Tier-B sites).
- **A log is not a fix.** The old `warn!` is exactly the "log it and move on"
  antipattern #2640 named. The diagnosis now becomes actionable work.

## Boundary and non-goals

- This change touches **how the recipe payload is delivered** and **what happens
  when the spawn fails**. It does not change `recipe-runner-rs`, does not touch the
  #2640 copilot stdin path, and does not broaden any tool permission.
- It adds **no** new stray `print!`/`println!`, invents **no** "Bridge"-named
  component, and is strictly **additive** — a new `ContextFile` brick and a new
  sibling classifier, reusing the existing `FailureCause`/`FailureDiagnosis`
  types and the existing failure sink.
- Any memory-layer scoring or recall changes remain in
  amplihack-memory-lib (guideline **G2**); this fix is transport + diagnosis only.
