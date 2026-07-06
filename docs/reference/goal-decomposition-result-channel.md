---
title: Goal-decomposition result channel (file-based agent→Simard handoff)
description: Normative reference for how the goal-decomposition agent hands its 2–6 proposed sub-goals back to Simard through a dedicated, git-ignored result file instead of the shared recipe-runner-rs stdout. Documents the sub_goals_output recipe context variable and the write-only-JSON-to-path prompt contract, the RecipeGoalDecomposer file-channel transport, the strict single-shot parser that replaced the stdout brace-scanner, the read_subgoals_from_file API, the temp-file allocation (in-tree O_EXCL TempDir under target/, absolute path, ~1 MiB size cap, 0700/0600 perms), the loud-error contract for a missing/empty/oversized/malformed result, and the forward migration path to a future typed result-channel substrate. Fixes the brittle-stdout-scrape antipattern that discarded successful decompositions (issue #2708).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
related:
  - ./goal-decomposition.md
  - ./text-parsing-wire-formats.md
  - ./recipe-context-var-sanitization.md
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./simard-cli.md
  - ../howto/decompose-a-large-goal.md
  - ../howto/troubleshoot-goal-store.md
  - ../../src/goal_curation/decompose.rs
  - ../../prompt_assets/simard/recipes/goal-decomposition.yaml
  - ../../prompt_assets/simard/goal_decomposition.md
---

# Goal-decomposition result channel (file-based agent→Simard handoff)

> **Implemented in issue
> [#2708](https://github.com/rysweet/Simard/issues/2708).** This page specifies
> the transport contract shipped by the #2708 fix. Under that contract the
> decomposition agent hands its proposed sub-goals back to Simard through a
> **dedicated result file**, not through the shared `recipe-runner-rs` stdout.
> The transport lives in
> [`src/goal_curation/decompose.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/decompose.rs)
> (`RecipeGoalDecomposer::propose_subgoals` → `read_subgoals_from_file` →
> `parse_subgoals_json`) and the file-write contract is carried by
> [`prompt_assets/simard/recipes/goal-decomposition.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/goal-decomposition.yaml)
> (recipe `version: 1.1.0`) and
> [`prompt_assets/simard/goal_decomposition.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_decomposition.md).
> This is a **transport-only** change: the typed
> [`SubGoalProposal`](./goal-decomposition.md#decomposition-driver-assets)
> model, the `2..=6` fan-out clamp, the deterministic child ids, and the typed
> `decomposes_into` / `depends_on` graph-edge writes are all unchanged.

This page is the transport companion to the
[goal decomposition reference](./goal-decomposition.md), which documents the
data model, the typed-edge graph, the `decompose_goal` driver, and the
`simard goal decompose` operator command. Read that page for **what**
decomposition produces; read this page for **how** the agent's output reaches
Simard.

## Contents

- [The problem this solves](#the-problem-this-solves)
- [What changed](#what-changed)
- [Data flow](#data-flow)
- [The result-channel contract](#the-result-channel-contract)
- [Wire format](#wire-format)
- [Configuration](#configuration)
- [API](#api)
- [Error handling — loud, never empty](#error-handling--loud-never-empty)
- [Security considerations](#security-considerations)
- [Examples](#examples)
- [Migrating to a typed result-channel substrate](#migrating-to-a-typed-result-channel-substrate)
- [Guarantees and non-guarantees](#guarantees-and-non-guarantees)
- [Related](#related)

## The problem this solves

Before this change, `RecipeGoalDecomposer::propose_subgoals` ran
`recipe-runner-rs goal-decomposition.yaml`, captured its **stdout**, and handed
the whole buffer to `parse_subgoals_json`. That parser did **not** deserialize
the stdout directly — it *scraped* it: a helper (`json_candidates`) sliced out
the outermost `{ … }` object and the outermost `[ … ]` array and tried
`serde_json::from_str::<SubGoalsPayload>` on each slice in turn.

`recipe-runner-rs` stdout is a **shared, noisy channel**. It carries ANSI
colour codes, `tracing` log lines, launcher banners, and progress chatter
around whatever the agent emits. The outermost-brace slice of that stream is
almost never valid JSON — a single `{` in a log line before the real payload,
or a `}` in a banner after it, and the scraped slice is garbage. The result was
the failure mode reported in
[#2708](https://github.com/rysweet/Simard/issues/2708): a **successful**
decomposition (the agent ran for ~48.7 s and produced correct sub-goals) was
**discarded** with

```
could not parse sub-goals from decomposition output: …
```

because the stdout brace-scan could not recover valid JSON from the surrounding
noise. The decomposition work was real; only the transport was broken.

This is the same class of bug as the distill parse-failures
([#2679](https://github.com/rysweet/Simard/issues/2679) /
[#2658](https://github.com/rysweet/Simard/issues/2658)) and the
`recipe_output/extract.rs` envelope scraping: **brittle parsing of an LLM/runner
stdout stream that was never a clean data channel.** The fix here is the same in
spirit — stop scraping the shared stream; give the payload its own channel.

## What changed

| | Before (#2708 bug) | After (this change) |
|---|---|---|
| Transport | Agent payload interleaved on shared `recipe-runner-rs` **stdout** | Agent writes payload to a **dedicated result file** |
| Simard read | `String::from_utf8_lossy(&output.stdout)` | `fs::read_to_string(<result file>)` |
| Parser | `json_candidates` brace-scan → try each slice (tolerant) | Single-shot **strict** `serde_json::from_str` on trimmed file contents |
| Failure on noise | Silent discard of a good decomposition | **Cannot happen** — stdout noise is never parsed |
| stdout role | The payload channel | Diagnostics only (read on non-zero exit) |

The stdout brace-scanning (`json_candidates`) is **deleted**. stdout is now read
**only** to build the diagnostic message when `recipe-runner-rs` exits non-zero;
it is never the source of the sub-goal payload.

## Data flow

```
simard goal decompose <goal_id>
        │
        ▼
RecipeGoalDecomposer::propose_subgoals
        │  1. allocate an O_EXCL TempDir under  <repo_root>/target/
        │     and compute the ABSOLUTE result-file path  <dir>/sub_goals.json
        │  2. spawn recipe-runner-rs goal-decomposition.yaml
        │        -c goal_id=…  -c goal_description=…  -c plan=…
        │        -c max_children=…
        │        -c sub_goals_output=<ABSOLUTE result-file path>
        ▼
recipe-runner-rs → agent ("default")
        │  runs the goal_decomposition prompt; the ONLY payload output is:
        │  writes  {"sub_goals":[…]}  to  {{sub_goals_output}}
        │  (stdout carries banners/logs/progress — ignored by Simard)
        ▼
child exits 0
        │
        ▼
RecipeGoalDecomposer::propose_subgoals (cont.)
        │  3. read_subgoals_from_file(<result file>)
        │        fs::read_to_string → size cap → trim → STRICT parse
        │  4. TempDir drops (RAII) → result file + dir removed
        ▼
Vec<SubGoalProposal>  →  decompose_goal  (2..=6 clamp, edge writes — unchanged)
```

The `TempDir` handle is held alive across the whole child run and the read; it
drops (deleting the file and directory) only after the proposals have been
parsed. Dropping it early would delete the file before the agent could write it.

## The result-channel contract

Three cooperating pieces implement the handoff. All three ship together; there
is **no** stdout fallback (a fallback would resurrect #2708).

### 1. Simard allocates the channel (`RecipeGoalDecomposer::propose_subgoals`)

- Creates a fresh, unpredictable `TempDir` **inside the repository working
  tree**, under `<repo_root>/target/` (git-ignored, and reliably writable by a
  workspace-sandboxed agent — a `/tmp` path can be refused by copilot/claude
  sandboxes).
- Computes the **absolute** path of the result file inside that dir and passes
  it as the recipe context variable `sub_goals_output` (`-c
  sub_goals_output=<abs>`).
- Does **not** pre-create the result file — some agents are create-only and
  refuse to overwrite an existing file. The agent creates it inside the
  pre-made directory.

### 2. The recipe threads the path into the prompt (`goal-decomposition.yaml`)

- Declares `sub_goals_output` in its context-var header and substitutes
  `{{sub_goals_output}}` into the agent prompt (`version: 1.1.0`).

### 3. The prompt binds the agent to write-only-JSON-to-path (`goal_decomposition.md`)

- Instructs the agent to write **only** the `{"sub_goals":[…]}` JSON object to
  the absolute path `{{sub_goals_output}}` using its file-write tool, and to
  emit **nothing** as a stdout payload.

Simard then reads back exactly that one file and strict-parses it. It never
inspects stdout for the payload and never globs `target/` for the result — it
reads only the absolute path it owns.

## Wire format

The result file contains a **single JSON value** and nothing else (leading and
trailing whitespace is trimmed before parsing). Two shapes are accepted, matching
the unchanged `SubGoalsPayload` model:

**Wrapped object (canonical, what the prompt asks for):**

```json
{"sub_goals": [
  {"description": "Add parent_goal_id + GoalNode data model", "done_criterion": "serde back-compat test green", "depends_on": []},
  {"description": "Implement typed-edge relationship facts",  "done_criterion": "edge round-trips via search_facts", "depends_on": [0]}
]}
```

**Bare array (also accepted):**

```json
[
  {"description": "Slice A", "done_criterion": "A is done"},
  {"description": "Slice B", "done_criterion": "B is done", "depends_on": [0]}
]
```

Field contract (unchanged — see
[goal decomposition reference](./goal-decomposition.md)):

| Field | Type | Required | Meaning |
|---|---|---|---|
| `description` | string (non-empty) | yes | What the sub-goal is. |
| `done_criterion` | string (non-empty) | yes | Independently-verifiable completion criterion. |
| `depends_on` | array of integers | no (default `[]`) | 0-based indices into this same array of the sibling sub-goals this one is gated on. |

The file is **strict JSON** — no markdown fences, no prose, no trailing
commentary. Unlike the deleted stdout scraper, prose or log noise wrapped
around the JSON is **not** tolerated; the parse is a single-shot
`serde_json::from_str` on the trimmed file contents. (The `2..=6` count clamp
and the `depends_on` index validation are enforced downstream by
[`decompose_goal`](./goal-decomposition.md#roll-up-parent-progress-from-children),
not by the parser.)

## Configuration

### Recipe context variable

| Variable | Passed as | Source | Consumed by |
|---|---|---|---|
| `sub_goals_output` | `-c sub_goals_output=<abs>` | Simard (`tempfile` only — **never** from `goal_description`/`plan` text) | `goal-decomposition.yaml` → `{{sub_goals_output}}` in the prompt |

`sub_goals_output` joins the existing decomposition context vars — `goal_id`,
`goal_description`, `plan`, `max_children` — documented in the
[goal decomposition reference](./goal-decomposition.md#decomposition-driver-assets).
It is derived **solely** from the `tempfile` allocation, so goal text can never
inject or redirect the result path. Because it is a namespaced transport var
(not user content), a goal description that literally contains
`{{sub_goals_output}}` cannot influence where the file lands — but that
guarantee holds **only** under `recipe-runner-rs`'s **single-pass Handlebars**
substitution, which renders `{{var}}` once and does **not** re-scan a
substituted value for further placeholders (so the literal `{{sub_goals_output}}`
inside a `goal_description` is passed through as inert text, never re-expanded).
The implementation should treat single-pass rendering as a load-bearing
assumption and assert it (a recursive/multi-pass engine would reopen the
injection path); see
[recipe context-var sanitization](./recipe-context-var-sanitization.md) for the
shared `{{var}}` substitution model and its escaping rules.

### Result-file allocation

| Property | Value | Rationale |
|---|---|---|
| Location | `<repo_root>/target/` (git-ignored) | In-tree so a workspace-sandboxed agent can write it; `target/` is already git-ignored (Simard `create_dir_all`s it first if absent). |
| Creation | `tempfile` **O_EXCL `TempDir`** (unpredictable name) | Defeats symlink / TOCTOU races on a shared host. |
| Path passed | **Absolute** | A relative path would resolve against the agent's cwd, not Simard's. |
| Directory perms | `0700` (set by `tempfile`) | Owner-only — **this is the enforced confidentiality control**; other users cannot traverse into the dir to reach the file. |
| Result-file mode | Agent-created — Simard does **not** pre-create it — so its mode follows the agent's umask | Confidentiality is provided by the owner-only `0700` directory above, not by the file's own mode. |
| Size cap | ~**1 MiB** | Bounds a runaway/hostile agent; over-cap reads fail loudly rather than OOM. |
| Lifetime | Held (RAII) across the child run **and** the read; dropped after | Prevents early deletion before the agent writes; guarantees cleanup on every exit path. |

`recipe-runner-rs` has **no** dedicated `--output-file` flag; the result path is
delivered exclusively as the `-c sub_goals_output=<abs>` context var, so the
recipe + prompt contract is the only delivery mechanism.

### Recipe version

`goal-decomposition.yaml` is bumped `1.0.0 → 1.1.0` — an **additive** change
(one new context var, one new output instruction). No behavioural fields of the
existing prompt change.

## API

All symbols live in `src/goal_curation/decompose.rs`. The
[`GoalDecomposer`](./goal-decomposition.md) trait and its
`propose_subgoals(&self, &ActiveGoal, usize) -> SimardResult<Vec<SubGoalProposal>>`
signature are **unchanged** — the file-channel change is internal to
`RecipeGoalDecomposer` — so the trait's test stubs (e.g. `CannedDecomposer` in
`src/goal_curation/tests_decompose.rs`) compile and pass without modification.

### `RecipeGoalDecomposer`

The production `GoalDecomposer` behind `simard goal decompose`.

```rust
pub struct RecipeGoalDecomposer { /* recipe_path, agent_binary, repo_root */ }

impl RecipeGoalDecomposer {
    /// Construct the decomposer if the recipe and `recipe-runner-rs` are both
    /// available; returns `None` otherwise so the caller surfaces a clear
    /// configuration error rather than silently degrading.
    pub fn new(repo_root: &Path) -> Option<Self>;
}
```

- **Added:** a private `repo_root: PathBuf` field (stored from `new`'s argument)
  so per-decomposition result files land in-tree under `<repo_root>/target/`.
- **Unchanged:** the public `new(repo_root: &Path) -> Option<Self>` signature and
  its availability checks (recipe resolution + `recipe-runner-rs --version`).

`propose_subgoals` now: allocates the in-tree result file, adds
`-c sub_goals_output=<abs>` to the argv, holds the `TempDir` alive across
`.output()`, and on **success** delegates to `read_subgoals_from_file`. On a
non-zero exit it still returns the existing
`InvalidGoalRecord { field: "decomposer", … }` built from stderr — stdout is
used only for that diagnostic, never for the payload.

### `read_subgoals_from_file` (new)

The single migration seam between "a file on disk" and "typed proposals".

```rust
/// Read the decomposition result file and strict-parse it into proposals.
/// Enforces a ~1 MiB size cap, trims, then strict-deserializes. Missing,
/// empty/whitespace-only, oversized, or malformed content returns a loud
/// `InvalidGoalRecord { field: "sub_goals", .. }`. Never returns `Ok(vec![])`.
fn read_subgoals_from_file(path: &Path) -> SimardResult<Vec<SubGoalProposal>>;
```

### `parse_subgoals_json` (strict)

Kept `pub` for API/test continuity, but its body is now a **single-shot strict**
parse — the `json_candidates` brace-scan is gone.

```rust
/// Strict-parse the decomposition result text into sub-goal proposals.
/// `serde_json::from_str::<SubGoalsPayload>` on the trimmed input; any prose,
/// log noise, or markdown fence around the JSON is rejected (that tolerance was
/// the #2708 bug). Malformed input → `InvalidGoalRecord { field: "sub_goals" }`.
pub fn parse_subgoals_json(text: &str) -> SimardResult<Vec<SubGoalProposal>>;
```

### `SubGoalProposal` / `SubGoalsPayload` (unchanged)

The typed proposal and the untagged accept-either-shape payload are preserved
exactly:

```rust
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SubGoalProposal {
    pub description: String,
    pub done_criterion: String,
    #[serde(default)]
    pub depends_on: Vec<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SubGoalsPayload {
    Wrapped { sub_goals: Vec<SubGoalProposal> },
    Bare(Vec<SubGoalProposal>),
}
```

## Error handling — loud, never empty

A missing or unusable structured result **must surface loudly** — it must never
degrade to an empty decomposition (an empty `Vec` would silently strip a good
umbrella goal of its children). Every failure below returns

```rust
Err(SimardError::InvalidGoalRecord { field: "sub_goals".to_string(), reason })
```

which renders (via `SimardError`'s `Display`) as:

```
invalid goal record field 'sub_goals': <reason>
```

| Condition | Detected in | Outcome |
|---|---|---|
| Result file missing after a `0` exit | `read_subgoals_from_file` | loud `InvalidGoalRecord{ sub_goals }` |
| Result file empty / whitespace-only | `read_subgoals_from_file` | loud `InvalidGoalRecord{ sub_goals }` |
| Result file exceeds the ~1 MiB cap | `read_subgoals_from_file` | loud `InvalidGoalRecord{ sub_goals }` |
| Result file malformed JSON | `parse_subgoals_json` | loud `InvalidGoalRecord{ sub_goals }` |
| `recipe-runner-rs` non-zero exit | `propose_subgoals` | loud `InvalidGoalRecord{ decomposer }` (from stderr) |
| Fewer than `MIN_SUBGOALS` (2) proposals | `decompose_goal` | loud `InvalidGoalRecord{ sub_goals }` (unchanged) |

The operator CLI (`src/operator_cli/goal.rs`) wraps any of these as
`decomposition failed: <err>` and exits non-zero, writing nothing to the board
or the graph. `Ok(vec![])` is **never** a valid return from the channel.

## Security considerations

- **No stdout brace-scanner.** Deleting `json_candidates` removes the exact
  mechanism that let surrounding noise (including attacker-influenceable log
  echoes of the goal text) rescue or corrupt a parse. Net posture is a
  **security improvement** over the deleted scraper.
- **Strict, bounded deserialization.** The result file is untrusted agent
  output: it is strict-parsed with serde's default recursion limit and a
  ~1 MiB size cap to prevent OOM/DoS from a runaway or hostile agent.
- **File-channel integrity.** The unpredictable **O_EXCL** `TempDir` defeats
  symlink/TOCTOU races; Simard reads back **only** the single absolute path it
  owns and never globs `target/`. File contents are never logged.
- **Path is never user-derived.** `sub_goals_output` comes solely from
  `tempfile`, never from `goal_description`/`plan`, so there is no path
  injection **given single-pass `{{var}}` rendering** (a `goal_description`
  containing a literal `{{sub_goals_output}}` is passed through inert, not
  re-expanded — see [Configuration](#configuration)). The owner-only `0700`
  `TempDir` keeps the result confidential on shared hosts (the agent creates the
  file, so its mode follows the agent's umask, but the `0700` directory shields
  it regardless).
- **No shell.** Every value is passed as a discrete `Command::arg` (no shell
  interpolation), so there is no argv/shell injection through the new context
  var — consistent with the
  [recipe context-var sanitization](./recipe-context-var-sanitization.md)
  hardening.
- **Prompt-injection containment.** The prompt already fences the goal text as
  untrusted **data**; the least-authority "write only the JSON object to this
  one path" contract plus output containment (Simard ignores stdout and reads
  exactly one owned file) keeps the blast radius capped — and the decomposed
  data is inert (recorded, never executed) and further bounded by the `2..=6`
  child clamp.

## Examples

### `simard goal decompose` — success (noise-immune)

The operator command is unchanged (see the
[operator CLI section](./goal-decomposition.md#operator-cli) and the
[decompose-a-large-goal runbook](../howto/decompose-a-large-goal.md)). The
difference is that it now succeeds regardless of how noisy `recipe-runner-rs`
stdout is, because that stdout is no longer parsed:

```console
$ simard goal decompose goal-7a1c --max-children 4
[simard] goal decompose: 'goal-7a1c' -> 4 child goal(s) [Board]: goal-7a1c-c1, goal-7a1c-c2, goal-7a1c-c3, goal-7a1c-c4
```

Before #2708, the same command could fail with `decomposition failed: … could
not parse sub-goals from decomposition output …` even though the agent had
produced correct sub-goals — because banners/log lines around the JSON broke the
stdout brace-scan. That failure mode is gone: the sub-goals arrive through the
`sub_goals_output` file, and stdout noise is irrelevant.

### What the agent writes

The agent writes exactly this to the path Simard passed in `sub_goals_output`
(any progress banners it also prints to stdout are ignored):

```json
{"sub_goals": [
  {"description": "Add parent_goal_id + GoalNode data model", "done_criterion": "serde back-compat test green", "depends_on": []},
  {"description": "Implement typed-edge relationship facts",  "done_criterion": "edge round-trips via search_facts", "depends_on": [0]},
  {"description": "Add decompose_goal driver + prompt asset", "done_criterion": "2-6 children, content-pin test green", "depends_on": [0]},
  {"description": "Wire simard goal decompose CLI verb",      "done_criterion": "verb routes through the writer path", "depends_on": [2]}
]}
```

### Behaviour matrix (what the tests pin)

| Result-file contents | Outcome |
|---|---|
| Clean `{"sub_goals":[…]}` (2 entries) | 2 typed `SubGoalProposal`s, `depends_on` preserved |
| Clean bare `[ … ]` array | parsed identically to the wrapped form |
| JSON embedded in ANSI/log/banner **prose** | **loud** `InvalidGoalRecord{ sub_goals }` — the old brace-scan would have "rescued" it; this proves the stdout transport is gone |
| Missing file (agent wrote nothing) | **loud** `InvalidGoalRecord{ sub_goals }` |
| Empty / whitespace-only file | **loud** `InvalidGoalRecord{ sub_goals }` |
| Malformed JSON | **loud** `InvalidGoalRecord{ sub_goals }` |
| Over ~1 MiB | **loud** `InvalidGoalRecord{ sub_goals }` |

The pairing of the first row (a clean file parses regardless of any stdout) with
the third row (JSON wrapped in prose is now rejected) is the proof that the
payload no longer comes from stdout: noisy stdout can neither break a good
decomposition nor rescue a bad file.

## Migrating to a typed result-channel substrate

The dedicated result **file** is the durable, ship-now transport. It also sets up
a clean migration to a future first-class **typed result channel** for agent→host
structured results (a "semantic channel" substrate anticipated in amplihack-rs).
If and when such a substrate lands, the goal-decomposition handoff migrates by
swapping the single `read_subgoals_from_file` seam for a typed-channel read; the
recipe `sub_goals_output` var becomes the channel handle, and the prompt's
"write-only-JSON-to-path" instruction becomes a structured channel write. The
`RecipeGoalDecomposer` / `parse_subgoals_json` / `SubGoalProposal` surface and
the loud-error contract stay the same. Until then, the file channel is the
supported mechanism — **not** a stdout fallback.

A fully agentic variant (the agent writing each sub-goal **directly** through a
goal-store tool so there is no payload for Simard to read at all) was evaluated
and deferred: no agent-writable goal-store interface exists today, and Simard
still owns all graph-edge writes (`goal_curation/edges.rs`) after reading the
proposals back. The file channel is the minimal, back-compatible step that
removes the #2708 root cause without inventing a new agent-side store API.

## Guarantees and non-guarantees

**Contract this change provides:**

- The decomposition agent's sub-goals reach Simard through a **dedicated result
  file**, never through the shared `recipe-runner-rs` stdout.
- **Stdout noise (ANSI, `tracing`, banners, progress) can no longer break a
  decomposition** — the payload is not parsed from stdout, so a successful agent
  run is never discarded for stdout formatting.
- A missing, empty, oversized, or malformed structured result surfaces a **loud**
  `InvalidGoalRecord{ field: "sub_goals" }`; the channel **never** returns an
  empty decomposition and **never** silently falls back to stdout.
- The result file is allocated with an unpredictable O_EXCL `TempDir` under
  `<repo_root>/target/`, passed by absolute path, size-capped at ~1 MiB, and
  cleaned up (RAII) on every exit path.
- The `GoalDecomposer` trait signature and its test stubs are unchanged;
  `parse_subgoals_json` stays `pub` (now strict); `SubGoalProposal` /
  `SubGoalsPayload` wire shapes are unchanged.
- The downstream invariants are untouched: the `2..=6` fan-out clamp,
  deterministic `<parent>-c<n>` child ids, and typed `decomposes_into` /
  `depends_on` edge writes (see
  [goal decomposition reference](./goal-decomposition.md)).

**Not guaranteed (deferred):**

- **Typed result-channel transport.** A future amplihack-rs typed result channel
  would be the durable substrate; this change uses a dedicated file now and can
  migrate later (see [above](#migrating-to-a-typed-result-channel-substrate)).
- **Fully agentic goal-store writes.** Having the agent write sub-goals directly
  into the goal store (no file at all) is deferred — no agent-writable store
  interface exists yet.
- **Structured `depends_on` cross-checking at the channel.** The parser accepts
  the typed shape; index-range validation and the `2..=6` count clamp remain the
  responsibility of `decompose_goal`, not the file channel.

## Related

- [Goal decomposition & the goal graph](./goal-decomposition.md) — the data
  model, typed-edge graph, `decompose_goal` driver, and `simard goal decompose`
  CLI this transport feeds
- [Text-parsing wire formats](./text-parsing-wire-formats.md) — the normative
  catalogue of the text/JSON contracts Simard parses from LLM/recipe output
- [Recipe context-var sanitization](./recipe-context-var-sanitization.md) — how
  user/log-derived `-c` context values are made safe for `recipe-runner-rs`
- [Distill recipe output capture](./distill-recipe-output-capture.md) and
  [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — the sibling recipe-output parse-failure work (#2679 / #2658) this fix mirrors
- [Simard CLI reference](./simard-cli.md) — the `simard goal` verb tree
- [How to decompose a large goal](../howto/decompose-a-large-goal.md) — the
  operator runbook
- [Troubleshoot the goal store](../howto/troubleshoot-goal-store.md) — where a
  loud `decomposition failed:` surfaces to an operator
