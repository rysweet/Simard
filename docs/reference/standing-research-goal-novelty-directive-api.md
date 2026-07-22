---
title: Standing-research novelty-directive API reference
description: Reference for the standing cognition/research novelty steer — the `description_marks_research` / `ActiveGoal::is_standing_research_goal()` predicates in src/goal_curation/types.rs, the research-marker set, and the static novelty-first directive injected by build_goal_advance_input in src/ooda_actions/goal_session/input.rs (#4347).
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/novelty-first-standing-research-steering.md
  - ../concepts/steerable-ooda-daemon.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../concepts/research-goal-never-idle.md
  - ./no-progress-breaker-api.md
  - ./research-goal-never-idle-rail-api.md
  - ./typed-ooda-goal-session-rails.md
  - ./goal-board-api.md
  - ../howto/steer-a-standing-research-goal-toward-novelty.md
  - ../howto/keep-the-research-goal-never-idle.md
  - ../../src/goal_curation/types.rs
  - ../../src/ooda_actions/goal_session/input.rs
  - ../../prompt_assets/simard/goal_session_objective.md
---

# Standing-research novelty-directive API reference

> **Status: implemented.** The predicates live in
> [`src/goal_curation/types.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
> alongside the existing `description_marks_standing` / `ActiveGoal::is_perpetual`.
> The directive-injection hook lives in
> [`src/ooda_actions/goal_session/input.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/goal_session/input.rs)
> (`build_goal_advance_input`). The canonical directive *prose* is owned by
> [`prompt_assets/simard/goal_session_objective.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md);
> the code-owned copy in `input.rs` is a static reinforcement string.

This reference specifies the API added in issue #4347. For the rationale, see
[Novelty-first steering for standing research/cognition goals](../concepts/novelty-first-standing-research-steering.md).

> **Directive contract updated by #4399.** The `is_standing_research_goal()`
> predicate and injection point on this page are unchanged and are **reused** by
> #4399. The *directive text contract* below (step 3, "fall back to incremental
> maintenance only when no novel direction is viable") is **superseded** for the
> never-idle mandate: #4399 replaces that fallback with "design + run a NEW
> measurable experiment; degrade to a LOCAL experiment when no external source is
> reachable — never idle, never a repeat". See the
> [never-idle rail API reference](./research-goal-never-idle-rail-api.md#lever-a-never-idle-directive-contract)
> for the current directive contract.
>
> **What is enforced vs. expected.** This directive is a **prompt-level
> expectation** injected into the goal's reasoning context — it asks the LLM to
> produce a novel, non-repeated action each cycle; it does **not** prove that it
> does (dedup / "materially distinct" is prompt-hoped, not code-verified). What the
> #4399 code enforces is the narrower, reactive rail: a research goal that *did*
> idle **and holds no live in-flight artifact** is recorded as a fault and
> re-oriented the next cycle. A goal still holding an open, unmerged PR (a live
> in-flight artifact) is treated as progress and left untouched — its `wip_refs`
> are preserved. See
> [In-flight progress is not idle](./research-goal-never-idle-rail-api.md#in-flight-progress-is-not-idle-crusty-finding-1).

## Contents

- [Research-marker set](#research-marker-set)
- [`description_marks_research`](#description_marks_research)
- [`ActiveGoal::is_standing_research_goal`](#activegoalis_standing_research_goal)
- [Directive injection: `build_goal_advance_input`](#directive-injection-build_goal_advance_input)
- [Directive text contract](#directive-text-contract)
- [Guarantees](#guarantees)
- [What is unchanged](#what-is-unchanged)

## Research-marker set

The research/cognition markers are a small, code-owned constant slice, mirroring
the shape of the existing `STANDING_DESCRIPTION_MARKERS`:

```rust
/// Whole-word, case-insensitive markers that flag a goal's description as a
/// cognition/research goal. Matched with a LEADING word boundary (via
/// `contains_phrase_on_word_boundary`), so ordinary words that merely *contain*
/// one of these substrings (e.g. "scorecall", "preretrieval") never trigger a
/// false positive.
const RESEARCH_DESCRIPTION_MARKERS: &[&str] = &[
    "cognition",
    "recall",
    "distillation",
    "reasoner",
    "memory",
    "consolidation",
    "retrieval",
    "embedding",
];
```

The set is intentionally about *cognition-research subject matter* — including
`memory`, since the motivating goal's charter is "graph **memory**, recall
quality, distillation fact-yield, and reasoner reliability". It is not a slug
list: the `70ab8541` goal id never appears in code. Matching uses a **leading
word boundary** (via `contains_phrase_on_word_boundary`), so a description that
merely *contains* a marker as a non-leading substring (e.g. "scorecall",
"preretrieval") does not falsely qualify.

## `description_marks_research`

```rust
/// True when `description` names cognition/research subject matter.
///
/// Reuses the same word-boundary matcher as `description_marks_standing`, so a
/// marker only matches on a token boundary (start-of-string or a non-alphanumeric
/// char before it). Pure, total, and panic-free over arbitrary input, including
/// empty, very-long, Unicode, and control-character strings.
pub fn description_marks_research(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    RESEARCH_DESCRIPTION_MARKERS
        .iter()
        .any(|phrase| contains_phrase_on_word_boundary(&lower, phrase))
}
```

- **Input:** any `&str`.
- **Output:** `true` iff at least one research marker matches on a word boundary.
- **Purity:** no allocation beyond the lowercase copy; no regex; no panic path.

## `ActiveGoal::is_standing_research_goal`

```rust
impl ActiveGoal {
    /// True when this goal is BOTH standing/perpetual AND about cognition/research.
    ///
    /// This is the single predicate that gates the novelty-first steer (#4347).
    /// It is the conjunction of the two independent, reused predicates — there is
    /// no third notion of "standing research goal" to drift:
    ///
    /// * `is_perpetual()` / `description_marks_standing` — durable standing marker
    /// * `description_marks_research` — cognition/research subject matter
    ///
    /// A goal that is standing-only (e.g. a CI-stewardship perpetual goal) or
    /// research-worded-but-bounded (an ordinary one-off cognition task) does NOT
    /// qualify. No goal-id / slug match is performed.
    pub fn is_standing_research_goal(&self) -> bool {
        self.is_perpetual() && description_marks_research(&self.description)
    }
}
```

Truth table:

| `is_perpetual()` | `description_marks_research` | `is_standing_research_goal` | Example |
| --- | --- | --- | --- |
| ✓ | ✓ | **✓ steer applies** | "…improve your own cognition… STANDING PERPETUAL goal" |
| ✓ | ✗ | ✗ | "Keep CI green across the ecosystem. STANDING PERPETUAL goal." |
| ✗ | ✓ | ✗ | "Fix the trailing-comma recall parse site in PR #4374." |
| ✗ | ✗ | ✗ | any ordinary bounded goal |

## Directive injection: `build_goal_advance_input`

The hook is a deterministic, additive branch in `build_goal_advance_input`
([`src/ooda_actions/goal_session/input.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/goal_session/input.rs)).
That function assembles a single local `objective: String` (seeded with the goal
header + trimmed `goal_session_objective.md`, then the environment/observe-only/
recalled-memory sections) before wrapping it in the returned
`BaseTypeTurnInput`. Its signature and return type are unchanged; the only new
behaviour is a guarded, idempotent append onto that same `objective` buffer:

