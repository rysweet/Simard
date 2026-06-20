---
title: Bootstrap procedures and trigger naming
description: How Simard seeds three baseline procedural memories on daemon boot and how the OODA cycle now stores procedures with recallable names so recall_procedure actually finds them at preparation time.
last_updated: 2026-06-14
owner: simard
doc_type: reference
related:
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-procedural-idempotency.md
  - ./cognitive-memory-preparation-filters.md
  - ./cognitive-memory-episodic-recall.md
  - ../architecture/cognitive-memory.md
  - ../memory.md
---

# Bootstrap procedures and trigger naming

> **De-fork Phase 2b.** The bootstrap-seeding and trigger-naming *behavior*
> described here is preserved: writes and recalls go through the
> `CognitiveMemoryOps` trait, now backed solely by `LibraryCognitiveMemory`
> over `amplihack-memory-lib`. The daemon-boot wiring that this page anchors to
> `NativeCognitiveMemory::open` is now `LibraryCognitiveMemory::open`, and the
> `NativeCognitiveMemory::in_memory` test helper is replaced by the library
> in-memory store; treat those native code citations as historical. See
> [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md).

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-C (procedural seeding + episodic recall). PR-C also adds
> episodic recall — see
> [Episodic recall in preparation](./cognitive-memory-episodic-recall.md).
> Supersedes the "always-empty procedural recall" behaviour described
> in [OODA procedural memory](./ooda-procedural-memory.md) prior to PR-C.

PR-C fixes two issues in procedural memory that, together, made
`recall_procedure` return zero hits on every cycle even when the
objective directly matched a known pattern:

1. **The OODA cycle wrote procedures with names like `ooda:advance-goal`**.
   `recall_procedure` searches names (and steps) with `CONTAINS`, so an
   objective `"merge PR #2281"` never matched `ooda:advance-goal`.
2. **No procedures existed at startup.** Even with better naming, a
   fresh install had nothing to recall. The OODA cycle had to bootstrap
   itself, and the brain had no procedural signal until enough cycles
   accumulated.

PR-C addresses both by:

- Storing OODA-derived procedures under **goal-scoped, trigger-bearing
  names** like `pr-merge:{goal_id} | triggers: merge,pr,review`.
- Seeding three **bootstrap procedures** (`pr-merge`, `ci-fix`,
  `run-tests`) on daemon boot, idempotently.

The net effect: `recall_procedure` returns ≥1 hit for any
objective mentioning common engineer-loop trigger keywords from the
very first cycle on a fresh install.

---

## Naming convention

Procedural memory names now follow this pattern:

```
{pattern}:{scope} | triggers: {comma-separated-keywords}
```

Where:

- `pattern` is a short verb-phrase tag for the procedure shape
  (`pr-merge`, `ci-fix`, `run-tests`, `engineer-loop`, …).
- `scope` is a context disambiguator: a goal id for OODA-derived
  procedures (`fix-auth-bug`), or `bootstrap` for seeded baselines.
- The `| triggers: …` suffix is **part of the name string** so that
  `recall_procedure`'s `CONTAINS` matcher hits when the objective
  text contains any trigger token.

### Examples

| Source                                        | Name                                                       |
|-----------------------------------------------|------------------------------------------------------------|
| OODA cycle: successful `AdvanceGoal` for `fix-auth-bug`, objective mentions `#2281` and `Cargo.toml` | `pr-merge:fix-auth-bug \| triggers: merge,pr,review,ci,2281,toml` |
| OODA cycle: successful `RunImprovement` with no PR number, no file extension in objective       | `ci-fix:improve-coverage \| triggers: ci,green,failing,fix-ci,improve` |
| Bootstrap seed                                | `pr-merge:bootstrap \| triggers: merge,pr,merge-pr,landing,ready-to-merge` |
| Bootstrap seed                                | `ci-fix:bootstrap \| triggers: ci,green,failing,fix-ci,red` |
| Bootstrap seed                                | `run-tests:bootstrap \| triggers: test,cargo test,nextest,unit,integration` |

### Why triggers live in the name

`recall_procedure`'s underlying query is
`MATCH (p:Procedure) WHERE p.name CONTAINS '{q}' OR p.steps CONTAINS '{q}'`.
There is no separate `triggers` column on the `Procedure` node — the
schema change required to add one is heavier than what PR-C scopes.
Stuffing the trigger list into the name gives every match-source a
single substring to hit without a schema migration. A future PR can
add a typed `triggers` column and migrate; the trait stays additive.

