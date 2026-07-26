---
title: "Reference: simard ooda record-orient / record-decide (typed OODA orient+decide tools)"
description: The two zero-privilege CLI tools the OODA orient and decide reasoners call to record exactly one typed, validated judgment each, and the file-backed OrientDecisionRecord / DecideDecisionRecord seams RecipeBrain reads instead of scraping prose. Covers usage, the closed-enum + bounded-float contracts, the fail-CLOSED read matrix, configuration, security, and examples.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-record-decision-cli.md
  - ./ooda-orient-recipe.md
  - ./ooda-decide-prompt.md
  - ./text-parsing-wire-formats.md
  - ./recipe-brain-verdict-parsing.md
  - ../concepts/text-based-brain-protocol.md
  - ../index.md
---

# Reference: `simard ooda record-orient` / `simard ooda record-decide` (typed OODA orient+decide tools)

CLI: `src/operator_cli/ooda.rs` (`dispatch_record_orient`, `dispatch_record_decide`)
Record types, schema pins, chokepoints, readers: `src/ooda_brain/orient_decide_record.rs`
(`OrientDecisionRecord`, `DecideDecisionRecord`, `OrientFields::from_fields`,
`DecideChoice::from_choice_fields`, `read_verified_orient`, `read_verified_decide`)
Re-exports: `src/ooda_brain/mod.rs`
Reader call sites: `src/ooda_brain/recipe_brain.rs` (`run_orient_recipe` / `judge_orientation`,
`run_decide_recipe` / `judge_decision`)

