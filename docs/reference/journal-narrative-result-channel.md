---
title: Journal narrative result channel
description: How Simard's daily-journal narrative and plain-language passes capture the agent's clean report — the dedicated result-file channel (narrative_output / plain_output), the shared harvest_narrative_file reader, the contract-marker recipe guard (recipe_declares_result_file / select_recipe), the loud failure taxonomy that degrades to the honest offline path, and the recipe-asset sync that keeps the hot-reload path current. The launcher banner and the agent's own tool-call trace can never enter the stored narrative because stdout is no longer read as the result.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: shipped
related:
  - ./journal-api.md
  - ./goal-decompose-result-channel.md
  - ./distill-recipe-output-capture.md
  - ./recipe-context-file-transport.md
  - ../concepts/simard-journal.md
  - ../concepts/journal-report-tone-contract.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../howto/verify-the-journal-clean-narrative.md
  - ../howto/browse-the-simard-journal.md
  - ../design/eliminate-deterministic-fallbacks.md
---

# Journal narrative result channel

> **Status — clean-result-channel capture for the journal narrative.**
> Both journal agent passes — the narrative **draft** and its plain-language
> **rewrite** — capture the agent's report from a **dedicated result file** the
> agent writes, not from `recipe-runner-rs` stdout. Present tense below
> describes the shipped behavior. Locations:
> reader + invocation `src/journal/recipe.rs`
> (`JournalRecipe::run`, `harvest_narrative_file`, `recipe_declares_result_file`,
> `select_recipe`); tests `src/journal/tests_clean_result_channel.rs`
> (registered in `src/journal/mod.rs`); recipes
> `prompt_assets/simard/recipes/journal-narrative.yaml` (drafter,
> `narrative_output`) and
> `prompt_assets/simard/recipes/journal-plain-language.yaml` (reviewer,
> `plain_output`); asset sync `scripts/redeploy-local.sh`.

The [journal generation pipeline](./journal-api.md) turns one day of Simard's
activity into a durable, layperson-readable **narrative engineering & research
report** in two agentic passes: a **drafter** runs the `journal-narrative`
recipe to write the professional third-person report from the day's structured
context, and a **reviewer** runs the `journal-plain-language` recipe to rewrite
that draft so a non-engineer can read it. This page documents the
**output-capture contract** between Simard and those two agents: how each pass's
finished report is captured, what happens on failure, and how the recipe assets
reach the running daemon.