---

## Bootstrap procedures

Three procedures are seeded on daemon boot if (and only if) they are
not already present:

### `pr-merge:bootstrap`

```
name:       pr-merge:bootstrap | triggers: merge,pr,merge-pr,landing,ready-to-merge
triggers:   merge, pr, merge-pr, landing, ready-to-merge
steps:
  1. Verify CI green on the target branch
  2. Verify scope clean (single concern, no unrelated edits)
  3. Verify quality-audit passed (≥3 cycles, no critical findings)
  4. Verify docs updated if the change is user-facing
  5. Verify PR description reflects current state
  6. Use the merge-ready skill to gate merge
prerequisites: []
```

Steps mirror the merge-ready criteria checklist in
`prompt_assets/simard/engineer_system.md`. The bootstrap text is
inlined verbatim in `BOOTSTRAP_PROCEDURES` rather than read from the
system prompt at runtime, so future drift in the system prompt does
not silently invalidate the bootstrap content — a contributor who
changes the merge-ready checklist must consciously decide whether to
re-sync the bootstrap procedure.

### `ci-fix:bootstrap`

```
name:       ci-fix:bootstrap | triggers: ci,green,failing,fix-ci,red
triggers:   ci, green, failing, fix-ci, red
steps:
  1. Fetch the failing CI run summary (gh run view --log-failed)
  2. Classify failure: compile / test / lint / flake
  3. Reproduce locally before changing code
  4. Patch the root cause, not the symptom
  5. Re-run the failing job locally if possible
  6. Push and wait for CI to re-validate
prerequisites: []
```

Steps reflect the diagnostic flow used by the
`ci-diagnostic-workflow` agent.

### `run-tests:bootstrap`

```
name:       run-tests:bootstrap | triggers: test,cargo test,nextest,unit,integration
triggers:   test, cargo test, nextest, unit, integration
steps:
  1. cargo nextest run --workspace --no-fail-fast
  2. Fall back to cargo test --workspace if nextest is unavailable
  3. For a single crate: cargo nextest run -p <crate>
  4. For a single test: cargo nextest run --test <name>
  5. Inspect failures; rerun only the failing tests with --no-capture
prerequisites: []
```

---

## Idempotent seeding

The seeder is exposed as a single public function:

```rust
/// Seed bootstrap procedures into cognitive memory if missing.
/// Returns the count of procedures newly stored (0 if all were already present).
///
/// Idempotent: safe to call on every daemon start.
pub fn seed_bootstrap_procedures(
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<usize>;
```

Algorithm:

```
for procedure in BOOTSTRAP_PROCEDURES {
    let hits = bridge.recall_procedure(&procedure.name, 1)?;
    if hits.is_empty() {
        bridge.store_procedure(&procedure.name, &procedure.steps, &procedure.prerequisites)?;
        seeded_count += 1;
    }
}
```

The recall-then-store pattern guarantees idempotency. Restarting the
daemon never produces duplicate procedures.

### Wiring

`seed_bootstrap_procedures` is called once during `OodaBridges`
construction, immediately after `NativeCognitiveMemory::open`
succeeds and before the OODA loop starts. The exact call site lands
at implementation time in whichever of `bin/simard/main.rs` or
`src/operator_commands_ooda/daemon/mod.rs` constructs the
`OodaBridges` (current daemon boot wiring threads both candidates);
the requirement is **once per daemon start, post-`open`,
pre-loop**, regardless of file.

The returned count is logged:

```
[simard] cognitive memory: 3 bootstrap procedures seeded
[simard] cognitive memory: 0 bootstrap procedures seeded (all present)
```

If `seed_bootstrap_procedures` errors, the daemon logs the error and
continues — seeding is best-effort, never fatal.

---

## OODA cycle storage changes (runtime — distinct from bootstrap seeding above)

The table and rules in this section describe how `cycle.rs` stores
procedures **at runtime** from successful action outcomes. They are
separate from the three `:bootstrap`-scoped procedures seeded once
at daemon start; the same naming convention applies to both, but the
data sources are different.