`simard ooda record-orient` and `simard ooda record-decide` are the **tools the
orient and decide reasoners call** to record their per-cycle judgments. They
replace the forbidden "recipe prints JSON/prose → Rust scrapes stdout with
`recipe_output::extract_and_parse_json` / `extract_json_payload` → Rust acts"
pattern on the **two core OODA phases** — orient (failure-penalty urgency
demotion) and decide (action-kind routing) — as the Group A slice of epic
[#4719](https://github.com/rysweet/Simard/issues/4719).

They follow the reference pattern established by
[`simard ooda record-decision`](./ooda-record-decision-cli.md) (WS-4,
[#4734](https://github.com/rysweet/Simard/issues/4734)) **exactly**: the recipe
**acts via a gated `simard <verb>` tool** that writes a typed, owner-only
(`0o600`), freshness-checked record; the thin Rust rail reads that record
**fail-CLOSED** — a bad, absent, or mismatched record surfaces as an `Err` and
becomes a safe no-op, **never** a default action.

!!! danger "These are the core OODA phases"
    Orient sets every goal's urgency and decide routes every goal to an action,
    every cycle. Every failure mode is **fail-CLOSED**: an absent, unreadable,
    malformed, out-of-enum, out-of-range, or goal/cycle/schema-mismatched record
    surfaces as an `Err`. There is **no default action** and **no silent
    default** anywhere in these paths. The pre-conversion silent defaults —
    decide → `advance_goal`, orient → the deterministic floor — are **removed**
    ([#1711](https://github.com/rysweet/Simard/issues/1711)).

## What the tools do (and do not do)

Both tools hold **zero privilege**. The sole side effect of each is writing one
JSON record file. Each records **intent only**:

- They do **not** roll cycles, mutate urgency in the live board, route actions,
  spawn engineers, or close goals.
- They do **not** call the Bridge, Python, or kuzu, and hold no tokens.
- All authority stays with the thin deterministic rails
  ([`src/ooda_loop/orient.rs`](../reference/ooda-brain-api.md),
  [`src/ooda_loop/decide.rs`](../reference/ooda-brain-api.md)), which apply the
  urgency and routing **after** the record is read and re-validated.

Separating *recording a judgment* (these tools) from *acting on it* (the rails)
is what keeps the reasoner unprivileged and the blast radius bounded.

---

## `simard ooda record-orient`

Records one orient-phase failure-penalty demotion judgment.

### Usage

```text
simard ooda record-orient \
  --adjusted-urgency <F> \
  --confidence <F> \
  --demotion-applied <F> \
  --base-urgency <F> \
  --reason "<short concrete reason>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--reason-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout
(the reader ignores stdout entirely). On any validation failure it writes **no
file** and exits non-zero with a diagnostic on stderr.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--adjusted-urgency` | yes | Final urgency, `f64`. Must be **finite** and in `[0.0, 1.0]`. |
| `--confidence` | yes | Brain self-reported confidence, `f64`. Must be **finite** and in `[0.0, 1.0]`. |
| `--demotion-applied` | yes | How much urgency was removed, `f64`. Must be **finite** and ≥ `0.0`. |
| `--base-urgency` | yes | The pre-penalty urgency the daemon supplied via the recipe's `-c base_urgency`. Must be **finite** and in `[0.0, 1.0]`. Persisted in the record so the reader can re-run the no-escalation check self-consistently. |
| `--reason` | yes\* | Short concrete rationale. Must be non-empty after sanitizing. |
| `--record-path` | yes | **Absolute** path supplied via the recipe's `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The goal this judgment is for. Embedded in the record and re-verified by the reader. |
| `--cycle-number` | yes | The cycle this judgment is for (`u32`). Embedded in the record and re-verified by the reader. |
| `--reason-path` | no | Read `reason` from a file instead of argv (for large text). Path must be **absolute** and free of `..`; the file is read under a 64 KiB cap. Mutually exclusive with `--reason`. |

\* Exactly one of `--reason` / `--reason-path` must be supplied and resolve to
non-empty text after sanitizing.

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist; the
tool never silently ignores an argument.

### The orient chokepoint

All four floats plus the reason are validated by a **single shared**
`OrientFields::from_fields` constructor — the same constructor the reader calls.
Because both the writer and the reader construct the fields through this one
chokepoint, a value the writer accepts is a value the reader accepts, and vice
versa; they cannot drift.

!!! note "Typed record deliberately tightens `confidence` and `demotion_applied`"
    In the legacy wire `OrientJudgment` these two fields are optional —
    `confidence` defaults to `1.0` and `demotion_applied` defaults to `0.0`
    (`#[serde(default …)]` in [`src/ooda_brain/orient.rs`](../reference/ooda-brain-api.md)).
    The typed record CLI intentionally **requires** both (`--confidence`,
    `--demotion-applied`) so every persisted judgment is fully explicit and the
    fail-CLOSED reader never has to synthesize a value. `OrientFields::from_fields`
    enforces presence for both the writer and the reader, and the round-trip
    tests assert a record missing either field is an `Err` (no default is
    applied). This is a spec-level tightening, not a wire-format change; the
    deterministic fallback path, which still uses the defaulted wire struct, is
    unaffected.

`OrientFields::from_fields` applies `OrientJudgment::validate`'s escalation /
finiteness semantics ([`src/ooda_brain/orient.rs`](../reference/ooda-brain-api.md))
**and adds** the range checks on the fields `validate` does not itself cover
(`validate` only checks `adjusted_urgency` finiteness, `[0,1]`, and no-escalation).
The chokepoint is therefore a superset of `validate`:

- `adjusted_urgency`, `confidence`, `base_urgency` — finite, in `[0.0, 1.0]`
  (`confidence`/`base_urgency` bounds added by the chokepoint).
- `demotion_applied` — finite, ≥ `0.0` (added by the chokepoint).
- **No escalation:** `adjusted_urgency ≤ base_urgency + 1e-9` (tiny FP slack so a
  brain echoing `base_urgency` exactly is not rejected). A misbehaving LLM
  **cannot** inflate a goal's priority — this is the primary security invariant
  of the orient phase.
- `reason` — non-empty after `sanitize_context_var(_, 500)`.

### `OrientDecisionRecord`

```rust
/// One typed, on-disk orient-phase demotion judgment, written by the
/// `simard ooda record-orient` tool and read by RecipeBrain via
/// `read_verified_orient`. Never scraped from agent prose.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrientDecisionRecord {
    /// Schema pin. Must equal ORIENT_SCHEMA ("simard.ooda.orient.v1").
    pub schema: String,
    /// The goal this judgment is for. Re-verified against the live ctx.
    pub goal_id: String,
    /// The cycle this judgment is for. Re-verified against the live ctx.
    pub cycle_number: u32,
    /// Pre-penalty urgency, persisted so the reader can re-run the
    /// no-escalation check (adjusted <= base) self-consistently.
    pub base_urgency: f64,
    /// Validated final urgency, in [0.0, 1.0], <= base_urgency.
    pub adjusted_urgency: f64,
    /// Validated confidence, in [0.0, 1.0].
    pub confidence: f64,
    /// Validated demotion magnitude, >= 0.0.
    pub demotion_applied: f64,
    /// Sanitized, bounded (<=500 chars) rationale.
    pub reason: String,
}

pub const ORIENT_SCHEMA: &str = "simard.ooda.orient.v1";
```

#### On-disk shape

```json
{
  "schema": "simard.ooda.orient.v1",
  "goal_id": "cognition-research",
  "cycle_number": 4287,
  "base_urgency": 0.80,
  "adjusted_urgency": 0.60,
  "confidence": 0.9,
  "demotion_applied": 0.20,
  "reason": "1 failure: standard floor demotion"
}
```

!!! note "Why `base_urgency` is persisted in the record"
    The no-escalation check needs `base_urgency`, which is **not** otherwise part
    of the judgment. Persisting it lets `read_verified_orient` re-run the exact
    same `OrientFields::from_fields` check the writer ran, closing the
    writer/reader drift gap without the reader having to trust an out-of-band
    value. The daemon still passes the live `base_urgency` to the writer via
    `-c base_urgency`; the record simply carries it forward so the read-side
    check is self-consistent.

### `read_verified_orient` — the fail-CLOSED reader

```rust
/// Read and fully verify an orient decision record.
///
/// Returns `Ok(OrientJudgment)` ONLY when the record exists, deserializes,
/// pins the expected schema, its embedded goal_id/cycle_number match the live
/// ctx, and its fields re-validate through `OrientFields::from_fields`.
/// EVERY other outcome is an `Err` — the caller keeps the base urgency (safe
/// no-op), never a floor default (#1711).
pub fn read_verified_orient(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<OrientJudgment>;
```

---

## `simard ooda record-decide`

Records one decide-phase action-kind routing judgment.

### Usage

```text
simard ooda record-decide \
  --choice <advance_goal|run_improvement|consolidate_memory|research_query|run_gym_eval|build_skill|launch_session|poll_developer_activity|extract_ideas|safe_update> \
  --reason "<short concrete reason>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--reason-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout.
On any validation failure it writes **no file** and exits non-zero.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--choice` | yes | One of the **ten** closed `DecideJudgment` variants (below). Matched **case-insensitively** against the `snake_case` tag; anything else is rejected. |
| `--reason` | yes\* | Short concrete rationale (becomes the variant's `rationale`). Must be non-empty after sanitizing. |
| `--record-path` | yes | **Absolute** path supplied via the recipe's `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The goal this judgment is for. Embedded in the record and re-verified by the reader. |
| `--cycle-number` | yes | The cycle this judgment is for (`u32`). Embedded in the record and re-verified by the reader. |
| `--reason-path` | no | Read `reason` from a file instead of argv. Same absolute / no-`..` / 64 KiB-cap rules as `record-orient`. Mutually exclusive with `--reason`. |

\* Exactly one of `--reason` / `--reason-path` must be supplied and resolve to
non-empty text after sanitizing.

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist.

### The closed choice enum

`--choice` is validated by the shared `DecideChoice::from_choice_fields`
chokepoint, which constructs the existing
[`DecideJudgment`](./ooda-decide-prompt.md#action-keywords) enum in
`src/ooda_brain/decide.rs` — the **single source of truth**. There is no
parallel enum to drift against, and the reader calls the same chokepoint. The
CLI keyword is the enum's `#[serde(tag = "choice", rename_all = "snake_case")]`
tag; the ten accepted values are exactly:

| Choice | Enum variant | When to use |
|---|---|---|
| `advance_goal` | `DecideJudgment::AdvanceGoal` | Default for any non-reserved `goal_id` — drive the goal forward. |
| `run_improvement` | `DecideJudgment::RunImprovement` | Reserved `__improvement__` synthetic ID. |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` | Reserved `__memory__` synthetic ID. |
| `research_query` | `DecideJudgment::ResearchQuery` | Route to a research query. |
| `run_gym_eval` | `DecideJudgment::RunGymEval` | Route to a gym evaluation. |
| `build_skill` | `DecideJudgment::BuildSkill` | Route to skill construction. |
| `launch_session` | `DecideJudgment::LaunchSession` | Route to a launched session. |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` | Reserved `__poll_activity__` synthetic ID. |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` | Reserved `__extract_ideas__` synthetic ID. |
| `safe_update` | `DecideJudgment::SafeUpdate` | Reserved `__safe_update__` synthetic ID. |

Adding a variant is a coordinated change — see
[Versioning & Compatibility](#versioning-compatibility).

!!! warning "Reserved synthetic IDs never reach `record-decide`"
    `src/ooda_loop/decide.rs` (lines 66–70) routes every **synthetic** priority
    (`__memory__`, `__improvement__`, `__poll_activity__`, `__extract_ideas__`,
    `__safe_update__`) to the **deterministic** brain and never invokes the
    recipe/LLM brain for them. In practice the `record-decide` tool is therefore
    only ever called for **non-synthetic** goals, whose judgments are
    `advance_goal` (the common case) plus the routable variants
    (`research_query`, `run_gym_eval`, `build_skill`, `launch_session`). The
    reserved-ID variants remain in the closed enum for completeness and are
    produced by the deterministic path — **not** written through this tool. The
    `__memory__` example below illustrates the record shape those variants would
    take, not a code path `record-decide` exercises today.

### `DecideDecisionRecord`

```rust
/// One typed, on-disk decide-phase routing judgment, written by the
/// `simard ooda record-decide` tool and read by RecipeBrain via
/// `read_verified_decide`. Never scraped from agent prose.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecideDecisionRecord {
    /// Schema pin. Must equal DECIDE_SCHEMA ("simard.ooda.decide.v1").
    pub schema: String,
    /// The goal this judgment is for. Re-verified against the live ctx.
    pub goal_id: String,
    /// The cycle this judgment is for. Re-verified against the live ctx.
    pub cycle_number: u32,
    /// The validated, closed-enum judgment (flattened `choice` + `rationale`).
    #[serde(flatten)]
    pub judgment: DecideJudgment,
}

pub const DECIDE_SCHEMA: &str = "simard.ooda.decide.v1";
```

#### On-disk shape

```json
{
  "schema": "simard.ooda.decide.v1",
  "goal_id": "__memory__",
  "cycle_number": 4287,
  "choice": "consolidate_memory",
  "rationale": "memory has not been consolidated in 12 hours"
}
```

The `choice` discriminator and `rationale` field come from `DecideJudgment`'s
existing `#[serde(tag = "choice", rename_all = "snake_case")]` representation,
flattened into the record — so the tool and the enum can never disagree on the
wire shape.

### `read_verified_decide` — the fail-CLOSED reader

```rust
/// Read and fully verify a decide decision record.
///
/// Returns `Ok(DecideJudgment)` ONLY when the record exists, deserializes,
/// pins the expected schema, its embedded goal_id/cycle_number match the live
/// ctx, and its `choice` + `rationale` re-validate through
/// `DecideChoice::from_choice_fields`. EVERY other outcome is an `Err` — the
/// caller skips this priority (safe no-op), never an `advance_goal` default
/// (#1711).
pub fn read_verified_decide(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<DecideJudgment>;
```

---

## The fail-CLOSED read matrix (R1–R8)

Both readers apply the same independent re-validation ladder (defense in depth
against a stale, replayed, or partially written record). Each row that is not R8
returns `Err`:

| # | Condition | Result |
|---|---|---|
| R1 | File absent (tool never ran / binary unresolvable / tool exited non-zero) | **`Err` → no-op** |
| R2 | File present but not valid JSON / truncated | **`Err` → no-op** |
| R3 | `schema` ≠ the record's expected schema (`simard.ooda.orient.v1` / `simard.ooda.decide.v1`) | **`Err` → no-op** |
| R4 | **decide:** `choice` not one of the ten closed variants — **orient:** any field non-finite, out of `[0,1]`, or `adjusted_urgency > base_urgency + 1e-9` | **`Err` → no-op** |
| R5 | `reason` / `rationale` missing or empty after sanitizing | **`Err` → no-op** |
| R6 | `goal_id` ≠ live ctx `goal_id` (stale / other-goal record) | **`Err` → no-op** |
| R7 | `cycle_number` ≠ live ctx `cycle_number` (prior-cycle record) | **`Err` → no-op** |
| R8 | All checks pass | `Ok(OrientJudgment)` / `Ok(DecideJudgment)` |

R6/R7 prevent the subtle **fail-open** risk of reading a previous cycle's
judgment as if it were this cycle's. In addition, each reader receives a
**fresh, unique per-cycle temp directory** (structural guarantee — see
[Configuration](#configuration)), so a stale record cannot even be at the path.

---

## How the reasoners call the tools

`RecipeBrain::run_orient_recipe` and `RecipeBrain::run_decide_recipe` wire each
recipe up so the agent can call its tool and the reader can find the result
(both modeled on `run_per_goal_cycle_recipe`):

1. Allocate a **fresh unique per-cycle temp directory** (owner-only, `0o600`
   record mode, cleaned up after the cycle).
2. Pass context vars to the recipe via `-c` (argv-only, never `sh -c`):
   - `-c record_path=<tempdir>/orient.json` (or `decide.json`)
   - `-c simard_bin=<current_exe absolute path>`
   - `-c goal_id=<goal_id>`, `-c cycle_number=<n>`
   - orient also: `-c base_urgency=<f>`
3. Run the `ooda-orient` / `ooda-decide` recipe. The agent's stdout is
   **ignored**; a stray JSON print has zero effect.
4. Read the judgment with `read_verified_orient(record_path, ctx.goal_id,
   ctx.cycle_number)` / `read_verified_decide(...)`.

If the tool cannot be resolved or exits non-zero, no record is written and the
reader fails CLOSED at R1.

The `ooda-orient` recipe's agent step invokes the tool roughly like:

```bash
"$simard_bin" ooda record-orient \
  --adjusted-urgency 0.60 \
  --confidence 0.9 \
  --demotion-applied 0.20 \
  --base-urgency "$base_urgency" \
  --reason "1 failure: standard floor demotion" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

The `ooda-decide` recipe's agent step invokes:

```bash
"$simard_bin" ooda record-decide \
  --choice consolidate_memory \
  --reason "memory has not been consolidated in 12 hours" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

Both recipes document `Output: NONE scraped from stdout` in their header, like
`ooda-per-goal-cycle.yaml`. See
[OODA Orient Recipe](./ooda-orient-recipe.md) and
[OODA Decide Recipe](./ooda-decide-prompt.md) for the recipe/prompt contracts.

### Fail-closed at the call sites (no caller changes)

The existing rail call sites already no-op on `Err`, so no caller logic changes:

- **decide** — `src/ooda_loop/decide.rs:71`: an `Err` from the brain `continue`s
  to the next priority (this cycle routes nothing for this goal).
- **orient** — `src/ooda_loop/orient.rs:102`: an `Err` keeps the base urgency
  unchanged.

The pre-conversion silent defaults (decide → `advance_goal`, orient →
deterministic floor) are **removed**: absence is an `Err`, and `Err` is the safe
no-op.

---

## Configuration

| Setting | Source | Default | Notes |
|---|---|---|---|
| `record_path` | recipe `-c record_path` | none (required) | Absolute path inside a daemon-owned, per-cycle temp dir. The tool rejects non-absolute or `..`-bearing paths. |
| `simard_bin` | recipe `-c simard_bin` | `current_exe()` | Absolute path to the running `simard` binary. |
| `base_urgency` | recipe `-c base_urgency` (orient only) | none (required for orient) | Pre-penalty urgency, forwarded to `--base-urgency` and persisted in the orient record. |
| orient `schema` pin | `ORIENT_SCHEMA` const | `simard.ooda.orient.v1` | Reader rejects any other value. |
| decide `schema` pin | `DECIDE_SCHEMA` const | `simard.ooda.decide.v1` | Reader rejects any other value. |
| record file mode | `persistence::persist_json` | `0o600` | Owner-only; atomic temp + fsync + rename. |
| free-text bound | `sanitize::sanitize_context_var(_, 500)` | 500 chars | Applies to `reason` / `rationale`. |
| reason-file cap | `read_bounded_field_file` | 64 KiB | Oversized file is a hard error, never truncated. |

There are **no new `SIMARD_*` environment knobs** and **no database** — each seam
is a single file-backed JSON record. Schema evolution is handled by serde plus
the pinned schema string.

### Large payloads (file, not argv)

Per the operator constraint, oversized text goes through a **file**, never argv.
Use `--reason-path` to point at a file the agent wrote with its file tool. The
path is hardened exactly like `--record-path` — it must be **absolute** and free
of `..` — and is read under a **64 KiB byte cap** (fail-closed: an oversized file
is a hard error, never silently truncated) before the text runs through
`sanitize_context_var(_, 500)` and is written to the record.

---

## Fail-CLOSED contract (the whole point)

Fail-CLOSED is the **only** failure mode on these paths. There is no fallback to
`advance_goal`, no orient floor, no default action, and no silent coercion:

- Any tool-side validation failure → **no file written**, non-zero exit.
- Any reader-side failure (R1–R7) → **`Err`** → the rail performs a **safe
  no-op** (decide skips the priority; orient keeps the base urgency).

This mirrors the no-silent-fallback guarantee established in
[#1711](https://github.com/rysweet/Simard/issues/1711) and the reference
implementation in [`record-decision`](./ooda-record-decision-cli.md).

---

## Security

| ID | Threat | Mitigation |
|---|---|---|
| SR-AUTHZ-1 | Reasoner over-reach | Each tool holds **zero privilege**: its only side effect is one `persist_json` write. No routing, no urgency mutation, no Bridge/Python/kuzu, no tokens. |
| SR-AUTHZ-2 | Bypassing the rail | Authority stays with the deterministic rails, which apply the judgment **after** the read. No `--admin` / `--no-verify` / bypass flag exists. |
| SR-VAL-1 | Injected / drifted choice | decide `--choice` validated by `DecideChoice::from_choice_fields` (constructs `DecideJudgment`, case-insensitive); no parallel enum. |
| SR-VAL-2 | Priority escalation via orient | orient fields validated by `OrientFields::from_fields`: finite + `[0,1]` + `adjusted_urgency ≤ base_urgency + 1e-9`. A misbehaving LLM cannot inflate a goal's priority. |
| SR-VAL-3 | Terminal-escape / log injection via free text | `reason` / `rationale` run through `sanitize_context_var(_, 500)` — strips ANSI/CSI + C0/DEL, folds newlines, bounds to 500 chars. Empty-after-sanitize fails CLOSED (R5). |
| SR-VAL-7 | Replay / stale-record fail-open | Reader independently checks `schema` pin, `goal_id == ctx`, `cycle_number == ctx`, and (orient) `adjusted ≤ base` → mismatch fails CLOSED (R3/R4/R6/R7). |
| SR-VAL-8 | Path traversal / symlink write | `--record-path` **and** `--reason-path` must be **absolute** and free of `..` (`harden_path`); the parent is the daemon-supplied per-cycle temp dir. |
| SR-DOS-1 | Transient OOM via huge input file | `--reason-path` read under a **64 KiB cap** (`read_bounded_field_file`), failing closed before the whole file is buffered. |
| SR-DATA-1 | World-readable record | `0o600` owner-only mode. |
| SR-DATA-2 | Torn / partial write | Atomic temp + fsync + rename. |
| SR-DATA-4 | Cross-cycle bleed | Ephemeral, unique per-cycle temp dir, cleaned up after the cycle. |
| SR-DRIFT-1 | Writer/reader validation drift | **Single shared chokepoint per record type** (`OrientFields::from_fields`, `DecideChoice::from_choice_fields`) invoked by BOTH the writer and the reader — a value that writes cannot fail to read, and vice versa. |

**Validate-all-then-write-once:** every validation runs before any file is
written; a single failure means **no** record on disk.

---

## Examples

### orient — standard floor demotion (1 failure)

```bash
simard ooda record-orient \
  --adjusted-urgency 0.60 \
  --confidence 0.9 \
  --demotion-applied 0.20 \
  --base-urgency 0.80 \
  --reason "1 failure: standard floor demotion" \
  --record-path /run/simard/ooda/cycle-4287-cognition/orient.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

Record written:

```json
{
  "schema": "simard.ooda.orient.v1",
  "goal_id": "cognition-research",
  "cycle_number": 4287,
  "base_urgency": 0.80,
  "adjusted_urgency": 0.60,
  "confidence": 0.9,
  "demotion_applied": 0.20,
  "reason": "1 failure: standard floor demotion"
}
```

### decide — route a reserved memory ID

```bash
simard ooda record-decide \
  --choice consolidate_memory \
  --reason "memory has not been consolidated in 12 hours" \
  --record-path /run/simard/ooda/cycle-4287-memory/decide.json \
  --goal-id __memory__ \
  --cycle-number 4287
```

### decide — ordinary goal (drive forward)

```bash
simard ooda record-decide \
  --choice advance_goal \
  --reason "standing research goal with open PR — drive to completion" \
  --record-path /run/simard/ooda/cycle-4287-cognition/decide.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

### Large reason via file (not argv)

```bash
# The agent wrote the long reason with its file tool:
simard ooda record-decide \
  --choice research_query \
  --reason-path /run/simard/ooda/cycle-4287-cognition/reason.txt \
  --record-path /run/simard/ooda/cycle-4287-cognition/decide.json \
  --goal-id cognition-research \
  --cycle-number 4287
```

### Rejections (no file written, non-zero exit)

```bash
# Out-of-enum choice
simard ooda record-decide --choice deploy ...          # error: unknown choice 'deploy'

# Empty reason
simard ooda record-decide --choice advance_goal --reason "" ...  # error: --reason must be non-empty

# Orient escalation (adjusted > base)
simard ooda record-orient --adjusted-urgency 0.90 --base-urgency 0.80 ...  # error: escalation forbidden

# Non-finite float
simard ooda record-orient --adjusted-urgency NaN ...   # error: --adjusted-urgency must be finite

# Non-absolute record path
simard ooda record-decide --record-path ./decide.json ...  # error: --record-path must be absolute

# Path traversal
simard ooda record-orient --record-path /run/simard/../etc/x ...  # error: --record-path must not contain '..'
```

---

## Versioning & Compatibility

### Adding a decide variant

1. Add the variant to `DecideJudgment` in `src/ooda_brain/decide.rs` and its
   `action_kind()` / `rationale()` arms.
2. `DecideChoice::from_choice_fields` picks it up automatically (it constructs
   the enum) — no parallel list to update.
3. Extend the `ooda-decide` recipe/prompt `OPTIONS` guidance so the agent knows
   to pass it to `--choice`.
4. Add an example here and in the recipe reference.
5. Add serde round-trip + `read_verified_decide` fail-closed tests covering the
   new variant.

### Changing orient fields

Changing the `OrientDecisionRecord` fields requires a coordinated change to
`OrientFields::from_fields`, the CLI writer, and the round-trip tests. The
no-escalation invariant (`adjusted ≤ base`) must be preserved.

### Bumping a schema

Bumping either record **schema** (`…v1` → `…v2`) is a hard change: the reader
rejects any value other than the pinned constant, so a new writer and a new
reader must ship together.

### Compatibility with the shared prose scraper

This change removes the orient/decide callers of `extract_and_parse_json` /
`extract_json_payload`. The shared scraper family in
`src/recipe_output/extract.rs` is **retained** because other, not-yet-converted
seams still call it (Groups B/C/D: admission + resource-admission; idea-dedup +
idea-consolidation; outcome-verify + RustyClawd `from_recipe_envelope`, plus the
engineer-lifecycle envelope). `extract.rs` is deleted only once
`grep -rn extract_json_payload src/` returns no remaining callers.

Likewise, the shared engineer-lifecycle machinery — `run_brain_ladder`,
`extract_decision_envelope`, `DecisionEnvelope`, `LifecycleParseOutcome`,
`envelope_rationale`, `finalize_ladder_result`, `record_verdict_parse_metric`,
`brain_verdict_parsed_total` / `VERDICT_PARSE_METRIC` — is **retained**. Only the
orient/decide-exclusive symbols (`OrientEnvelope`, `extract_orient_envelope`,
`orient_judgment_from_envelope`, `parse_orient_outcome`, `deterministic_floor`,
`decide_judgment_from_variant`, `default_advance_goal`, `decide_decision_choice`)
are deleted, after grep-confirming zero callers.

---

## Regression tests

| Test | Asserts |
|---|---|
| `read_verified_orient` / `_decide` absent | R1 → `Err` (no-op) |
| malformed JSON | R2 → `Err` (no-op) |
| wrong schema (`…v2`) | R3 → `Err` (no-op) |
| decide out-of-enum choice | R4 → `Err` (no-op) |
| orient non-finite / out-of-range / escalation | R4 → `Err` (no-op) |
| empty reason / rationale | R5 → `Err` (no-op) |
| goal mismatch | R6 → `Err` (no-op) |
| cycle mismatch | R7 → `Err` (no-op) |
| decide ten-variant round-trip | Each `DecideJudgment` writes and reads back bit-for-bit |
| orient field round-trip | Valid orient fields write and read back bit-for-bit |
| orient missing required field | a record without `confidence` or `demotion_applied` → `Err` (no default synthesized) |
| CLI enum reject | `--choice deploy` → non-zero, **no file** |
| CLI escalation reject | `adjusted > base` → non-zero, **no file** |
| CLI empty-reason reject | `--reason ""` → non-zero, **no file** |
| CLI oversized reason | reason bounded to 500 chars in the record |
| CLI sanitize | ANSI/C0 bytes stripped from `reason` / `rationale` |
| CLI file mode | record is `0o600` |
| CLI path guard | non-absolute / `..` `--record-path` → non-zero, **no file** |
| rework contract | orient/decide seams no longer reference `extract_and_parse_json` / `extract_json_payload` / `parse_orient_outcome` |
| rail integration | a brain `Err` → decide skips priority, orient keeps base urgency; no urgency inflation, no default routing |

Tests live in `src/ooda_brain/orient_decide_record.rs` (inline `#[cfg(test)]`
round-trip + chokepoint), `tests/tests_record_orient_decide.rs` (R1–R8 matrix),
and `tests/tests_rework_contract.rs` (grep contract).

---

## See Also

- [Reference: `simard ooda record-decision` (typed decision tool)](./ooda-record-decision-cli.md) — the WS-4 reference implementation this mirrors
- [Reference: OODA Orient Recipe & Prompt Schema](./ooda-orient-recipe.md) — how the orient recipe calls `record-orient`
- [Reference: OODA Decide Recipe & Prompt Schema](./ooda-decide-prompt.md) — how the decide recipe calls `record-decide`
- [Reference: text-parsing wire formats](./text-parsing-wire-formats.md) — legacy orient/decide prose formats (superseded on the OODA path)
- [Reference: recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md) — retained engineer-lifecycle ladder
- [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
- [Issue #1711 — no silent fallback on the decision path](https://github.com/rysweet/Simard/issues/1711)
- [Issue #4719 — remove recipe-emits-JSON → Rust-scrapes → Rust-acts (epic)](https://github.com/rysweet/Simard/issues/4719)
