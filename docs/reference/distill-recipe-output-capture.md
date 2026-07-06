---
title: Distill recipe output capture
description: How Simard's distillation pass reliably captures the distill agent's { "facts": [...], "procedures": [...] } JSON — the dedicated facts-file channel (facts_output_path), the parse_facts_document / parse_facts parser, field-tolerant deserialization, failure semantics, and the redeploy-local.sh recipe-asset sync that keeps the hot-reload path current.
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../architecture/episode-distillation.md
  - ./distill-parse-failure-recovery.md
  - ./automatic-distillation-scheduler.md
  - ./cognitive-memory-provenance.md
  - ./text-parsing-wire-formats.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../memory.md
---

# Distill recipe output capture

> **Status — implements the
> [#2622](https://github.com/rysweet/Simard/issues/2622) /
> [#2619](https://github.com/rysweet/Simard/issues/2619) fix.**
> The distill agent's answer is captured from a **dedicated facts file** the
> agent writes, not from `recipe-runner-rs` stdout. Present tense below
> describes the shipped behavior. Locations:
> parser + invocation `src/memory_consolidation/distillation.rs`;
> tests `src/memory_consolidation/distillation_tests.rs` and the hermetic
> `issue_2622_file_channel_tests` in `distillation.rs`;
> recipe `prompt_assets/simard/recipes/distill-episodes.yaml`;
> asset sync `scripts/redeploy-local.sh`.

The episode-distillation pass turns batches of episodic memory into
semantic **facts** and reusable **procedures** by shelling out to
`recipe-runner-rs` and reading the distill agent's JSON output. This page
documents the **output-capture contract** between Simard and the distill
agent: how the agent's `{ "facts": [...], "procedures": [...] }` object is
reliably captured, what happens on failure, and how the recipe asset reaches
the running daemon.

For the surrounding pipeline (when the pass fires, the threshold gate, how
facts are stored), see
[Episode distillation](../architecture/episode-distillation.md). For the
automatic scheduler and the procedures extension, see the
[automatic distillation scheduler API](./automatic-distillation-scheduler.md).

---

## Why this contract exists — the file channel

`recipe-runner-rs` runs the `distill-episodes` recipe — a single `distill`
agent step that asks the LLM to produce `{ "facts": [...], "procedures": [...] }`.

Capturing that object from the subprocess's **stdout** is brittle: the
copilot binary prints a launcher banner and log lines to the inherited
stdout *before and around* the agent's answer, e.g.

```text
2026-07-06T02:45:12Z  INFO launching copilot binary=/home/.../copilot version="GitHub Copilot CLI 1.0.69-1."
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference)
Run 'copilot update' to update
```

Live daemon logs (02:45–02:47) confirmed the failure mode: the captured
step "output" was literally this launcher banner, and a stdout scan for a
`{ "facts": [...] }` object matched the banner instead of the answer — every
distill pass failed with `class="parse-failure"` and memory fact-yield was
near zero.

The fix removes stdout from the result path entirely. The agent **writes**
its JSON envelope to a private, per-invocation file whose absolute path is
handed to the recipe as a context variable. Simard reads **that file** after
the runner exits. A launcher banner on stdout can no longer contaminate the
parse, because stdout is never read as the result.

---

## Invocation

The Rust-side invocation shells out to `recipe-runner-rs` with an
argv-vector (no shell):

```text
recipe-runner-rs <recipe_path>
    --output-format json
    -c episodes=<compact-json-array>
    -c strict_json_instruction=<empty | retry-reinforcement>
    -c facts_output_path=<absolute path to a fresh tempdir/facts.json>
```

with `AMPLIHACK_AGENT_BINARY` in the environment.

- `facts_output_path` is a fresh, mode-`0700` tempdir (`tempfile` crate) plus
  a `facts.json` filename, unique per invocation. The recipe interpolates it
  into the prompt via `{{facts_output_path}}` and instructs the agent to write
  **only** the JSON envelope there.
- `--output-format json` is retained only so that a **runner-level** failure
  surfaces a structured error on stdout for the terminal-failure message. The
  distill **result** is never read from stdout.
- After the runner exits, Simard reads `facts_output_path`, and the tempdir
  (and its contents) is removed when the invocation returns.

`invoke_recipe` builds the argv; `harvest_facts_file` post-processes the
finished `std::process::Output`: a non-zero exit is an explicit terminal
error carrying truncated stderr/stdout; a clean run reads the facts file.
`harvest_facts_file` is factored out so the "stdout noise is ignored"
contract is hermetically testable without spawning a subprocess.

---

## Parser API

Two `pub(crate)` functions parse the **facts document** (the file contents):

- `parse_facts_document(document: &str) -> SimardResult<DistillOutput>` — the
  full parser (facts AND procedures).
- `parse_facts(document: &str) -> SimardResult<Vec<DistilledFact>>` — a thin
  facts-only wrapper over `parse_facts_document`, retained for the legacy
  `DistillRecipeRunner::run` entry point.

`parse_facts_document`:

1. An **empty** document ⇒ explicit `Err` (the agent produced no output;
   never a hollow `Ok`; no stdout fallback).
2. Otherwise `scan_cleaned_for_facts` deserializes the
   `{ "facts": [...], "procedures": [...] }` envelope. Because the file is a
   clean channel, this is a strict `serde` parse with only light **format**
   leniency: an LLM may wrap the answer in a Markdown code fence or a little
   leading/trailing prose in the file, so the parser prefers the last balanced
   `{...}` object that carries a grounded fact. This is field/format leniency
   on a clean channel — **not** the launcher-banner stdout scraping this fix
   removed.
3. If no facts object is present ⇒ explicit `Err`.

### Field-tolerant deserialization

Individual `facts[]` fields are deserialized with `de_lenient_string`: a
missing / `null` / bare-scalar field (a common LLM deviation) coerces to the
empty string rather than sinking the whole envelope. Off-spec concepts are
dropped by `RecipeEnvelope::into_facts` (concept allow-list, with surface-form
canonicalization); an empty/ungrounded fact is later quarantined by the ISAO
reliability gate (`assess_fact_reliability`). One malformed fact never sinks
its well-formed siblings.

---

## Failure semantics

Every failure is explicit — never silent degradation.
`classify_distill_error` buckets each failure by the stable leading prefix of
its message:

| Class                     | Trigger                                                                 | Retried in-cycle? |
|---------------------------|-------------------------------------------------------------------------|-------------------|
| `SpawnFailure`            | `recipe-runner-rs` not spawnable, or facts tempdir not creatable        | no                |
| `CopilotTerminalFailure`  | the recipe **process** exited non-zero                                  | yes               |
| `ParseFailure`            | process exited 0 but the facts file was **missing / empty / unparseable** | yes             |
| `SerializeFailure`        | the episodes payload failed to serialize                                | no                |
| `Other`                   | anything else                                                           | no                |

Transient classes (`ParseFailure`, `CopilotTerminalFailure`) are retried once
in-cycle (`DISTILL_PARSE_RETRY_MAX`) with JSON-format reinforcement threaded
into the recipe. On final failure the pass returns `Err` **without** marking
any episode, so the batch stays fully retry-eligible next pass. `ParseFailure`
is the class the healthy-path fix drives toward zero.

---

## Examples

### Example 1 — successful capture

The runner's stdout carries a launcher banner; the agent wrote a clean
envelope to `facts_output_path`. Simard reads the file and yields facts:

```json
{ "facts": [ { "concept": "pr-pattern", "content": "warm the shared cache before lbug pin bumps", "source_episode_id": "epi_1" } ], "procedures": [] }
```

### Example 2 — missing facts file (no stdout fallback)

The process exits 0 but the agent never wrote the file — even if stdout
happens to carry a well-formed facts object, it is **not** scraped. The pass
returns an explicit `ParseFailure` and retries.

### Example 3 — nothing worth distilling

A valid `{ "facts": [], "procedures": [] }` document is a **success** (zero
facts), never a parse failure.

---

## Recipe asset sync

The daemon resolves the recipe hot-reload-first from
`~/.simard/prompt_assets/simard/recipes/`. If that directory is missing the
`distill-episodes.yaml` asset, the runner falls back to the in-tree copy — but
on the deployed VM the daemon's working directory is a worktree, so a missing
hot-reload asset previously left the daemon running a recipe set that did not
match the deployed code.

`scripts/redeploy-local.sh` syncs **all** prompt assets — including
`prompt_assets/simard/recipes/*.yaml` — into `~/.simard/prompt_assets/simard/`
on every redeploy via a scoped `cp -rf`, then lists every synced
`recipes/*.yaml`, reports a synced-vs-source **count**, **fails the redeploy**
(`exit 1`) if zero recipes reached the hot-reload path, and **warns** on count
drift:

```text
[redeploy] synced 7/7 recipe asset(s) → /home/azureuser/.simard/prompt_assets/simard/recipes
[redeploy]   recipe: distill-episodes.yaml
…
```

> **Operational note.** Verify the deployed set with
> `ls ~/.simard/prompt_assets/simard/recipes/`. A stale hot-reload
> `distill-episodes.yaml` will run the in-tree fallback recipe; the sync fix
> keeps the hot-reload recipe set current, and the file-channel fix ensures the
> captured result is actually parsed.

---

## Security

- **No command injection.** The argv is built with
  `Command::new("recipe-runner-rs").arg(...)`; the episodes payload and the
  facts path are single `-c key=value` arguments. Content is never interpolated
  into a shell line. This argv-only construction must be preserved.
- **Private result file.** `facts_output_path` is inside a mode-`0700`
  per-invocation tempdir and is removed when the invocation returns; the
  agent's answer never lands on a shared stdout stream.
- **Bounded error output.** The brace scan is linear/iterative and each
  candidate parse is bounded by `serde_json`'s recursion limit; error messages
  reuse `truncate(…, 200)`, so a large hostile document never echoes its full
  content.
- **No stdout fallback.** A missing/empty/unparseable facts file is an
  explicit `Err`; stdout is never scraped as a backup result channel (a silent
  fallback is a silent failure).
- **Process visibility (known limitation).** `episodes=<json>` is passed on the
  argv, so it is visible via `ps`/`/proc` on a shared host. Moving the payload
  off the argv is a recommended follow-up if a future `recipe-runner-rs`
  supports it.

---

## Code location

| Item                                   | File                                                 |
|----------------------------------------|------------------------------------------------------|
| `invoke_recipe` (adds `facts_output_path`) | `src/memory_consolidation/distillation.rs`       |
| `harvest_facts_file` (read file / terminal error) | `src/memory_consolidation/distillation.rs` |
| `parse_facts_document` / `parse_facts`  | `src/memory_consolidation/distillation.rs`           |
| `scan_cleaned_for_facts`, `RecipeEnvelope`, `de_lenient_string` | `src/memory_consolidation/distillation.rs` |
| Recipe (writes `{{facts_output_path}}`) | `prompt_assets/simard/recipes/distill-episodes.yaml` |
| Recipe asset sync                       | `scripts/redeploy-local.sh`                          |
| Tests                                   | `src/memory_consolidation/distillation_tests.rs` + `issue_2622_file_channel_tests` |

---

## Testing

The file-channel behavior is pinned by the hermetic `issue_2622_file_channel_tests`
(in `distillation.rs`) and the document-parse tests in `distillation_tests.rs`:

| Test                                                    | Coverage                                                                                   |
|---------------------------------------------------------|--------------------------------------------------------------------------------------------|
| `launcher_banner_on_stdout_does_not_cause_parse_failure` | A launcher banner on stdout + a valid facts file ⇒ **success** with facts (the headline).  |
| `missing_facts_file_is_parse_failure_never_stdout_fallback` | A missing file ⇒ `ParseFailure`, even when stdout carries a facts object.                |
| `nonzero_exit_is_terminal_failure_with_context`         | A non-zero exit ⇒ explicit `CopilotTerminalFailure`.                                        |
| `clean_run_reads_facts_verbatim_from_file`              | A clean run reads the envelope from the file.                                               |
| `empty_facts_document_is_parse_failure` / `banner_only_document_is_parse_failure` | Empty / answerless documents error explicitly.                   |
| `fenced_facts_document_still_parses`                    | A Markdown-fenced document still parses (clean-channel format leniency).                    |
| `document_yields_facts_and_procedures`                  | Facts + procedures with provenance intact.                                                 |
| `document_drops_unknown_concepts`                       | The concept allow-list still applies.                                                       |
| `document_error_does_not_leak_full_payload` / `document_tolerates_deeply_nested_input_without_panic` | Bounded, panic-free error output.             |
| `document_handles_large_valid_input`                    | A large valid document (1 000 facts) extracts every fact.                                   |

---

## History — the retired stdout-envelope capture

Before this fix the distill result was scraped from `recipe-runner-rs`'s
`--output-format json` **stdout envelope** (`{ "success", "step_results":
[{ "output": "…" }] }`), with a tolerant multi-tier parser
(`parse_recipe_output_full`, `RecipeRunnerEnvelope`, `scan_for_facts_object`)
and shared launcher-banner stripping. That approach (issues #2401, #2461,
#2496, #2504, #2512, #2517, #2570) chased banner/ANSI/log contamination on
stdout with ever-more-defensive string scanning. Issues #2622/#2619 replaced
it with the file channel: the agent writes a clean file and stdout is no longer
the result, so the entire class of launcher-banner parse failures is
structurally eliminated rather than post-processed. The runner-envelope
deserialization types and their stdout-scraping tiers were retired.

---

## Out of scope

- **Sibling recipe shims** (`recipe_merge_judge`, `recipe_progress_checker`)
  parse their own text output and are **not** affected by this change.
- **Subprocess wall-clock timeout** for a hung agent step is a recommended
  follow-up.
- **Moving the episodes payload off the argv** (stdin / `0600` temp file) is a
  follow-up hardening, contingent on `recipe-runner-rs` support.
