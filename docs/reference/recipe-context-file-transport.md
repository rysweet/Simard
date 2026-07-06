---
title: Recipe context-file transport reference
description: >
  Reference for the file-channel transport that keeps large `recipe-runner-rs`
  context payloads out of `argv` — the `ContextFile` helper, the
  `-c <key>_path=<abs>` invocation grammar, the journal draft/review migration
  (day_context_path / draft_path), the whole-repo spawn-site audit and its
  per-site dispositions, the pre-exec `classify_spawn_failure` errno classifier
  that turns an E2BIG `io::Error` into a structured `FailureDiagnosis`, the
  no-silent-fallback wiring into `overseer::failure_sink`, the recipe-asset
  `{{*_path}}` reads, and the hermetic argv-free tests (#2692).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/journal-recipe-spawn-e2big.md
  - ../howto/diagnose-journal-e2big-spawn-failures.md
  - ./argv-free-copilot-invocation.md
  - ./terminal-failure-diagnosis-api.md
  - ./distill-recipe-output-capture.md
  - ./journal-api.md
  - ./recipe-context-var-sanitization.md
  - ../concepts/self-diagnose-on-step-error.md
  - ../../src/recipe_context_file.rs
  - ../../src/journal/recipe.rs
  - ../../src/memory_consolidation/distillation.rs
  - ../../src/stewardship/recipe_merge_judge.rs
  - ../../src/overseer/diagnosis.rs
  - ../../src/overseer/failure_sink.rs
---

# Recipe context-file transport reference

> **Status: implemented.** The shared helper lives in
> [`src/recipe_context_file.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_context_file.rs);
> the journal caller is
> [`src/journal/recipe.rs`](https://github.com/rysweet/Simard/blob/main/src/journal/recipe.rs).
> The pre-exec spawn-failure classifier lives in
> [`src/overseer/diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs)
> and records into
> [`src/overseer/failure_sink.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/failure_sink.rs).
> Closes [#2692](https://github.com/rysweet/Simard/issues/2692).

Simard shells out to `recipe-runner-rs` from several base types (the journal,
episode distillation, the merge judge, and others). Each passes its context to
the runner as `-c KEY=VALUE` **command-line arguments**. When a value is large —
a full 24 h of episodic memories, a whole narrative draft, an arbitrary PR body —
the concatenated `argv` can exceed the kernel's `ARG_MAX`, and `execve` fails with
**`E2BIG`** (`errno 7`, `"Argument list too long"`) before the runner ever starts.

This page specifies the fix: a **file-channel transport** that carries the large
payload in a private temp file and puts only its short **path** on `argv`, plus a
**pre-exec failure classifier** that guarantees a residual spawn failure is
diagnosed and surfaced — never swallowed into a silent fallback.

For the narrative and the operator principle, see
[The journal E2BIG recipe-spawn incident](../concepts/journal-recipe-spawn-e2big.md).
For diagnosing a live occurrence, see
[Diagnose journal E2BIG spawn failures](../howto/diagnose-journal-e2big-spawn-failures.md).

## Contents

- [The invariant](#the-invariant)
- [Why the old invocation overflowed argv](#why-the-old-invocation-overflowed-argv)
- [Relationship to the copilot argv-free fix (#2640)](#relationship-to-the-copilot-argv-free-fix-2640)
- [`ContextFile` API](#contextfile-api)
- [Invocation grammar](#invocation-grammar)
- [Journal migration](#journal-migration)
- [Recipe-asset changes](#recipe-asset-changes)
- [Spawn-site audit and dispositions](#spawn-site-audit-and-dispositions)
- [`classify_spawn_failure`](#classify_spawn_failure)
- [No silent fallback](#no-silent-fallback)
- [Configuration](#configuration)
- [Security](#security)
- [Tests](#tests)
- [Code location](#code-location)

## The invariant

> **A recipe context value whose size is unbounded never appears in `argv` or
> `envp`. It is written to a private temp file, and only the file's path is passed
> as `-c <key>_path=<abs_path>`.**

Because the payload no longer contributes to the process's argument vector, its
size can no longer push `argv` + `envp` past `ARG_MAX`. `E2BIG` at the
`recipe-runner-rs` spawn is eliminated **by construction**, with zero truncation
and full fidelity of the payload (guideline **G3** — agentic over brittle: the
model still sees the whole context, we just deliver it differently).

This is the **input** mirror of the distillation **output** channel
(`facts_output_path`, [#2622/#2619](./distill-recipe-output-capture.md)): there
the agent *writes* a file whose path is on `argv`; here the caller *writes* a file
whose path is on `argv` and the agent *reads* it.

## Why the old invocation overflowed argv

The journal marshalled its entire context into `argv`
(`src/journal/recipe.rs`, old):

```rust
for (key, value) in ctx {
    cmd.arg("-c").arg(format!("{key}={value}"));   // value = full day_context / full draft
}
let output = cmd.output()                          // fails pre-exec with E2BIG
    .map_err(|e| invocation_failed(format!("recipe-runner-rs spawn failed: {e}")))?;
```

`day_context` is `day_context_json(day)` — every episode's `content`, every PR
summary, all facts/triggers/procedures for the day — and is unbounded in both the
number and size of episodes. On a busy day it exceeds the per-argument /
total-`argv` budget, so `cmd.output()` returns an `io::Error` whose
`raw_os_error()` is `Some(7)` (`E2BIG`), and whose `Display` is
`"… (os error 7)"`. The old code turned that into a `warn!` and fell back to the
deterministic drafter — see [No silent fallback](#no-silent-fallback).

## Relationship to the copilot argv-free fix (#2640)

The earlier
[argv-free Copilot/OODA invocation fix (#2640)](./argv-free-copilot-invocation.md)
removed the `-p "$(cat …)"` antipattern at the **three copilot launch sites**
(meeting turn, builder PTY turn, OODA decision-cycle launch) by piping the prompt
on **stdin**. That fix did **not** touch the `recipe-runner-rs` spawn path, which
has **no** stdin/`--context-file` channel — `recipe-runner-rs` accepts context
**only** as `-c KEY=VALUE` on `argv`. So the journal's `recipe-runner-rs` spawn
kept overflowing after #2640 shipped.

`recipe-runner-rs` **does** substitute any `-c` variable into the recipe prompt,
including one that holds a **path**. That is the seam this fix uses: keep `argv`
tiny by putting the path there, and let the recipe read the file. No change to
`recipe-runner-rs` is required.

## `ContextFile` API

A single shared brick (`src/recipe_context_file.rs`) that every large-payload
spawn site reuses:

```rust
/// A recipe context value delivered via a private temp file rather than argv,
/// so the payload size never contributes to ARG_MAX (issues #2640/#2692). The
/// returned value OWNS its tempdir; keep it alive until `Command::output()`
/// returns.
pub struct ContextFile { /* private: tempdir guard + key + absolute path */ }

impl ContextFile {
    /// Write `value` to a fresh, per-invocation, mode-`0700` tempdir as
    /// `<key>.ctx` (tempdir prefix `simard-<base_type>-ctx-`). The recipe is then
    /// handed `-c <key>_path=<abs>` and reads the file itself.
    pub fn write(base_type: &str, key: &str, value: &str) -> std::io::Result<Self>;

    /// The `-c` value to append: `"<key>_path=<abs_path>"`.
    pub fn arg_value(&self) -> String;

    /// The absolute path the recipe will read.
    pub fn path(&self) -> &str;
}
```

Semantics:

- **Ownership / lifetime.** `ContextFile` holds a `tempfile::TempDir`; the file
  and directory are removed when the value is dropped. Callers keep the guard in
  scope **across** `cmd.output()` so the file exists while the runner reads it.
- **Uniqueness.** One tempdir per invocation (`tempfile` crate), so concurrent
  journal ticks and distillation passes never collide.
- **Bytes.** The value is written UTF-8, verbatim, no truncation.
- **Errors.** A tempdir/create/write failure is an `io::Error` the caller
  classifies with [`classify_spawn_failure`](#classify_spawn_failure) (e.g. a
  full disk ⇒ `DiskFull`) and records — never a silent degrade.

## Invocation grammar

Before (large value inline):

```text
recipe-runner-rs <recipe_path> --output-format json
    -c day_context=<...tens–hundreds of KiB of JSON...>      # ✗ E2BIG
```

After (path on argv, content in the file):

```text
recipe-runner-rs <recipe_path> --output-format json
    -c day_context_path=/tmp/simard-journal-ctx-XXXX/day_context.ctx   # ✓ ~few hundred bytes, constant
```

Small scalar keys that are already bounded (dates, PR numbers, repo slugs, the
`strict_json_instruction` reinforcement sentence) stay **inline** — only
unbounded content moves to a file.

## Journal migration

`src/journal/recipe.rs`:

| Caller | Old inline var | New file-channel var |
| --- | --- | --- |
| `RecipeDrafter::draft` → `journal-narrative` | `day_context = day_context_json(day)` | `day_context_path` via `ContextFile::write("journal", "day_context", …)` |
| `RecipeReviewer::review` → `journal-plain-language` | `draft = draft.to_string()` | `draft_path` via `ContextFile::write("journal", "draft", …)` |

`JournalRecipe::run` holds each `ContextFile` guard across `cmd.output()` and,
on a spawn `Err`, records a diagnosis (below) before degrading. The
`--output-format json` envelope handling, the `success == false` check, and the
empty-output check are unchanged.

## Recipe-asset changes

The prompt assets must **read the file** instead of interpolating the raw value.
Each affected recipe swaps its `{{var}}` for a `{{var_path}}` read instruction,
mirroring the file-read wording already proven in `distill-episodes.yaml`:

| Recipe asset | Old placeholder | New placeholder |
| --- | --- | --- |
| `prompt_assets/simard/recipes/journal-narrative.yaml` | `{{day_context}}` | read the JSON at `{{day_context_path}}` |
| `prompt_assets/simard/recipes/journal-plain-language.yaml` | `{{draft}}` | read the Markdown at `{{draft_path}}` |
| `prompt_assets/simard/recipes/distill-episodes.yaml` | `{{episodes}}` | read the JSON at `{{episodes_path}}` |
| the merge-judge recipe | `{{pr_body}}` | read the text at `{{pr_body_path}}` |

The `context:` default block in each recipe declares the new `*_path` key so the
substitution always resolves. Assets reach the running daemon through the existing
`scripts/redeploy-local.sh` recipe-asset sync (see
[distill recipe output capture](./distill-recipe-output-capture.md#recipe-asset-sync)).

## Spawn-site audit and dispositions

Issue #2692 requires auditing **every** site that can carry a large payload to
`recipe-runner-rs`, `amplihack recipe run`, or `copilot`, because this is a
**class** of bug (unbounded payload in `argv`/`envp`), not a single defect. Each
site is tiered by payload risk:

| Tier | Site | Var(s) | Disposition |
| --- | --- | --- | --- |
| **A — file channel** | `journal/recipe.rs` (draft) | `day_context` | **FIX** — `ContextFile` (`day_context_path`) |
| **A** | `journal/recipe.rs` (review) | `draft` | **FIX** — `ContextFile` (`draft_path`) |
| **A** | `memory_consolidation/distillation.rs` | `episodes` | **FIX** — `ContextFile` (`episodes_path`); `facts_output_path` output channel unchanged |
| **A** | `stewardship/recipe_merge_judge.rs` | `pr_body` | **FIX** — `ContextFile` (`pr_body_path`) |
| **B — bounded guard** | `goal_curation/decompose.rs` (recipe-runner-rs) | `goal_description`, `plan` | **GUARD** — `sanitize_context_var(…, 8000)` (also fixes latent #2127 newline/YAML) |
| **B** | `goal_curation/recipe_progress_checker.rs` (recipe-runner-rs) | `problem`, `plan`, `wip_summary` | **GUARD** — `sanitize_context_var(…, 8000)` |
| **B** | `overseer/launch.rs` (`amplihack recipe run`) | `task_description` | **GUARD** — `sanitize_context_var(…, 8000)` |
| **B** | `bin/simard_engineer_loop_recipe.rs` (`amplihack recipe run`) | `objective` | **GUARD** — `sanitize_context_var(…, 8000)` |
| **B** | `bin/simard_self_improve_recipe.rs` (`amplihack recipe run`) | `proposal` | **GUARD** — `sanitize_context_var(…, 8000)` |
| **C — safe (no change)** | `ooda_brain/recipe_brain.rs` | `goal_id`/`reason`/`escalation_note`/… | **SAFE** — already `sanitize_context_var(…, 500/4000)` |
| **C** | `recipe_merge_judge.rs` | `escalation_note` | **SAFE** — bounded-by-construction via `build_merge_escalation_note` |
| **C** | `brain_introspection.rs` | `stats` | **SAFE** — small serialized `MemoryHygieneOutcome` (counts) |
| **C** | `disk_health.rs`, `self_quality_audit.rs`, `brain_introspection.rs` | `state_root`/`repo_path`/`max_prune`/… | **SAFE** — paths / small integer scalars |
| **C** | `launch.rs` `target_repo`; merge-judge `pr_number`/`repo`; distill `strict_json_instruction`/`facts_output_path`; engineer `topology`/`state_root`/`workspace_root` | tiny consts / IDs / paths | **SAFE** |
| **Prior** | `base_type_copilot/mod.rs` (copilot + meeting base types) | prompt/objective | **DONE (#2640)** — argv-free via stdin / prompt tempfile, see [copilot reference](./argv-free-copilot-invocation.md) |
| **n/a** | any `--version` probe | — | not a payload |

Rationale:

- **Tier A** carries semantic content the agent must consume **in full** →
  file channel. Truncating structured JSON would corrupt the payload, so
  truncation is disallowed here; the file channel is mandatory.
- **Tier B** is realistically small but operator-influenced free text → the
  existing bounded `ooda_brain::sanitize::sanitize_context_var(s, max_len)`
  (whitespace-collapse + char-boundary truncate + ellipsis) as a cheap ceiling.
  See [recipe context-var sanitization](./recipe-context-var-sanitization.md).
- **Tier C** is fixed-size (IDs, paths, short consts) → unaffected.

Every audited site and its disposition is enumerated here and in the PR
description, so the audit is a durable artifact, not a one-time grep.

## `classify_spawn_failure`

The [#2640 self-diagnose seam](./terminal-failure-diagnosis-api.md) classifies a
failure from an `ExitStatus` + transcript. **This E2BIG never produces an
`ExitStatus`** — it is an `io::Error` from `cmd.output()` *before* the child
exists. So a sibling classifier keys off the **errno**:

```rust
// src/overseer/diagnosis.rs
/// Classify a PRE-EXEC spawn failure (an io::Error, no ExitStatus) by errno, so
/// an E2BIG recipe spawn is diagnosed structurally rather than warn!-and-dropped.
pub fn classify_spawn_failure(err: &std::io::Error) -> FailureDiagnosis {
    let cause = match err.raw_os_error() {
        Some(7)  => FailureCause::ArgListTooLong,  // E2BIG
        Some(28) => FailureCause::DiskFull,        // ENOSPC
        Some(12) => FailureCause::OutOfMemory,     // ENOMEM
        _        => FailureCause::Unknown,
    };
    FailureDiagnosis { cause, exit_code: None, evidence: bounded_spawn_evidence(&err.to_string()) }
}
```

- Reuses the existing
  [`FailureCause`](./terminal-failure-diagnosis-api.md#failurecause) /
  [`FailureDiagnosis`](./terminal-failure-diagnosis-api.md#failurediagnosis)
  types — **no new causes**. `exit_code` is `None` (there was no exit).
- A string fallback matches `"argument list too long"` / `"os error 7"` for
  platforms that do not surface a numeric `raw_os_error()`.
- `evidence` is bounded and redacted (never the full payload or full error).
- **Transcript-less diagnosis.** The `diagnosis.rs` module is otherwise
  transcript/`ExitStatus`-first and routes the agentic "WHY + remedy" reasoning to
  `prompt_assets/simard/overseer/self_diagnose.md`. A pre-exec spawn failure has
  **no** transcript, so `classify_spawn_failure` supplies errno-derived `evidence`
  instead; the module header and the self-diagnose prompt are extended to cover
  this transcript-less case (the prompt reasons from the errno + bounded error
  string rather than from shell output).

At each Tier-A spawn-failure arm, the caller records the diagnosis into the
bounded sink **before** propagating or degrading:

```rust
let output = cmd.output().map_err(|e| {
    overseer::failure_sink::record_step_failure(classify_spawn_failure(&e));
    invocation_failed(format!("recipe-runner-rs spawn failed: {e}"))
})?;
```

On its next Observe pass the Overseer drains the sink and lifts the diagnosis to
[`Signal::StepFailureDiagnosed`](./terminal-failure-diagnosis-api.md#signalstepfailurediagnosed)
→ `ProblemKind::ProcessHealth` → a corrective `Intervention` — exactly like the
#2640 exit-126 path, but reached from a pre-exec `io::Error`.

## No silent fallback

The operator rule is **fallback == silent failure**. The old journal behaviour
violated it: it caught the E2BIG with `tracing::warn!` and silently used the
deterministic `TemplateDrafter` / `scrub_jargon`, which then dumped raw episodic
`content` (including *historical* E2BIG error text) with jargon — the exact
"journal full of raw error dumps" the operator saw. This fix changes three things:

1. **The healthy path works.** With the file channel, the draft + de-jargon
   recipes succeed on realistic 24 h volumes, so the agentic narrative
   ([#2654](./journal-api.md)) is the normal hourly outcome, not the exception.
2. **Genuine failures are surfaced, not swallowed.** Any residual spawn failure is
   classified (`classify_spawn_failure`) and recorded to the failure sink so the
   Overseer can act — an `error`-level, structured, Overseer-actionable signal, not
   a bare `warn!` that no loop reads.
3. **The last-resort fallback is loud and readable.** The deterministic drafter is
   retained **only** as a genuine last resort. When it runs it is (a) loudly
   surfaced via the recorded diagnosis + explicit error telemetry, and (b) made
   **readable** — it strips raw error-log episodes and jargon from
   `## Remembered moments` (no verbatim `ep.content` dumps of historical error
   text). It is never the silent normal path.

## Configuration

The transport itself has **no env knobs** — it is always-on so a large context is
never at risk of overflowing `argv`. The pieces it reuses keep their existing
controls:

- **Recipe hot-reload path** — assets resolve hot-reload-first from
  `~/.simard/prompt_assets/simard/recipes/`, then the in-tree copy
  (`resolve_recipe_path`), synced by `scripts/redeploy-local.sh`.
- **Diagnosis surfacing** — always-on; whether a corrective `LaunchRecipe`
  actually launches is governed by the existing Overseer acting controls
  (`SIMARD_OVERSEER_ENABLED`, autonomy, daily budget, per-cycle launch cap). See
  [terminal failure diagnosis — configuration](./terminal-failure-diagnosis-api.md#configuration).
- **Tier-B bound** — `sanitize_context_var`'s `max_len` is the existing bound; see
  [recipe context-var sanitization](./recipe-context-var-sanitization.md).

## Security

- **No command injection.** `argv` is still built with
  `Command::new("recipe-runner-rs").arg(...)`; the only new token is a
  `-c <key>_path=<abs>` where the path comes from `tempfile` (safe alphabet),
  never interpolated into a shell line.
- **Private payload file.** The context file lives in a per-invocation,
  mode-`0700` tempdir and is unlinked on guard drop. Because the large payload is
  now in a `0700` file rather than on `argv`, it is **no longer** visible via
  `ps` / `/proc/<pid>/cmdline` on a shared host — a side benefit over the old
  inline `-c value` (the known `ps`-visibility limitation called out for the
  distillation `episodes` argv is closed for these sites).
- **Bounded diagnostics.** `FailureDiagnosis.evidence` from a spawn failure is
  bounded via `bounded_spawn_evidence` so the whole excerpt (ellipsis included)
  never exceeds `MAX_EVIDENCE_LEN` (400 chars, head-retained — an io-error message
  is short and front-loaded), and the recipe stderr excerpt uses
  `truncate(…, 200)`, so a large hostile payload never echoes in full.
- **No stdout scraping, no silent fallback.** Genuine failures error explicitly
  and are recorded; a fallback is loud, not silent.

## Tests

Hermetic tests pin the invariant (mirroring
`tests/ooda_argv_free_invocation.rs` / `tests/ooda_e2big_transport.rs`):

| Test | Coverage |
| --- | --- |
| `tests/journal_e2big_transport.rs` | A `> ARG_MAX` (and `> 256 KiB`) `day_context` spawns `recipe-runner-rs` successfully via the file channel — no `os error 7` / E2BIG. |
| `tests/journal_argv_free.rs` | Regression: the constructed journal invocation carries **no** large payload in `argv`/`envp` — only a small `*_path` value; the payload is byte-for-byte recoverable from the file. |
| `recipe_context_file` unit tests | `write` round-trips value→file; `arg_value()` == `"<key>_path=<abs>"`; guard drop removes the dir. |
| `classify_spawn_failure` unit tests | `io::Error::from_raw_os_error(7)` ⇒ `ArgListTooLong`; `28` ⇒ `DiskFull`; `12` ⇒ `OutOfMemory`; other ⇒ `Unknown`; evidence is bounded. |
| sink-wiring integration test | An injected spawn `Err` at a Tier-A site yields exactly one `ArgListTooLong` diagnosis in `drain_recent()` — no silent drop. |
| recipe-asset regression | The four YAMLs no longer interpolate the raw payload var and do reference the `*_path` var. |
| audited-site coverage | The distillation `episodes_path`, merge-judge `pr_body_path`, and Tier-B `sanitize_context_var` guards are each exercised. |

## Code location

| Item | File |
| --- | --- |
| `ContextFile` helper | `src/recipe_context_file.rs` |
| module registration (`pub mod recipe_context_file;`) | `src/lib.rs` |
| journal draft/review file channel + spawn-failure record | `src/journal/recipe.rs` |
| distillation `episodes_path` | `src/memory_consolidation/distillation.rs` |
| merge-judge `pr_body_path` | `src/stewardship/recipe_merge_judge.rs` |
| `classify_spawn_failure` | `src/overseer/diagnosis.rs` |
| `record_step_failure` / `drain_recent` | `src/overseer/failure_sink.rs` |
| Tier-B `sanitize_context_var` guards (the helper itself lives in `src/ooda_brain/sanitize.rs` and is reused) | `src/goal_curation/decompose.rs`, `src/goal_curation/recipe_progress_checker.rs`, `src/overseer/launch.rs`, `src/bin/simard_engineer_loop_recipe.rs`, `src/bin/simard_self_improve_recipe.rs` |
| recipe assets (`{{*_path}}` reads) | `prompt_assets/simard/recipes/journal-narrative.yaml`, `journal-plain-language.yaml`, `distill-episodes.yaml`, merge-judge recipe |
| tests | `tests/journal_e2big_transport.rs`, `tests/journal_argv_free.rs`, plus the unit/integration tests above |