`src/ooda_loop/cycle.rs` no longer stores procedures with the
generic `ooda:{action_kind}` name (the current code at
`cycle.rs:343` writes `format!("ooda:{}", outcome.action.kind)`).
The new logic constructs a goal-scoped, trigger-bearing name from
the in-flight `ActionOutcome`:

```rust
// pseudocode — see cycle.rs for the exact code
let pattern  = pattern_for(action.kind);              // "pr-merge", "ci-fix", ...
let scope    = goal_id_for(&action).unwrap_or("ad-hoc".to_string());
let base     = triggers_for(action.kind);              // ["merge", "pr", ...]
let derived  = derive_triggers_from_objective(&objective, &action.description);
let triggers = merge_dedup(base, derived);
let name     = format!("{pattern}:{scope} | triggers: {}", triggers.join(","));

let steps = vec![
    action.description.clone(),  // what was planned
    outcome.detail.clone(),      // what happened
];

bridge.store_procedure(&name, &steps, &[])?;
```

The `pattern`, `scope`, `base-triggers`, and `derive_triggers_from_objective`
helpers all live in `src/ooda_loop/cycle.rs` next to the existing
`store_procedure` call site (no new module). The runtime `ActionKind →
pattern` and base-trigger mapping is small and explicit:

| `ActionKind`         | `pattern`         | Base triggers (always merged in)            |
|----------------------|-------------------|---------------------------------------------|
| `AdvanceGoal`        | `pr-merge`        | `merge, pr, review, ci`                     |
| `RunImprovement`     | `ci-fix`          | `ci, green, failing, fix-ci, improve`       |
| `ConsolidateMemory`  | `consolidate`     | `consolidate, memory, distill`              |
| `RunGymEval`         | `run-tests`       | `test, gym, eval, benchmark`                |
| `BuildSkill`         | `build-skill`     | `skill, build, scaffold`                    |
| `LaunchSession`      | `engineer-loop`   | `engineer, session, spawn`                  |
| `ResearchQuery`      | `research`        | `research, investigate, explore`            |
| `PollDeveloperActivity` | `poll-activity`| `poll, activity, status`                    |
| `ExtractIdeas`       | `extract-ideas`   | `idea, extract, brainstorm`                 |
| `SafeUpdate`         | `safe-update`     | `update, upgrade, version`                  |

### Objective-derived triggers

`derive_triggers_from_objective(&objective, &action.description)`
scans the cycle objective text and the planned action description
for two narrow patterns and folds the captures into the trigger
list. This is **not** a generic tokenizer — it targets the two
identifier shapes that empirically improve recall hit rate the most:

| Pattern    | Regex                  | Capture used as trigger  |
|------------|------------------------|--------------------------|
| PR number  | `#(\d+)`               | The digits, e.g. `2281`  |
| File ext   | `\.([A-Za-z][A-Za-z0-9]{2,4})\b` | The extension, e.g. `toml`, `json`, `yaml` |

Captures are lowercased and deduplicated against the base trigger
list. The merge order is `base ++ derived`, with later duplicates
dropped, so base triggers always appear first in the rendered name.

