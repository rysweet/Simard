---
title: "Reference: simard ooda record-idea-dedup / record-idea-consolidation (typed creative-ideas tools)"
description: The two zero-privilege CLI tools the Creative-Ideas semantic dedup and consolidation reasoners call to record exactly one typed, validated result each, and the file-backed IdeaDedupDecisionRecord / IdeaConsolidationRecord seams RecipeBrain reads instead of scraping prose JSON. Covers usage, the closed-enum + field-ownership contract, the cluster-list sanitizing chokepoint, the fail-closed read matrix (incl. the empty-but-present vs absent distinction), configuration, security, and examples.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-record-admission-cli.md
  - ./ooda-record-decision-cli.md
  - ./ooda-record-orient-decide-cli.md
  - ./creative-idea-dedup-recipe.md
  - ./creative-ideas-dedup-gate-api.md
  - ../concepts/semantic-creative-ideas-dedup.md
  - ../index.md
---

# Reference: `simard ooda record-idea-dedup` / `simard ooda record-idea-consolidation` (typed creative-ideas tools)

CLI: `src/operator_cli/ooda.rs` (`dispatch_record_idea_dedup`,
`dispatch_record_idea_consolidation`)
Record types, schema pins, chokepoints, readers: `src/ooda_brain/mod.rs`
(`IdeaDedupDecisionRecord`, `IdeaConsolidationRecord`,
`IdeaDedupDecision::from_choice_fields`, `IdeaCluster::sanitized`,
`read_verified_idea_dedup`, `read_verified_idea_consolidation`)
Reader call sites: `src/ooda_brain/recipe_brain.rs` (`decide_idea_dedup`,
`decide_idea_consolidation`)

