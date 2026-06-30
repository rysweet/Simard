---
title: Distill recipe output capture
description: How Simard's distillation pass reliably captures the distill agent's { "facts": [...], "procedures": [...] } JSON from recipe-runner-rs — the --output-format json invocation, the RecipeRunnerEnvelope / RecipeRunnerStepResult deserialization types, the three-tier parser in parse_recipe_output_full, the agent-step selection rule, failure semantics, and the redeploy-local.sh recipe-asset sync that keeps the hot-reload path current.
last_updated: 2026-06-29
owner: simard
doc_type: reference
related:
  - ../architecture/episode-distillation.md
  - ./automatic-distillation-scheduler.md
  - ./cognitive-memory-provenance.md
  - ./text-parsing-wire-formats.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../memory.md
---

# Distill recipe output capture

> **Status — implements the
> [#2401](https://github.com/rysweet/Simard/issues/2401) fix.**
> The `recipe-runner-rs` 0.3.6 envelope shape documented below is **verified**
> against the installed binary, and the Simard-side invocation, parser, and
> deserialization types described here are **implemented** in this PR. Present
> tense below describes the shipped behavior. Locations:
> parser + invocation `src/memory_consolidation/distillation.rs`;
> tests `src/memory_consolidation/distillation_tests.rs`;
> recipe `prompt_assets/simard/recipes/distill-episodes.yaml`;
> asset sync `scripts/redeploy-local.sh`.

The episode-distillation pass turns batches of episodic memory into
semantic **facts** and reusable **procedures** by shelling out to
`recipe-runner-rs` and parsing the distill agent's JSON output. This
page documents the **output-capture contract** between Simard and
`recipe-runner-rs` 0.3.6: how the agent's
`{ "facts": [...], "procedures": [...] }` object is reliably extracted
from the recipe runner's structured envelope, what happens on failure,
and how the recipe asset reaches the running daemon.

For the surrounding pipeline (when the pass fires, the threshold gate,
how facts are stored), see
[Episode distillation](../architecture/episode-distillation.md). For the
automatic scheduler and the procedures extension, see the
[automatic distillation scheduler API](./automatic-distillation-scheduler.md).

---

## Why this contract exists

`recipe-runner-rs` runs the `distill-episodes` recipe — a single
`distill` agent step that asks the LLM to return
`{ "facts": [...], "procedures": [...] }` and nothing else. Simard
captures that object from the subprocess's **stdout**.

The runner has **two** stdout formats, selected by `--output-format`:

| Format          | Stdout content                                                                 |
|-----------------|--------------------------------------------------------------------------------|
| `text` (default) | A human status banner only. The agent step's output is **not** on stdout.     |
| `json`          | A structured envelope whose `step_results[].output` holds the agent's output. |

In `text` mode, stdout is just:

```
Recipe: distill-episodes (v1.0.0)
Steps: 1
Recipe 'distill-episodes': SUCCESS (18.0s)
  [completed] distill (18.0s)
```

The agent's actual `{ "facts": [...] }` payload is absent. Any parser
that scans this banner for a facts object fails on **every** cycle, even
though the recipe step itself succeeded (the LLM ran for 18–28s). That
failure is non-fatal — no markers are set, the batch retries — so the
pass silently no-ops forever and distillation produces zero facts in
production.

The fix is to invoke the runner with `--output-format json` and parse
the **envelope**. This page pins the envelope shape to the real 0.3.6
output and specifies the tolerant parser that extracts the agent payload
from it.

---

## The recipe-runner-rs 0.3.6 JSON envelope

With `--output-format json`, `recipe-runner-rs` writes a single JSON
object to stdout. Captured verbatim from the installed
`recipe-runner 0.3.6` binary:

```json
{
  "recipe_name": "distill-episodes",
  "success": true,
  "step_results": [
    {
      "step_id": "distill",
      "status": "completed",
      "output": "{\"facts\":[{\"concept\":\"bug-pattern\",\"content\":\"…\",\"source_episode_id\":\"e1\"}],\"procedures\":[]}",
      "error": "",
      "duration": 18.04
    }
  ],
  "duration": 18.05
}
```

Field reference:

| Field                     | Type            | Meaning                                                                 |
|---------------------------|-----------------|-------------------------------------------------------------------------|
| `recipe_name`             | string          | The recipe's `name` (`distill-episodes`).                               |
| `success`                 | bool            | `true` only if every step succeeded. `false` if any step failed.        |
| `step_results`            | array           | One entry per executed step, in order.                                  |
| `step_results[].step_id`  | string          | The recipe step's `id` (`distill`).                                     |
| `step_results[].status`   | string          | `completed` on success, `failed` on error.                             |
| `step_results[].output`   | string          | The step's captured stdout. For the `distill` agent step this is the agent's `{ "facts": …, "procedures": … }` JSON **as a string** (escaped inside the envelope). |
| `step_results[].error`    | string          | Empty (`""`) on success; a diagnostic message on failure.              |
| `step_results[].duration` | number          | Step wall-clock seconds.                                               |
| `duration`                | number          | Total recipe wall-clock seconds.                                       |

> **The agent payload lives in `step_results[].output` as a string.**
> The parser must select the right step, read `output`, then parse the
> facts object out of that string. The runner does **not** merge the
> agent JSON into the envelope's top level.

### Failure envelope

When a step fails, the runner still emits a well-formed envelope **and**
exits with a non-zero process code:

```json
{
  "recipe_name": "distill-episodes",
  "success": false,
  "step_results": [
    {
      "step_id": "distill",
      "status": "failed",
      "output": "",
      "error": "Step 'distill' failed: …",
      "duration": 0.004
    }
  ],
  "duration": 0.004
}
```

| Condition          | Process exit code | `success` | `status`     |
|--------------------|-------------------|-----------|--------------|
| All steps OK       | `0`               | `true`    | `completed`  |
| A step failed      | `1`               | `false`   | `failed`     |

Both signals are present, but they are checked on **different layers**, and
the order matters:

- **`invoke_recipe` checks the process exit code first.** A failed run exits
  non-zero, so `invoke_recipe` returns `Err` *before the parser ever sees the
  envelope*. This is the path that actually fires in production — see
  [Example 2](#example-2--failed-recipe-run), whose log line is
  `recipe exited with status 1`.
- **The parser's `success == false` guard is defense-in-depth.** Because the
  exit-code guard fires first, the parser's check is effectively unreachable
  today; it only matters if a future `recipe-runner-rs` emits
  `success == false` while still exiting `0`. It is retained deliberately so a
  failed run can never yield facts even if the exit-code contract changes.

Either way, the parser never extracts facts from a failed run.

---

## Invocation

The production runner is `RecipeRunnerSubprocess::invoke_recipe`
(`src/memory_consolidation/distillation.rs`). It already builds the argv as a
vector (no shell) and serializes the episode batch to compact JSON. The #2401
change adds `--output-format json` to that existing invocation so the agent
payload reaches stdout:

```text
recipe-runner-rs <recipe_path> \
  --output-format json \
  -c episodes=<compact-json-array>
```

with the agent binary supplied via the environment:

```text
AMPLIHACK_AGENT_BINARY=<copilot|claude|codex|…>
```

Notes:

- **`--output-format json` is required.** Without it the agent payload
  never reaches stdout (see [Why this contract exists](#why-this-contract-exists)).
- **The agent binary is selected via the `AMPLIHACK_AGENT_BINARY`
  environment variable**, resolved by
  `session_builder::LlmProvider::resolve_agent_binary()`. This is the
  same mechanism that already made the recipe step *run* successfully in
  production, so it is retained unchanged. The equivalent
  `--agent-binary` flag is **not** added — one configuration surface is
  enough, and adding a second risks the two disagreeing.
- **The episodes payload is passed as a single `-c episodes=<json>`
  argument** using `Command::new(...).arg(...)` argv construction — never
  interpolated into a shell line. Episode content cannot break out of the
  argument into flags, the recipe path, or the binary name. See
  [Security](#security).
- **Failures are surfaced, never swallowed.** A non-zero exit returns a
  non-fatal `Err` carrying a truncated excerpt of both stderr and stdout
  (the structured error lives in the JSON envelope on stdout), and the
  parser's error messages are truncated to ≤ 200 chars. An explicit
  captured-stdout byte cap is a recommended follow-up (see
  [Security](#security)), not part of this fix.

The recipe path is resolved with Simard's standard hot-reload-first
order (see [Recipe asset sync](#recipe-asset-sync)):

1. `~/.simard/prompt_assets/simard/recipes/distill-episodes.yaml` (hot-reload / user override)
2. `<repo>/prompt_assets/simard/recipes/distill-episodes.yaml` (in-tree default)

---

## Parser API

The captured stdout is parsed by two functions in
`src/memory_consolidation/distillation.rs`, both `pub(crate)` so the
test module can exercise them directly:

```rust
/// Parse recipe-runner-rs stdout into facts AND procedures.
///
/// Three-tier strategy (see below). Returns `Err` on a failed recipe
/// run or when no facts object can be located — the caller treats `Err`
/// as the retry-safe "no markers set" path.
pub(crate) fn parse_recipe_output_full(raw: &str) -> SimardResult<DistillOutput>;

/// Facts-only wrapper retained for the legacy `DistillRecipeRunner::run`
/// entry point and its unit tests.
pub(crate) fn parse_recipe_output(raw: &str) -> SimardResult<Vec<DistilledFact>>;
```

`parse_recipe_output` delegates to `parse_recipe_output_full` and
returns only `output.facts`.

### Deserialization types

Two transient structs model the 0.3.6 envelope. They are distinct from
the inner `RecipeEnvelope` (which models the agent's
`{ "facts": …, "procedures": … }` object) so there is no name collision:

```rust
/// recipe-runner-rs 0.3.6 top-level JSON envelope.
#[derive(serde::Deserialize)]
struct RecipeRunnerEnvelope {
    success: bool,                            // REQUIRED — no serde default
    step_results: Vec<RecipeRunnerStepResult>, // REQUIRED — no serde default
}

/// One entry of `step_results`.
#[derive(serde::Deserialize)]
struct RecipeRunnerStepResult {
    #[serde(default)]
    step_id: String,
    #[serde(default)]
    status: String,
    /// `serde_json::Value` so a String output (0.3.6) OR a future
    /// object output both deserialize without a contract break.
    #[serde(default)]
    output: serde_json::Value,
}
```

`success` and `step_results` are **required** (no `#[serde(default)]`):
text from the old human banner, a bare facts object, or any other
non-envelope payload fails Tier 1 deserialization cleanly and falls
through to Tier 2. `output` is typed as `serde_json::Value` to tolerate
both a JSON **string** (the 0.3.6 contract) and a future JSON **object**.

### Three-tier parse strategy

`parse_recipe_output_full` tries three tiers in order and stops at the
first that yields a facts object:

**Tier 1 — runner envelope (the production path).**

1. Deserialize stdout as `RecipeRunnerEnvelope`.
   - **Tier 1a (fast path):** parse the trimmed stdout directly. Clean
     production output (the envelope is the first byte) parses here with zero
     extra work.
   - **Tier 1b (noise-tolerant recovery, [#2512](https://github.com/rysweet/Simard/issues/2512)):**
     if the direct parse fails, recover the envelope through the **shared**
     `recipe_output` chokepoint (`recover_runner_envelope`). The copilot
     subprocess prints its launch banner (`ℹ NODE_OPTIONS=…`,
     `launching copilot binary=… version="GitHub Copilot CLI …"`,
     `Run 'copilot update'…`) and intermittent ANSI tracing lines to the
     **inherited stdout *before*** recipe-runner-rs emits its JSON envelope, so
     the captured stdout **begins with the banner instead of the `{`** and the
     Tier-1a parse rejects it — losing the envelope and its escaped facts
     payload. Tier 1b strips that preamble (the same `strip_recipe_noise` +
     `strip_ansi` dual view + string-aware `balanced_objects` end-first scan the
     inner step-output path uses) and recovers the envelope. This is the
     *outer-envelope* analog of the inner step-output banner stripping below,
     and of the #2504 decide/orient launch-banner fix. A leading non-envelope
     log record (e.g. `{"level":"info",…}`) lacks `success`/`step_results`, so
     it is never mistaken for the envelope.
2. If `success == false`, return `Err` immediately — never extract facts
   from a failed run. (Defense-in-depth: a failed run already exits non-zero,
   so `invoke_recipe`'s exit-code guard normally returns `Err` before this
   point — see [Failure semantics](#failure-semantics).) A banner-prefixed
   `success == false` envelope recovered by Tier 1b still routes here (it is
   *committed to* once recovered), so it surfaces a `RecipeReportedFailure`,
   never a silent drop.
3. Select the step: the entry with `step_id == "distill"`; if absent,
   the **last** entry with `status == "completed"`. (The `step_id` rule
   pins to the recipe's named step; the last-completed fallback tolerates
   a future step rename.)
4. Read that step's `output`:
   - If `output` is a **string**, scan it for the balanced
     `{ "facts": … }` object (see [`scan_for_facts_object`](#scan_for_facts_object)).
   - If `output` is already an **object** containing `"facts"`,
     deserialize it directly.
5. Filter and return the facts/procedures (concept allow-list +
   procedure validity, identical to the existing inner parser).

**Tier 2 — tolerant raw scan (backward compatibility).**

If stdout does not parse as a `RecipeRunnerEnvelope` at all (e.g. a unit
test or mock that emits a bare `{ "facts": … }` object, or prose with the
object embedded), scan the **raw stdout** directly for a balanced
`{ "facts": … }` object. This keeps all existing `DistillRecipeRunner`
mock tests and the plain-object / prose-wrapped tests green.

**Tier 3 — explicit failure.**

If neither tier locates a parseable facts object — or Tier 1 saw
`success == false` or no completed `distill` step — return an explicit
`Err` whose message includes a **truncated** (≤ 200 char) excerpt of the
stdout. No full payloads are placed in `info`/`warn` logs; only the
truncated excerpt surfaces, and full stdout appears only at `debug`
level.

```
distill: recipe run did not yield a parseable { "facts": [...] } object: <≤200-char excerpt>
```

The truncation keeps the error bounded (the
`parser_error_does_not_leak_full_payload` test asserts the message stays
short and never echoes content past the truncation window).

There is **no silent degradation**: every failure path returns `Err` and
is surfaced upstream.

### `scan_for_facts_object`

Tiers 1 and 2 share one helper:

```rust
/// Locate and parse a balanced `{ "facts": … }` object in `s`, returning the
/// facts from the LAST balanced object that parses (so a leading banner or
/// thinking object does not shadow the agent's answer — issue #2461).
/// Iterative (no recursion in the scan itself), so deeply nested input cannot
/// grow the stack; each candidate parse is bounded by `serde_json`'s own
/// recursion limit, which rejects pathological nesting.
fn scan_for_facts_object(s: &str) -> Option<DistillOutput>;
```

It first tries the fast path (the whole string is the object), then collects
every balanced top-level `{...}` substring with a **string-aware** brace
scan (braces inside JSON string literals are ignored, so a brace in a fact's
`content` cannot corrupt depth accounting) and returns the facts from the
**last non-empty** one that parses (falling back to an empty `{"facts":[]}`
only when no object carries facts/procedures). The scan is linear in the input
length; pathologically nested braces parse to an `Err` (via serde's recursion
limit) rather than panicking — see `parser_tolerates_deeply_nested_input_without_panic`.

> **Shared noise stripping ([#2496](https://github.com/rysweet/Simard/issues/2496)).**
> Before scanning, the distill parser strips ANSI codes and non-payload lines
> through the **shared** `recipe_output::strip_recipe_noise` chokepoint — the
> same `is_noise_line` predicate the OODA brains use — rather than a distill-private
> cleaner. This is what lets the parser survive the Copilot CLI launch-log
> preamble (`ℹ NODE_OPTIONS=…`, `launching copilot binary=… version="GitHub
> Copilot CLI …"`, `Run 'copilot update'…`) that PR
> [#2500](https://github.com/rysweet/Simard/pull/2500) first pinned as a distill
> regression. Because the launcher-shape detection now lives at the single
> chokepoint, hardening it once re-hardens distill **and** decide/orient/
> lifecycle/merge-judge together; distill carries **no** parallel launcher
> cleaner. See
> [Text-parsing wire formats § Protocol 0](./text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output)
> and [Concept: Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md).

---

## Failure semantics

The output-capture fix **preserves** the existing graceful-skip and
non-fatal-error contract. Distillation never blocks or crashes the OODA
cycle.

| Situation                                              | Layer that returns the result                          | Pass outcome                                   |
|--------------------------------------------------------|--------------------------------------------------------|------------------------------------------------|
| Runner missing / no agent binary / recipe file absent  | runner not constructed (`RecipeRunnerSubprocess::new`) | `Ok(DistillReport::skipped())`, no LLM call    |
| Process exits non-zero (the production failure path)   | `invoke_recipe` exit-code guard → `Err`                | non-fatal; no markers set; retry next pass     |
| Envelope `success == false` *with* exit `0`            | parser Tier 1 guard → `Err` (defense-in-depth)         | non-fatal; no markers set; retry next pass     |
| No completed `distill` step                            | parser Tier 1 → `Err`                                  | non-fatal; no markers set; retry next pass     |
| `output` present but no parseable facts object         | parser Tier 3 → `Err`                                  | non-fatal; no markers set; retry next pass     |
| Launch-banner / ANSI noise **before** the envelope `{` | parser Tier 1b recovery → `Ok(DistillOutput { … })`    | facts stored ([#2512](https://github.com/rysweet/Simard/issues/2512)) |
| Valid envelope, facts extracted                        | parser Tier 1 → `Ok(DistillOutput { … })`              | facts stored; **every** input episode marked   |

Every `Err` is mapped to a non-fatal log line at
`src/ooda_actions/simple_actions.rs` (`distill failed (non-fatal): {e}`).
No `distilled` markers are written on failure, so the same batch is fully
eligible on the next pass — distillation is idempotent and retry-safe.

---

## Examples

### Example 1 — successful capture

The daemon pulls 25 undistilled episodes and invokes the runner:

```text
recipe-runner-rs ~/.simard/prompt_assets/simard/recipes/distill-episodes.yaml \
  --output-format json \
  -c episodes=[{"id":"e1",…}, …]      # AMPLIHACK_AGENT_BINARY=copilot
```

Stdout (envelope, agent ran ~20s):

```json
{
  "recipe_name": "distill-episodes",
  "success": true,
  "step_results": [
    {
      "step_id": "distill",
      "status": "completed",
      "output": "{\"facts\":[{\"concept\":\"pr-pattern\",\"content\":\"CI flakes on lbug pin bumps until cache is warmed\",\"source_episode_id\":\"e7\"},{\"concept\":\"lesson-learned\",\"content\":\"Stale worktrees mask deploy drift\",\"source_episode_id\":\"e12\"}],\"procedures\":[{\"name\":\"ci-fix:auto\",\"steps\":[\"re-run failed job\",\"warm shared target cache\",\"re-push --no-verify\"],\"source_episode_ids\":[\"e7\",\"e9\"]}]}",
      "error": "",
      "duration": 19.7
    }
  ],
  "duration": 19.8
}
```

Tier 1 selects the `distill` step, reads `output` (a string), scans it,
and yields:

```text
DistillOutput {
  facts:      [ pr-pattern(e7), lesson-learned(e12) ],
  procedures: [ ci-fix:auto from (e7, e9) ],
}
```

Result: 2 facts and 1 procedure stored; all 25 episodes marked
distilled.

```
[simard] distill: 25 episodes → 2 facts, 1 procedures, 25 marked
```

### Example 2 — failed recipe run

The agent step errors (e.g. the LLM call times out). The runner exits
`1` and emits `success: false`:

```json
{ "recipe_name": "distill-episodes", "success": false,
  "step_results": [ { "step_id": "distill", "status": "failed",
    "output": "", "error": "Step 'distill' failed: …", "duration": 0.9 } ],
  "duration": 0.9 }
```

`parse_recipe_output_full` returns `Err`. No markers are set; the batch
retries next pass:

```
[simard] distill failed (non-fatal): distill: recipe exited with status 1: Step 'distill' failed: …
```

### Example 3 — legacy / mock output (Tier 2)

A unit test (or any caller that emits a bare facts object, not a runner
envelope):

```json
{ "facts": [ { "concept": "bug-pattern", "content": "…", "source_episode_id": "e3" } ], "procedures": [] }
```

This is not a `RecipeRunnerEnvelope` (missing `success` /
`step_results`), so Tier 1 fails and Tier 2 scans the raw string,
extracting the single fact. Existing mock tests stay green with no
change.

---

## Tutorial: capture a real envelope by hand

To reproduce the production output-capture path against the live binary
(this invokes the configured LLM once — safe; it touches no Simard
store):

```bash
export PATH="$HOME/.local/bin:$PATH"
export AMPLIHACK_AGENT_BINARY=copilot   # or claude / codex

recipe-runner-rs \
  prompt_assets/simard/recipes/distill-episodes.yaml \
  --output-format json \
  -c episodes='[
    {"id":"e1","source_label":"ci","temporal_index":1,
     "content":"CI failed; re-ran job after warming shared target cache; passed"},
    {"id":"e2","source_label":"ci","temporal_index":2,
     "content":"CI failed again on lbug pin bump; warmed cache; re-pushed --no-verify; passed"}
  ]'
```

You will see the envelope on stdout. The `step_results[0].output` string
holds the agent's `{ "facts": …, "procedures": … }` JSON — exactly what
`parse_recipe_output_full` extracts.

To confirm the envelope structure quickly **without** an LLM call, run a
trivial bash-step recipe. The recipe is passed as a **file path**
positional argument (the runner treats a bare `-` as a recipe *name* to
look up, not as stdin):

```bash
cat > /tmp/probe.yaml <<'YAML'
name: probe
version: "1.0.0"
steps:
  - id: distill
    type: bash
    command: "printf '%s' '{\"facts\":[],\"procedures\":[]}'"
YAML
recipe-runner-rs /tmp/probe.yaml --output-format json
```

The `step_results[0].output` field will contain
`{"facts":[],"procedures":[]}` as a string, demonstrating the
string-typed payload the parser scans.

---

## Recipe asset sync

The daemon resolves the recipe hot-reload-first from
`~/.simard/prompt_assets/simard/recipes/`. If that directory is missing
the `distill-episodes.yaml` asset, the runner falls back to the in-tree
copy — but on the deployed VM the daemon's working directory is a
worktree, so a missing hot-reload asset previously left the daemon
running a recipe set that did not match the deployed code.

`scripts/redeploy-local.sh` **already** syncs **all** prompt assets — including
`prompt_assets/simard/recipes/*.yaml` — into
`~/.simard/prompt_assets/simard/` on every redeploy via a single scoped
`cp -rf`, and enumerates the synced top-level `*.md` prompt files so an
operator can confirm the sync:

```text
[redeploy] synced prompt assets → /home/azureuser/.simard/prompt_assets
[redeploy]   prompt: ooda-decide.md
…
```

The recipe `.yaml` files live one directory down in `recipes/`, so before
this change they were copied but **not** individually verified. The #2401
change adds an explicit recipe-sync verification block after the `cp`: it
lists every synced `recipes/*.yaml`, reports a synced-vs-source **count**,
**fails the redeploy** (`exit 1`) if zero recipes reached the hot-reload
path, and **warns** on any count drift. An operator can now confirm
`distill-episodes.yaml` actually reached the hot-reload path:

```text
[redeploy] synced 7/7 recipe asset(s) → /home/azureuser/.simard/prompt_assets/simard/recipes
[redeploy]   recipe: disk-health-check.yaml
[redeploy]   recipe: distill-episodes.yaml
[redeploy]   recipe: merge-readiness-judge.yaml
…
```

The sync is confined to `prompt_assets/simard/**` → the corresponding
`~/.simard/prompt_assets/simard/` paths, with quoted path variables and a
scoped `cp` (no `eval`, no unquoted globs). It never touches the live
cognitive-memory store or the daemon's data directory. The compile-time
embedded recipe set remains a safety net if the source tree is absent.

> **Operational note.** A stale hot-reload directory can compound this
> bug: if `~/.simard/prompt_assets/simard/recipes/` is missing (or has an
> out-of-date copy of) `distill-episodes.yaml`, the daemon silently runs
> the in-tree fallback recipe instead of the deployed one. Combined with
> the parsing bug, every result was swallowed without a trace. Verify the
> deployed set with
> `ls ~/.simard/prompt_assets/simard/recipes/`. The sync fix ensures
> redeploys keep the hot-reload recipe set current; the output-capture fix
> ensures the captured result is actually parsed.

---

## Security

- **No command injection.** The argv is built with
  `Command::new("recipe-runner-rs").arg(...)`; the episodes payload is a
  single `episodes=<json>` argument. Content is never interpolated into a
  shell line, a flag, the recipe path, or the binary name. This argv-only
  construction must be preserved.
- **Bounded error output.** The brace scanner is linear in input length
  and iterative (no recursion in the scan), so deeply nested input cannot
  grow the stack; each candidate parse is bounded by `serde_json`'s own
  recursion limit. Error messages reuse `truncate(…, 200)`, so a large
  hostile stdout never echoes its full content. An explicit captured-stdout
  byte cap (rejecting multi-MiB stdout before parsing) is a recommended
  follow-up; today an unbounded stdout would be read into memory once before
  the linear scan, which is acceptable for the runner's small envelopes.
- **Strict typing.** `success` and `step_results` are required fields. A
  failed run (`success == false`) never yields facts.
- **No payload leakage in routine logs.** Error messages reuse
  `truncate(…, 200)` and a generic message. Full stdout, episode content,
  and facts payloads appear only at `debug` level — never at `info` or
  `warn`.
- **Process visibility (known limitation).** `episodes=<json>` is passed
  on the argv, so it is visible via `ps`/`/proc` on a shared host. Moving
  the payload to stdin or a `0600` temp file is a recommended follow-up if
  a future `recipe-runner-rs` supports it; it does not block this fix.

---

## Code location

| Item                                            | File                                                 | #2401 change                          |
|-------------------------------------------------|------------------------------------------------------|---------------------------------------|
| `invoke_recipe`                                 | `src/memory_consolidation/distillation.rs`           | modified — add `--output-format json` |
| `RecipeRunnerEnvelope` / `RecipeRunnerStepResult` | `src/memory_consolidation/distillation.rs`         | new                                   |
| `parse_recipe_output_full`                      | `src/memory_consolidation/distillation.rs`           | modified — 2-tier → 3-tier            |
| `parse_recipe_output` (facts-only wrapper)      | `src/memory_consolidation/distillation.rs`           | unchanged                             |
| `scan_for_facts_object`                         | `src/memory_consolidation/distillation.rs`           | new                                   |
| Non-fatal upstream mapping                      | `src/ooda_actions/simple_actions.rs`                 | unchanged                             |
| Recipe                                          | `prompt_assets/simard/recipes/distill-episodes.yaml` | unchanged                             |
| Recipe asset sync                               | `scripts/redeploy-local.sh`                          | modified — list + verify synced recipes (count, fail-on-zero) |
| Tests                                           | `src/memory_consolidation/distillation_tests.rs`     | new tests added                       |

---

## Testing

The fix adds these tests to `src/memory_consolidation/distillation_tests.rs`:

| Test                                                              | Coverage                                                                                  |
|------------------------------------------------------------------|-------------------------------------------------------------------------------------------|
| `parser_extracts_facts_from_verbatim_real_envelope`              | A verbatim `recipe-runner 0.3.6` envelope (captured from the binary via a bash probe) parses its `step_results[].output` string into facts. |
| `parser_extracts_facts_and_procedures_from_real_prose_prefixed_envelope` | A **verbatim real agent** envelope — `output` carries a `NODE_OPTIONS` banner *before* the JSON — parses to **both** facts and procedures. This is the production-faithful proof. |
| `parser_extracts_facts_and_procedures_from_distill_step_output`  | A synthetic real-shaped envelope yields both facts and procedures with provenance intact.  |
| `parser_selects_distill_step_among_multiple_steps`               | Step selection picks `step_id == "distill"` even when other completed steps precede it.    |
| `parser_falls_back_to_last_completed_step_when_no_distill_id`    | Falls back to the last `completed` step when no `distill` step exists.                      |
| `parser_drops_unknown_concepts_inside_envelope`                  | The concept allow-list still applies when extracting from the envelope.                     |
| `parser_tolerates_output_as_json_object`                         | A future `output` emitted as a JSON object (not a string) still yields facts.               |
| `facts_only_wrapper_reads_runner_envelope` / `…_real_prose_prefixed_envelope` | The facts-only `parse_recipe_output` wrapper also reads the envelope.          |
| `parser_returns_err_on_failure_envelope` / `parser_does_not_extract_facts_from_failed_run` | `success == false` returns `Err`; no facts extracted, even with a payload present. |
| `parser_errors_on_text_mode_status_banner`                       | The `--output-format text` banner (the production failure input) returns an explicit `Err`. |
| `parser_tolerant_fallback_accepts_bare_facts_object` / `…_extracts_facts_from_prose` | Tier 2 still parses a bare / prose-wrapped `{ "facts": … }` object. |
| `parser_error_does_not_leak_full_payload`                        | The Tier 3 error message is truncated and carries no content past the truncation window.    |
| `parser_tolerates_deeply_nested_input_without_panic`             | Pathologically nested braces terminate with `Err` (no stack blowup).                        |
| `parser_handles_large_valid_envelope`                            | A large valid envelope (1 000 facts) extracts every fact — the linear scan handles size.    |

All existing `DistillRecipeRunner` mock tests
(`parse_recipe_output_accepts_plain_object`,
`parse_recipe_output_extracts_json_from_prose`,
`parse_recipe_output_drops_unknown_concepts`,
`parse_recipe_output_errors_when_no_object`) remain green via Tier 2.

---

## Out of scope

- **Sibling recipe shims** (`recipe_merge_judge`,
  `recipe_progress_checker`) parse their own text banners and are **not**
  affected by this change. They are intentionally left untouched.
- **Subprocess wall-clock timeout** for a hung agent step is a
  recommended follow-up (spawn + timed wait/kill so a stuck LLM cannot
  stall the OODA pass); it is not required for the output-capture fix.
- **Moving the episodes payload off the argv** (stdin / `0600` temp file)
  is a follow-up hardening, contingent on `recipe-runner-rs` support.