> **Read/write floor alignment (ws2 #2295).** The file-extension
> capture floors at **3 characters** (`{2,4}` ⇒ 3–5 chars total). This
> deliberately matches the **read-side** floor in
> `memory_consolidation::tokenize_objective`, which drops every
> objective token shorter than 3 chars before issuing a recall query. A
> 1- or 2-char derived trigger (`g`, `rs`, `md`, …) could therefore
> never be matched by a tokenized recall — it would only sit in the
> procedure name as visible-but-dead weight and, when it landed as the
> trailing trigger, look exactly like the mid-word truncation symptom
> reported in ws2 #2295. Aligning both floors removes that confusion
> without losing any recall power.

Example:

```
ActionKind::AdvanceGoal
objective = "merge PR #2281 updating Cargo.toml"
base      = ["merge", "pr", "review", "ci"]
derived   = ["2281", "toml"]
name      = "pr-merge:fix-cog-mem | triggers: merge,pr,review,ci,2281,toml"
```

(Touching a 1–2 char extension such as `cycle.rs` adds no `rs` trigger
under the 3-char floor; only the `#2281` PR-number capture survives.)

Cost is one regex scan per stored procedure (already on a code path
that only fires on successful outcomes — i.e. infrequently), and
the recall-hit-rate gain is real because `recall_procedure`'s
`CONTAINS` matcher then hits the procedure when the next objective
mentions the same PR number or touches the same extension.

If either pattern fails to match (no `#N`, no `.ext`), the derived
list is simply empty and the rendered name omits the extras — there
is no fallback or wildcard.

### Intentional accumulation (revised by #2298)

The "one procedure node per successful cycle" semantics described here
were **superseded by issue
[#2298](https://github.com/rysweet/Simard/issues/2298)**. `store_procedure`
is now idempotent on exact name: multiple successful `AdvanceGoal` runs
against the same goal that derive the **same** `pr-merge:{goal_id} |
triggers: …` name collapse to a single node (with `usage_count`
incremented), rather than creating duplicate rows. Runs that derive
**distinct** names still accumulate distinct nodes. `recall_procedure`
returns the de-duplicated set (recall does not currently rank by
`usage_count` — rows return in store order under its `CONTAINS`/`LIMIT`
query; `usage_count` is recorded for future ranking). See
[Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md).

---

## Recall behaviour

`recall_procedure(query, limit)` is unchanged at the trait level —
PR-C only changes what procedures *exist* and what their *names* look
like. The query semantics remain `CONTAINS` over name and steps.

### Unified tokenized recall path (ws2 #2295)

Before ws2 #2295 two recall paths coexisted and disagreed:

- The OODA **preparation phase**
  (`memory_consolidation::preparation_memory_operations_with_active_slugs`)
  tokenized the objective and issued one `recall_procedure(token, …)`
  call per token, deduped by `node_id` — so both bootstrap *and*
  distilled procedures surfaced.
- The **base-type adapter** turn preparation
  (`base_type_turn::prepare_turn_context`, used by the engineer loop)
  passed the **entire raw objective sentence** to a single
  `recall_procedure(objective, 5)` call. The Cypher
  `name CONTAINS '<full sentence>'` predicate never matched any stored
  procedure (no procedure name embeds a natural sentence), so only the
  bootstrap procedures — injected by other means — ever appeared in
  `prepared context (… procedures …)`, regardless of how many cycles
  had run. This was the cycle-238 symptom.

Both call sites now share one entry point,
`memory_consolidation::recall_procedures_for_objective(bridge,
objective, max)` (re-exported as `simard::recall_procedures_for_objective`):

1. Tokenize the objective with the shared `tokenize_objective`
   (lowercased, 3-char floor, stopwords removed).
2. Issue one `recall_procedure(token, max)` per token, dedup by
   `node_id`.
3. Sort by `usage_count` desc, then `name` asc, then `node_id` asc for
   a fully deterministic order (names are **not** unique — repeated
   cycles store the same composed name under different `node_id`s — so
   `node_id` is the final tiebreaker that keeps `truncate` stable across
   runs).
4. Truncate to `max`.

**Empty-token fallback.** When the objective produces no 3+ char tokens
(very short or punctuation-only input), the helper issues a single
`recall_procedure(objective, max)` call so callers that pass a
pre-tokenized or exact-name query (e.g. the bootstrap idempotency
check) keep working.

**Case-folding contract.** `tokenize_objective` lowercases every token;
both `BOOTSTRAP_PROCEDURES` and `compose_procedure_name` emit lowercase
trigger text. Cypher `CONTAINS` is case-sensitive, so the all-lowercase
invariant on both sides is what makes recall actually fire.

The new naming convention means:

| Objective text                       | Procedures recalled (at minimum)                |
|--------------------------------------|-------------------------------------------------|
| `"merge PR #2281"`                   | `pr-merge:bootstrap`, any prior `pr-merge:*`    |
| `"fix the CI failure"`               | `ci-fix:bootstrap`, any prior `ci-fix:*`        |
| `"run cargo test"`                   | `run-tests:bootstrap`, any prior `run-tests:*`  |
| `"do something completely novel"`    | (empty — no trigger keywords match)             |

Empty recall is still a valid outcome. PR-C does not introduce a
fallback procedure; the brain handles the empty case as it always
has.

---

## Observability

Two log lines surface PR-C's procedural changes:

```
[simard] cognitive memory: 3 bootstrap procedures seeded
[simard] OODA consolidation: stored procedure 'pr-merge:fix-auth-bug | triggers: merge,pr,review,ci,2281,toml'
```

ws2 #2295 adds **structured `tracing` events** alongside the free-form
`eprintln!` lines so operators can answer "which procedures fired this
cycle?" and "is my trigger list truncated?" directly from the JSON
journal, without tail-following or risk of shipper line-length
truncation:

```
INFO recalled procedures for objective   procedure_count=2 tokens=[…] procedure_names="pr-merge:bootstrap … | pr-merge:g1 …"
INFO OODA consolidation: stored procedure   procedure_name="pr-merge:fix-auth-bug | triggers: merge,pr,review,ci,2281,toml"
```

The structured `procedure_name` / `procedure_names` fields are written
verbatim by every `fmt` layer (JSON and the default human formatter),
so the full name is captured even if a downstream log shipper truncates
the free-form message string.

The preparation-phase line (shared with episodic recall) also reports
procedure count:

```
[simard] preparation: 2 procedures, 1 episodes recalled (1 raw, 0 session-filtered)
```

---

## Examples

### Example 1 — fresh install

1. Daemon starts; `seed_bootstrap_procedures` runs.
2. All three bootstrap procedures absent → seeded. Log:
   `[simard] cognitive memory: 3 bootstrap procedures seeded`.
3. First OODA cycle starts. Objective: `"merge PR #2281"`.
4. `recall_procedure("merge PR #2281", 5)` returns
   `[pr-merge:bootstrap]` (1 hit) — the trigger `"merge"` matches
   inside the name suffix.
5. `PreparedContext.recalled_procedures` has 1 entry; the brain sees
   a non-empty `## Recalled Procedures` section.

### Example 2 — restart with prior work

1. Daemon restarts after the prior session merged 3 PRs.
2. `seed_bootstrap_procedures` sees the three bootstrap names already
   present → seeds nothing. Log:
   `[simard] cognitive memory: 0 bootstrap procedures seeded (all present)`.
3. First OODA cycle. Objective: `"merge PR #2295"`.
4. `recall_procedure("merge PR #2295", 5)` returns
   `[pr-merge:bootstrap, pr-merge:fix-a, pr-merge:fix-b, pr-merge:fix-c]`
   (4 hits, ranked by usage count).

### Example 3 — novel objective

1. Objective: `"investigate disk usage growth over the last week"`.
2. None of the trigger keywords match for any bootstrap procedure.
3. `recall_procedure` returns `[]` (or 1 entry for `research:bootstrap`
   if we shipped one — PR-C does not, but a future iteration could).
4. The brain sees an empty `## Recalled Procedures` section, falls
   back to its default reasoning path.

---

## Code location

| Item                                          | File                                                  |
|-----------------------------------------------|-------------------------------------------------------|
| `seed_bootstrap_procedures`                   | `src/cognitive_memory/bootstrap_procedures.rs`         |
| `BOOTSTRAP_PROCEDURES` constant               | `src/cognitive_memory/bootstrap_procedures.rs`         |
| Daemon boot wiring                            | Once-per-start call from the `OodaBridges` constructor, post-`NativeCognitiveMemory::open`, pre-loop. Exact file is `bin/simard/main.rs` or `src/operator_commands_ooda/daemon/mod.rs` depending on which path constructs the bridges; confirmed at PR-C implementation time. |
| OODA cycle procedure storage                  | `src/ooda_loop/cycle.rs` (currently at `cycle.rs:343`, the `format!("ooda:{}", outcome.action.kind)` site) |
| Runtime pattern + trigger mapping             | `src/ooda_loop/cycle.rs` (next to the storage site)   |
| `derive_triggers_from_objective` helper       | `src/ooda_loop/cycle.rs`                              |
| `recall_procedures_for_objective` unified helper | `src/memory_consolidation/mod.rs` (re-exported as `simard::recall_procedures_for_objective`) |
| `tokenize_objective` (shared read-side tokenizer / 3-char floor) | `src/memory_consolidation/mod.rs` |
| Base-type adapter call site (now routed through the helper) | `src/base_type_turn.rs` (`prepare_turn_context`) |
| Tests                                         | `src/cognitive_memory/bootstrap_procedures_tests.rs`, |
|                                               | `src/ooda_loop/cycle.rs`,                             |
|                                               | `tests/cognitive_memory_procedure_recall_unified.rs` |

---

## Testing

### Bootstrap-seeding tests

In `src/cognitive_memory/bootstrap_procedures_tests.rs`:

| Test                                                      | Coverage                                            |
|-----------------------------------------------------------|-----------------------------------------------------|
| `seed_is_idempotent`                                      | Two calls store 3 total, not 6                      |
| `seed_skips_existing_procedures_by_name`                  | Pre-populate `pr-merge:bootstrap` → only 2 seeded   |
| `recall_finds_seeded_procedures_for_typical_objectives`   | Objective `"merge PR #2281"` → ≥1 recall hit        |
| `seed_propagates_storage_errors`                          | `store_procedure` Err → function returns Err        |

### Cycle-storage tests

In `src/ooda_loop/cycle.rs` tests module:

| Test                                                          | Coverage                                          |
|---------------------------------------------------------------|---------------------------------------------------|
| `successful_advance_goal_stores_pr_merge_procedure`           | Name pattern + base triggers populated             |
| `successful_run_improvement_stores_ci_fix_procedure`          | Mapping table coverage                             |
| `procedure_name_contains_objective_derived_triggers`          | `derive_triggers_from_objective` populates `#NNNN` PR numbers and `.ext` file extensions into the name, deduplicated against base triggers, base-first order preserved |
| `derive_triggers_handles_no_pr_or_ext_match`                  | Objective without `#N` and without `.ext` → derived list empty, name omits extras, no panic |
| `failed_outcomes_do_not_store_procedures`                     | Regression guard for the existing "successes only" rule |

### Unified recall regression gates (ws2 #2295)

In `tests/cognitive_memory_procedure_recall_unified.rs` — run against
the live `cognitive_memory.ladybug` schema via
`NativeCognitiveMemory::in_memory` (real `lbug::Database` + real
`SCHEMA_DDL`, no storage-layer mocks):

| Test                                                          | Coverage                                          |
|---------------------------------------------------------------|---------------------------------------------------|
| `distilled_procedure_with_foo_bar_trigger_surfaces_for_foo_objective` | A `foo,bar`-triggered procedure surfaces under objective `"fix the foo issue"`; recalled name is byte-for-byte intact (no mid-word truncation) |
| `bootstrap_and_distilled_pr_merge_procedures_both_surface`    | Bootstrap **and** distilled `pr-merge` procedures both surface through the unified path |
| `recall_is_case_insensitive_via_consistent_lowercase_folding` | Lowercase-stored procedure is hit by a SHOUTED uppercase objective (write/read case-folding invariant) |
| `derived_triggers_no_longer_emit_sub_three_char_extensions`  | 1/2-char file extensions dropped; 3+ char extensions kept (read/write floor alignment) |
| `distilled_procedure_surfaces_through_prepare_turn_context`  | End-to-end through the **fixed call site** `base_type_turn::prepare_turn_context` — catches a future revert even if the shared helper stays correct |

Outside-in CLI gate: `tests/gadugi/procedure-recall-unified.yaml`
drives each gate via `gadugi-test run`.

---

## Migration / backwards compatibility

- **Existing `ooda:{kind}` procedures** stored by pre-PR-C cycles
  remain in the database. They are still queryable by
  `recall_procedure`, just less likely to be hit because their names
  do not contain trigger keywords. They can be left in place or
  pruned by a future hygiene CLI; PR-C does not delete them.
- **Trait shape** is unchanged. PR-C does not add any trait method
  for procedural memory.
- **Tests** that previously asserted `name == "ooda:advance-goal"`
  are updated to assert `name.starts_with("pr-merge:")` and contain
  the expected trigger keywords.

---

## Out of scope

- **Typed `triggers` column on `Procedure`** — current "triggers in
  the name suffix" workaround is acceptable per the PR-C brief.
  Adding a typed column is a schema migration, not a logic fix.
- **Bootstrap `research:bootstrap` and others** — PR-C ships exactly
  the three procedures called out in the brief (`pr-merge`, `ci-fix`,
  `run-tests`). Adding more is cheap and can land in a follow-up if
  the recall stats motivate it.
- **Per-trigger ranking inside `recall_procedure`** — exact-match,
  prefix-match, and contains-match all currently rank the same. A
  ranking refinement is a separate concern from the seeding fix.
- **Pruning old `ooda:{kind}` procedures** — left intentionally in
  place; a `simard memory prune-procedures` CLI is a follow-up.