This page is the transport-layer companion to the [Journal API](./journal-api.md);
it changes **only** how each agent's report reaches Simard, not what the pipeline
does with it. The good narrative body, the mandatory
[plain-language reviewer pass](./journal-api.md#the-two-pass-generation-pipeline), and the
unconditional `scrub_secrets` / de-jargon post-passes in
`src/journal/generate.rs` are preserved **exactly**.

It is the journal-side sibling of the goal-decompose
[result channel](./goal-decompose-result-channel.md) and the distillation
[clean result channel](./distill-recipe-output-capture.md): the same
brittle-parsing-of-agent-stdout antipattern, closed the same structural way. The
journal reader is **modeled on** — not a byte-for-byte copy of —
`harvest_subgoals_file`: it keeps the file-channel shape, the
"non-zero exit surfaces stderr **and** stdout" diagnostic, and the same **1 MiB
size cap** and **empty / whitespace-only rejection**, but it captures **prose**
(Markdown), not a JSON envelope — so there is no deserialize step and no
`parse_*` companion, just a trimmed, non-empty string.

---

## The bug this closes

Before this fix, `JournalRecipe::run` invoked `recipe-runner-rs --output-format
json`, deserialized a `RecipeEnvelope` from **stdout**, and took
`step_results.last().output` as the narrative. That captured stdout included
everything the copilot launcher and the agent print **around** the answer:

```text
2026-07-07T01:03:03.969016Z  WARN nested amplihack session detected — launching anyway session_id="session-18bfdc2635821690" depth=2
2026-07-07T01:03:04.604657Z  INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version="GitHub Copilot CLI 1.0.69-2."
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
● Read draft.ctx  │ /tmp/simard-journal-ctx-XXXX/draft.ctx  └ 153 lines read
```

The rewriter step's output began with the **launcher banner** (`WARN nested
amplihack session`, `INFO launching copilot`, `ℹ NODE_OPTIONS=…`) and the
agent's **own tool-call trace** (`● Read draft.ctx`, the `│` continuation, and
`└ 153 lines read`). Because that noise was captured as the step output, it
landed verbatim as the **leading paragraph** of the stored narrative. The rest
of the journal read well — the journaltone contract had landed — but the live
`GET /api/journal/render/<date>` returned this garbage as the first `<p>` of
`journal-narrative`, only reaching the real prose ("On July 7, 2026, Simard
operated in a largely self-directed decision cycle…") **after** it.

This is the exact
[raw-stdout-scrape antipattern](../concepts/copilot-launcher-preamble-stripping.md)
already retired for distillation ([#2679](https://github.com/rysweet/Simard/issues/2679))
and goal-decompose ([#2708](https://github.com/rysweet/Simard/issues/2708)). The
fix is **not** to strip `WARN` / `INFO` / `ℹ` / `●` / `│` / `└` lines with a
regex — that is the same brittle parsing wearing a different hat, and it rots
the moment the launcher changes a prefix. The fix removes stdout from the result
path entirely: each agent **writes** its finished report to a private,
per-invocation file, and Simard reads **that file**. A launcher banner or a
tool-call trace on stdout can no longer contaminate the narrative, because
stdout is never read as the result.

---

## Invocation

`JournalRecipe::run` shells out to `recipe-runner-rs` with an argv-vector (no
shell), one invocation per pass. The day-context / draft **input** still rides
the shared [`ContextFile`](./recipe-context-file-transport.md) channel exactly
as before (the E2BIG hardening from
[#2640 / #2692](../concepts/journal-recipe-spawn-e2big.md) is unchanged); the
new piece is the dedicated **output** file:

```text
recipe-runner-rs <recipe_path>
    -c <input_key>_path=<abs ContextFile path>   # day_context (drafter) | draft (reviewer)
    -c <output_key>=<abs result-file path>        # narrative_output | plain_output
```

with `AMPLIHACK_AGENT_BINARY` in the environment. Note the retired
`--output-format json` flag is **gone** — the result no longer comes from a
stdout envelope.

- Each pass owns an `output_key` threaded into `JournalRecipe::new`:
  the drafter (`journal-narrative.yaml`) uses **`narrative_output`**; the
  reviewer (`journal-plain-language.yaml`) uses **`plain_output`**. The same
  `run` body serves both, so **both** recipes are fixed — the reviewer is the
  live contaminant, but the drafter shares the vulnerable path.
- The output path is a fresh, mode-`0700` per-invocation tempdir
  (`tempfile::Builder`) plus a `<output_key>.md` filename, unique per call. The
  recipe interpolates it into the prompt via `{{narrative_output}}` /
  `{{plain_output}}` and instructs the agent to write **only** the finished
  report there. It is a **self-generated** absolute temp path — never LLM- or
  user-supplied text — so it is not sanitized.
- The `TempDir` guard is bound to a local that outlives both `.output()` **and**
  the file read, so the path exists while the recipe writes it and while Simard
  reads it; the directory and its contents are removed when the guard drops at
  the end of the call, **after** the file has been read.

`JournalRecipe::run` builds the argv and holds the tempdir guard;
`harvest_narrative_file` post-processes the finished `std::process::Output`.
`harvest_narrative_file` is factored out so the "stdout noise is inert" contract
is hermetically testable without spawning a subprocess.

---

## Reader API

One function captures the **report** (the result-file contents):

- `harvest_narrative_file(output: &std::process::Output, path: &Path) -> SimardResult<String>`
  — post-processes a finished invocation. It:
  1. surfaces a **non-zero exit** as a loud error carrying **both** truncated
     stderr **and** stdout (recipe-runner emits its structured failure on stdout
     and stderr is often empty, so the error string must include stdout or a
     non-zero exit risks a context-free message);
  2. uses the **`std::fs::metadata(path)` call itself as the missing-file check**
     — if the agent wrote nothing the `metadata` call errors and surfaces a loud
     "result file was not written" error — and then **size-guards** the present
     file: `metadata(path).len() > MAX_NARRATIVE_FILE_BYTES` (1 MiB) is a loud
     error **before** the read, so a runaway agent write cannot OOM the process
     (a *missing* file is therefore rejected here, at the `metadata` call, not
     after a read);
  3. reads the file via `String::from_utf8_lossy` (deliberately more lenient than
     the decompose sibling's `read_to_string`, and never `unwrap`, so malformed
     UTF-8 cannot panic the reader), then **trims**;
  4. rejects an **empty / whitespace-only** file loudly (the agent created the
     file but wrote no report);
  5. otherwise returns `Ok(prose)` — the trimmed, non-empty report.

There is deliberately **no stdout fallback**. Scraping stdout is exactly the
launcher-banner contamination this fix removes, and a silent fallback is a silent
failure. Because the result channel is clean prose (not a JSON envelope), there
is no deserialize step, no `RecipeEnvelope`, and no `parse_*` companion — the
old `RecipeEnvelope` / `RecipeStepResult` stdout-parsing types are **deleted**.

---

## Contract-marker recipe guard

Like goal-decompose, the daemon resolves each recipe hot-reload-first
(`~/.simard/prompt_assets/simard/recipes/`) then in-tree — but a **stale**
hot-reloaded recipe that predates this fix would tell the agent to print the
report to stdout and never write the result file, turning every journal pass
into a "result file was not written" failure. To keep hot-reload useful without
that footgun, resolution mirrors `decompose.rs`:

- Each recipe declares its `output_key` as the **result-file contract marker**
  (`narrative_output` for the drafter, `plain_output` for the reviewer).
- `recipe_declares_result_file(path)` returns whether the file at `path`
  contains that marker; an unreadable file is treated as non-compatible so it
  can never win the selection.
- `select_recipe(candidates, marker)` searches the candidates in priority order
  (hot, then in-tree) for the **first that both exists and declares the
  marker**; only if none is contract-aware does it fall back to the
  highest-priority existing recipe (so the run surfaces a loud, explicit error
  rather than silently reintroducing the stdout scrape).
- Two implementation notes distinguish this from the decompose sibling. First,
  because the two journal passes carry **different** markers, `select_recipe`
  takes the marker as a **parameter** rather than keying off decompose's single
  `RESULT_FILE_CONTRACT_MARKER` const. Second, resolution shifts from the old
  `resolve_recipe_path` "return the first existing path" shape to **collecting
  both candidate paths** (hot then in-tree) and handing that slice to
  `select_recipe`, so a contract-aware in-tree recipe can win over a stale hot
  one.

A stale-but-present recipe therefore never shadows a compatible one, and a
missing marker fails loudly rather than silently — the code fix and the asset
update must land together.

---

## Failure semantics

Every failure is explicit and **loud**, then degrades to the **honest offline
path** — never a stored stdout dump, never a silent success. The failure classes
for a single pass (`JournalRecipe::run` / `harvest_narrative_file`):

| Trigger | Error | Degrades to |
|---|---|---|
| `recipe-runner-rs` not spawnable / context-file write fails | `SimardError::AdapterInvocationFailed` (pre-exec spawn class → `classify_spawn_failure` + `record_step_failure`) | offline path |
| result tempdir cannot be created | `AdapterInvocationFailed` (recorded for the Overseer) | offline path |
| recipe **process** exited non-zero | `AdapterInvocationFailed` (truncated stderr **and** stdout) | offline path |
| result file **missing** (agent wrote nothing) | `AdapterInvocationFailed` | offline path |
| result file **empty / whitespace-only** | `AdapterInvocationFailed` | offline path |
| result file **oversized** (> 1 MiB) | `AdapterInvocationFailed` (before the read) | offline path |

The spawn-class failures are **not** swallowed into a bare `warn!`: they are
classified ([`classify_spawn_failure`](./terminal-failure-diagnosis-api.md)) and
recorded into the Overseer's failure sink
([`record_step_failure`](./overseer-tick-details.md)) so the next Observe pass
lifts them into a corrective signal; the degrade is then logged at `error` level.
Both are **preserved** by this change.

The **degrade target is the existing honest offline path**, unchanged:

- the drafter degrades to the deterministic
  [`TemplateDrafter`](./journal-api.md#the-two-pass-generation-pipeline) report assembler
  (whose "Remembered moments" section already drops raw error-log episodes via
  `is_raw_error_log_episode`, so a degraded journal is never a dump of historical
  error text);
- the reviewer degrades to the deterministic glossary
  [`scrub_jargon`](./journal-api.md#the-two-pass-generation-pipeline) reviewer.

And regardless of which path produced the text, `Generator::generate` still runs
its **unconditional** `scrub_secrets` redaction post-pass over the stored
narrative, so a credential survives neither path. The stored narrative is
therefore always one of: the agent's clean result-file report, the deterministic
report, or the glossary rewrite — **never** captured stdout.

---

## Examples

### Example 1 — clean capture (the headline case)

The runner's stdout carries the launcher banner (`WARN nested amplihack
session`, `INFO launching copilot`, `ℹ NODE_OPTIONS=…`) and the agent's
tool-call trace (`● Read draft.ctx`, `│`, `└ 153 lines read`); the agent wrote
its finished report to `plain_output`. Simard reads the file and the stored
narrative **begins with the real prose** — "On July 7, 2026, Simard operated in
a largely self-directed decision cycle…" — and contains **none** of the banner or
tool-trace lines.

### Example 2 — missing result file (no stdout fallback)

The process exits 0 but the agent never wrote the file — even if stdout happens
to carry a well-formed report, it is **not** scraped. `harvest_narrative_file`
returns an explicit `AdapterInvocationFailed`, the pass degrades to the honest
offline drafter/reviewer, and `scrub_secrets` still runs. The contaminated
stdout is never stored.

### Example 3 — non-zero exit is loud

A non-zero recipe exit yields an `AdapterInvocationFailed` carrying the truncated
stderr **and** stdout for the operator log and the Overseer, then degrades to the
offline path.

### Example 4 — oversized file is rejected before the read

A result file over the 1 MiB cap is rejected by the `metadata().len()` guard
**before** the file bytes are read, so a runaway agent write cannot exhaust memory; the
pass degrades to the offline path.

---

## Recipe & prompt asset sync

The daemon resolves each recipe hot-reload-first from
`~/.simard/prompt_assets/simard/recipes/` (see the contract-marker guard above),
falling back to the in-tree copy. On the deployed VM the daemon's working
directory is a worktree, so a **stale** hot-reload asset would run the old
"print your report to stdout" instructions even though the code now reads a file
— which is why the marker guard prefers a contract-aware recipe and otherwise
fails loudly.

`scripts/redeploy-local.sh` syncs **all** prompt assets — including both journal
recipes — into `~/.simard/prompt_assets/simard/` on every redeploy, and fails
the redeploy if zero recipes reached the hot-reload path (see the
[goal-decompose sync note](./goal-decompose-result-channel.md#recipe-prompt-asset-sync)).
As there, the redeploy guard enforces **presence, not freshness** (count-based),
so content freshness is a **separate** check.

> **Operational note.** After deploying this fix, verify the hot-reload assets
> carry the file-channel instructions:
>
> ```console
> $ grep -l narrative_output ~/.simard/prompt_assets/simard/recipes/journal-narrative.yaml
> $ grep -l plain_output     ~/.simard/prompt_assets/simard/recipes/journal-plain-language.yaml
> ```
>
> A stale asset that still says "emit the report to stdout" will make the agent
> write nothing to the file — a loud `AdapterInvocationFailed` by design that
> degrades to the honest offline path (the code fails loudly rather than
> silently reintroducing the stdout scrape). The code fix and both asset updates
> must land together.

> **Asset-edit checklist.** When updating each recipe, edit the whole file, not
> just the agent prompt body:
>
> - add the `output_key` to the `context:` block (`narrative_output: ""` /
>   `plain_output: ""`) so the substitution always resolves even if the caller
>   omits it;
> - add a **write-to-file-only** contract instruction to the prompt: write the
>   FULL report ONLY to the file at `{{narrative_output}}` / `{{plain_output}}`,
>   **not** to stdout;
> - keep the existing read-from-path instruction (`{{day_context_path}}` /
>   `{{draft_path}}`) — the **input** ContextFile channel is unchanged;
> - update the header comment block's `# Output:` line from "agent stdout" to the
>   result file (the `grep -l` freshness check matches the interpolated
>   `{{…_output}}` in the prompt body and will **not** flag a stale `# Output:
>   agent stdout` comment — review that header by eye).

---

## Security

- **No command injection.** The argv is built with
  `Command::new("recipe-runner-rs").arg(...)`; every context variable is a
  single `-c key=value` argument, so untrusted PR / episode text in the day
  context can never inject a shell command or extra flag. `<output_key>` is a
  compile-time constant and the result path is a tempfile-generated absolute
  path (never attacker-supplied). This argv-only construction must be preserved.
- **Private result file.** The output file lives inside a mode-`0700`,
  randomized-path per-invocation tempdir (mirroring the input `ContextFile`) and
  is removed when the invocation returns; the file may transiently hold
  pre-redaction narrative text, so it must never land on a shared stdout stream —
  closing the predictable-`/tmp`-path TOCTOU / symlink / info-leak surface.
- **Bounded file read.** `harvest_narrative_file` size-guards the file (rejects
  `> 1 MiB` before it reads any file bytes) so a runaway agent cannot exhaust
  memory, and reads via `from_utf8_lossy` rather than a `String::from_utf8(…)`
  `unwrap` that could panic on non-UTF-8.
- **Bounded error output.** Error messages reuse `truncate(…, 200)`, so a large
  hostile document never echoes its full content into a log; no new `println!` /
  `eprintln!` is added, and the raw file body is never logged.
- **Secret redaction preserved.** `Generator::generate` runs its unconditional
  `scrub_secrets` post-pass downstream of `harvest_narrative_file`, so a
  credential never reaches the durable narrative regardless of which path
  produced the text.
- **XSS unchanged.** `render_entry_html` still HTML-escapes the narrative (the
  existing HTML-escaping XSS tests, `html_is_xss_safe` /
  `html_report_is_still_xss_safe`): the clean channel changes *content* only, and
  the render-time escape is preserved.
- **No stdout fallback.** A missing / empty / oversized file is an explicit
  `Err`; stdout is never scraped as a backup result channel.

---

## Code location

| Item | File |
|---|---|
| `JournalRecipe::run` (adds `-c <output_key>=<path>`, drops `--output-format json`, holds the tempdir guard) | `src/journal/recipe.rs` |
| `harvest_narrative_file` (read file / size-guard / loud errors, no stdout fallback) | `src/journal/recipe.rs` |
| `recipe_declares_result_file` / `select_recipe` (contract-marker guard) | `src/journal/recipe.rs` |
| `RecipeDrafter` / `RecipeReviewer` (degrade to offline on `Err`, unchanged) | `src/journal/recipe.rs` |
| Drafter recipe (writes `{{narrative_output}}`) | `prompt_assets/simard/recipes/journal-narrative.yaml` |
| Reviewer recipe (writes `{{plain_output}}`) | `prompt_assets/simard/recipes/journal-plain-language.yaml` |
| `Generator::generate` (mandatory reviewer + unconditional `scrub_secrets`, unchanged) | `src/journal/generate.rs` |
| Recipe asset sync | `scripts/redeploy-local.sh` |
| Tests | `src/journal/tests_clean_result_channel.rs` (registered in `src/journal/mod.rs`) |

The `JournalDrafter` / `JournalReviewer` traits, `TemplateDrafter`,
`GlossaryReviewer`, `scrub_jargon`, `scrub_secrets`, the `ContextFile` **input**
channel, and the `is_raw_error_log_episode` offline filter are **unchanged** —
the transport fix is additive and back-compatible.

---

## Testing

The clean-channel behavior is pinned by `src/journal/tests_clean_result_channel.rs`
(subprocess-free, using a synthetic `Output` built with an `output_with(stdout,
code)` helper and a `tempfile` result path). The headline test is a **RED
regression** against the reported bug:

| Test | Coverage |
|---|---|
| noisy-stdout-is-inert (RED on the bug) | An `Output` whose stdout carries the launcher banner **plus** the tool-call trace, and a result file containing clean prose ⇒ the harvested narrative **starts with the prose** and contains **none** of: `nested amplihack session`, `launching copilot binary`, `NODE_OPTIONS=`, `Read draft.ctx`, `lines read`, or the `●` / `│` / `└` box-drawing glyphs. |
| missing-file-is-loud | A clean exit with **no** result file ⇒ `AdapterInvocationFailed`, even when stdout carries a well-formed report. |
| empty-file-is-loud | An empty / whitespace-only file ⇒ loud `AdapterInvocationFailed`. |
| oversized-file-is-loud | A file over the 1 MiB cap ⇒ loud error **before** a full read. |
| nonzero-exit-is-loud | A non-zero recipe exit ⇒ loud error carrying truncated stderr **and** stdout. |
| marker guard | `recipe_declares_result_file` detects `narrative_output` / `plain_output`; `select_recipe` never lets a stale (marker-less) hot recipe shadow a contract-aware one, and returns `None` only when no candidate exists. |

Run the suite:

```console
$ cargo test -p simard --lib journal::tests_clean_result_channel
```

The headline test **fails** against the pre-fix stdout-scrape code (the harvested
text begins with the banner) and **passes** once `run` reads the result file.

---

## Migration — from a dedicated file to the durable channel

The dedicated `narrative_output` / `plain_output` file is the **available-now**
clean substrate, matching the shape of the shipped goal-decompose and distill
file channels. The durable substrate is the `semanticchannel`
clean-agent-result-channel work in `amplihack-rs`
([#856](https://github.com/rysweet/Simard/issues/856), pinned in Simard via
amplihack-agent-eval), and this consumer **satisfies the semanticchannel
"a consumer uses the clean channel" verification**. Once `recipe-runner-rs`
exposes a first-class structured result channel, the agent → Simard handoff
migrates from the temp file to that channel with no change to
`harvest_narrative_file`'s trim/validate contract, the `JournalDrafter` /
`JournalReviewer` traits, or `Generator::generate` — only `JournalRecipe::run`'s
transport wiring moves.

Per the memory-architecture policy (G2): the clean-result capture that belongs
in the shared agent-invocation layer stays upstream in amplihack-rs; Simard keeps
only the **thin journal-side seam** (this file channel), so no engine pin bump is
required for this fix.

---

## Out of scope

- **The narrative body and tone.** The report structure, the journaltone /
  [tone contract](../concepts/journal-report-tone-contract.md), and the
  plain-language rewrite rules are unchanged — this change fixes only how the
  agent's report reaches Simard.
- **The input `ContextFile` channel** and the
  [E2BIG spawn hardening](../concepts/journal-recipe-spawn-e2big.md) are
  unchanged; this fix adds the **output** file, it does not touch the input file.
- **The offline fallback.** `TemplateDrafter`, `scrub_jargon`, `scrub_secrets`,
  and the `is_raw_error_log_episode` filter are the honest last resort and are
  preserved exactly.
- **`semanticchannel` implementation** in `amplihack-rs` (the durable substrate)
  is tracked upstream; this Simard-side fix uses the dedicated file now and
  migrates later.
