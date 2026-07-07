---
title: Goal-decompose result channel
description: How Simard's goal-decomposition pass reliably captures the decomposition agent's { "sub_goals": [...] } JSON — the dedicated sub-goals-file channel (sub_goals_output), the harvest_subgoals_file reader and repurposed parse_subgoals_json deserializer, the loud failure taxonomy, and the recipe-asset sync that keeps the hot-reload path current (issue #2708).
last_updated: 2026-07-06
owner: simard
doc_type: reference
status: shipped — issue #2708
related:
  - ./goal-decomposition.md
  - ./distill-recipe-output-capture.md
  - ../howto/decompose-a-large-goal.md
  - ./text-parsing-wire-formats.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../design/eliminate-deterministic-fallbacks.md
---

# Goal-decompose result channel

> **Status — implements the
> [#2708](https://github.com/rysweet/Simard/issues/2708) fix.**
> The decomposition agent's answer is captured from a **dedicated sub-goals
> file** the agent writes, not from `recipe-runner-rs` stdout. Present tense
> below describes the shipped behavior. Locations:
> reader + invocation `src/goal_curation/decompose.rs`
> (`RecipeGoalDecomposer::propose_subgoals`, `harvest_subgoals_file`,
> `parse_subgoals_json`); tests the in-file `#[cfg(test)] mod tests` in
> `decompose.rs`; recipe
> `prompt_assets/simard/recipes/goal-decomposition.yaml`; prompt
> `prompt_assets/simard/goal_decomposition.md`; asset sync
> `scripts/redeploy-local.sh`.

The [goal-decomposition pass](./goal-decomposition.md) turns **one** large goal
into 2–6 bounded, independently-verifiable sub-goals by shelling out to
`recipe-runner-rs` and reading the decomposition agent's JSON output. This page
documents the **output-capture contract** between Simard and the decomposition
agent: how the agent's `{ "sub_goals": [...] }` object is reliably captured,
what happens on failure, and how the recipe/prompt assets reach the running
daemon.

For the surrounding capability (the data model, the typed parent↔child edges,
`decompose_goal`'s fan-out bounds and roll-up, the `simard goal decompose`
CLI), see [Goal decomposition & the goal graph](./goal-decomposition.md). This
page is the transport-layer companion; it changes **only** how the agent's
proposed sub-goals reach Simard, not what Simard does with them.

This is the goal-side sibling of the distillation
[clean result channel](./distill-recipe-output-capture.md): the same
brittle-parsing-of-LLM-output antipattern, closed the same structural way. The
goal reader is **modeled on** — not a byte-for-byte copy of — distill's
`harvest_facts_file`: it keeps the same file-channel shape and the same
"non-zero exit surfaces stderr **and** stdout" diagnostic, but adds two guards
the distill reader does **not** yet enforce — a **1 MiB size cap** and an
**empty / whitespace-only rejection** — so `parse_subgoals_json` never receives
empty or oversized input. Back-porting those two guards to `harvest_facts_file`
for true parity is a recommended follow-up (see
[Out of scope](#out-of-scope)); until then the two readers are deliberately
described as *stricter here*, not identical.

---

## Why this contract exists — the file channel

`recipe-runner-rs` runs the `goal-decomposition` recipe — a single `decompose`
agent step that asks the LLM to produce `{ "sub_goals": [ … ] }`.

Capturing that object from the subprocess's **stdout** is brittle: the copilot
binary prints a launcher banner, tracing lines, ANSI control codes, and log
noise to the inherited stdout *before and around* the agent's answer, e.g.

```text
2026-07-06T02:45:12Z  INFO launching copilot binary=/home/.../copilot version="GitHub Copilot CLI 1.0.69-2."
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference)
Run 'copilot update' to update
```

The retired transport scanned that stdout for the **outermost** `{…}` object
(then `[…]` array) and `serde`-parsed the slice. Because the banner and log
lines are full of braces, the outermost-brace slice was **not** valid JSON, so
the parse failed and the whole (successful, ~48.7 s) decomposition was
discarded with:

```text
could not parse sub-goals from decomposition output: …
```

This is the exact failure class the distill pass hit
([#2622](https://github.com/rysweet/Simard/issues/2622) /
[#2619](https://github.com/rysweet/Simard/issues/2619)) and the same
`recipe_output` brace-scan family (#2679 / #2658).

The fix removes stdout from the result path entirely. The agent **writes** its
JSON envelope to a private, per-invocation file whose absolute path is handed
to the recipe as a context variable. Simard reads **that file** after the
runner exits. A launcher banner on stdout can no longer contaminate the parse,
because stdout is never read as the result. stdout is still **captured** — but
only for the non-zero-exit diagnostic message, never parsed for sub-goals.

---

## Invocation

`RecipeGoalDecomposer::propose_subgoals` shells out to `recipe-runner-rs` with
an argv-vector (no shell):

```text
recipe-runner-rs <recipe_path>
    -c goal_id=<parent id>
    -c goal_description=<sanitized parent description>
    -c plan=<sanitized current_activity>
    -c max_children=<effective 2..=6 ceiling>
    -c sub_goals_output=<absolute path to a fresh tempdir/sub_goals.json>
```

with `AMPLIHACK_AGENT_BINARY` in the environment.

- `sub_goals_output` is a fresh, mode-`0700` tempdir (`tempfile::Builder`) plus
  a `sub_goals.json` filename, unique per invocation. The recipe interpolates
  it into the prompt via `{{sub_goals_output}}` and instructs the agent to
  write **only** the `{ "sub_goals": [...] }` object there.
- `goal_description` and `plan` remain bounded/newline-collapsed by
  `ooda_brain::sanitize::sanitize_context_var(_, 8000)` before they ride on
  argv (the E2BIG / YAML-interpolation hardening from #2640 / #2692 / #2127).
  `sub_goals_output` is a **self-generated** absolute temp path — never
  LLM- or user-supplied text — so it is not sanitized.
- After the runner exits, Simard reads `sub_goals_output`, and the tempdir
  (and its contents) is removed when the `TempDir` guard drops at the end of
  the call. The guard is bound to a local that outlives both `.output()` **and**
  the file read, so the path exists while the recipe writes it and while Simard
  reads it.

`propose_subgoals` builds the argv and holds the tempdir guard;
`harvest_subgoals_file` post-processes the finished `std::process::Output`: a
non-zero exit is an explicit error carrying **both** truncated stderr **and**
stdout (recipe-runner emits its structured failure on stdout and stderr is
often empty, so the error string must include stdout or a non-zero exit risks a
context-free message); a clean run reads and deserializes the sub-goals file.
`harvest_subgoals_file` is factored out so the "stdout noise is inert" contract
is hermetically testable without spawning a subprocess.

---

## Parser API

Two functions capture the **sub-goals document** (the file contents):

- `harvest_subgoals_file(output: &std::process::Output, path: &Path) -> SimardResult<Vec<SubGoalProposal>>`
  — post-processes a finished invocation. It surfaces a non-zero exit as a loud
  error (carrying truncated stderr **and** stdout), size-guards and reads the
  file, then **itself** rejects a missing / empty / whitespace-only / oversized
  (> 1 MiB) / non-UTF-8 file loudly **before** delegating. The empty /
  whitespace-only check lives here, in `harvest_subgoals_file` — **not** in
  `parse_subgoals_json` — so the deserializer only ever sees a non-empty,
  size-bounded string. This is stricter than distill's `harvest_facts_file`,
  which reads the file directly with no size cap and returns `Ok("")` for an
  empty file (deferring the empty case to its caller). Subprocess-free, so the
  "stdout is inert" contract is unit-testable with a synthetic `Output`.
- `pub fn parse_subgoals_json(text: &str) -> SimardResult<Vec<SubGoalProposal>>`
  — the strict deserializer over the **file contents**. Its name, signature,
  and public re-export (`crate::goal_curation::parse_subgoals_json`) are
  unchanged; only its body changed — from an stdout brace-scanner into a strict
  clean-channel deserializer.

`parse_subgoals_json`:

1. Accepts the untagged `SubGoalsPayload`: either the wrapped object
   `{ "sub_goals": [ … ] }` **or** a bare `[ … ]` array, deserialized with a
   single strict `serde_json::from_str::<SubGoalsPayload>`.
2. Applies **bounded** format tolerance only: leading/trailing whitespace and
   at most **one** wrapping Markdown code fence (```` ```json … ``` ````) are
   stripped before the parse. This is clean-channel *format* leniency — **not**
   the outermost-brace scanning / prose-skipping the fix removed. There is no
   `json_candidates`, no substring extraction, no "try the whole string, then
   the first `{`, then the first `[`" cascade.
3. A parse failure ⇒ explicit `Err` (see [Failure semantics](#failure-semantics)).

The typed [`SubGoalProposal`](./goal-decomposition.md#data-model) model
(`description`, `done_criterion`, optional `depends_on: Vec<usize>`) is
unchanged: the goal graph legitimately needs typed sub-goals. Only the
agent → Simard **transport** changed.

---

## Failure semantics

Every failure is explicit and **loud** — never a silent empty decomposition.
`propose_subgoals` / `harvest_subgoals_file` never return `Ok(vec![])` as a
disguise for "the agent wrote nothing". The failure classes:

| Trigger | Error | `field` |
|---|---|---|
| `recipe-runner-rs` not spawnable | `SimardError::InvalidGoalRecord` | `decomposer` |
| recipe **process** exited non-zero | `SimardError::InvalidGoalRecord` (truncated stderr **and** stdout) | `decomposer` |
| sub-goals file **missing** (agent wrote nothing) | `SimardError::InvalidGoalRecord` | `sub_goals` |
| sub-goals file **empty / whitespace-only** | `SimardError::InvalidGoalRecord` | `sub_goals` |
| sub-goals file **oversized** (> 1 MiB) or non-UTF-8 | `SimardError::InvalidGoalRecord` | `sub_goals` |
| file present but **malformed** JSON | `SimardError::InvalidGoalRecord` | `sub_goals` |

These distinguish three defect classes cleanly: *agent wrote nothing*
(missing / empty → loud), *agent wrote garbage* (parse fail → loud), and
*agent proposed too few* (< 2 sub-goals) — the last is **not** a transport
failure and is handled downstream by `decompose_goal`'s fan-out floor
(`MIN_SUBGOALS = 2`), which surfaces its own loud
`InvalidGoalRecord { field: "sub_goals", … }` and leaves the board and graph
untouched. There is deliberately **no stdout fallback**: scraping stdout is
exactly the launcher-banner contamination this fix removes, and a silent
fallback is a silent failure.

Non-transport bounds enforced by `decompose_goal` are unchanged: the proposal
list is clamped to `[2, 6]` (`MIN_SUBGOALS`/`MAX_SUBGOALS`), each child id is
`validate_goal_id`-checked before any mutation, and every LLM-supplied
`depends_on` index is bounds-checked (`< proposals.len()`, no self-reference)
before a `DependsOn` edge is written — so a malformed index can neither panic
nor forge a graph edge.

---

## Examples

### Example 1 — successful capture

The runner's stdout carries a launcher banner and ANSI/log noise; the agent
wrote a clean envelope to `sub_goals_output`. Simard reads the file and yields
typed sub-goals:

```json
{ "sub_goals": [
  { "description": "Add parent_goal_id + GoalNode data model", "done_criterion": "serde back-compat test green", "depends_on": [] },
  { "description": "Implement typed-edge relationship facts", "done_criterion": "edge round-trips via search_facts", "depends_on": [0] }
] }
```

`simard goal decompose <big-goal>` succeeds end-to-end — the ~48 s
decomposition is applied, not discarded.

### Example 2 — missing sub-goals file (no stdout fallback)

The process exits 0 but the agent never wrote the file — even if stdout happens
to carry a well-formed `{ "sub_goals": [...] }` object, it is **not** scraped.
`harvest_subgoals_file` returns an explicit
`InvalidGoalRecord { field: "sub_goals", … }` and the board/graph stay
untouched.

### Example 3 — fenced document still parses

The agent wrapped its answer in a single ```` ```json … ``` ```` fence in the
file. Clean-channel format leniency strips the one fence and the strict parse
succeeds. Prose *around* the JSON in the file is **not** skipped — that is the
scanning behavior the fix removed.

### Example 4 — bare array

A file containing a bare `[ {…}, {…} ]` array (no `sub_goals` wrapper)
deserializes via the untagged `SubGoalsPayload::Bare` arm, unchanged.

---

## Recipe & prompt asset sync

The daemon resolves the recipe hot-reload-first from
`~/.simard/prompt_assets/simard/recipes/` (see `resolve_recipe_path`); the
prompt asset resolves the same way. If those directories are missing the
updated `goal-decomposition.yaml` / `goal_decomposition.md`, the runner falls
back to the in-tree copy — but on the deployed VM the daemon's working
directory is a worktree, so a **stale** hot-reload asset would run the old
"write your JSON to stdout" instructions even though the code now reads a file.

`scripts/redeploy-local.sh` syncs **all** prompt assets — including
`prompt_assets/simard/recipes/*.yaml` and `prompt_assets/simard/*.md` — into
`~/.simard/prompt_assets/simard/` on every redeploy, and fails the redeploy if
zero recipes reached the hot-reload path (see the
[distill result-channel sync note](./distill-recipe-output-capture.md#recipe-asset-sync)).

Note the redeploy script enforces **presence, not freshness**: its `exit 1`
guard is purely count-based (it fires only when the destination has **zero**
recipes, with a softer `WARN` when the synced count is lower than the source
count). It does **not** inspect any recipe's *content*, so a stale-but-present
`goal-decomposition.yaml` (old stdout instructions, correct filename) passes the
guard cleanly. Content freshness is a **separate** check — the operational
`grep -l sub_goals_output` below — and the two must not be conflated: the guard
answers "did a recipe land?", the grep answers "is the landed recipe current?".

> **Operational note.** After deploying the #2708 fix, verify the hot-reload
> assets carry the file-channel instructions:
>
> ```console
> $ grep -l sub_goals_output ~/.simard/prompt_assets/simard/recipes/goal-decomposition.yaml \
>       ~/.simard/prompt_assets/simard/goal_decomposition.md
> ```
>
> A stale asset that still says "emit the JSON to stdout" will make the agent
> write nothing to the file — a loud `field: "sub_goals"` error by design (the
> code fails loudly rather than silently reintroducing the stdout scrape). The
> code fix and the asset update must land together.

> **Asset-edit checklist.** When updating `goal-decomposition.yaml`, edit the
> whole file, not just the agent prompt body. The recipe currently opens with a
> header comment block that documents the old contract and must be brought in
> line with the file channel:
>
> - the `# Output: agent stdout — a single JSON object:` header (and its example
>   line) must be rewritten to describe the `sub_goals_output` file, **not**
>   stdout;
> - the `# Context vars (passed via -c):` list must add `sub_goals_output`
>   alongside `goal_id, goal_description, plan, max_children`.
>
> The `grep -l sub_goals_output` freshness check above matches the interpolated
> `{{sub_goals_output}}` in the prompt body, so it will **not** flag a stale
> `# Output: agent stdout` comment — that header must be reviewed by eye.

---

## Security

- **No command injection.** The argv is built with
  `Command::new("recipe-runner-rs").arg(...)`; every context variable is a
  single `-c key=value` argument. Content is never interpolated into a shell
  line. This argv-only construction must be preserved.
- **Strict typed deserialize.** The result is deserialized as the typed
  `SubGoalsPayload`; deleting `json_candidates` removes a parser-differential /
  injection-tolerant substring path.
- **Bounded file read.** `harvest_subgoals_file` size-guards the file (rejects
  `> 1 MiB` before `read_to_string`) so a runaway agent cannot exhaust memory,
  and rejects non-UTF-8 rather than lossily decoding the transport payload.
- **Private result file.** `sub_goals_output` lives inside a mode-`0700`,
  randomized-path per-invocation tempdir and is removed when the invocation
  returns; the agent's answer never lands on a shared stdout stream, closing
  the predictable-`/tmp`-path TOCTOU / symlink / info-leak surface.
- **Bounded error output.** Error messages reuse `truncate(…, 200)`, so a large
  hostile document or goal text never echoes its full content into a log; no
  new `println!` / `eprintln!` is added.
- **`depends_on` index safety.** Every LLM-supplied `depends_on` index is
  bounds-checked (`< len`, no self-reference) before any slice indexing or
  `write_edge`, preventing an OOB panic or a corrupt graph edge.
- **No stdout fallback.** A missing / empty / oversized / malformed file is an
  explicit `Err`; stdout is never scraped as a backup result channel.

---

## Code location

| Item | File |
|---|---|
| `RecipeGoalDecomposer::propose_subgoals` (adds `-c sub_goals_output`, holds the tempdir guard) | `src/goal_curation/decompose.rs` |
| `harvest_subgoals_file` (read file / size-guard / loud errors) | `src/goal_curation/decompose.rs` |
| `parse_subgoals_json` (strict clean-channel deserializer) | `src/goal_curation/decompose.rs` |
| `SubGoalsPayload` (untagged `{ "sub_goals": [...] }` \| bare `[...]`) | `src/goal_curation/decompose.rs` |
| `decompose_goal` (fan-out bounds, `depends_on` index check, edge writes) | `src/goal_curation/decompose.rs` |
| Recipe (writes `{{sub_goals_output}}`) | `prompt_assets/simard/recipes/goal-decomposition.yaml` |
| Prompt (writes to `{{sub_goals_output}}`) | `prompt_assets/simard/goal_decomposition.md` |
| Recipe/prompt asset sync | `scripts/redeploy-local.sh` |
| Tests | in-file `#[cfg(test)] mod tests` in `src/goal_curation/decompose.rs` |

The `GoalDecomposer` trait signature, `CannedDecomposer`, and
`src/goal_curation/tests_decompose.rs` are **unchanged** — the transport fix is
additive and back-compatible with the trait's test stubs.

---

## Testing

The file-channel behavior is pinned by the in-file `mod tests` in
`decompose.rs` (subprocess-free, using a synthetic `Output` with a zero exit
status):

| Test | Coverage |
|---|---|
| noisy-stdout-is-inert | An `Output` whose stdout is ANSI/log/banner noise **plus** a valid sub-goals file ⇒ **success** with the file's sub-goals (the headline #2708 case: noisy stdout no longer breaks decomposition because it is no longer parsed). |
| missing-file-is-loud | A clean exit with **no** file ⇒ `InvalidGoalRecord { field: "sub_goals" }`, even when stdout carries a well-formed sub-goals object. |
| empty-file-is-loud | An empty / whitespace-only file ⇒ loud `field: "sub_goals"` error. |
| malformed-file-is-loud | A present-but-unparseable file ⇒ loud `field: "sub_goals"` error. |
| oversized-file-is-loud | A file over the 1 MiB cap ⇒ loud `field: "sub_goals"` error before a full read. |
| nonzero-exit-is-loud | A non-zero recipe exit ⇒ loud `field: "decomposer"` error carrying truncated stderr and stdout. |
| `parse_wrapped_object` | `{ "sub_goals": [...] }` deserializes (retained). |
| `parse_bare_array` | A bare `[...]` array deserializes via the `Bare` arm (retained). |
| `parse_rejects_non_json` | Non-JSON input errors explicitly (retained). |
| fence-only tolerance | A single ```` ```json … ``` ```` fence is stripped; a document with surrounding prose is **not** salvaged by substring scanning (reworked from the old prose-tolerant test). |

---

## Migration — from a dedicated file to the durable channel

The dedicated `sub_goals_output` file is the **available-now** clean substrate,
matching the shape of the shipped distill file channel (with the added
size-cap / empty-rejection hardening noted above). The durable substrate is the
`semanticchannel` clean-result-channel work in `amplihack-rs`: once
`recipe-runner-rs` exposes a first-class structured result channel, the
agent → Simard handoff migrates from the temp file to that channel with no
change to `parse_subgoals_json`, the `SubGoalProposal` model, or
`decompose_goal`'s fan-out/edge logic — only `propose_subgoals`'s transport
wiring moves. The file channel is the bridge substrate; the typed model and the
graph writes are stable across the migration.

A **fully-agentic** variant — where the decomposition agent writes each
sub-goal directly through the goal-store interface as a structured tool-call,
leaving no payload for Simard to read — is the longer-term direction. It is
**not** implemented here (the recipe agent has no wired goal-store tool-call
today); the dedicated-file channel is the feasible clean handoff now.

---

## Out of scope

- **Sibling parse-fail sites.** The distill file channel
  (#2622 / #2619, [reference](./distill-recipe-output-capture.md)) and the
  `recipe_output/extract.rs` family (#2679 / #2658) are the same antipattern in
  other passes; this change fixes only the goal-decompose transport.
- **Back-porting the reader hardening to distillation.** `harvest_subgoals_file`
  adds a 1 MiB size cap and an empty / whitespace-only rejection that distill's
  `harvest_facts_file` does not yet enforce. Applying those two guards to
  `harvest_facts_file` for true reader parity is a recommended follow-up, out of
  scope for the #2708 goal-decompose fix.
- **`semanticchannel` implementation** in `amplihack-rs` (the durable
  substrate) is tracked upstream; this Simard-side fix uses the dedicated file
  now and migrates later.
- **Fan-out / placement / roll-up algorithm.** `decompose_goal`'s bounds,
  `ChildPlacement`, edge writes, and parent-progress roll-up are unchanged — see
  [Goal decomposition & the goal graph](./goal-decomposition.md).
- **The OODA auto-decompose trigger** and the broader `RecipeGoalDecomposer`
  refactor are unchanged follow-ups tracked off #2405.
