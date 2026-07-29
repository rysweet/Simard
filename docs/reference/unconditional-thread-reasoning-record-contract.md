---
title: "Reference: Unconditional thread-reasoning record contract"
description: >
  The normative contract that every reflective cognitive-thread recipe writes its
  typed ThreadReasoningRecord on every execution path — including the "nothing
  durable to keep" path — so the fail-CLOSED R1 reader (read_verified_thread_reasoning)
  never trips on a spurious absent record. Covers the unconditional-ACT invariant,
  the nine reflective recipes it guards (seven edited, two already compliant), the
  strengthened rework contract test,
  the record path and freshness model, the optional additive ENOENT self-diagnosis
  signature, configuration, security, and worked examples. Companion explanation:
  concepts/unconditional-thread-reasoning-record.md (#4986).
last_updated: 2026-07-29
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/unconditional-thread-reasoning-record.md
  - ./simard-cognition-record-thread-reasoning-cli.md
  - ./cognitive-threads-catalog.md
  - ./cognitive-thread-observability.md
  - ./recipe-invoker-seam.md
  - ./terminal-failure-diagnosis-api.md
  - ../concepts/self-diagnose-on-step-error.md
  - ../index.md
---

# Reference: Unconditional thread-reasoning record contract

Recipes: `prompt_assets/simard/recipes/<thread>.yaml`
Rail (reader call site): `src/cognitive_threads/recipe_rail.rs` (`run_reflective_thread`)
Record type + reader: `src/ooda_brain/thread_reasoning_record.rs`
(`read_verified_thread_reasoning`, `ThreadReasoningRecord`)
Contract test: `src/cognitive_threads/tests_rework_contract.rs`
Optional diagnosis: `src/overseer/diagnosis.rs` (`classify_cause`, `FailureCause`)

This page pins the contract that fixes the OODA reflection-step failure
`cognitive-thread: reflection: FAILED — R1 no record at expected path: No such file
or directory (os error 2)` ([#4986](https://github.com/rysweet/Simard/issues/4986)).
The narrative is in
[Unconditional thread-reasoning record](../concepts/unconditional-thread-reasoning-record.md).

!!! info "Normative contract (this spec is the source of truth)"
    1. **Unconditional ACT.** Every reflective recipe MUST call
       `simard cognition record-thread-reasoning` on **every** execution path. No
       path — including "nothing durable to keep" / "pure noise" / "exact repeat of a
       prior record" — may exit before that step.
    2. **The record step is the single terminal ACT.** It is the last effect on all
       paths; nothing after it may skip it.
    3. **R1 stays fail-CLOSED.** An absent record is `R1 → Err`. The reader is NOT
       changed to default. The fix guarantees the record is *present*, never
       tolerates its absence.

## The invariant

A reflective thread's rail (`run_reflective_thread`) treats the typed record as the
**sole** source of truth after a recipe exits `0`:

```
derive record_path  →  delete stale record  →  capture invoke_start
                    →  invoke recipe (-c record_path=<abs>)
                    →  read_verified_thread_reasoning(record_path, thread, invoke_start)
```

Because the read is fail-CLOSED (R1 = absent ⇒ `Err`), the recipe MUST write the
record on **every** path it can exit through. **Seven** of the nine pre-#4986
recipes contradicted this by offering an early `finish successfully` escape; the
other two (`salience-appraise`, `narrative-identity`) already fell through to the
record step. The fix removes the escape from those seven: the memory-write
decision (`simard memory remember`) is decoupled from the reasoning-record write,
and only the former is optional.

### Before (contradiction) vs after (unconditional)

| Path | Before #4986 | After #4986 |
| --- | --- | --- |
| Durable takeaway worth keeping | `memory remember` **+** `record-thread-reasoning` | unchanged |
| Nothing durable to keep / pure noise | **exit 0, write nothing** → rail reads `R1 → Err` | **skip `memory remember`, still call `record-thread-reasoning`** → rail reads a valid record |
| Verified recurring failure | `remember-procedure` + `record-thread-reasoning` | unchanged |

On the "nothing durable" path the agent still records a one-to-three sentence
conclusion, for example: *"Nothing durable to keep this cycle: trivial success
already covered by a prior post-mortem."*

## The nine reflective recipes

The unconditional-ACT contract **guards all nine** recipes in `REFLECTIVE_RECIPES`
([`tests_rework_contract.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/tests_rework_contract.rs)).
Seven carried the early-exit escape and were edited by #4986; two were already
compliant and needed no change:

| Recipe | Thread | Domain tag | Pre-#4986 state |
| --- | --- | --- | --- |
| `reflect-postmortem` | `reflection` | `notes` | escape → **edited** |
| `metacognition-appraise` | `metacognition` | `notes` | escape → **edited** |
| `prospect-foresight` | `prospection` | `notes` | escape → **edited** |
| `operator-model` | `operator_model` | `notes` | escape → **edited** |
| `consolidate-sleep` | `consolidation` | `notes` | escape → **edited** |
| `analogy-map` | `analogy` | `notes` | escape → **edited** |
| `values-deliberate` | `values_deliberation` | `notes` | escape → **edited** |
| `salience-appraise` | `salience` | (specialized) | already compliant |
| `narrative-identity` | `narrative` | `notes` | already compliant |

A recipe violates the contract when it calls `record-thread-reasoning` **and**
still permits exiting before it — signalled by the true escape phrase
`finish successfully` (or `write nothing` / `return early`), or an **unguarded**
`skip it`. The bare substrings `and finish` and `do not skip it` are **not**
violations: `salience-appraise` legitimately ends a branch with `... signal) and
finish.`, and every reflective recipe carries the REQUIRED guardrail line `... do
not print JSON, and do not skip it.` The test below excludes both.

## Record path, freshness, anti-replay

Unchanged from the
[record-thread-reasoning CLI reference](./simard-cognition-record-thread-reasoning-cli.md);
restated here because the fix depends on them:

- **Path:** `state_root/cognitive_threads/reasoning/<thread_name>.json`, passed to
  the recipe as `-c record_path=<abs>` by the rail.
- **Anti-replay:** the rail deletes any leftover record **before** spawning, so a
  prior run's reasoning is never read as current. A missing file at that point is
  expected and fine.
- **Freshness (R7):** the record's `mtime` MUST be `>= invoke_start`, captured after
  the pre-truncate and before the spawn.

## Strengthened contract test

`every_existing_recipe_writes_the_reasoning_record` is strengthened from "the string
`record-thread-reasoning` is present" to "the record step is **unconditional**." The
test fails if any reflective recipe co-locates a true early-exit escape with the
record step — while tolerating the two legitimate substrings (`and finish`, the
guarded `do not skip it`) that would otherwise be false positives on already-compliant
recipes:

```rust
#[test]
fn every_existing_recipe_writes_the_reasoning_record() {
    for recipe in REFLECTIVE_RECIPES {
        let yaml = read_recipe(recipe);

        // (1) The ACT step exists.
        assert!(
            yaml.contains("record-thread-reasoning"),
            "{recipe}.yaml must call `simard cognition record-thread-reasoning` \
             as its terminal ACT step"
        );

        // (2) It is UNCONDITIONAL — no early-exit escape may precede it.
        //
        // Strip the guarded negation first: every reflective recipe carries the
        // REQUIRED guardrail line "... do not print JSON, and do not skip it.",
        // so "do not skip it" must never count as an escape. We then anchor on the
        // TRUE escape phrase `finish successfully` (what the seven pre-#4986
        // recipes used to exit 0 without recording) and never on the bare
        // `and finish` substring — `salience-appraise` legitimately says
        // "... signal) and finish." and is fully compliant.
        let scan = yaml.replace("do not skip it", "");

        for escape in ["finish successfully", "write nothing", "return early"] {
            assert!(
                !scan.contains(escape),
                "{recipe}.yaml must not permit exiting before the REQUIRED \
                 record-thread-reasoning step (found early-exit phrase: {escape:?})"
            );
        }

        // "skip it" is an escape ONLY when it is not the guarded "do not skip it"
        // (already stripped from `scan` above).
        assert!(
            !scan.contains("skip it"),
            "{recipe}.yaml uses an unguarded \"skip it\" that could bypass the \
             REQUIRED record-thread-reasoning step"
        );
    }
}
```

This gate passes the two already-compliant recipes (`salience-appraise`,
`narrative-identity`) and would fail any of the seven edited recipes if a
`finish successfully` escape were ever reintroduced.

The existing rail contracts remain green and unchanged:
`recipe_rail_reads_the_typed_reasoning_record_fail_closed`,
`recipe_rail_emits_the_canonical_failure_log_format`, and
`reflective_threads_have_no_unwrap_or_silent_defaults_on_recipe_fields`.

## Optional additive ENOENT self-diagnosis signature

`classify_cause` in
[`src/overseer/diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs)
may recognise the absent-record signature so a recurrence self-diagnoses cleanly.
This is **purely additive** — it adds a match arm, never changes the shape or
existing arms of `classify_terminal_failure`:

```rust
// Additive only: an absent durable record surfaces its own cause instead of
// the catch-all `Unknown`. Existing arms and the function signature are unchanged.
if transcript.contains("no record at expected path")
    || transcript.contains("No such file or directory (os error 2)")
{
    return FailureCause::MissingReasoningRecord; // additive variant
}
```

Because every `FailureCause` variant maps to a stable label, the additive variant
also needs a matching `FailureCause::as_str` arm — the enum is exhaustive there:

```rust
// In `impl FailureCause { fn as_str(&self) -> &'static str { match self { ... } } }`
FailureCause::MissingReasoningRecord => "missing-reasoning-record", // additive
```

The default without this arm remains `FailureCause::Unknown` (code-fixable,
`escalate = null`) — correct today; the arm only sharpens the WHY for operators.

## Configuration

No new configuration. The feature is prompt-asset text plus a strengthened test:

- `NODE_OPTIONS=--max-old-space-size=32768` and other saved preferences at
  `~/.amplihack/config` are unaffected.
- No new CLI flags, env vars, sockets, or record schema fields. `record_path` is
  supplied by the rail exactly as before.

## Security

- **Untrusted agent output stays validated at the Rust writer.** The recipe agent's
  `--reasoning-summary` is sanitized by `sanitize_reasoning_summary` and capped at
  the 64 KiB `MAX_SUMMARY_FILE_BYTES`; `record-thread-reasoning` still hardens the
  path (absolute, rejects `..`) and `create_dir_all`s the parent.
- **`--reasoning-summary` XOR `--reasoning-summary-path`** and strict typed
  `ThreadReasoningRecord` deserialization are unchanged.
- **Fail-CLOSED R1 is an integrity control** and is preserved: an absent record is
  still an error, so the fix cannot mask a genuinely broken recipe.
- **No payload bodies logged** — the rail logs paths, sizes, and the `FAILED — R{n}`
  code only; no secrets appear in YAML, records, or logs.

## Worked example: the "nothing durable" reflection

A reflection cycle over a trivial, already-covered success:

1. Rail derives `~/.simard/cognitive_threads/reasoning/reflection.json`, deletes any
   stale copy, captures `invoke_start`, and invokes `reflect-postmortem` with
   `-c record_path=<abs>`.
2. The agent decides nothing durable is worth remembering, so it **skips**
   `simard memory remember`.
3. The agent **still** runs the terminal ACT:

   ```
   simard cognition record-thread-reasoning \
     --thread reflection \
     --domain notes \
     --reasoning-summary "Nothing durable to keep: trivial success already covered by a prior post-mortem." \
     --note "Outcome matched postmortem:bugfix from an earlier cycle." \
     --written-at-epoch 1785000000 \
     --record-path <abs>
   ```

4. The recipe exits `0`; the rail reads the record fail-CLOSED, passes R1–R7, and
   surfaces the summary into `ThreadOutcome.summary`. The operator log shows the
   reflection's reasoning — **not** `FAILED — R1 no record at expected path`.

## Acceptance criteria

- The reflection cognitive-thread step no longer fails with `R1 no record at
  expected path` on the "nothing durable" path.
- `cargo test -p <crate> tests_rework_contract` is green, including the strengthened
  `every_existing_recipe_writes_the_reasoning_record`.
- The fail-CLOSED R1 matrix tests in `tests_thread_reasoning_record.rs` remain green
  (absent record ⇒ `Err`).
- All nine reflective recipes write the record unconditionally (seven edited by
  #4986, two already compliant), enforced by the strengthened contract test;
  failures surface via structured `tracing`/OTel only.