```rust
// Inside build_goal_advance_input, after the base `objective` String is
// assembled (env context appended) and before it is wrapped in the returned
// BaseTypeTurnInput:
if goal.is_standing_research_goal() {
    objective.push_str(NOVELTY_FIRST_DIRECTIVE);
}
```

- **Guard:** the append happens **iff** `goal.is_standing_research_goal()`.
- **No-op otherwise:** ordinary and standing-non-research goals get byte-for-byte
  the previous `objective`.
- **Static string only:** `NOVELTY_FIRST_DIRECTIVE` is a `const`/`&'static str`
  owned by the code. **No** goal field (`description`, slug), WIP text, or recalled
  memory is interpolated into it — this closes the reflected-prompt-injection
  vector.
- **Idempotent:** appended at most once per input build.
- **Buffer, not a separate `context` var:** the real builder has no `context`
  local — the directive is pushed onto the `objective` String that is placed into
  `BaseTypeTurnInput` (`input.rs:29`), so it reaches the model verbatim.

## Directive text contract

`NOVELTY_FIRST_DIRECTIVE` is a static reinforcement of the canonical prose in
`goal_session_objective.md`. It MUST instruct the standing-research goal, each
cycle, to:

1. **FIRST survey** novel/unexplored cognition-research directions it has not yet
   tried, drawing on its own memory and recent PRs to avoid repeating work.
2. **PREFER** a genuinely new direction — prototype + benchmark against the
   current recall-precision / fact-yield baseline, delivering either a durable PR
   implementing the novel technique **or** a memory-recorded, reasoned NEGATIVE
   result — **over** another incremental parse-site / dedup refinement.
3. **Fall back** to incremental maintenance **only** when no novel direction is
   currently viable, and **say so** explicitly.

The code-owned copy is kept short and canonical; the authoritative, fuller prose
lives in the prompt asset so the two never drift into contradiction.

## Guarantees

- **Determinism.** For a fixed goal the injection decision is a pure function of
  `goal.description`; no clock, RNG, or IO influences it.
- **Totality / no panic.** Both predicates are total over arbitrary input; there
  is no `unwrap`/`expect` on untrusted data (enforced under
  `clippy -D warnings`).
- **No new trust boundary.** The hook runs in-process at the existing OODA
  privilege, reads only the in-memory `goal.description`, and touches no secrets,
  env, config, network, or new dependency.
- **Single source of truth.** The steer keys on the conjunction of the two
  existing/near-existing predicates; there is exactly one "standing research goal"
  notion.

## What is unchanged

- **Lifecycle.** `is_perpetual()`, the completion-evidence gate, and
  `roll_to_new_cycle` are untouched — the goal remains non-completable.
- **No-progress breaker.** The
  [standing/perpetual exemption](../concepts/perpetual-goal-no-progress-exemption.md)
  in `no_progress.rs` / `heal_stale_no_progress_blocks` is untouched.
- **Output contract.** The goal-session response contract
  (`ACTION: SPAWN_ENGINEER` / `NO ACTION` / `PROGRESS: NN`, see
  [typed OODA goal-session rails](./typed-ooda-goal-session-rails.md)) is
  unchanged — the steer shapes reasoning input, not output shape.
- **Ordinary goals.** Any goal for which `is_standing_research_goal()` is false
  receives the exact prior reasoning context.

## Tests

- **Predicate unit tests** (`#[cfg(test)]` in `types.rs`): positive
  (standing **and** research); negatives (standing-only, research-only, ordinary);
  pathological input (empty, very-long, Unicode, control chars) asserting no
  panic and correct booleans.
- **Injection tests** (`src/ooda_actions/tests_goal_session.rs`): the directive
  **is** present in the built context for a standing-research goal and **absent**
  for an ordinary goal; the directive body contains the survey / prefer-novel /
  disclosed-fallback clauses.
