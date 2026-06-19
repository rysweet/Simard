---
title: Prospective-trigger firing
description: How OODA preparation makes prospective-memory triggers actually fire — the slug-phrase-enriched objective probe built in ooda_loop/cycle.rs and the case-insensitive substring match in cognitive_memory/ops.rs (issue #2300).
last_updated: 2026-06-19
owner: simard
doc_type: reference
related:
  - ./goal-prospective-memory-mirror.md
  - ./cognitive-memory-preparation-filters.md
  - ./ooda-procedural-memory.md
  - ../architecture/cognitive-memory.md
  - ../memory.md
---

# Prospective-trigger firing

> Shipped in issue [#2300](https://github.com/rysweet/Simard/issues/2300)
> (fix the "prospective triggers never fire" defect). Companion to the
> write side documented in
> [Goal–prospective memory mirror](./goal-prospective-memory-mirror.md)
> (issues [#2207](https://github.com/rysweet/Simard/issues/2207) /
> [#2280](https://github.com/rysweet/Simard/issues/2280)).

Active goals are mirrored into prospective memory as trigger-action pairs
(see the [mirror reference](./goal-prospective-memory-mirror.md)). For
those triggers to be useful they must **fire**: every OODA preparation
pass calls `check_triggers(objective)` and surfaces the matches as
`PreparedContext.triggered_prospectives`. This page documents the read
side — how the objective probe is shaped so stored triggers match, and
how the match itself is performed.

---

## The defect this fixes

Before #2300, preparation logged the same line every cycle:

```
[simard] OODA cycle: prepared context (0 facts, 0 triggers, 5 procedures, 0 episodes)
```

The `0 triggers` was permanent: `check_triggers` never matched a stored
prospective even though Active goals were being written correctly.

**Root cause — classification (b), a read/match mismatch.** The write
path was already correct and proven by the passing
`put_active_goal_creates_prospective_trigger` test in
`src/goals/cognitive_memory_store.rs`: putting an Active goal stores a
`Prospective` whose `trigger_condition` is the goal **slug-phrase** (the
slug with dashes replaced by spaces, e.g. `"fix authentication bug"`).
Neither "nothing writes a trigger" (a) nor "both" applied.

The failure was entirely on the read side, where two independent
divergences each broke the substring match:

1. **Content divergence (primary).** The OODA objective probe was built
   only from active-goal **descriptions** joined with `"; "`. A goal's
   free-text description (`"Fix the broken auth flow in the parser"`)
   does not contain the slug-phrase (`"fix authentication bug"`)
   verbatim, so the predicate `'{probe}' CONTAINS trigger_condition`
   could not match.
2. **Case divergence (secondary).** The match was case-sensitive.
   Descriptions are written in natural title case (`"Fix ..."`) while
   `trigger_condition` is lowercased by `goal_slug`, so even a probe that
   happened to contain the right words would miss on case alone.

The fix addresses both: it **enriches the probe** so the stored
`trigger_condition` substring is guaranteed present, and **folds the
match to case-insensitive** as a robustness safety net.

The two changes are not co-equal. The probe enrichment is the
**load-bearing** fix: the appended slug-phrase is byte-identical to the
already-lowercase `trigger_condition`, so the goal-trigger path fires
*even under the old case-sensitive predicate*. Case-folding is genuinely
**defensive** — for the goal path it changes nothing, and it exists to
generalise the guarantee to any prospective whose `trigger_condition`
casing might differ from the probe (for example a non-goal prospective
written with a mixed-case condition). This is why the sections below frame
the content fix as primary and the case fix as a safety net.

---

## The match contract

`check_triggers(content)` runs a single read-only Cypher query and
returns every pending `Prospective` whose `trigger_condition` is a
substring of `content`, compared case-insensitively:

```rust
fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>>;
```

```cypher
MATCH (p:Prospective)
WHERE p.status = 'pending'
  AND toLower('{c}') CONTAINS toLower(p.trigger_condition)
RETURN p.id, p.description, p.trigger_condition,
       p.action_on_trigger, p.status, p.priority
```

where `{c}` is `escape_cypher(content)` — the caller-supplied probe,
escaped exactly once at this boundary.

Properties:

- **Direction:** `content CONTAINS trigger_condition`. The probe is the
  haystack; the stored trigger phrase is the needle. A short trigger
  phrase matches a long objective, not the reverse.
- **Case-insensitive:** both sides are wrapped in `toLower(...)`. A
  mixed-case probe matches a lowercase `trigger_condition` and vice
  versa.
- **Pending only:** resolved/triggered entries are never returned. The
  status lifecycle is `pending → triggered → resolved`.
- **Read-only:** the method only issues `self.query(...)`. It performs no
  writes and no `execute`.

### Escaping and fold order (security invariant)

`check_triggers` is the single escaping chokepoint for the probe. The
ordering is **escape → interpolate → fold**:

```rust
let c = escape_cypher(content);        // 1. escape the literal once
// 2. interpolate into the query string as '{c}'
// 3. toLower('{c}') ... toLower(p.trigger_condition)   ← fold in Cypher
```

- Only the `'{c}'` literal is attacker-reachable; it is escaped before
  interpolation. `p.trigger_condition` is a schema-controlled column
  (written through the equally escaped `store_prospective`), so wrapping
  it in `toLower(...)` introduces no new injection surface.
- Folding happens **in Cypher** via `toLower(...)`, never by lowercasing
  the raw string in Rust before escaping. If a future change folds in
  Rust instead, it must be `escape_cypher(content.to_lowercase())` —
  escape the already-lowercased string, never `to_lowercase()` an
  already-escaped string.

Quotes, backslashes, and newlines in the probe produce a valid,
non-breaking query (they match nothing rather than altering the query),
and this is covered by an injection-regression test.

---

## The objective probe: `build_objective_probe`

For triggers to fire, the probe handed to `check_triggers` must contain
the stored `trigger_condition` substring. `src/ooda_loop/cycle.rs`
builds that probe with a small pure helper:

```rust
/// Build the OODA objective probe from the live active goals.
///
/// For each active goal the probe includes BOTH the goal's free-text
/// description AND its slug-phrase (`goal_slug(id)` with dashes replaced
/// by spaces). The slug-phrase is byte-identical to the
/// `trigger_condition` written by the prospective mirror, which
/// guarantees `check_triggers` can substring-match the stored trigger.
///
/// Pure in-memory string assembly: builds NO Cypher and performs NO
/// escaping or interpolation. The result flows through
/// `preparation_memory_operations_with_active_slugs` → `check_triggers`,
/// where it is escaped exactly once.
fn build_objective_probe(active: &[ActiveGoal]) -> String;
```

The helper replaces the previous inline construction in the prepare
phase, which joined descriptions only:

```rust
// Before #2300 — descriptions only; never contained the slug-phrase.
let objective_summary: String = state
    .active_goals
    .active
    .iter()
    .map(|g| g.description.as_str())
    .collect::<Vec<_>>()
    .join("; ");

// After #2300 — descriptions + slug-phrases.
let objective_summary = build_objective_probe(&state.active_goals.active);
```

### The slug-phrase invariant

The probe and the stored trigger **must use the identical transform** or
the substring match silently fails again. Both derive the phrase from the
goal slug with dashes replaced by spaces:

| Side  | Location | Expression |
|-------|----------|------------|
| Write | `prospective_trigger_for()` in `src/goals/cognitive_memory_store.rs` | `record.slug.replace('-', " ")` |
| Read  | `build_objective_probe()` in `src/ooda_loop/cycle.rs` | `goal_slug(&g.id).replace('-', ' ')` |

The stored `record.slug` is itself derived from `g.id`: the
`GoalBoard → GoalRecord` adapter `active_goals_as_records()` in
`src/goal_curation/operations.rs` sets `slug: goal_slug(&active.id)`. The
write side then builds the trigger from that slug
(`record.slug.replace('-', " ")`), making it exactly
`goal_slug(g.id).replace('-', " ")`. The read side computes the same
`goal_slug(&g.id).replace('-', ' ')`, so the two phrases are
byte-identical **by construction** — this is why the probe must call
`goal_slug(&g.id)` rather than using the raw `g.id` (which is not
guaranteed to already be normalised; the adapter normalises it precisely
because it may not be). Any future change must keep all three call sites
in lock-step: the adapter's `goal_slug(&active.id)`,
`prospective_trigger_for()`, and `build_objective_probe()`.

### Probe shape

The probe concatenates, per active goal, the description followed by the
slug-phrase, so a single string carries both the human-readable objective
text (useful for the fact/procedure/episode probes that share it) and the
exact trigger needles:

```
Fix the broken auth flow in the parser fix authentication bug; \
Refactor the gym scoring module refactor gym scoring
```

The leading description preserves the existing behaviour for the semantic
fact search and procedure/episode recall that consume the same
`objective_summary`; the appended slug-phrase is what makes the
prospective trigger fire.

---

## End-to-end flow

```
GoalStore::put(Active goal "Fix authentication bug")
  └─ store_prospective(
        description       = "goal:Fix authentication bug",
        trigger_condition = "fix authentication bug",   ← slug-phrase
        action_on_trigger = "Pursue goal: ...",
        priority          = 1)
              │
              ▼ (next OODA cycle, prepare phase)
build_objective_probe(active)
  = "... Fix authentication bug fix authentication bug; ..."   ← contains needle
              │
              ▼
preparation_memory_operations_with_active_slugs(objective = probe, …)
  └─ check_triggers(probe)
        MATCH (p:Prospective)
        WHERE p.status='pending'
          AND toLower(probe) CONTAINS toLower("fix authentication bug")
        → 1 row
              │
              ▼
PreparedContext.triggered_prospectives = [ "goal:Fix authentication bug" ]
```

Log line after the fix (note the non-zero trigger count):

```
[simard] OODA cycle: prepared context (3 facts, 1 triggers, 5 procedures, 0 episodes)
```

---

## Examples

### Example 1 — single active goal fires

Stored prospective: `trigger_condition = "fix authentication bug"`.

```rust
let probe = build_objective_probe(&[active("fix-authentication-bug",
                                           "Fix the broken auth flow")]);
// probe contains "fix authentication bug"
let triggered = mem.check_triggers(&probe).unwrap();
assert!(!triggered.is_empty());          // fires
```

### Example 2 — case-insensitive match

A title-cased probe still matches a lowercase trigger:

```rust
mem.store_prospective("goal:Ship release", "ship release", "act", 1).unwrap();
let triggered = mem.check_triggers("Time to SHIP RELEASE now").unwrap();
assert_eq!(triggered.len(), 1);          // fires despite case
```

### Example 3 — no active goals, nothing fires

```rust
let probe = build_objective_probe(&[]);  // ""
let triggered = mem.check_triggers(&probe).unwrap();
assert!(triggered.is_empty());           // correct: no goals → no triggers
```

### Example 4 — unrelated objective does not match

```rust
mem.store_prospective("goal:Ship release", "ship release", "act", 1).unwrap();
let triggered = mem.check_triggers("review the dashboard layout").unwrap();
assert!(triggered.is_empty());           // correct: needle absent
```

---

## Code location

| Item                                | File                                          |
|-------------------------------------|-----------------------------------------------|
| `check_triggers` (match query)      | `src/cognitive_memory/ops.rs`                 |
| `build_objective_probe`             | `src/ooda_loop/cycle.rs`                       |
| Probe call site (prepare phase)     | `src/ooda_loop/cycle.rs`                       |
| `prospective_trigger_for` (write)   | `src/goals/cognitive_memory_store.rs`         |
| `store_prospective` (write)         | `src/cognitive_memory/ops.rs`                  |
| Live consumer (`check_triggers(objective)`) | `src/memory_consolidation/mod.rs`     |

> The live consumer in `src/memory_consolidation/mod.rs` is **not
> modified** by #2300. It already passed the probe through to
> `check_triggers`; the fix changes only what the probe *contains*
> (cycle.rs) and how the match *compares* (ops.rs). Distillation and
> episodic-recall code in that module are out of scope.

---

## Testing

| Test                                                  | File | Coverage |
|-------------------------------------------------------|------|----------|
| Probe contains each active goal's slug-phrase         | `src/ooda_loop/cycle.rs` | `build_objective_probe` appends `goal_slug(id).replace('-', ' ')` per goal |
| End-to-end fire (the regression reproduction)         | `src/cognitive_memory/ops.rs` | Store an Active-goal-style prospective, probe with a realistic enriched objective, assert `triggered > 0` |
| Case-insensitive match                                | `src/cognitive_memory/ops.rs` | Mixed-case probe vs lowercase `trigger_condition` → fires |
| Injection regression                                  | `src/cognitive_memory/ops.rs` | Probe with `'`, `\`, and newline → valid, non-breaking query, no match |

The end-to-end test is the **TDD red** for #2300: it fails on `main`
(0 triggers) and passes after the probe enrichment lands.

### Existing tests that must stay green

The case-fold is a permissive superset — anything that matched
case-sensitively still matches once both sides are folded — so the
pre-existing trigger tests in `src/cognitive_memory/ops.rs`
(`check_triggers_matches_substring`, `check_triggers_returns_empty_on_no_match`,
`check_triggers_only_returns_pending`) remain green. For example,
`check_triggers_matches_substring` stores `trigger_condition = "FAIL"` and
probes with `"build FAILED with errors"`; it matches under both the old
case-sensitive predicate and the new folded one. The write-side proof
(`put_active_goal_creates_prospective_trigger` in
`src/goals/cognitive_memory_store.rs`) and the seven `escape_cypher_*`
tests are likewise unaffected. Run the full suite with `cargo test`.

---

## Out of scope

Deliberately excluded from #2300:

- **Bootstrap seed triggers.** The fix repairs the existing
  write → read → match path. It does **not** seed any prospective
  triggers at startup; that is a separate concern.
- **Distillation and episodic recall.** No changes to
  `src/memory_consolidation/` distillation logic or episodic-recall
  paths — those are owned by separate workstreams.
- **Probe-length capping.** `CONTAINS` is `O(n·m)`; the active-goal count
  is operationally small, so no cap is applied. A per-description length
  cap is a possible future optimisation, not a requirement.

---

## Related reading

- [Goal–prospective memory mirror](./goal-prospective-memory-mirror.md)
  — the write side: how Active goals become prospective triggers and how
  the slug-phrase `trigger_condition` is produced.
- [Preparation-phase memory filters](./cognitive-memory-preparation-filters.md)
  — the surrounding preparation pass and the shared `objective` probe and
  `active_slugs` set.
- [OODA procedural memory](./ooda-procedural-memory.md) — the sibling
  recall surface that consumes the same objective string.
- [Cognitive memory architecture](../architecture/cognitive-memory.md)
  — the six-type memory model and the prospective lifecycle.
- [Memory architecture](../memory.md) — the six memory types overview.