`simard ooda record-idea-dedup` and `simard ooda record-idea-consolidation` are
the **tools the Creative-Ideas semantic dedup and consolidation reasoners call**
to record their verdicts. They replace the forbidden "recipe prints JSON/prose →
Rust scrapes stdout with `recipe_output::extract_and_parse_json` /
`extract_json_payload` → Rust acts" pattern on the **two creative-ideas seams** —
per-candidate semantic dedup and the one-time pool consolidation — as the Group C
slice of epic [#4719](https://github.com/rysweet/Simard/issues/4719) (feature
[#2925](https://github.com/rysweet/Simard/issues/2925)).

They follow the reference pattern established by
[`simard ooda record-decision`](./ooda-record-decision-cli.md)
([#4734](https://github.com/rysweet/Simard/issues/4734)) and the Group A/B
siblings ([`record-orient` / `record-decide`](./ooda-record-orient-decide-cli.md),
[`record-admission` / `record-resource-admission`](./ooda-record-admission-cli.md))
**exactly**: the recipe **acts via a gated `simard <verb>` tool** that writes a
typed, owner-only (`0o600`), freshness-checked record; the thin Rust seam reads
that record and turns absence/invalidity into an `Err`.

!!! warning "Both seams fail CLOSED — the conversion preserves that exactly"
    Unlike the two admission gates (which fail in opposite directions), **both
    creative-ideas seams fail CLOSED**, and the conversion preserves each
    direction precisely:

    - **idea-dedup** fails **CLOSED** — a bad/absent/mismatched record ⇒ `Err` ⇒
      `decide_idea_dedup` surfaces the error and the dedup gate maps it to
      `PlannedAction::FailClosed`: the candidate idea is **dropped this cycle**
      (never a silent duplicate, never a wrong-node enhance) and retried next
      run. There is **no** auto-create-on-the-brain's-behalf.
    - **idea-consolidation** fails **CLOSED** — a bad/absent/mismatched record ⇒
      `Err` ⇒ `decide_idea_consolidation` returns `Err`, the applier writes
      **nothing**, and the error is surfaced (retry later). Crucially, a
      **present record with an empty cluster list** is **not** an error: it is a
      valid `Ok(vec![])` "nothing to consolidate" result, preserved distinct
      from the unparseable `None` case.

    The conversion changes **only the trigger** for those `Err`s ("scrape
    returned `None`" → "record absent/invalid"). It introduces **no new default
    or fallback action** and leaves the act-sites, the coarse-Jaccard backstop
    (used only on the kill-switch-off path), and the `IdeaStatus` state machine
    untouched. See [Fail directions are preserved](#fail-directions-are-preserved).

## What the tools do (and do not do)

Both tools hold **zero privilege**. The sole side effect of each is writing one
JSON record file. Each records **intent only**:

- They do **not** persist ideas, mutate memory nodes, transition idea status,
  strengthen a canonical, or reject a redundant idea.
- They do **not** call the Bridge, Python, or kuzu, and hold no tokens.
- All authority stays with the deterministic seams in
  [`dedup_gate.rs`](./creative-ideas-dedup-gate-api.md) — the dedup gate applies
  the verdict (with its shortlist-membership check on `enhance_existing`), and
  `consolidate_existing` applies the clusters through the fail-closed
  `IdeaStatus` state machine — **after** the record is read and re-validated.

Separating *recording a verdict* (these tools) from *acting on it* (the seams)
is what keeps the reasoner unprivileged and the blast radius bounded.

---

## `simard ooda record-idea-dedup`

Records one per-candidate semantic-dedup verdict.

### Usage

```text
simard ooda record-idea-dedup \
  --choice <create_new|skip|enhance_existing> \
  --reason "<short concrete reason>" \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N> \
  [--target-node-id <NODE_ID>] \
  [--reason-path <FILE>]
```

On success the tool writes the record atomically and prints nothing to stdout
(the reader ignores stdout entirely). On any validation failure it writes **no
file** and exits non-zero with a diagnostic on stderr.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--choice` | yes | One of the three closed `IdeaDedupDecision` variants. Matched **case-insensitively** against the `snake_case` tag; anything else is rejected. |
| `--reason` | yes\* | Short concrete rationale for the decision. Must be non-empty after sanitizing. Stored as the variant's `rationale`. |
| `--record-path` | yes | **Absolute** path the daemon supplied via the recipe's `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The fixed synthetic seam id `creative-idea-dedup` supplied via `-c goal_id`. Embedded in the record and re-verified by the reader (R6). |
| `--cycle-number` | yes | `REASONER_RECORD_CYCLE = 0` sentinel supplied via `-c cycle_number`. Embedded in the record and re-verified by the reader (R7). See [How the reasoners call the tools](#how-the-reasoners-call-the-tools). |
| `--target-node-id` | variant | The `node_id` of the shortlisted idea to strengthen. **Owned by `enhance_existing`** (required, non-empty); **rejected** on `create_new` / `skip`. |
| `--reason-path` | no | Read `reason` from a file instead of argv (for large text). Path must be **absolute** and free of `..`; the file is read under a 64 KiB cap. Mutually exclusive with `--reason`. |

\* Exactly one of `--reason` / `--reason-path` must be supplied and resolve to
non-empty text after sanitizing.

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist
(`parse_named_args`); the tool never silently ignores an argument.

### The closed choice enum

`--choice` is validated by the shared `IdeaDedupDecision::from_choice_fields`
chokepoint — the **single source of truth** the reader also calls, so writer and
reader cannot drift. It constructs the existing
[`IdeaDedupDecision`](./creative-ideas-dedup-gate-api.md#ideadedupdecision) enum
in `src/ooda_brain/mod.rs`. The three accepted values are:

| Choice | Variant | Meaning | Owned extra field |
|---|---|---|---|
| `create_new` | `CreateNew { rationale }` | Genuinely novel — persist as a new idea. The honest default when unsure. | _(none)_ |
| `skip` | `Skip { rationale }` | True duplicate that adds nothing — drop the candidate. | _(none)_ |
| `enhance_existing` | `EnhanceExisting { target_node_id, rationale }` | Same underlying idea as one shortlisted entry — strengthen it. | `--target-node-id` |

Adding a variant is a coordinated change — see
[Versioning & Compatibility](#versioning-compatibility).

#### Field-ownership matrix

`from_choice_fields` enforces **per-variant field ownership**: supplying
`--target-node-id` on a variant that does not own it is a hard rejection (no file
written), and omitting it on `enhance_existing` is likewise a hard rejection — an
enhance without a target is unactionable and guessing a target would be a
wrong-node write. Round-trip tests assert both the accept and the reject
direction for every variant.

| Variant | Accepts | Rejects if supplied | Requires |
|---|---|---|---|
| `create_new` | `--reason` | `--target-node-id` | — |
| `skip` | `--reason` | `--target-node-id` | — |
| `enhance_existing` | `--reason`, `--target-node-id` | — | `--target-node-id` (non-empty) |

!!! note "The chokepoint proves presence; the gate proves membership"
    `from_choice_fields` guarantees a non-empty `target_node_id` on
    `enhance_existing`. It does **not** — and cannot — check that the target is a
    member of the candidate's shortlist (the chokepoint has no shortlist). That
    second layer of defense stays in the dedup gate
    ([`dedup_gate.rs`](./creative-ideas-dedup-gate-api.md)): an `enhance_existing`
    naming a `target_node_id` **not** in the shortlist still maps to
    `PlannedAction::FailClosed`. Both layers are retained unchanged.

### `IdeaDedupDecisionRecord`

```rust
/// One typed, on-disk semantic-dedup verdict, written by the
/// `simard ooda record-idea-dedup` tool and read by RecipeBrain via
/// `read_verified_idea_dedup`. Never scraped from agent prose.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdeaDedupDecisionRecord {
    /// Schema pin. Must equal IDEA_DEDUP_SCHEMA
    /// ("simard.creative.idea_dedup.v1").
    pub schema: String,
    /// The synthetic seam id "creative-idea-dedup". Re-verified on read.
    pub goal_id: String,
    /// The REASONER_RECORD_CYCLE = 0 sentinel. Re-verified on read.
    pub cycle_number: u32,
    /// The validated, closed-enum decision (flattened `choice` + fields).
    #[serde(flatten)]
    pub decision: IdeaDedupDecision,
}

pub const IDEA_DEDUP_SCHEMA: &str = "simard.creative.idea_dedup.v1";
```

`IdeaDedupDecision` keeps its existing `#[serde(tag = "choice", rename_all =
"snake_case")]` representation and its `#[derive(… PartialEq, Eq)]`, and — like
the other reasoning enums — has **no `Default`**: the fail-closed path is chosen
explicitly by the seam, never by defaulting a decision on the brain's behalf.

#### On-disk shape

```json
{
  "schema": "simard.creative.idea_dedup.v1",
  "goal_id": "creative-idea-dedup",
  "cycle_number": 0,
  "choice": "enhance_existing",
  "target_node_id": "node-7a3f",
  "rationale": "same goal-board caching idea as node-7a3f, but adds a measured 12% fewer reads — append as evidence"
}
```

The `choice` discriminator and its fields come from `IdeaDedupDecision`'s
existing `#[serde(tag = "choice", rename_all = "snake_case")]` representation,
flattened into the record — so the tool and the enum can never disagree on the
wire shape.

### `read_verified_idea_dedup` — the fail-CLOSED reader

```rust
/// Read and fully verify a semantic-dedup record.
///
/// Returns `Ok(IdeaDedupDecision)` ONLY when the record exists, deserializes,
/// pins the expected schema, its embedded goal_id/cycle_number match the
/// seam sentinels, and its fields re-validate through
/// `IdeaDedupDecision::from_choice_fields` with a non-empty rationale. EVERY
/// other outcome is an `Err`.
///
/// The caller (`decide_idea_dedup`) surfaces that `Err`; the dedup gate maps it
/// to `PlannedAction::FailClosed` (drop the candidate this cycle). The reader
/// itself never picks a default; it only reports Ok/Err.
pub fn read_verified_idea_dedup(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<IdeaDedupDecision>;
```

---

## `simard ooda record-idea-consolidation`

Records one pool-consolidation result: a validated **list** of semantic-duplicate
clusters (not a single enum).

### Usage

```text
simard ooda record-idea-consolidation \
  --clusters-path <ABSOLUTE_PATH> \
  --record-path <ABSOLUTE_PATH> \
  --goal-id <GOAL_ID> \
  --cycle-number <N>
```

On success the tool writes the record atomically and prints nothing to stdout.
On any validation failure it writes **no file** and exits non-zero.

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--clusters-path` | yes | **Absolute** path to a JSON-array file of clusters the agent wrote with its file tool. Must not contain `..`; read under a byte cap. See [Clusters go through a file, not argv](#clusters-go-through-a-file-not-argv). An **empty array `[]` is valid** ("nothing to consolidate"). |
| `--record-path` | yes | **Absolute** path the daemon supplied via `-c record_path`. Must not contain `..`. |
| `--goal-id` | yes | The fixed synthetic seam id `creative-idea-consolidation` supplied via `-c goal_id`. Embedded in the record and re-verified by the reader (R6). |
| `--cycle-number` | yes | `REASONER_RECORD_CYCLE = 0` sentinel supplied via `-c cycle_number`. Embedded and re-verified (R7). |

Unknown or duplicate flags are rejected against a `KNOWN_FLAGS` allowlist.

!!! note "Clusters are input; the record is output"
    `--clusters-path` is the reasoner's **input** (the clusters the agent chose),
    read and validated by the tool. `--record-path` is where the tool writes the
    **validated output** record that `read_verified_idea_consolidation` reads back.
    They are two distinct files and must not be confused.

### The cluster-list sanitizing chokepoint

Each cluster is validated by the shared `IdeaCluster::sanitized` chokepoint — the
**single source of truth** the reader also calls, so writer and reader cannot
drift. It sanitizes and bounds one
[`IdeaCluster`](./creative-ideas-dedup-gate-api.md#module-layout) from
`src/ooda_brain/mod.rs`:

| Field | Rule |
|---|---|
| `canonical_id` | Trimmed; a cluster whose `canonical_id` is empty after sanitizing is **dropped** (returns `None`) — an anonymous cluster has nothing to keep. |
| `redundant_ids` | Each id sanitized; empties dropped. |
| `merged_rationale` | Sanitized and bounded to `MAX_RATIONALE_CHARS = 500`. |
| `evidence` | Each entry sanitized + bounded; the list capped. |

The **cluster list itself is capped at 64** (mirroring the 64-entry prompt-cost
DoS guard in `render_existing_shortlist`). Raw serde is used for wire transport
only; record semantics NEVER trust raw serde — every cluster on both the write
and read side passes through `IdeaCluster::sanitized`.

### `IdeaConsolidationRecord`

```rust
/// One typed, on-disk consolidation result, written by the
/// `simard ooda record-idea-consolidation` tool and read by RecipeBrain via
/// `read_verified_idea_consolidation`. Never scraped from agent prose.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdeaConsolidationRecord {
    /// Schema pin. Must equal IDEA_CONSOLIDATION_SCHEMA
    /// ("simard.creative.idea_consolidation.v1").
    pub schema: String,
    /// The synthetic seam id "creative-idea-consolidation". Re-verified on read.
    pub goal_id: String,
    /// The REASONER_RECORD_CYCLE = 0 sentinel. Re-verified on read.
    pub cycle_number: u32,
    /// The validated, sanitized cluster list. An empty vec is VALID
    /// ("nothing to consolidate") and distinct from an absent record.
    pub clusters: Vec<IdeaCluster>,
}

pub const IDEA_CONSOLIDATION_SCHEMA: &str = "simard.creative.idea_consolidation.v1";
```

#### On-disk shape

```json
{
  "schema": "simard.creative.idea_consolidation.v1",
  "goal_id": "creative-idea-consolidation",
  "cycle_number": 0,
  "clusters": [
    {
      "canonical_id": "node-7a3f",
      "redundant_ids": ["node-91cc", "node-4d20"],
      "merged_rationale": "cache the goal-board reads: three entries all propose caching goal_board.json across OODA cycles",
      "evidence": ["node-4d20 measured 12% fewer reads"]
    }
  ]
}
```

An empty list is written verbatim and is a valid record:

```json
{
  "schema": "simard.creative.idea_consolidation.v1",
  "goal_id": "creative-idea-consolidation",
  "cycle_number": 0,
  "clusters": []
}
```

### `read_verified_idea_consolidation` — the fail-CLOSED reader (empty-list-safe)

```rust
/// Read and fully verify a consolidation record.
///
/// Returns `Ok(Vec<IdeaCluster>)` ONLY when the record exists, deserializes,
/// pins the expected schema, and its embedded goal_id/cycle_number match the
/// seam sentinels. Every cluster is re-run through `IdeaCluster::sanitized`
/// and the list re-capped at 64 on read. A present-but-EMPTY list returns
/// `Ok(vec![])` ("nothing to consolidate"). EVERY other outcome (absent,
/// unreadable, malformed, wrong-schema, goal/cycle mismatch) is an `Err`.
///
/// The caller (`decide_idea_consolidation`) surfaces that `Err`; the applier
/// then writes nothing and retries later. The reader itself never picks a
/// default; it only reports Ok/Err.
pub fn read_verified_idea_consolidation(
    path: &Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<Vec<IdeaCluster>>;
```

!!! important "Preserve `Ok(vec![])` vs `Err` — do not collapse them"
    The pre-conversion seam distinguished `Some(vec![])` ("brain said nothing to
    consolidate") from `None` ("could not parse a result"). The typed reader
    preserves that exactly: **present record with `clusters: []` ⇒ `Ok(vec![])`**;
    **absent / malformed / mismatched ⇒ `Err`**. Collapsing an empty-but-present
    list into an `Err` would be a fail-open→fail-closed regression in the other
    direction (needlessly re-running the pass) and is asserted against by a
    dedicated test.

---

## The read matrix (R1–R7)

Both readers apply the same independent re-validation ladder (defense in depth
against a stale, replayed, or partially written record). Each failure row returns
`Err`:

| # | Condition | dedup | consolidation |
|---|---|---|---|
| R1 | File absent (tool never ran / binary unresolvable / tool exited non-zero) | **`Err`** | **`Err`** |
| R2 | File present but not valid JSON / truncated | **`Err`** | **`Err`** |
| R3 | `schema` ≠ the record's expected schema | **`Err`** | **`Err`** |
| R4 | `choice` not a closed variant, or a non-owned field supplied / a required field missing (dedup) | **`Err`** | _(n/a — no choice enum)_ |
| R5 | `rationale` missing or empty after sanitizing (dedup) | **`Err`** | _(n/a)_ |
| R6 | `goal_id` ≠ the seam sentinel (stale / other-seam record) | **`Err`** | **`Err`** |
| R7 | `cycle_number` ≠ `REASONER_RECORD_CYCLE` (prior-cycle record) | **`Err`** | **`Err`** |
| ✔ | All checks pass | `Ok(IdeaDedupDecision)` | `Ok(Vec<IdeaCluster>)` (possibly empty) |

For consolidation, a **present-but-empty** cluster list is the success row
`Ok(vec![])`, **not** a failure. A cluster whose `canonical_id` is empty after
sanitizing is silently **dropped** by `IdeaCluster::sanitized` on read (not an
error for the whole list) — the surviving clusters still return `Ok`.

!!! note "Both seams bind `cycle_number` to the `REASONER_RECORD_CYCLE = 0` sentinel"
    Like the Group A/B records, the creative-ideas records live in a **fresh,
    unique per-call temp dir** created and torn down inside a single reasoner
    call, so cross-cycle replay is structurally impossible. The writer and reader
    therefore bind `cycle_number` to the constant `REASONER_RECORD_CYCLE = 0`
    (`src/ooda_brain/recipe_brain.rs`) rather than a live cycle counter. Because
    neither `IdeaDedupCtx` nor `IdeaConsolidationCtx` is naturally goal-scoped,
    `goal_id` is bound to a **fixed synthetic per-seam constant**
    (`"creative-idea-dedup"` / `"creative-idea-consolidation"`) passed identically
    to the writer and the reader; R6/R7 thus enforce write/read self-consistency,
    which is all the fresh-temp-dir design requires.

---

## How the reasoners call the tools

`RecipeBrain::decide_idea_dedup` and `RecipeBrain::decide_idea_consolidation` wire
each recipe up so the agent can call its tool and the reader can find the result
(modeled on `run_per_goal_cycle_recipe`; they replace the former
`invoke_idea_dedup_raw` / `invoke_idea_consolidation_raw` stdout-scraping
writers):

1. Allocate a **fresh unique per-call temp directory** (owner-only, `0o600`
   record mode, cleaned up after the call).
2. Pass context vars to the recipe via `-c` (argv-only, never `sh -c`):
   - `-c record_path=<tempdir>/idea_dedup.json` (or `idea_consolidation.json`)
   - `-c simard_bin=<current_exe absolute path>` — resolved via
     `std::env::current_exe()`, never a bare `simard` on `PATH`.
   - `-c goal_id=<seam sentinel>`, `-c cycle_number=0`
   - for consolidation, `-c clusters_path=<tempdir>/clusters.json` so the agent
     knows where to write its cluster list before calling the tool.
   - plus every existing render var already documented in the recipe reference
     (`candidate_idea` / `candidate_rationale` / `existing_shortlist` for dedup;
     `existing_pool` for consolidation), each already `sanitize_context_var`-bounded.
3. Run the `creative-idea-dedup` / `creative-ideas-consolidation` recipe. The
   agent's stdout is **ignored**; a stray JSON print has zero effect.
4. Read the result with
   `read_verified_idea_dedup(record_path, "creative-idea-dedup", REASONER_RECORD_CYCLE)`
   / `read_verified_idea_consolidation(record_path, "creative-idea-consolidation",
   REASONER_RECORD_CYCLE)`.

If the tool cannot be resolved or exits non-zero, no record is written and the
reader reports R1 `Err`.

The `creative-idea-dedup` recipe's agent step invokes the tool roughly like:

```bash
"$simard_bin" ooda record-idea-dedup \
  --choice enhance_existing \
  --target-node-id node-7a3f \
  --reason "same goal-board caching idea as node-7a3f; adds a measured 12% fewer reads" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

The `creative-ideas-consolidation` recipe's agent step first writes the cluster
list to `$clusters_path` with its file tool, then invokes:

```bash
"$simard_bin" ooda record-idea-consolidation \
  --clusters-path "$clusters_path" \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

Both recipes document `Output: NONE scraped from stdout` in their header, like
`ooda-per-goal-cycle.yaml`. See
[Creative-idea dedup recipe & prompt schema](./creative-idea-dedup-recipe.md) for
the recipe/prompt contracts.

### Fail directions are preserved

The conversion changes **only what makes the two `decide_idea_*` methods return
`Err`** ("scrape returned `None`" → "record absent/invalid"). The downstream
act-sites and their fail-closed defaults are **unchanged**:

- **idea-dedup** ([`dedup_gate.rs`](./creative-ideas-dedup-gate-api.md)) — a
  `decide_idea_dedup` `Err` drives the **existing** `PlannedAction::FailClosed`
  path: the candidate is dropped this cycle (never a silent duplicate, never a
  wrong-node enhance) and retried next run. The shortlist-membership check on
  `enhance_existing` still fires regardless of the record.
- **idea-consolidation** (`consolidate_existing`) — a `decide_idea_consolidation`
  `Err` drives the **existing** write-nothing path (the applier makes no status
  transitions and surfaces the error; retry later). A present-but-empty list
  still cleanly means "nothing to consolidate" (`Ok(vec![])`), leaving the pool
  untouched without an error.

No new default or fallback action is introduced anywhere, and the coarse-Jaccard
backstop remains exclusively on the kill-switch-off path (never an error
fallback). These fail-closed guarantees are asserted by explicit per-seam tests
(a stub that produces an unreadable record ⇒ dedup drops the candidate;
consolidation writes nothing).

---

## Configuration

| Setting | Source | Default | Notes |
|---|---|---|---|
| `record_path` | recipe `-c record_path` | none (required) | Absolute path inside a daemon-owned, per-call temp dir. The tool rejects non-absolute or `..`-bearing paths. |
| `clusters_path` | recipe `-c clusters_path` (consolidation) | none (required for consolidation) | Absolute path to the agent-written JSON-array cluster file. Same `harden_path` rules. |
| `simard_bin` | recipe `-c simard_bin` | `current_exe()` | Absolute path to the running `simard` binary, so the recipe sandbox resolves the tool deterministically (never a bare `simard` on `PATH`). |
| `goal_id` | recipe `-c goal_id` | `creative-idea-dedup` / `creative-idea-consolidation` | Fixed synthetic per-seam constant; re-verified on read (R6). |
| `cycle_number` | recipe `-c cycle_number` | `REASONER_RECORD_CYCLE` = `0` | Fixed sentinel; re-verified on read (R7). |
| dedup `schema` pin | `IDEA_DEDUP_SCHEMA` const | `simard.creative.idea_dedup.v1` | Reader rejects any other value (R3). |
| consolidation `schema` pin | `IDEA_CONSOLIDATION_SCHEMA` const | `simard.creative.idea_consolidation.v1` | Reader rejects any other value (R3). |
| record file mode | `persistence::persist_json` | `0o600` | Owner-only; atomic temp + fsync + rename. |
| free-text bound | `sanitize::sanitize_context_var(_, 500)` | 500 chars | Applies to `rationale` / `merged_rationale` / each `evidence` entry. |
| cluster-list cap | `IdeaCluster` list bound | 64 | Mirrors the 64-entry prompt-cost DoS guard; excess clusters rejected. |
| reason-file cap | bounded field-file read | 64 KiB | Oversized file is a hard error, never truncated. |

There are **no new `SIMARD_*` environment knobs** and **no database** — each seam
is a single file-backed JSON record. The existing creative-ideas kill-switch and
the coarse-Jaccard backstop are **unchanged**. Schema evolution is handled by
serde plus the pinned schema string.

### Clusters go through a file, not argv

Per the operator constraint (a large cluster list would hit `E2BIG` on argv), the
consolidation clusters are always passed by **file** via `--clusters-path`, never
inline. The path is hardened exactly like `--record-path` — it must be
**absolute** and free of `..` — and the JSON array is read under a byte cap and
validated cluster-by-cluster through `IdeaCluster::sanitized` before the record is
written. Likewise, an oversized dedup `--reason` goes through `--reason-path`
under the same 64 KiB cap.

---

## Security

| ID | Threat | Mitigation |
|---|---|---|
| SR-AUTHZ-1 | Reasoner over-reach | Each tool holds **zero privilege**: its only side effect is one `persist_json` write. No idea persistence, no memory mutation, no status transition, no Bridge/Python/kuzu, no tokens. |
| SR-AUTHZ-2 | Bypassing the gate | Authority stays with the deterministic `dedup_gate.rs` seams, which apply the verdict — and the shortlist-membership + `IdeaStatus` checks — **after** the read. No `--admin` / `--no-verify` / bypass flag exists. |
| SR-AUTHZ-3 | Binary substitution | `simard_bin` is resolved via `current_exe()`, never a `PATH` lookup, so a hostile `simard` on `PATH` cannot be invoked. |
| SR-VAL-1 | Injected / drifted choice | `--choice` validated by `IdeaDedupDecision::from_choice_fields` (closed enum, case-insensitive); both writer and reader use the same chokepoint. |
| SR-VAL-2 | Smuggled / missing variant field | The dedup chokepoint enforces the [field-ownership matrix](#field-ownership-matrix): `--target-node-id` is required on `enhance_existing` and rejected on `create_new` / `skip`, so an enhance can never land without a target and a target can never leak onto a create/skip. |
| SR-VAL-3 | Terminal-escape / log injection via free text | `rationale` / `merged_rationale` / `evidence` run through `sanitize_context_var(_, 500)` — strips ANSI/CSI + C0/DEL, folds newlines, bounds to 500 chars ([#2751](https://github.com/rysweet/Simard/issues/2751)). Empty-after-sanitize `rationale` fails at R5; an empty-after-sanitize `canonical_id` drops the cluster. |
| SR-VAL-7 | Replay / stale-record | Reader independently checks the `schema` pin, `goal_id == sentinel`, and `cycle_number == sentinel` → any mismatch is an `Err` (R3/R6/R7); a fresh per-call temp dir makes a stale record structurally unreachable. |
| SR-VAL-8 | Path traversal / symlink write | `--record-path`, `--clusters-path`, **and** `--reason-path` must be **absolute** and free of `..` (`harden_path`); the parent is the daemon-supplied per-call temp dir. |
| SR-DOS-1 | Transient OOM via huge input file | `--reason-path` and `--clusters-path` read under byte caps, failing closed before the whole file is buffered; the cluster list is capped at 64. |
| SR-DATA-1 | World-readable record | `0o600` owner-only mode. |
| SR-DATA-2 | Torn / partial write | Atomic temp + fsync + rename. |
| SR-DATA-4 | Cross-cycle bleed | Ephemeral, unique per-call temp dir, cleaned up after the call. |
| SR-DRIFT-1 | Writer/reader validation drift | **Single shared chokepoint per record type** (`IdeaDedupDecision::from_choice_fields`, `IdeaCluster::sanitized`) invoked by BOTH the writer and the reader — a value that writes cannot fail to read, and vice versa. The reader re-sanitizes free text and re-runs the cluster chokepoint on read, so a hostile record is cleaned/rejected, never trusted verbatim. |
| SR-POLARITY-1 | Fail-direction flip | The conversion changes only the `Err` trigger; the act-sites and defaults are untouched (dedup `Err`→drop candidate; consolidation `Err`→write nothing; present-empty list→`Ok(vec![])`). See [Fail directions are preserved](#fail-directions-are-preserved). |

**Validate-all-then-write-once:** every validation runs before any file is
written; a single failure means **no** record on disk.

> **Net effect on attack surface.** Group C **removes** an attack surface — the
> stdout scraping of model-controlled prose on the two creative-ideas seams —
> rather than adding one. The tools and readers replace fuzzy
> `extract_and_parse_json` recovery with closed-enum / sanitized-cluster,
> owner-only, freshness-checked records.

---

## Examples

!!! note "`cycle-*` in paths vs. `cycle_number: 0` in records"
    The temp-dir path may be named by the live cycle for operator legibility, but
    the record's identity field is bound to the fixed `REASONER_RECORD_CYCLE = 0`
    sentinel (see [How the reasoners call the tools](#how-the-reasoners-call-the-tools)).
    That is why every example below passes `--cycle-number 0`.

### idea-dedup — `create_new` (genuinely novel)

```bash
simard ooda record-idea-dedup \
  --choice create_new \
  --reason "proposes a new episodic-memory compaction pass; no shortlisted entry targets memory compaction" \
  --record-path /run/simard/ooda/creative-dedup-8f21/idea_dedup.json \
  --goal-id creative-idea-dedup \
  --cycle-number 0
```

### idea-dedup — `enhance_existing` (same idea, adds evidence)

```bash
simard ooda record-idea-dedup \
  --choice enhance_existing \
  --target-node-id node-7a3f \
  --reason "same goal-board caching idea as node-7a3f, but adds a measured 12% fewer reads — append as evidence" \
  --record-path /run/simard/ooda/creative-dedup-8f21/idea_dedup.json \
  --goal-id creative-idea-dedup \
  --cycle-number 0
```

### idea-dedup — `skip` (near-verbatim restatement)

```bash
simard ooda record-idea-dedup \
  --choice skip \
  --reason "restates node-91cc ('cache goal-board reads') with no new rationale, evidence, or angle" \
  --record-path /run/simard/ooda/creative-dedup-8f21/idea_dedup.json \
  --goal-id creative-idea-dedup \
  --cycle-number 0
```

### idea-consolidation — write clusters, then record

```bash
# The agent first wrote its cluster list to the clusters file with its file tool:
cat /run/simard/ooda/creative-consol-3b90/clusters.json
# [
#   {"canonical_id": "node-7a3f",
#    "redundant_ids": ["node-91cc", "node-4d20"],
#    "merged_rationale": "cache the goal-board reads: three entries all propose caching goal_board.json across OODA cycles",
#    "evidence": ["node-4d20 measured 12% fewer reads"]}
# ]

simard ooda record-idea-consolidation \
  --clusters-path /run/simard/ooda/creative-consol-3b90/clusters.json \
  --record-path /run/simard/ooda/creative-consol-3b90/idea_consolidation.json \
  --goal-id creative-idea-consolidation \
  --cycle-number 0
```

### idea-consolidation — nothing to consolidate (valid empty result)

```bash
# clusters.json contains: []
simard ooda record-idea-consolidation \
  --clusters-path /run/simard/ooda/creative-consol-3b90/clusters.json \
  --record-path /run/simard/ooda/creative-consol-3b90/idea_consolidation.json \
  --goal-id creative-idea-consolidation \
  --cycle-number 0
```

Record written (a valid "nothing to consolidate" result):

```json
{
  "schema": "simard.creative.idea_consolidation.v1",
  "goal_id": "creative-idea-consolidation",
  "cycle_number": 0,
  "clusters": []
}
```

### Rejections (no file written, non-zero exit)

```bash
# Out-of-enum choice
simard ooda record-idea-dedup --choice merge ...            # error: unknown choice 'merge' for record-idea-dedup

# enhance_existing without a target
simard ooda record-idea-dedup --choice enhance_existing --reason "..." ...   # error: --target-node-id required for choice 'enhance_existing'

# target smuggled onto create_new
simard ooda record-idea-dedup --choice create_new --target-node-id node-7a3f ...  # error: --target-node-id not valid for choice 'create_new'

# empty reason
simard ooda record-idea-dedup --choice skip --reason "" ...  # error: --reason must be non-empty

# non-absolute record path
simard ooda record-idea-consolidation --record-path ./rec.json ...  # error: --record-path must be absolute

# clusters file with a traversal path
simard ooda record-idea-consolidation --clusters-path /run/simard/../etc/x ...  # error: --clusters-path must not contain '..'
```

---

## Versioning & Compatibility

### Adding an idea-dedup variant

1. Add the variant to `IdeaDedupDecision` in `src/ooda_brain/mod.rs`, its
   `from_choice_fields` arm (including the field-ownership rule), the
   `variant_label` / `rationale` accessors, and the `dedup_gate.rs` apply match.
2. `from_choice_fields` accepts the new keyword once its arm is added; the CLI
   writer and `read_verified_idea_dedup` pick it up through the shared chokepoint.
3. Extend the `creative-idea-dedup` recipe `OPTIONS` guidance so the agent knows
   to pass it to `--choice`.
4. Add an example here and in the recipe reference.
5. Add serde round-trip + field-ownership + `read_verified_idea_dedup`
   fail-closed tests covering the new variant.

### Changing the cluster shape

Add or change an `IdeaCluster` field in `src/ooda_brain/mod.rs`, extend
`IdeaCluster::sanitized` with the field's sanitize/bound rule (both writer and
reader pick it up through the shared chokepoint), extend the
`creative-ideas-consolidation` recipe `OUTPUT FORMAT`, and add a round-trip test.

### Bumping a schema

Bumping either record **schema** (`…v1` → `…v2`) is a hard change: the reader
rejects any value other than the pinned constant, so a new writer and a new
reader must ship together.

### Compatibility with the shared prose scraper

This change removes the idea-dedup and idea-consolidation callers of
`extract_and_parse_json` / `extract_json_payload`, and deletes the four
Group-C-only scrape helpers — `IdeaDedupEnvelope`, `IdeaConsolidationEnvelope`,
`parse_idea_dedup_decision`, `parse_idea_consolidation`. The shared scraper family
in `src/recipe_output/extract.rs` (`extract_json_payload`,
`extract_and_parse_json`, …) is **retained** because Group D seams
(`cognitive_threads`, `memory_consolidation`, `stewardship`, outcome-verify) still
call it. `extract.rs` is deleted only once `grep -rn extract_json_payload src/`
returns no remaining callers; the Group C contract test asserts only the **two
creative-ideas seams** no longer reference it.

---

## Regression tests

| Test | Asserts | File |
|---|---|---|
| `read_verified_idea_dedup` / `_idea_consolidation` absent | R1 → `Err` | `ooda_brain` |
| malformed JSON | R2 → `Err` | `ooda_brain` |
| wrong schema (`…v2`) | R3 → `Err` | `ooda_brain` |
| out-of-enum choice (dedup) | R4 → `Err` | `ooda_brain` |
| `enhance_existing` missing target (dedup) | R4 → `Err` | `ooda_brain` |
| non-owned `target_node_id` on `create_new` / `skip` (dedup) | R4 → `Err` | `ooda_brain` |
| empty rationale (dedup) | R5 → `Err` | `ooda_brain` |
| goal mismatch | R6 → `Err` | `ooda_brain` |
| cycle mismatch | R7 → `Err` | `ooda_brain` |
| dedup three-variant round-trip | Each `IdeaDedupDecision` writes and reads back bit-for-bit incl. `enhance_existing`'s required target | `ooda_brain` |
| dedup field-ownership reject | Each variant rejects every field it does not own; `enhance_existing` rejects a missing target | `ooda_brain` |
| consolidation round-trip | A populated cluster list writes and reads back, re-sanitized on read | `ooda_brain` |
| **consolidation empty-present** | A present `clusters: []` record ⇒ `Ok(vec![])` (NOT an `Err`) | `ooda_brain` |
| **consolidation absent** | An absent record ⇒ `Err` (preserving `Some(vec![])` vs `None`) | `ooda_brain` |
| cluster sanitize | An empty `canonical_id` cluster is dropped; free text bounded to 500; list capped at 64 | `ooda_brain` |
| **dedup fail-CLOSED** | An unreadable record ⇒ `decide_idea_dedup` `Err` ⇒ dedup gate `PlannedAction::FailClosed` (candidate dropped); shortlist-membership check still fires on enhance | `ooda_brain` |
| **consolidation fail-CLOSED** | An unreadable record ⇒ `decide_idea_consolidation` `Err` ⇒ applier writes nothing | `ooda_brain` |
| CLI enum reject | `--choice merge` → non-zero, **no file** | `operator_cli` |
| CLI empty-reason reject | `--reason ""` → non-zero, **no file** | `operator_cli` |
| CLI oversized reason | reason bounded to 500 chars in the record | `operator_cli` |
| CLI sanitize | ANSI/C0 bytes stripped from `rationale` / `merged_rationale` / `evidence` | `operator_cli` |
| CLI file mode | record is `0o600` | `operator_cli` |
| CLI path guard | non-absolute / `..` `--record-path` / `--clusters-path` → non-zero, **no file** | `operator_cli` |
| CLI clusters-file cap | oversized clusters file → non-zero, **no file** | `operator_cli` |
| rework contract | both creative-ideas seams no longer reference `extract_and_parse_json` / `extract_json_payload` / the four deleted helpers; a grep guard keeps `extract_json_payload src/` non-empty (Group D machinery retained) | `tests_rework_contract` |

Where `ooda_brain` = `src/ooda_brain/tests_record_idea_dedup_consolidation.rs`,
`operator_cli` = `src/operator_cli/tests_record_idea_dedup_consolidation.rs`, and
`tests_rework_contract` = `src/ooda_brain/tests_rework_contract.rs`.

Tests are **split by owning module**, mirroring the admission reference:

- **CLI-writer / dispatch-verb rows** (`CLI enum reject`, `CLI empty-reason
  reject`, `CLI oversized reason`, `CLI sanitize`, `CLI file mode`, `CLI path
  guard`, `CLI clusters-file cap`) live in
  `src/operator_cli/tests_record_idea_dedup_consolidation.rs`. These exercise
  the `record-idea-dedup` / `record-idea-consolidation` dispatch verbs directly:
  enum rejection, empty-reason rejection, `0o600` file mode, `--record-path` /
  `--clusters-path` path guards, the oversized clusters-file cap, and
  `IdeaCluster::sanitized` on write.
- **Reader / record-matrix rows** (R1–R7, both round-trips, field-ownership,
  cluster sanitize on read, the empty-present vs absent distinction, and both
  fail-CLOSED rows) live in
  `src/ooda_brain/tests_record_idea_dedup_consolidation.rs`.

The **rework contract** row lives in `src/ooda_brain/tests_rework_contract.rs`
(grep contract scoped to the two seams), and the recipe-asset assertions (the
two new schemas + verbs in the recipe assets, and the `Output: NONE scraped
from stdout` doc block) live in `tests/creative_ideas_dedup_assets.rs`.

> **Placement note for implementers.** The record round-trip and reader-matrix
> tests are **in-crate `#[cfg(test)]` sibling modules under `src/`** — split
> between `src/operator_cli/…` (CLI writer) and `src/ooda_brain/…` (readers),
> exactly as `record-admission` does. Do **not** create a top-level
> `tests/tests_record_idea_dedup_consolidation.rs` integration crate; the only
> top-level `tests/` file is the recipe-asset check
> `tests/creative_ideas_dedup_assets.rs`. A design-spec `test_files` entry that
> names `tests/tests_record_idea_dedup_consolidation.rs` is superseded by this
> layout.

---

## See Also

- [Reference: `simard ooda record-decision` (typed decision tool)](./ooda-record-decision-cli.md) — the reference implementation this mirrors
- [Reference: `simard ooda record-admission` / `record-resource-admission`](./ooda-record-admission-cli.md) — the Group B sibling
- [Reference: `simard ooda record-orient` / `record-decide`](./ooda-record-orient-decide-cli.md) — the Group A sibling
- [Reference: Creative-idea dedup recipe & prompt schema](./creative-idea-dedup-recipe.md) — how the two recipes call these tools
- [Reference: Creative-ideas dedup-gate API](./creative-ideas-dedup-gate-api.md) — the typed surface these tools map to
- [Concept: semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md)
- [Issue #2925 — semantic creative-ideas dedup + consolidation](https://github.com/rysweet/Simard/issues/2925)
- [Issue #4719 — remove recipe-emits-JSON → Rust-scrapes → Rust-acts (epic)](https://github.com/rysweet/Simard/issues/4719)
- [Issue #1711 — no silent fallback on the decision path](https://github.com/rysweet/Simard/issues/1711)
