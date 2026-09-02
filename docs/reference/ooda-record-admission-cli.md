---
title: "Reference: simard ooda record-admission / record-resource-admission (typed admission tools)"
description: The two zero-privilege CLI tools the OODA engineer-admission and resource-admission reasoners call to record exactly one typed, validated verdict each, and the file-backed AdmissionDecisionRecord / ResourceAdmissionDecisionRecord seams RecipeBrain reads instead of scraping prose JSON. Covers usage, the closed-enum + field-ownership contracts, the fail-direction-preserving read matrix, configuration, security, and examples.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-record-decision-cli.md
  - ./ooda-record-orient-decide-cli.md
  - ./engineer-admission-api.md
  - ./resource-admission-api.md
  - ./ooda-engineer-admission-recipe.md
  - ./ooda-resource-admission-recipe.md
  - ../index.md
---

# Reference: `simard ooda record-admission` / `simard ooda record-resource-admission` (typed admission tools)

CLI: `src/operator_cli/ooda.rs` (`dispatch_record_admission`, `dispatch_record_resource_admission`)
Record types, schema pins, chokepoints, readers: `src/ooda_brain/mod.rs`
(`AdmissionDecisionRecord`, `ResourceAdmissionDecisionRecord`,
`EngineerAdmissionDecision::from_choice_fields`,
`ResourceAdmissionDecision::from_choice_fields`, `read_verified_admission`,
`read_verified_resource_admission`)
Reader call sites: `src/ooda_brain/recipe_brain.rs` (`run_admission_recipe` /
`decide_engineer_admission`, `run_resource_admission_recipe` /
`decide_resource_admission`)

