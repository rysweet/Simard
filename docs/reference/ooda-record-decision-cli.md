---
title: "Reference: simard ooda record-decision (typed decision tool)"
description: The zero-privilege CLI tool the OODA per-goal-cycle reasoner calls to record exactly one typed, validated decision, and the file-backed PerGoalDecisionRecord seam RecipeBrain reads instead of scraping prose JSON. Covers usage, the closed-enum contract, the fail-CLOSED read matrix, configuration, security, and examples.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-per-goal-cycle-recipe.md
  - ./ooda-per-goal-cycle-api.md
  - ./ooda-brain-decision-protocol.md
  - ../concepts/agentic-per-goal-per-cycle.md
  - ../index.md
---

# Reference: `simard ooda record-decision` (typed decision tool)

CLI: `src/operator_cli/ooda.rs` (`dispatch_record_decision`)
Record types + reader: `src/ooda_brain/mod.rs` (`PerGoalDecisionRecord`, `read_verified`)
Reader call site: `src/ooda_brain/recipe_brain.rs` (`run_per_goal_cycle_recipe` / `decide_per_goal_cycle`)

`simard ooda record-decision` is the **tool the reasoner calls** to record its
per-goal, per-cycle verdict. It replaces the forbidden
"recipe prints JSON → Rust scrapes prose → Rust acts" pattern
([#2573](https://github.com/rysweet/Simard/issues/2573),
[#2658](https://github.com/rysweet/Simard/issues/2658)) on the **core decision
path** that governs every autonomous decision the daemon makes
(`continue` / `spawn` / `reorient` / `investigate` / `wait` / `complete`).

Instead of emitting a prose JSON envelope for the Rust layer to recover with
`recipe_output::extract_json_payload`, the `ooda-per-goal-cycle` recipe now calls
this tool **exactly once**. The tool validates the choice against the closed
`PerGoalAction` enum and atomically writes a single typed record. `RecipeBrain`
then reads that typed record with `read_verified` — it never parses the agent's
stdout.

!!! danger "This is the core loop"
    This seam is on the highest-blast-radius path in Simard: it decides whether
    to spawn engineers, wait, complete, or reorient **every** active goal, every
    cycle. Every failure mode is **fail-CLOSED**: an absent, unreadable,
    malformed, out-of-enum, or goal/cycle-mismatched record surfaces as an
    `Err` → a safe no-op cycle failure. There is **no default action** anywhere
    in this path ([#1711](https://github.com/rysweet/Simard/issues/1711)).

## What the tool does (and does not do)

The tool holds **zero privilege**. Its sole side effect is writing one JSON
record file. It records **intent only**:

- It does **not** spawn engineers, mutate refs, roll cycles, or close goals.
- It does **not** call the Bridge, Python, or kuzu, and holds no tokens.
- All authority stays with the thin deterministic rail in
  [`src/ooda_loop/cycle.rs`](./ooda-per-goal-cycle-api.md#driver-loop-cyclers),
  which applies admission/safety/`mutates_refs` gating **after** the enum is
  read.

Separating *recording a decision* (this tool) from *acting on it* (the rail)
is what keeps the reasoner unprivileged and the blast radius bounded.

## Usage

```text
simard ooda record-decision \
  --choice <continue|spawn|reorient|investigate|wait|complete> \
  --reason "<short concrete reason>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--task-hint "<hint>"] \
  [--reason-path <FILE>] \
  [--task-hint-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout
(the reader ignores stdout entirely). On any validation failure it writes
**no file** and exits non-zero with a diagnostic on stderr.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--choice` | yes | One of the six closed `PerGoalAction` variants. Matched **case-insensitively**; anything else is rejected. |
| `--reason` | yes* | Short concrete reason for the decision. Must be non-empty after trimming. |
| `--record-path` | yes | **Absolute** path the daemon supplied via the recipe's `-c record_path` context var. Must not contain `..`. |
| `--goal-id` | yes | The goal this decision is for. Embedded in the record and re-verified by the reader. |
| `--cycle-number` | yes | The cycle this decision is for (`u32`). Embedded in the record and re-verified by the reader. |
| `--task-hint` | no | Optional next-piece guidance. Only meaningful for `spawn`; ignored for other choices. |
| `--reason-path` | no | Read `reason` from a file instead of argv (for large text; see [Large payloads](#large-payloads-file-not-argv)). Mutually exclusive with `--reason`. |
| `--task-hint-path` | no | Read `task_hint` from a file instead of argv. Mutually exclusive with `--task-hint`. |

\* Exactly one of `--reason` / `--reason-path` must be supplied and resolve to
non-empty text. The same applies pairwise to `--task-hint` / `--task-hint-path`,
except the hint pair is entirely optional (supplying neither is valid).

Unknown or duplicate flags are rejected (`reject_extra_args`); the tool never
silently ignores an argument.

!!! note "Inline **and** file inputs — deliberate deviation from `dispatch_terminal`"
    `record-decision` is modeled on `dispatch_terminal`, but it does **not**
    copy terminal's `read_opaque` verbatim. `read_opaque`
    (`src/operator_cli/ooda.rs`) is **file-only** — it reads a
    single opaque payload from a path and has no inline value form. This tool
    instead supports **both** an ergonomic inline value (`--reason`,
    `--task-hint`) **and** a file-backed variant (`--reason-path`,
    `--task-hint-path`) for each free-text field, with the two forms **mutually
    exclusive per field** (supplying both for the same field is a rejected-args
    error). This is a conscious design decision, not an accident of reuse:

    - **Inline** keeps the common case (a short one-line reason) a single argv
      token, matching every example in this document.
    - **File-backed** satisfies the operator's "large text goes through a file,
      never argv" constraint without forcing a temp file for short reasons.

    Implementers must therefore build the inline path explicitly; reusing
    `read_opaque` alone would drop `--reason`/`--task-hint` and break the
    documented usage. Both paths converge on the same
    `sanitize_context_var(_, 500)` normalization before the value is written.

### The closed choice enum

`--choice` is validated by constructing the existing
[`PerGoalAction`](./ooda-per-goal-cycle-api.md#pergoalaction) enum — the **single
source of truth**. There is no parallel enum to drift against.

| Choice | Meaning | Mutates refs / rolls cycle? |
|---|---|---|
| `continue` | Work is in flight and healthy; leave it. | no |
| `spawn` | No live work; start the next concrete piece. May carry `task_hint`. | no |
| `reorient` | Deliberately redirect the goal to a new angle. | **yes** |
| `investigate` | Something looks wrong; inspect logs/tools first. The only gate to a later reclaim. | no |
| `wait` | Legitimately blocked on an external event. | no |
| `complete` | Success criteria observably met; close the goal. | **yes** |

Adding a variant is a coordinated change — see
[Versioning & Compatibility](#versioning--compatibility).

## The typed record: `PerGoalDecisionRecord`

The tool writes a single JSON object to `--record-path`. `RecipeBrain` reads and
verifies it with `read_verified`.

```rust
/// One typed, on-disk per-goal-cycle decision, written by the
/// `simard ooda record-decision` tool and read by RecipeBrain via
/// `read_verified`. Never scraped from agent prose.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PerGoalDecisionRecord {
    /// Schema pin. Must equal EXPECTED_SCHEMA ("simard.ooda.per_goal_decision.v1").
    pub schema: String,
    /// The goal this decision is for. Re-verified against the live ctx.
    pub goal_id: String,
    /// The cycle this decision is for. Re-verified against the live ctx.
    pub cycle_number: u32,
    /// The validated, closed-enum action (flattened `choice` + fields).
    #[serde(flatten)]
    pub action: PerGoalAction,
}

pub const EXPECTED_SCHEMA: &str = "simard.ooda.per_goal_decision.v1";
```

### On-disk shape

```json
{
  "schema": "simard.ooda.per_goal_decision.v1",
  "goal_id": "cognition-research",
  "cycle_number": 4287,
  "choice": "spawn",
  "reason": "last PR merged; standing research goal must not sit idle",
  "task_hint": "design a distillation fact-yield experiment"
}
```

The `choice` discriminator and its fields come from `PerGoalAction`'s existing
`#[serde(tag = "choice", rename_all = "snake_case")]` representation, flattened
into the record — so the tool and the enum can never disagree on the wire shape.

### `read_verified` — the fail-CLOSED reader

```rust
/// Read and fully verify a per-goal decision record.
///
/// Returns `Ok(PerGoalAction)` ONLY when the record exists, deserializes,
/// pins the expected schema, and its embedded goal_id/cycle_number match the
/// live ctx. EVERY other outcome is an `Err` — the caller surfaces it as a
/// cycle failure (safe no-op), never a default action (#1711).
pub fn read_verified(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<PerGoalAction>;
```

The reader re-validates independently of the tool (defense in depth against a
stale, replayed, or partially written record):

| # | Condition | Result |
|---|---|---|
| R1 | File absent (tool never ran / binary unresolvable / tool exited non-zero) | **`Err` → no-op** |
| R2 | File present but not valid JSON / truncated | **`Err` → no-op** |
| R3 | `schema != EXPECTED_SCHEMA` (e.g. a future `…v2`) | **`Err` → no-op** |
| R4 | `choice` not one of the six closed variants | **`Err` → no-op** |
| R5 | `reason` missing or empty | **`Err` → no-op** |
| R6 | `goal_id` ≠ live ctx `goal_id` (stale/other-goal record) | **`Err` → no-op** |
| R7 | `cycle_number` ≠ live ctx `cycle_number` (prior-cycle record) | **`Err` → no-op** |
| R8 | All checks pass | `Ok(PerGoalAction)` |

R6/R7 are what prevent the subtle **fail-open** risk of reading a previous
cycle's decision as if it were this cycle's. In addition to this check, the
reader receives a **fresh, unique per-cycle temp directory** (structural
guarantee — see [Configuration](#configuration)), so a stale record cannot even
be at the path.

## How the reasoner calls it

`RecipeBrain::run_per_goal_cycle_recipe` wires the recipe up so the agent can
call the tool and the reader can find the result:

1. Allocate a **fresh unique per-cycle temp directory** (owner-only, `0o600`
   record mode, cleaned up after the cycle).
2. Pass two context vars to the recipe via `-c` (argv-only, never `sh -c`):
   - `-c record_path=<tempdir>/decision.json`
   - `-c simard_bin=<current_exe absolute path>` — resolved the same way
     `recipe-runner-rs` is resolved (`std::env::current_exe()`).
3. Run the `ooda-per-goal-cycle` recipe unchanged — **no timeout** on the
   agentic step.
4. Read the verdict with `read_verified(record_path, ctx.goal_id, ctx.cycle_number)`.
   The agent's stdout is **ignored**; a stray JSON print has zero effect.

If the tool cannot be resolved or exits non-zero, no record is written and the
reader fails CLOSED at R1.

The recipe's agent step invokes the tool roughly like:

```bash
"$simard_bin" ooda record-decision \
  --choice spawn \
  --reason "last PR merged; standing research goal must not sit idle" \
  --task-hint "design a distillation fact-yield experiment" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

See [Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema](./ooda-per-goal-cycle-recipe.md)
for the recipe/prompt contract.

## Configuration

| Setting | Source | Default | Notes |
|---|---|---|---|
| `record_path` | recipe `-c record_path` | none (required) | Absolute path inside a daemon-owned, per-cycle temp dir. The tool rejects non-absolute or `..`-bearing paths (SR-VAL-8). |
| `simard_bin` | recipe `-c simard_bin` | `current_exe()` | Absolute path to the running `simard` binary, so the recipe sandbox resolves the tool deterministically. |
| `schema` pin | `EXPECTED_SCHEMA` const | `simard.ooda.per_goal_decision.v1` | Bumping the version is a hard, coordinated change (reader rejects any other value). |
| record file mode | `persistence::persist_json` | `0o600` | Owner-only; atomic temp+fsync+rename. |
| free-text bound | `sanitize_context_var(_, 500)` | 500 chars | Applies to `reason` and `task_hint`. |

There are **no new `SIMARD_*` environment knobs** and **no database** — the
seam is a single file-backed JSON record. Schema evolution is handled by serde
plus the pinned schema string.

### Large payloads (file, not argv)

Per the operator constraint, oversized text goes through a **file**, never argv.
Use `--reason-path` / `--task-hint-path` to point at files the agent wrote with
its file tool — the per-field file alternative to inline `--reason` /
`--task-hint` (see the [inline-and-file note](#arguments) above; the two forms
are mutually exclusive per field). Both inline and file-sourced text are run
through `sanitize_context_var(_, 500)` (strips ANSI/CSI and C0/DEL control
bytes, folds newlines, bounds length) before being written to the record.

## Fail-CLOSED contract (the whole point)

Fail-CLOSED is the **only** failure mode on this path. There is no fallback to
`continue`, no default action, and no silent coercion:

- Any tool-side validation failure → **no file written**, non-zero exit.
- Any reader-side failure (R1–R7) → **`Err`** → the cycle records an explicit
  failure and performs a **safe no-op** (no ref mutation, no spawn, no roll).

This mirrors the no-silent-fallback guarantee established in
[#1711](https://github.com/rysweet/Simard/issues/1711): the only brain that ever
returns `Continue` without reasoning is the explicit
`DeterministicLifecycleBrain` (no LLM available), which by construction never
rolls or reaps.

## Security

The tool and reader are hardened against the classes of misbehavior that matter
on the core loop.

| ID | Threat | Mitigation |
|---|---|---|
| SR-AUTHZ-1 | Reasoner over-reach | The tool holds **zero privilege**: its only side effect is one `persist_json` write. No spawn, no ref mutation, no Bridge/Python/kuzu, no tokens. |
| SR-AUTHZ-2 | Bypassing the rail | Authority stays with the deterministic rail, which gates on the validated enum **after** the read. No `--admin` / `--no-verify` / bypass flag exists. |
| SR-VAL-1 | Injected / drifted choice | `--choice` validated by constructing `PerGoalAction` (case-insensitive); no parallel enum. |
| SR-VAL-3 | Terminal-escape / log injection via free text | `reason` / `task_hint` run through `sanitize_context_var(_, 500)` — strips ANSI/CSI + C0/DEL, folds newlines, bounds to 500 chars ([#2751](https://github.com/rysweet/Simard/issues/2751)). |
| SR-VAL-7 | Replay / stale-record fail-open | Reader independently checks `schema == EXPECTED_SCHEMA`, `goal_id == ctx`, `cycle_number == ctx` → mismatch fails CLOSED. |
| SR-VAL-8 | Path traversal / symlink write | `--record-path` must be **absolute** and free of `..`; the parent must be the daemon-supplied per-cycle temp dir. The daemon-owned fresh temp dir is the primary control (the underlying `persist_bytes` uses a plain rename). |
| SR-DATA-1 | World-readable record | `0o600` owner-only mode. |
| SR-DATA-2 | Torn / partial write | Atomic temp + fsync + rename. |
| SR-DATA-3 | Secret leakage | No secrets in the record; free text is sanitized and bounded. |
| SR-DATA-4 | Cross-cycle bleed | Ephemeral, unique per-cycle temp dir, cleaned up after the cycle. |
| SR-DATA-5 | Log flooding / raw transcript | No raw-transcript or unsanitized-reason logging. |

**Validate-all-then-write-once:** every validation runs before any file is
written; a single failure means **no** record on disk.

## Examples

### `spawn` with a hint (standing research goal, idle between bursts)

```bash
simard ooda record-decision \
  --choice spawn \
  --reason "no live work; standing research goal must seek the next source" \
  --task-hint "survey arXiv 2026 for new results on <topic>" \
  --record-path /run/simard/ooda/cycle-4287-cognition/decision.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

Record written:

```json
{
  "schema": "simard.ooda.per_goal_decision.v1",
  "goal_id": "cognition-research",
  "cycle_number": 4287,
  "choice": "spawn",
  "reason": "no live work; standing research goal must seek the next source",
  "task_hint": "survey arXiv 2026 for new results on <topic>"
}
```

### `investigate` (quiet worker — NOT an auto-reap)

```bash
simard ooda record-decision \
  --choice investigate \
  --reason "stale_claim_secs=9000 and log tail truncated mid-tool-call; read logs before any reclaim" \
  --record-path /run/simard/ooda/cycle-4287-cognition/decision.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

### `wait` (blocked on external CI)

```bash
simard ooda record-decision \
  --choice wait \
  --reason "PR #4501 awaiting required CI checks; nothing actionable this cycle" \
  --record-path /run/simard/ooda/cycle-4287-cognition/decision.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

### Large reason via file (not argv)

```bash
# The agent wrote the long reason with its file tool:
simard ooda record-decision \
  --choice reorient \
  --reason-path /run/simard/ooda/cycle-4287-cognition/reason.txt \
  --record-path /run/simard/ooda/cycle-4287-cognition/decision.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

### Rejections (no file written, non-zero exit)

```bash
# Out-of-enum choice
simard ooda record-decision --choice deploy ...     # error: unknown choice 'deploy'

# Empty reason
simard ooda record-decision --choice spawn --reason "" ...   # error: --reason must be non-empty

# Non-absolute record path
simard ooda record-decision --record-path ./decision.json ...  # error: --record-path must be absolute

# Path traversal
simard ooda record-decision --record-path /run/simard/../etc/x ...  # error: --record-path must not contain '..'
```

## Versioning & Compatibility

Adding an action is a coordinated change (unchanged from the recipe contract):

1. Add the variant to `PerGoalAction` in `src/ooda_brain/mod.rs`.
2. Add its `apply_per_goal_action_to_state` mutation (respecting the A6
   `wip_refs` invariant).
3. Extend the recipe/prompt `OPTIONS` guidance so the agent knows to pass it to
   `--choice`.
4. Add an example here and in the recipe reference.
5. Add serde round-trip + `read_verified` fail-closed tests.

Bumping the record **schema** (`…v1` → `…v2`) is a hard change: the reader
rejects any value other than `EXPECTED_SCHEMA`, so a new writer and a new reader
must ship together.

### Compatibility with the shared prose scraper

This change removes the **only** `extract_json_payload` call on the per-goal-cycle
decision path. The shared scraper family in `src/recipe_output/extract.rs`
(`extract_json_payload`, `extract_and_parse_json`, `strip_recipe_noise`,
`last_balanced_object`) is **retained** because other seams still call it
(admission / resource-admission / idea-dedup / consolidation / outcome / decision
/ lifecycle envelopes, plus the shared re-export). It is deleted only once
`grep -rn extract_json_payload src/` returns no remaining callers.

## Regression tests

| Test | Asserts |
|---|---|
| `read_verified` absent | R1 → `Err` (no-op) |
| `read_verified` malformed JSON | R2 → `Err` (no-op) |
| `read_verified` wrong schema (`…v2`) | R3 → `Err` (no-op) |
| `read_verified` out-of-enum choice | R4 → `Err` (no-op) |
| `read_verified` empty reason | R5 → `Err` (no-op) |
| `read_verified` goal mismatch | R6 → `Err` (no-op) |
| `read_verified` cycle mismatch | R7 → `Err` (no-op) |
| six-variant round-trip | Each `PerGoalAction` writes and reads back bit-for-bit |
| CLI enum reject | `--choice deploy` → non-zero, **no file** |
| CLI empty-reason reject | `--reason ""` → non-zero, **no file** |
| CLI oversized reason | reason bounded to 500 chars in the record |
| CLI sanitize | ANSI/C0 bytes stripped from `reason`/`task_hint` |
| CLI file mode | record is `0o600` |
| CLI path guard | non-absolute / `..` `--record-path` → non-zero, **no file** |
| rail integration | a brain `Err` → rail no-op, **no ref mutation**; `mutates_refs` unchanged for `reorient` / `complete` |

## See Also

- [Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema](./ooda-per-goal-cycle-recipe.md) — how the recipe calls this tool
- [Reference: OODA Per-Goal-Cycle API](./ooda-per-goal-cycle-api.md) — `PerGoalCycleCtx`, `PerGoalAction`, the driver loop
- [Concept: Agentic Per-Goal, Per-Cycle Decision](../concepts/agentic-per-goal-per-cycle.md)
- [Reference: OODA Brain Decision Protocol](./ooda-brain-decision-protocol.md)
- [Reference: Simard CLI](./simard-cli.md)
- [Issue #1711 — no silent fallback on the decision path](https://github.com/rysweet/Simard/issues/1711)
- [Issue #2573 — forbid recipe-emits-JSON → Rust-scrapes → Rust-acts](https://github.com/rysweet/Simard/issues/2573)