`simard ooda record-admission` and `simard ooda record-resource-admission` are
the **tools the engineer-admission and resource-admission reasoners call** to
record their per-spawn verdicts. They replace the forbidden "recipe prints
JSON/prose → Rust scrapes stdout with `recipe_output::extract_and_parse_json` /
`extract_json_payload` → Rust acts" pattern on the **two admission gates** —
overlap-aware engineer admission and resource-aware admission — as the Group B
slice of epic [#4719](https://github.com/rysweet/Simard/issues/4719).

They follow the reference pattern established by
[`simard ooda record-decision`](./ooda-record-decision-cli.md)
([#4734](https://github.com/rysweet/Simard/issues/4734)) and extended to two
verbs by [`record-orient` / `record-decide`](./ooda-record-orient-decide-cli.md)
(Group A) **exactly**: the recipe **acts via a gated `simard <verb>` tool** that
writes a typed, owner-only (`0o600`), freshness-checked record; the thin Rust
seam reads that record and turns absence/invalidity into an `Err`.

!!! warning "These gates preserve OPPOSITE fail directions"
    Unlike the core decide/orient/per-goal paths (which are uniformly
    fail-CLOSED), the two admission gates fail in **opposite** directions, and
    the conversion **preserves each direction exactly**:

    - **engineer-admission** fails **OPEN** — a bad/absent/mismatched record ⇒
      `Err` ⇒ the existing Rail-2 path admits **loudly** (`tracing::warn` +
      `fallback = true` judgment). Scheduling is an optimization; it must never
      stall a spawn.
    - **resource-admission** fails **CLOSED** — a bad/absent/mismatched record ⇒
      `Err` ⇒ the existing benign `Defer` (skip this cycle, retry next round).

    The conversion changes **only the trigger** for those `Err`s ("scrape
    returned `None`" → "record absent/invalid"). It introduces **no new default
    action** and leaves the act-sites, the deterministic hard rails
    (engineer exact-path `is_subset`; resource disk ceiling), and the
    kill-switches untouched. See
    [Fail directions are preserved](#fail-directions-are-preserved).

## What the tools do (and do not do)

Both tools hold **zero privilege**. The sole side effect of each is writing one
JSON record file. Each records **intent only**:

- They do **not** spawn engineers, allocate worktrees, reclaim disk, mutate
  refs, roll cycles, or close goals.
- They do **not** call the Bridge, Python, or kuzu, and hold no tokens.
- All authority stays with the deterministic seams in
  [`admission.rs`](./engineer-admission-api.md#the-seam-and-the-two-rails) and
  [`resource_admission.rs`](./resource-admission-api.md#the-seam-and-the-hard-rail),
  which apply the verdict — and their hard rails — **after** the record is read
  and re-validated.

Separating *recording a verdict* (these tools) from *acting on it* (the seams)
is what keeps the reasoner unprivileged and the blast radius bounded.

---

## `simard ooda record-admission`

Records one overlap-aware engineer-admission verdict.

### Usage

```text
simard ooda record-admission \
  --choice <admit|defer|serialize_after> \
  --rationale "<short concrete reason naming the overlapping files/goals>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--blocked-by <csv-of-goal-ids>] \
  [--retry-after-secs <u64>] \
  [--after-goal-id <GOAL_ID>] \
  [--overlap-files <csv-of-paths>] \
  [--rationale-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout
(the reader ignores stdout entirely). On any validation failure it writes **no
file** and exits non-zero with a diagnostic on stderr.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--choice` | yes | One of the three closed `EngineerAdmissionDecision` variants. Matched **case-insensitively** against the `snake_case` tag; anything else is rejected. |
| `--rationale` | yes\* | Short concrete rationale naming the overlapping files/goals. Must be non-empty after sanitizing. |
| `--record-path` | yes | **Absolute** path the daemon supplied via the recipe's `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The candidate goal this verdict is for (`ctx.candidate.id`). Embedded in the record and re-verified by the reader. |
| `--cycle-number` | yes | The cycle this verdict is for (`u32`). Embedded in the record and re-verified by the reader. **The reasoner always passes the `REASONER_RECORD_CYCLE = 0` sentinel** (the live cycle only names the temp dir) — see [How the reasoners call the tools](#how-the-reasoners-call-the-tools). |
| `--blocked-by` | variant | CSV of goal id(s) in the way. **Owned by `defer`**; rejected on `admit` / `serialize_after`. |
| `--retry-after-secs` | variant | Optional advisory retry hint (`u64`). **Owned by `defer`**; rejected on `admit` / `serialize_after`. |
| `--after-goal-id` | variant | The goal to rebase after. **Owned by `serialize_after`** (required for it); rejected on `admit` / `defer`. |
| `--overlap-files` | variant | CSV of repo-relative paths to rebase around. **Owned by `serialize_after`**; rejected on `admit` / `defer`. |
| `--rationale-path` | no | Read `rationale` from a file instead of argv (for large text). Path must be **absolute** and free of `..`; the file is read under a 64 KiB cap. Mutually exclusive with `--rationale`. |

\* Exactly one of `--rationale` / `--rationale-path` must be supplied and resolve
to non-empty text after sanitizing.

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist
(`parse_named_args`); the tool never silently ignores an argument.

!!! note "List flags are single-value CSV, not repeatable"
    `parse_named_args` — reused verbatim from the reference implementation —
    treats a duplicated flag as an error, so `--blocked-by` and
    `--overlap-files` accept **one comma-separated value** (e.g.
    `--blocked-by render-goals-status,rename-adapter`) rather than being
    repeated. This is a deliberate reuse decision: adding repeatable-flag
    support is separate, out-of-scope infrastructure work. Empty CSV entries are
    dropped; a wholly empty list flag is equivalent to omitting it.

### The closed choice enum

`--choice` is validated by the shared `EngineerAdmissionDecision::from_choice_fields`
chokepoint — the **single source of truth** the reader also calls, so writer and
reader cannot drift. It constructs the existing
[`EngineerAdmissionDecision`](./engineer-admission-api.md#engineeradmissiondecision)
enum in `src/ooda_brain/mod.rs`. The three accepted values are:

| Choice | Variant | Meaning | Owned extra fields |
|---|---|---|---|
| `admit` | `Admit { rationale }` | No blocking overlap — spawn now. | _(none)_ |
| `defer` | `Defer { blocked_by, rationale, retry_after_secs }` | A live engineer holds files this goal needs — skip this cycle. | `--blocked-by`, `--retry-after-secs` |
| `serialize_after` | `SerializeAfter { after_goal_id, overlap_files, rationale }` | Spawn now, but rebase onto `after_goal_id` before editing `overlap_files`. | `--after-goal-id`, `--overlap-files` |

Adding a variant is a coordinated change — see
[Versioning & Compatibility](#versioning-compatibility).

#### Field-ownership matrix

`from_choice_fields` enforces **per-variant field ownership**: supplying a flag a
variant does not own is a hard rejection (no file written), so a `defer`'s
`blocked_by` can never leak onto an `admit`, and a `serialize_after`'s
`after_goal_id` can never leak onto a `defer`. Round-trip tests assert both the
accept and the reject direction for every variant.

| Variant | Accepts | Rejects if supplied |
|---|---|---|
| `admit` | `--rationale` | `--blocked-by`, `--retry-after-secs`, `--after-goal-id`, `--overlap-files` |
| `defer` | `--rationale`, `--blocked-by`, `--retry-after-secs` | `--after-goal-id`, `--overlap-files` |
| `serialize_after` | `--rationale`, `--after-goal-id`, `--overlap-files` | `--blocked-by`, `--retry-after-secs` |

### `AdmissionDecisionRecord`

```rust
/// One typed, on-disk engineer-admission verdict, written by the
/// `simard ooda record-admission` tool and read by RecipeBrain via
/// `read_verified_admission`. Never scraped from agent prose.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdmissionDecisionRecord {
    /// Schema pin. Must equal ADMISSION_SCHEMA ("simard.ooda.admission.v1").
    pub schema: String,
    /// The candidate goal this verdict is for. Re-verified against the live ctx.
    pub goal_id: String,
    /// The cycle this verdict is for. Re-verified against the live ctx.
    pub cycle_number: u32,
    /// The validated, closed-enum decision (flattened `choice` + fields).
    #[serde(flatten)]
    pub decision: EngineerAdmissionDecision,
}

pub const ADMISSION_SCHEMA: &str = "simard.ooda.admission.v1";
```

`EngineerAdmissionDecision` keeps its existing `#[derive(… PartialEq, Eq)]` and
its fail-open `Default` (`Admit`), so `AdmissionDecisionRecord` can derive `Eq`.

#### On-disk shape

```json
{
  "schema": "simard.ooda.admission.v1",
  "goal_id": "add-int8-embeddings",
  "cycle_number": 0,
  "choice": "defer",
  "blocked_by": ["render-goals-status"],
  "rationale": "live engineer render-goals-status is rewriting src/operator_commands_ooda/goals_status.rs, the only file this goal edits"
}
```

The `choice` discriminator and its fields come from `EngineerAdmissionDecision`'s
existing `#[serde(tag = "choice", rename_all = "snake_case")]` representation,
flattened into the record — so the tool and the enum can never disagree on the
wire shape.

### `read_verified_admission` — the fail-OPEN-at-the-act-site reader

```rust
/// Read and fully verify an engineer-admission record.
///
/// Returns `Ok(EngineerAdmissionDecision)` ONLY when the record exists,
/// deserializes, pins the expected schema, its embedded goal_id/cycle_number
/// match the live ctx, and its fields re-validate through
/// `EngineerAdmissionDecision::from_choice_fields`. EVERY other outcome is an
/// `Err`.
///
/// The caller (`decide_engineer_admission`) surfaces that `Err` on the
/// EXISTING Rail-2 path — a loud `Admit` (fail-OPEN). The reader itself never
/// picks a default; it only reports Ok/Err.
pub fn read_verified_admission(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<EngineerAdmissionDecision>;
```

---

## `simard ooda record-resource-admission`

Records one resource-aware admission verdict.

### Usage

```text
simard ooda record-resource-admission \
  --choice <admit|defer|reclaim_first> \
  --rationale "<short concrete reason citing the resource figures>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--rationale-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout.
On any validation failure it writes **no file** and exits non-zero.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--choice` | yes | One of the three closed `ResourceAdmissionDecision` variants (below). Matched **case-insensitively** against the `snake_case` tag; anything else is rejected. |
| `--rationale` | yes\* | Short concrete rationale citing the resource figures. Must be non-empty after sanitizing. |
| `--record-path` | yes | **Absolute** path supplied via the recipe's `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The candidate goal this verdict is for (`ctx.goal_id`). Embedded in the record and re-verified by the reader. |
| `--cycle-number` | yes | The cycle this verdict is for (`u32`). Embedded in the record and re-verified by the reader. **Always the `REASONER_RECORD_CYCLE = 0` sentinel** — see [How the reasoners call the tools](#how-the-reasoners-call-the-tools). |
| `--rationale-path` | no | Read `rationale` from a file instead of argv. Same absolute / no-`..` / 64 KiB-cap rules as `record-admission`. Mutually exclusive with `--rationale`. |

\* Exactly one of `--rationale` / `--rationale-path` must be supplied and resolve
to non-empty text after sanitizing.

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist.

### The closed choice enum

`--choice` is validated by the shared `ResourceAdmissionDecision::from_choice_fields`
chokepoint, which constructs the existing
[`ResourceAdmissionDecision`](./resource-admission-api.md#resourceadmissiondecision)
enum in `src/ooda_brain/mod.rs`. All three variants carry **only** a `rationale`
(no variant-owned extra fields), so any admission-owned flag — `--blocked-by`,
`--after-goal-id`, `--overlap-files`, `--retry-after-secs` — is rejected. The
reader calls the same chokepoint, so writer and reader cannot drift.

| Choice | Variant | Meaning |
|---|---|---|
| `admit` | `Admit { rationale }` | The host has resource headroom — proceed (subject to the disk-ceiling hard rail). |
| `defer` | `Defer { rationale }` | Resources are tight — skip this cycle, retry next round (benign). |
| `reclaim_first` | `ReclaimFirst { rationale }` | Reclaim disk first (invoke the disk-health capability), then skip and retry (benign). |

### `ResourceAdmissionDecisionRecord`

```rust
/// One typed, on-disk resource-admission verdict, written by the
/// `simard ooda record-resource-admission` tool and read by RecipeBrain via
/// `read_verified_resource_admission`. Never scraped from agent prose.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResourceAdmissionDecisionRecord {
    /// Schema pin. Must equal RESOURCE_ADMISSION_SCHEMA
    /// ("simard.ooda.resource_admission.v1").
    pub schema: String,
    /// The candidate goal this verdict is for. Re-verified against the live ctx.
    pub goal_id: String,
    /// The cycle this verdict is for. Re-verified against the live ctx.
    pub cycle_number: u32,
    /// The validated, closed-enum decision (flattened `choice` + `rationale`).
    #[serde(flatten)]
    pub decision: ResourceAdmissionDecision,
}

pub const RESOURCE_ADMISSION_SCHEMA: &str = "simard.ooda.resource_admission.v1";
```

!!! note "This record derives `PartialEq` only — no `Eq`, no `Default`"
    `ResourceAdmissionDecision` deliberately has **no `Default`** (the
    fail-closed `Defer` is chosen in the seam, never by defaulting the enum) and,
    because its `rationale` reasoning path historically carried no `Eq`, it
    derives `PartialEq` but not `Eq`. `ResourceAdmissionDecisionRecord` therefore
    derives `Clone, Debug, PartialEq, Serialize, Deserialize` **only** — the
    tests compare with `PartialEq`. This is the K7 caveat: mirroring the engineer
    record's `Eq` here would fail to compile.

#### On-disk shape

```json
{
  "schema": "simard.ooda.resource_admission.v1",
  "goal_id": "add-int8-embeddings",
  "cycle_number": 0,
  "choice": "defer",
  "rationale": "disk at 88% used with 41 worktrees on disk; hold the next spawn until the periodic disk-health check reclaims"
}
```

### `read_verified_resource_admission` — the fail-CLOSED reader

```rust
/// Read and fully verify a resource-admission record.
///
/// Returns `Ok(ResourceAdmissionDecision)` ONLY when the record exists,
/// deserializes, pins the expected schema, its embedded goal_id/cycle_number
/// match the live ctx, and its fields re-validate through
/// `ResourceAdmissionDecision::from_choice_fields`. EVERY other outcome is an
/// `Err`.
///
/// The caller (`decide_resource_admission`) surfaces that `Err` on the EXISTING
/// fail-closed path — a benign `Defer`. The reader itself never picks a default;
/// it only reports Ok/Err.
pub fn read_verified_resource_admission(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<ResourceAdmissionDecision>;
```

---

## The read matrix (R1–R8)

Both readers apply the same independent re-validation ladder (defense in depth
against a stale, replayed, or partially written record). Each failure row
(R1–R7) returns `Err`; R8 is the sole success row. How that `Err` is surfaced
differs by gate (see
[Fail directions are preserved](#fail-directions-are-preserved)):

| # | Condition | Result |
|---|---|---|
| R1 | File absent (tool never ran / binary unresolvable / tool exited non-zero) | **`Err`** |
| R2 | File present but not valid JSON / truncated | **`Err`** |
| R3 | `schema` ≠ the record's expected schema (`simard.ooda.admission.v1` / `simard.ooda.resource_admission.v1`) | **`Err`** |
| R4 | `choice` not one of the closed variants, or a non-owned field supplied (both readers reject any field the recorded variant does not own — for resource-admission, whose variants own no extra fields, this means any admission-owned flag) | **`Err`** |
| R5 | `rationale` missing or empty after sanitizing | **`Err`** |
| R6 | `goal_id` ≠ live ctx `goal_id` (stale / other-goal record) | **`Err`** |
| R7 | `cycle_number` ≠ live ctx `cycle_number` (prior-cycle record) | **`Err`** |
| R8 | All checks pass | `Ok(EngineerAdmissionDecision)` / `Ok(ResourceAdmissionDecision)` |

R6/R7 prevent the subtle risk of reading another goal's or a previous cycle's
verdict as if it were this one's. In addition, each reader receives a **fresh,
unique per-cycle temp directory** (structural guarantee — see
[Configuration](#configuration)), so a stale record cannot even be at the path.

!!! note "Both gates bind `cycle_number` to the `REASONER_RECORD_CYCLE = 0` sentinel"
    Like the Group A orient/decide records, the admission records live in a
    **fresh, unique per-call temp dir** created and torn down inside a single
    reasoner call, so cross-cycle replay is structurally impossible. The writer
    and reader therefore bind `cycle_number` to the constant
    `REASONER_RECORD_CYCLE = 0` (`src/ooda_brain/recipe_brain.rs`) rather than the
    live cycle counter — mirroring orient/decide exactly. The **`goal_id` binding
    is live** (`ctx.candidate.id` / `ctx.goal_id`): a record written for one goal
    can never be read for another (R6). The R7 check still runs — it pins the
    record to the same sentinel the writer used, closing the wire against a
    record carrying any other cycle value.

---

## How the reasoners call the tools

`RecipeBrain::run_admission_recipe` and `RecipeBrain::run_resource_admission_recipe`
wire each recipe up so the agent can call its tool and the reader can find the
result (both modeled on `run_per_goal_cycle_recipe`; they replace the former
`invoke_admission_raw` / `invoke_resource_admission_raw` stdout-scraping
writers):

1. Allocate a **fresh unique per-cycle temp directory** (owner-only, `0o600`
   record mode, cleaned up after the cycle).
2. Pass context vars to the recipe via `-c` (argv-only, never `sh -c`):
   - `-c record_path=<tempdir>/admission.json` (or `resource_admission.json`)
   - `-c simard_bin=<current_exe absolute path>` — resolved via
     `std::env::current_exe()`, never a bare `simard` on `PATH`.
   - `-c goal_id=<live ctx goal id>`, `-c cycle_number=0`
   - plus every existing render var already documented in the recipe references
     (candidate scope / live-engineer blocks for engineer-admission; disk / load
     / worktree figures for resource-admission).
3. Run the `ooda-engineer-admission` / `ooda-resource-admission` recipe. The
   agent's stdout is **ignored**; a stray JSON print has zero effect.
4. Read the verdict with
   `read_verified_admission(record_path, ctx.candidate.id, REASONER_RECORD_CYCLE)`
   / `read_verified_resource_admission(record_path, ctx.goal_id, REASONER_RECORD_CYCLE)`.

If the tool cannot be resolved or exits non-zero, no record is written and the
reader reports R1 `Err`.

The `ooda-engineer-admission` recipe's agent step invokes the tool roughly like:

```bash
"$simard_bin" ooda record-admission \
  --choice defer \
  --blocked-by render-goals-status \
  --rationale "live engineer render-goals-status is rewriting goals_status.rs" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

The `ooda-resource-admission` recipe's agent step invokes:

```bash
"$simard_bin" ooda record-resource-admission \
  --choice reclaim_first \
  --rationale "disk at 92% used with 43 worktrees; reclaim before admitting" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

Both recipes document `Output: NONE scraped from stdout` in their header, like
`ooda-per-goal-cycle.yaml`. See
[OODA engineer-admission recipe](./ooda-engineer-admission-recipe.md) and
[OODA resource-admission recipe](./ooda-resource-admission-recipe.md) for the
recipe/prompt contracts.

### Fail directions are preserved

The conversion changes **only what makes the two `decide_*_admission` methods
return `Err`** ("scrape returned `None`" → "record absent/invalid"). The
downstream act-sites, their deterministic hard rails, and their defaults are
**unchanged**:

- **engineer-admission** ([`admission.rs`](./engineer-admission-api.md#the-seam-and-the-two-rails)) —
  a `decide_engineer_admission` `Err` drives the **existing Rail-2** path:
  `engineer_admission_fallback(ctx)` → **loud `Admit`** (`tracing::warn` +
  `fallback = true` judgment). **Fail-OPEN.** Rail 1 (the deterministic
  exact-path `is_subset` collision block) still fires regardless of the record.
- **resource-admission** ([`resource_admission.rs`](./resource-admission-api.md#the-seam-and-the-hard-rail)) —
  a `decide_resource_admission` `Err` drives the **existing** benign **`Defer`**
  (skip this cycle, `success = true`, retry next round) + loud `error!`.
  **Fail-CLOSED.** The disk-ceiling hard rail and the byte-level `MIN_FREE_GB`
  precheck still run regardless of the record.

No new default action is introduced anywhere. This opposite-polarity guarantee
is the single highest-risk property of the change and is asserted by explicit
per-seam tests (a stub that produces an unreadable record ⇒ engineer admits,
resource defers).

---

## Configuration

| Setting | Source | Default | Notes |
|---|---|---|---|
| `record_path` | recipe `-c record_path` | none (required) | Absolute path inside a daemon-owned, per-cycle temp dir. The tool rejects non-absolute or `..`-bearing paths. |
| `simard_bin` | recipe `-c simard_bin` | `current_exe()` | Absolute path to the running `simard` binary, so the recipe sandbox resolves the tool deterministically (never a bare `simard` on `PATH`). |
| `goal_id` | recipe `-c goal_id` | none (required) | `ctx.candidate.id` (engineer) / `ctx.goal_id` (resource); re-verified on read (R6). |
| `cycle_number` | recipe `-c cycle_number` | `REASONER_RECORD_CYCLE` = `0` | Fixed sentinel; re-verified on read (R7). |
| engineer `schema` pin | `ADMISSION_SCHEMA` const | `simard.ooda.admission.v1` | Reader rejects any other value (R3). |
| resource `schema` pin | `RESOURCE_ADMISSION_SCHEMA` const | `simard.ooda.resource_admission.v1` | Reader rejects any other value (R3). |
| record file mode | `persistence::persist_json` | `0o600` | Owner-only; atomic temp + fsync + rename. |
| free-text bound | `sanitize::sanitize_context_var(_, 500)` | 500 chars | Applies to `rationale`. |
| rationale-file cap | bounded field-file read | 64 KiB | Oversized file is a hard error, never truncated. |

There are **no new `SIMARD_*` environment knobs** and **no database** — each seam
is a single file-backed JSON record. The two existing kill-switches
([`SIMARD_ENGINEER_ADMISSION`](./engineer-admission-api.md#kill-switch),
[`SIMARD_RESOURCE_ADMISSION`](./resource-admission-api.md#kill-switch)) and the
[`SIMARD_DISK_ADMISSION_CEILING_PCT`](./resource-admission-api.md#configuration)
ceiling are **unchanged**. Schema evolution is handled by serde plus the pinned
schema string.

### Large payloads (file, not argv)

Per the operator constraint, oversized text goes through a **file**, never argv.
Use `--rationale-path` to point at a file the agent wrote with its file tool. The
path is hardened exactly like `--record-path` — it must be **absolute** and free
of `..` — and is read under a **64 KiB byte cap** (fail-closed: an oversized file
is a hard error, never silently truncated) before the text runs through
`sanitize_context_var(_, 500)` and is written to the record.

---

## Security

| ID | Threat | Mitigation |
|---|---|---|
| SR-AUTHZ-1 | Reasoner over-reach | Each tool holds **zero privilege**: its only side effect is one `persist_json` write. No spawn, no worktree alloc, no reclaim, no ref mutation, no Bridge/Python/kuzu, no tokens. |
| SR-AUTHZ-2 | Bypassing the rails | Authority stays with the deterministic seams, which apply the verdict — and their hard rails — **after** the read. No `--admin` / `--no-verify` / bypass flag exists. |
| SR-AUTHZ-3 | Binary substitution | `simard_bin` is resolved via `current_exe()`, never a `PATH` lookup, so a hostile `simard` on `PATH` cannot be invoked. |
| SR-VAL-1 | Injected / drifted choice | `--choice` validated by `from_choice_fields` (closed enums, case-insensitive); both writer and reader use the same chokepoint. |
| SR-VAL-2 | Smuggled variant fields | The engineer chokepoint enforces the [field-ownership matrix](#field-ownership-matrix): a non-owned flag is a hard rejection, so a `defer`'s `blocked_by` cannot leak onto an `admit`. Resource decisions accept no admission-owned field at all. |
| SR-VAL-3 | Terminal-escape / log injection via free text | `rationale` runs through `sanitize_context_var(_, 500)` — strips ANSI/CSI + C0/DEL, folds newlines, bounds to 500 chars ([#2751](https://github.com/rysweet/Simard/issues/2751)). Empty-after-sanitize fails at R5. |
| SR-VAL-7 | Replay / stale-record | Reader independently checks the `schema` pin, `goal_id == ctx`, and `cycle_number == ctx` → any mismatch is an `Err` (R3/R6/R7); a fresh per-cycle temp dir makes a stale record structurally unreachable. |
| SR-VAL-8 | Path traversal / symlink write | `--record-path` **and** `--rationale-path` must be **absolute** and free of `..` (`harden_path`); the parent is the daemon-supplied per-cycle temp dir. |
| SR-DOS-1 | Transient OOM via huge input file | `--rationale-path` read under a **64 KiB cap**, failing closed before the whole file is buffered. |
| SR-DATA-1 | World-readable record | `0o600` owner-only mode. |
| SR-DATA-2 | Torn / partial write | Atomic temp + fsync + rename. |
| SR-DATA-4 | Cross-cycle bleed | Ephemeral, unique per-cycle temp dir, cleaned up after the cycle. |
| SR-DRIFT-1 | Writer/reader validation drift | **Single shared chokepoint per record type** (`EngineerAdmissionDecision::from_choice_fields`, `ResourceAdmissionDecision::from_choice_fields`) invoked by BOTH the writer and the reader — a value that writes cannot fail to read, and vice versa. The reader re-sanitizes free text on read, so a hostile record with a control-byte rationale or a non-owned field is rejected/cleaned, never trusted verbatim. |
| SR-POLARITY-1 | Fail-direction flip | The conversion changes only the `Err` trigger; the act-sites and defaults are untouched (engineer `Err`→loud `Admit`; resource `Err`→benign `Defer`). See [Fail directions are preserved](#fail-directions-are-preserved). |

**Validate-all-then-write-once:** every validation runs before any file is
written; a single failure means **no** record on disk.

> **Net effect on attack surface.** Group B **removes** an attack surface — the
> stdout scraping of model-controlled prose — rather than adding one. The tool
> and reader replace fuzzy `extract_and_parse_json` recovery with a closed-enum,
> owner-only, freshness-checked record on both admission gates.

---

## Examples

!!! note "`cycle-4287-*` in paths vs. `cycle_number: 0` in records"
    The temp-dir path is named by the **live** cycle (e.g. `cycle-4287-…`) for
    operator legibility, but the record's identity field is bound to the fixed
    `REASONER_RECORD_CYCLE = 0` sentinel (see
    [How the reasoners call the tools](#how-the-reasoners-call-the-tools)). That
    is why every example below passes `--cycle-number 0` yet writes into a
    `cycle-4287` directory.

### engineer-admission — `admit` (independent work, parallelize)

```bash
simard ooda record-admission \
  --choice admit \
  --rationale "candidate scope is docs/ only; no live engineer touches docs — independent" \
  --record-path /run/simard/ooda/cycle-4287-int8/admission.json \
  --goal-id add-int8-embeddings \
  --cycle-number 0
```

### engineer-admission — `defer` (hot shared file collision)

```bash
simard ooda record-admission \
  --choice defer \
  --blocked-by render-goals-status \
  --rationale "live engineer render-goals-status is rewriting src/operator_commands_ooda/goals_status.rs, the only file this goal edits" \
  --record-path /run/simard/ooda/cycle-4287-int8/admission.json \
  --goal-id add-int8-embeddings \
  --cycle-number 0
```

### engineer-admission — `serialize_after` (rebase after a rename)

```bash
simard ooda record-admission \
  --choice serialize_after \
  --after-goal-id rename-adapter-symbol \
  --overlap-files src/agent_supervisor/adapter.rs \
  --rationale "candidate edits call sites that rename-adapter-symbol is moving; rebase after it to avoid a broken-main union" \
  --record-path /run/simard/ooda/cycle-4287-int8/admission.json \
  --goal-id add-int8-embeddings \
  --cycle-number 0
```

### resource-admission — `reclaim_first` (disk tight)

```bash
simard ooda record-resource-admission \
  --choice reclaim_first \
  --rationale "disk at 92% used with 43 worktrees; reclaim build cache before admitting another engineer" \
  --record-path /run/simard/ooda/cycle-4287-int8/resource_admission.json \
  --goal-id add-int8-embeddings \
  --cycle-number 0
```

Record written:

```json
{
  "schema": "simard.ooda.resource_admission.v1",
  "goal_id": "add-int8-embeddings",
  "cycle_number": 0,
  "choice": "reclaim_first",
  "rationale": "disk at 92% used with 43 worktrees; reclaim build cache before admitting another engineer"
}
```

### Large rationale via file (not argv)

```bash
# The agent wrote the long rationale with its file tool:
simard ooda record-admission \
  --choice defer \
  --blocked-by render-goals-status,rename-adapter-symbol \
  --rationale-path /run/simard/ooda/cycle-4287-int8/rationale.txt \
  --record-path /run/simard/ooda/cycle-4287-int8/admission.json \
  --goal-id add-int8-embeddings \
  --cycle-number 0
```

### Rejections (no file written, non-zero exit)

```bash
# Out-of-enum choice
simard ooda record-admission --choice reclaim_first ...   # error: unknown choice 'reclaim_first' for record-admission

# Smuggled non-owned field
simard ooda record-admission --choice admit --blocked-by x ...  # error: --blocked-by not valid for choice 'admit'

# Empty rationale
simard ooda record-resource-admission --choice defer --rationale "" ...  # error: --rationale must be non-empty

# Non-absolute record path
simard ooda record-admission --record-path ./admission.json ...  # error: --record-path must be absolute

# Path traversal
simard ooda record-resource-admission --record-path /run/simard/../etc/x ...  # error: --record-path must not contain '..'
```

---

## Versioning & Compatibility

### Adding an engineer-admission variant

1. Add the variant to `EngineerAdmissionDecision` in `src/ooda_brain/mod.rs`,
   its `from_choice_fields` arm (including the field-ownership rule), and the
   `dispatch_spawn_engineer` apply match in `admission.rs`.
2. `from_choice_fields` accepts the new keyword once its arm is added; the CLI
   writer and `read_verified_admission` pick it up through the shared chokepoint.
3. Extend the `ooda-engineer-admission` recipe `OPTIONS` guidance so the agent
   knows to pass it to `--choice`.
4. Add an example here and in the recipe reference.
5. Add serde round-trip + field-ownership + `read_verified_admission`
   fail-closed tests covering the new variant.

### Adding a resource-admission variant

Same as above against `ResourceAdmissionDecision`, its `from_choice_fields`, the
`ResourceAdmissionOutcome` apply arm in `resource_admission.rs`, and the
`ooda-resource-admission` recipe. Preserve the "no `Default`" property.

### Bumping a schema

Bumping either record **schema** (`…v1` → `…v2`) is a hard change: the reader
rejects any value other than the pinned constant, so a new writer and a new
reader must ship together.

### Compatibility with the shared prose scraper

This change removes the engineer-admission and resource-admission callers of
`extract_and_parse_json` / `extract_json_payload`, and deletes the six
Group-B-only scrape helpers — `AdmissionEnvelope`, `ResourceAdmissionEnvelope`,
`parse_admission_decision`, `admission_decision_from_variant`,
`parse_resource_admission_decision`, `resource_admission_decision_from_variant`.
The shared scraper family in `src/recipe_output/extract.rs`
(`extract_json_payload`, `extract_and_parse_json`, `LifecycleParseOutcome`, …) is
**retained** because other, not-yet-converted seams still call it (Groups C/D:
idea-dedup + idea-consolidation; outcome-verify; the engineer-lifecycle
envelope). `extract.rs` is deleted only once `grep -rn extract_json_payload src/`
returns no remaining callers; the Group B contract test asserts only the **two
admission seams** no longer reference it.

---

## Regression tests

| Test | Asserts |
|---|---|
| `read_verified_admission` / `_resource_admission` absent | R1 → `Err` |
| malformed JSON | R2 → `Err` |
| wrong schema (`…v2`) | R3 → `Err` |
| out-of-enum choice | R4 → `Err` |
| engineer non-owned field on a variant | R4 → `Err` |
| empty rationale | R5 → `Err` |
| goal mismatch | R6 → `Err` |
| cycle mismatch | R7 → `Err` |
| engineer three-variant round-trip | Each `EngineerAdmissionDecision` writes and reads back bit-for-bit incl. owned fields |
| engineer field-ownership reject | Each variant rejects every field it does not own |
| resource three-variant round-trip | Each `ResourceAdmissionDecision` writes and reads back bit-for-bit |
| **engineer fail-OPEN** | An unreadable record ⇒ `decide_engineer_admission` `Err` ⇒ Rail-2 loud `Admit` (spawn proceeds); Rail 1 still blocks a certain collision |
| **resource fail-CLOSED** | An unreadable record ⇒ `decide_resource_admission` `Err` ⇒ benign `Defer` (`success = true`); disk-ceiling rail still fires |
| CLI enum reject | `--choice deploy` → non-zero, **no file** |
| CLI empty-rationale reject | `--rationale ""` → non-zero, **no file** |
| CLI oversized rationale | rationale bounded to 500 chars in the record |
| CLI sanitize | ANSI/C0 bytes stripped from `rationale` |
| CLI file mode | record is `0o600` |
| CLI path guard | non-absolute / `..` `--record-path` → non-zero, **no file** |
| rework contract | both admission seams no longer reference `extract_and_parse_json` / `extract_json_payload` / the six deleted helpers; a grep guard keeps `extract_json_payload src/` non-empty (C/D machinery retained) |

Tests live in `src/ooda_brain/tests_record_admission.rs` (round-trip +
field-ownership + R1–R8 reader matrix for both records),
`src/ooda_brain/tests_rework_contract.rs` (grep contract scoped to the two
seams), and `tests/typed_ooda_recipe_assets.rs` (the two new schemas + verbs in
the recipe assets).

---

## See Also

- [Reference: `simard ooda record-decision` (typed decision tool)](./ooda-record-decision-cli.md) — the reference implementation this mirrors
- [Reference: `simard ooda record-orient` / `record-decide`](./ooda-record-orient-decide-cli.md) — the Group A two-verb sibling
- [Engineer-admission API reference](./engineer-admission-api.md) — the typed engineer-admission surface and its two rails
- [Resource-admission API reference](./resource-admission-api.md) — the typed resource-admission surface and its hard rail
- [OODA engineer-admission recipe & prompt schema](./ooda-engineer-admission-recipe.md) — how the engineer-admission recipe calls `record-admission`
- [OODA resource-admission recipe & prompt schema](./ooda-resource-admission-recipe.md) — how the resource-admission recipe calls `record-resource-admission`
- [Issue #1711 — no silent fallback on the decision path](https://github.com/rysweet/Simard/issues/1711)
- [Issue #4719 — remove recipe-emits-JSON → Rust-scrapes → Rust-acts (epic)](https://github.com/rysweet/Simard/issues/4719)
