---
title: Goal labels / tags API reference
description: Rust and operator-surface reference for first-class free-form labels (tags) on Simard goals — the additive labels field on ActiveGoal / GoalNode / GoalRecord, the deterministic goal_curation::labels brick, the per-creation-path source:* provenance auto-tagging, the simard goal label CLI verbs, the goal list --tag filter, and the dashboard / TUI label rendering and filtering.
last_updated: 2026-07-06
owner: simard
doc_type: reference
status: reference
related:
  - ./goal-board-api.md
  - ./goal-target-repo-routing.md
  - ./goal-decomposition.md
  - ./dashboard-goal-hierarchy-priority.md
  - ./creative-ideas-api.md
  - ./simard-cli.md
  - ../howto/label-and-filter-goals.md
---

# Goal labels / tags API reference

> **Issue [#2743](https://github.com/rysweet/Simard/issues/2743).** Every
> Simard goal now carries a first-class list of free-form **labels** (tags), so
> goals can be **categorized, filtered, and traced to their source**. In
> particular, a goal promoted from a creative idea is stamped
> `source:creative-ideas` at creation, so the operator can finally answer
> "**which goals came from creative ideas?**" from the CLI, the dashboard, and
> the TUI.

Before #2743 a goal exposed only a coarse `kind`/`source` string, so a goal
promoted from a creative idea was indistinguishable on the board from an
operator-set or OODA-generated goal — there was no way to categorize or filter
by origin. This reference documents the four pieces that fix that:

1. The [`labels` field](#the-labels-field) — an additive,
   serde-back-compatible `Vec<String>` on every goal carrier (`ActiveGoal`,
   `GoalNode`, `GoalRecord`, and the TUI mirror).
2. The [`goal_curation::labels` brick](#the-goal_curationlabels-brick) — the
   single, deterministic authority for label normalization, add/remove, the
   AND filter, and the `source:*` provenance constants.
3. [Provenance auto-tagging](#provenance-auto-tagging) — every code path that
   *creates* a goal stamps exactly one `source:*` label at first
   materialization, so origin is queryable.
4. The [operator surfaces](#operator-surfaces) — the `simard goal label`
   CLI verbs, the `simard goal list --tag` filter, and the dashboard / TUI
   label chips and tag filter.

> **Determinism boundary.** Label CRUD and provenance stamping are
> **deterministic structured-data operations** — they are plain code, not
> parsing. Simard performs **no** semantic / topical inference from goal text
> in this feature. If topical auto-tagging is ever added (inferring `research`
> or `security` from a goal's wording), that inference must be an **agentic**
> step, never a hard-coded keyword matcher — and it is explicitly **out of
> scope** here.

---

## The `labels` field

A goal's labels are a `Vec<String>` of short, free-form tags. Tags are opaque
tokens matched by exact, case-sensitive string equality; Simard imposes no
namespace grammar, but two conventions are recommended:

| Convention | Examples | Meaning |
|------------|----------|---------|
| `source:<origin>` | `source:creative-ideas`, `source:operator`, `source:ooda` | **Provenance** — stamped automatically at creation (see [below](#provenance-auto-tagging)). |
| `area:<topic>` / bare | `area:dashboard`, `research`, `security` | Operator- or agent-applied categorization. |

The field is added, additively, to every authoritative goal carrier with the
**same serde template** already used for the `Vec` fields `GoalRecord.evidence`
/ `GoalUpdate.evidence` (`src/goals/types.rs`), and echoing the additive `repo`
field (#2359):

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub labels: Vec<String>,
```

### `ActiveGoal` (`src/goal_curation/types.rs`)

```rust
/// An active goal on the board. Active goals are limited to `MAX_ACTIVE_GOALS`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveGoal {
    pub id: String,
    pub description: String,
    pub priority: u32,
    pub status: GoalProgress,
    pub assigned_to: Option<String>,
    #[serde(default)]
    pub current_activity: Option<String>,
    #[serde(default)]
    pub wip_refs: Vec<WipRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_update_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,

    /// Free-form labels (tags) for categorization, filtering, and provenance.
    /// Defaults to empty. A `source:*` provenance tag is stamped at creation
    /// (see the labels reference). Matched by exact, case-sensitive equality.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}
```

`labels` is appended as the **last** field of `ActiveGoal`, after the existing
`parent_goal_id` (#2405) and `priority_explicit` (#2695) fields that the
snippet above elides for brevity (the snippet shows only the fields relevant to
labels, not the full struct). `ActiveGoal::new` initializes it to `Vec::new()`;
two builder-style setters
compose with the existing `.with_repo(..)` / `.with_parent(..)` chain:

```rust
impl ActiveGoal {
    /// Replace the label set (used at seed / provenance-stamping sites).
    pub fn with_labels(mut self, labels: Vec<String>) -> Self;

    /// Add a single label (idempotent, order-preserving via `add_label`).
    pub fn with_label(mut self, label: impl Into<String>) -> Self;
}
```

`labels` is **not** included in `concise_label` output — it does not change the
one-line human summary of a goal.

### `GoalNode` (`src/goal_curation/types.rs`) — the graph anchor

`GoalNode`, the durable graph projection / edge anchor written by
`goal_curation::edges::write_node` (issue #2405), carries the same field so a
goal's labels survive as part of its queryable graph identity:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalNode {
    pub id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_criterion: Option<String>,

    /// The goal's labels, projected onto the graph node so provenance and
    /// categorization are queryable from the decomposition graph too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}
```

`GoalNode::new` gains a trailing `labels` argument (defaulting to empty at the
call sites that do not yet have labels to hand). Because the node is derived
from a goal, its labels are a **snapshot copy** taken when the node is written;
the `ActiveGoal` on the board remains the source of truth for a live goal.

### `GoalRecord` (`src/goals/types.rs`) — the persisted record

The file-backed goal store's `GoalRecord` carries the same field so labels
persist for goals that never sit on the active board (e.g. a creative-idea
promotion recorded directly into the store):

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub labels: Vec<String>,
```

`GoalRecord::from_update` initializes `labels` to empty for its callers. The
creative-ideas routing site does **not** go through `from_update`; it builds
`GoalRecord` with a direct struct literal (see
[the headline case below](#creative-ideas-the-headline-case)) and sets
`labels: vec![SOURCE_CREATIVE_IDEAS]` inline. **`GoalUpdate` is unchanged** —
labels are set on the record at construction, not carried on the update payload.

### TUI mirror (`src/bin/simard_tui/types.rs`)

`simard-tui` deserializes goals into its own display-only `ActiveGoal` mirror.
The mirror gains a `#[serde(default)] labels: Vec<String>` so the tab can
render and filter on labels. The mirror already ignores fields it does not
model (serde's default unknown-field behaviour — it has no
`deny_unknown_fields`), so this addition is purely to **surface** labels; it
cannot break the JSON round-trip.

---

## The `goal_curation::labels` brick

Module: `src/goal_curation/labels.rs`, re-exported from
`src/goal_curation/mod.rs`. This is a **self-contained, zero-I/O brick** and the
**single source of truth** for both the `source:*` token strings (so they never
drift across creation sites) and the four label operations. Its whole public
contract:

```rust
// Provenance constants — the ONLY place these strings are spelled.
pub const SOURCE_CREATIVE_IDEAS: &str = "source:creative-ideas";
pub const SOURCE_OPERATOR:       &str = "source:operator";
pub const SOURCE_OODA:           &str = "source:ooda";
pub const SOURCE_OVERSEER:       &str = "source:overseer";
pub const SOURCE_MEETING:        &str = "source:meeting";
pub const SOURCE_SEED:           &str = "source:seed";
pub const SOURCE_DECOMPOSITION:  &str = "source:decomposition";

/// Trim surrounding whitespace. Returns `None` if the tag is empty after
/// trimming (the only validation Simard imposes — tags are otherwise opaque).
pub fn normalize_tag(raw: &str) -> Option<String>;

/// Add `raw` (after `normalize_tag`) to `labels` if not already present.
/// Idempotent and **order-preserving** (insertion order is stable; a
/// duplicate is a no-op). Returns `true` if a label was actually added.
pub fn add_label(labels: &mut Vec<String>, raw: &str) -> bool;

/// Remove `raw` (after `normalize_tag`) from `labels`. Removing a tag that is
/// not present is a **no-op**. Returns `true` if a label was actually removed.
pub fn remove_label(labels: &mut Vec<String>, raw: &str) -> bool;

/// `true` iff `labels` contains **every** tag in `wanted` (logical AND).
/// An empty `wanted` slice matches every goal.
pub fn matches_all_tags(labels: &[String], wanted: &[String]) -> bool;

/// Map a backlog item's coarse `source` string to the `source:*` label to
/// stamp when the item is promoted to an active goal (its first
/// label-bearing materialization). The real backlog sources are structured
/// `prefix:…` tokens, so this matches on the **prefix** (`operator:*`,
/// `meeting:*`, `overseer:*`) with a `SOURCE_OODA` default for anything
/// unrecognized. See the
/// [mapping table](#backlog-promotion-is-a-first-class-provenance-case).
pub fn source_for_backlog(backlog_source: &str) -> &'static str;
```

### Semantics

- **Matching is exact and case-sensitive.** `source:creative-ideas` and
  `Source:Creative-Ideas` are different tags. `normalize_tag` only trims; it
  does not lowercase, because labels are opaque tokens and forcing a case would
  silently rewrite operator input.
- **`add_label` is idempotent and order-preserving.** Re-adding an existing
  tag changes nothing and returns `false`. New tags append at the end, so the
  displayed order is the order tags were first applied.
- **`remove_label` of an absent tag is a no-op** (`false`), so remove is safe
  to rerun.
- **`matches_all_tags` is AND**, matching the repeatable `--tag` CLI flag. An
  empty filter matches everything (an unfiltered `goal list`).

All four functions are pure and independently unit-tested; no other module
spells a `source:*` string or re-implements add/remove/match.

---

## Provenance auto-tagging

**Rule: every code path that *creates* a goal — or first materializes a
label-less backlog item as an `ActiveGoal` — stamps exactly one `source:*`
label.** In-place lifecycle moves on an already-labelled active goal — unblock,
reprioritize, progress updates, demote — **never** re-stamp or overwrite its
`source:*`. (Backlog → active *promotion* is a first materialization, not an
in-place move: a `BacklogItem` carries no labels, so it is stamped via
`source_for_backlog` — see
[below](#backlog-promotion-is-a-first-class-provenance-case).) This makes origin
deterministic and queryable.

| Creation site | File | Carrier | Stamped label |
|---------------|------|---------|---------------|
| Creative idea → goal | `creative_ideas/routing.rs` (`route_idea_to_goal`) | `GoalRecord` | `SOURCE_CREATIVE_IDEAS` |
| Operator CLI add | `operator_cli/goal.rs` (`handle_add`) | `ActiveGoal` | `SOURCE_OPERATOR` |
| Dashboard add (`POST /api/goals`) | `operator_commands_dashboard/goals.rs` | `ActiveGoal` | `SOURCE_OPERATOR` |
| Dashboard demo-seed | `operator_commands_dashboard/goals.rs` | `ActiveGoal` | `SOURCE_SEED` |
| Backlog → active promotion (both promotion sites) | `goal_curation/operations.rs` (`promote_to_active`) **and** `operator_commands_dashboard/goals.rs` (`promote_backlog_item_at`) | `ActiveGoal` | `source_for_backlog(&item.source)` |
| Meeting decisions → active (direct) | `ooda_loop/curate.rs` | `ActiveGoal` | `SOURCE_MEETING` |
| Meeting goal curation | `operator_commands_meeting/goal_curation.rs` | `ActiveGoal` | `SOURCE_MEETING` |
| OODA overflow / Overseer contributions (backlog-mediated) | `ooda_loop/curate.rs` (files `meeting:*`), `overseer/wiring.rs` (files `overseer:*`) | `BacklogItem`, stamped on promotion | via `source_for_backlog` ⇒ `SOURCE_MEETING` / `SOURCE_OVERSEER` (`SOURCE_OODA` is the unrecognized-source fallback) |
| Seed board / seed store | `goals/seed.rs`, `goal_curation/operations.rs` (`seed_default_board`) | builder | `SOURCE_SEED` |
| Decomposition → sub-goals | `goal_curation/decompose.rs` | `ActiveGoal` + `GoalNode` | inherits parent labels **+** `SOURCE_DECOMPOSITION` |

### Creative-ideas: the headline case

`route_idea_to_goal` (in `src/creative_ideas/routing.rs`) is the promotion path
from the creative-ideas thread onto the goal store. It builds the `GoalRecord`
with a **direct struct literal** (not `from_update`), so the provenance tag is
set inline as the new `labels` field:

```rust
let record = GoalRecord {
    slug: goal_slug(&idea.idea),
    // …title, rationale, status, priority, owner_identity,
    //   source_session_id, updated_in, evidence unchanged…
    labels: vec![labels::SOURCE_CREATIVE_IDEAS.to_string()],
};
```

Every goal that originated as a creative idea therefore carries
`source:creative-ideas` from birth and is identifiable on the board — the exact
gap #2743 closes. See the [creative-ideas API](./creative-ideas-api.md).

### Sub-goals inherit provenance (`inherit + mark`)

Decomposition (`simard goal decompose`, issue #2405) creates child sub-goals
from a parent. Each child **copies the parent's full `labels`** and then adds
`SOURCE_DECOMPOSITION`:

```rust
let mut child_labels = parent.labels.clone();     // inherit origin, e.g. source:creative-ideas
labels::add_label(&mut child_labels, labels::SOURCE_DECOMPOSITION);
let child = ActiveGoal::new(child_id, child_desc, child_priority)
    .with_parent(parent.id.clone())               // #2405 parent link
    .with_labels(child_labels);
```

This is deliberate: a child of a `source:creative-ideas` goal stays discoverable
as creative-ideas-originated (the origin tag propagates), while
`source:decomposition` records *how* the child itself came to exist. The
parent↔child edge is the existing #2405 `parent_goal_id` / `decomposes_into`
link — provenance inheritance rides on top of it, it does not replace it.

### Backlog promotion is a first-class provenance case

A `BacklogItem` has no `labels` field, so promoting it to an active goal is the
item's **first** label-bearing materialization. Both promotion sites —
`goal_curation::operations::promote_to_active` (the CLI / core path) and
`operator_commands_dashboard::goals::promote_backlog_item_at` (the dashboard
path), which each build the `ActiveGoal` from the removed backlog item — stamp
`source_for_backlog(&item.source)` on the new goal, so a promoted goal is never
left un-provenanced.

The real production backlog `source` strings are structured `prefix:…` tokens
(`operator:demote`, `meeting:{topic}…`, `overseer:{repo}`), so
`source_for_backlog` matches on the **prefix**:

| Backlog `source` prefix | Filed by | Stamped on promotion |
|-------------------------|----------|----------------------|
| `operator:…` (e.g. `operator:demote`) | `operator_cli/goal.rs` (`handle_demote`) | `SOURCE_OPERATOR` |
| `meeting:…` (e.g. `meeting:{topic}`) | `ooda_loop/curate.rs` (handoff overflow / action items) | `SOURCE_MEETING` |
| `overseer:…` (e.g. `overseer:{repo}`) | `overseer/wiring.rs` (steward enqueue) | `SOURCE_OVERSEER` |
| anything else (default) | OODA loop / unclassified | `SOURCE_OODA` |

Because `BacklogItem` carries no labels, a goal that is **demoted and later
re-promoted** does **not** retain the label set it held while active — it is
re-provenanced from its backlog `source` prefix (a demoted goal carries
`operator:demote`, so it comes back as `source:operator`). Not adding a `labels`
field to `BacklogItem` is a deliberate scope boundary (see
[Constraints](#constraints-non-goals)); the backlog carries only the coarse
`source` string it has always had.

### Test literals

Only **production** creation sites stamp a `source:*`. The many
`ActiveGoal { .. }` / `GoalRecord { .. }` literals inside `#[cfg(test)]`
modules simply set `labels: Vec::new()` — they are fixtures, not creation
paths, so they carry no provenance.

---

## Serde back-compatibility (no migration)

`labels` uses `#[serde(default, skip_serializing_if = "Vec::is_empty")]` on
every carrier, so the change is **additive and back-compatible with no
migration**:

- **Reading old snapshots.** `goal_board.json`, `goal_store.json`, and any
  `goal-board:snapshot` / `goal-node` facts written before #2743 have no
  `labels` key. `#[serde(default)]` deserializes those goals with
  `labels = Vec::new()` (empty) — **existing goals load fine**, no migration
  step, no load-time error.
- **Writing unlabelled goals.** `skip_serializing_if = "Vec::is_empty"` omits
  the key entirely when `labels` is empty. A board of unlabelled goals
  serializes **byte-identically** to its pre-#2743 form, so the
  `board_snapshot_hash` used by goal-carryover records is unchanged and there
  is no diff churn in the snapshot fact.
- **Forward compatibility.** The TUI mirror and any older reader ignore unknown
  fields, so a newer daemon that writes labels does not break an older reader.

No existing field is touched: `kind`, `source`, `owner_identity`, `priority`,
`repo`, lifecycle, and `MAX_ACTIVE_GOALS` are all unchanged.

---

## Operator surfaces

### CLI

The `simard goal label` verb group and the `--tag` filter on `simard goal list`
are the operator's deterministic label CRUD + query surface. They match the
existing `simard goal` subcommand style (goal id immediately after the verb,
same state-root resolution, same anti-clobber persistence). See the
[CLI reference](./simard-cli.md#simard-goal-label) for the full contract and
the [how-to](../howto/label-and-filter-goals.md) for a walkthrough.

```text
simard goal label <goal-id> add <tag>       # add a tag (idempotent)
simard goal label <goal-id> remove <tag>    # remove a tag (no-op if absent)
simard goal label <goal-id> list            # print this goal's tags, one per line
simard goal list [--tag <tag>]...           # filter the board by tag (repeatable ⇒ AND)
```

- **`add`** normalizes the tag (trim; empty-after-trim is rejected with a
  clear error and a non-zero exit), then `add_label`s it — idempotent, so
  re-adding an existing tag is a successful no-op.
- **`remove`** removes the tag if present; removing an absent tag is a no-op
  that still exits `0` (with a short note on stderr).
- **`list`** (the label sub-verb) prints the goal's tags to **stdout**, one per
  line, or `(none)` when the goal has no labels.
- **`goal list --tag`** is repeatable and combines with **AND**: a goal must
  carry **all** requested tags to be shown. The header annotates the filtered
  count, e.g. `active goals: 2 / 7 (filtered by tag)`.
- Mutations persist through the existing goal-board store (the same
  flock-guarded read-modify-write path as `simard goal add`/`remove`), so the
  operator never has to pause the daemon and concurrent unrelated writers are
  preserved. Unknown `<goal-id>` exits non-zero. No new stray `println!` /
  `eprintln!` is introduced; audit lines use the established stderr pattern.

`simard goal list` also gains a **trailing `LABELS` column** (comma-joined) on
each active-goal row, appended after the existing columns so scripts that parse
the first five tab-separated fields keep working.

### Dashboard — Goals tab

`GET /api/goals` includes each goal's `labels` array (additively — see below),
and the Goals tab renders labels as chips on each goal alongside the existing
status / priority / hierarchy rendering (see
[Goals tab hierarchy & priorities](./dashboard-goal-hierarchy-priority.md)).
A client-side **tag filter** lets the operator narrow the board to a tag — e.g.
select `source:creative-ideas` to see exactly the creative-ideas-originated
goals — styled to match the existing Goals-tab controls. Filtering is
client-side over the already-fetched live data; it adds no new route and no new
auth surface.

#### `/api/goals` — the `labels` field

The change is **additive**. Each active-goal object gains a `labels` array
alongside the existing fields (`repo`, `parent_goal_id`, `priority_explicit`,
`wip_refs`, …):

```jsonc
{
  "active": [
    {
      "id": "promote-idea-live-tag-filter",
      "description": "Ship the live tag filter the creative-ideas thread proposed",
      "priority": 2,
      "status": "not-started",
      "assigned_to": null,
      "labels": ["source:creative-ideas", "area:dashboard"],   // NEW
      "wip_refs": []
    },
    {
      "id": "enhance-simard-meeting-experience",
      "description": "Enhance Simard meeting experience",
      "priority": 1,
      "status": "in-progress(20%)"
      // no "labels" key ⇒ this goal has no labels (omitted, per skip_serializing_if)
    }
  ],
  "backlog": [ /* … unchanged … */ ],
  "active_count": 2
}
```

`labels` is omitted for a goal with no labels (matching the serde contract), so
existing `/api/goals` consumers are unaffected. No route, auth, or schema
change beyond the additive field.

### TUI — Goals tab

`simard-tui`'s Goals tab renders each goal's labels as badges and offers a tag
filter, matching the tab's existing styling and reading the same live goal data
(see [How to monitor Simard with the TUI](../howto/monitor-simard-with-tui.md)).

---

## Examples

Tag an operator goal and confirm it round-trips:

```bash
simard goal label enhance-simard-meeting-experience add area:meeting
simard goal label enhance-simard-meeting-experience list
# area:meeting
```

Find every goal that came from a creative idea:

```bash
simard goal list --tag source:creative-ideas
# active goals: 1 / 7 (filtered by tag)
# ID                            PRIORITY  STATUS       ASSIGNED  DESCRIPTION                                   LABELS
# promote-idea-live-tag-filter  p2        not-started  -         Ship the live tag filter …                    source:creative-ideas,area:dashboard
```

Combine tags (AND) — creative-ideas goals about the dashboard:

```bash
simard goal list --tag source:creative-ideas --tag area:dashboard
```

Remove a tag (idempotent — safe to rerun):

```bash
simard goal label promote-idea-live-tag-filter remove area:dashboard
simard goal label promote-idea-live-tag-filter remove area:dashboard   # no-op, exit 0
```

Query provenance over the API:

```bash
curl -s -u "operator:$(cat ~/.simard/.dashkey)" http://localhost:8080/api/goals \
  | jq '.active[] | select((.labels // []) | index("source:creative-ideas")) | .id'
```

---

## Tests

The feature is verified by:

- **Brick unit tests** — `normalize_tag` (trim; empty rejected), `add_label`
  (idempotent, order-preserving), `remove_label` (no-op when absent),
  `matches_all_tags` (AND; empty matches all), `source_for_backlog`.
- **Serde back-compat tests** — a legacy snapshot with no `labels` key
  deserializes to an empty `Vec`; an unlabelled goal re-serializes
  byte-identically (no key), keeping the board snapshot hash stable.
- **Provenance tests, one per production creation path** — creative-ideas →
  `source:creative-ideas`, operator CLI/dashboard add → `source:operator`,
  meeting decision (direct) → `source:meeting`, seed → `source:seed`, and
  backlog promotion via `source_for_backlog` (`overseer:*` → `source:overseer`,
  `meeting:*` → `source:meeting`, `operator:*` → `source:operator`, unrecognized
  → `source:ooda` fallback). Decomposition children inherit the parent's labels
  **plus** `source:decomposition`. A separate test asserts an in-place lifecycle
  move (unblock / reprioritize / progress update) does **not** re-stamp or
  double-stamp.
- **CLI tests** — `label add`/`remove`/`list` round-trip (idempotent add,
  no-op remove), and `goal list --tag` filters (AND for repeated tags,
  trailing `LABELS` column present).
- **Dashboard / TUI tests** — `/api/goals` payload contains `labels`, the
  Goals-tab template renders label markup, and the TUI row-formatting includes
  label text.

---

## Constraints & non-goals

- **Additive / back-compatible only** — no migration; old persisted goals load
  with empty labels. No changes to `kind` / `source` / `owner_identity`, no
  `BacklogItem` labels field, no lifecycle or `MAX_ACTIVE_GOALS` changes.
- **No semantic inference** — topical auto-tagging from goal text is out of
  scope; if added later it must be an agentic step, not a keyword matcher.
- **No `Bridge` identifiers**, no `--admin` / `--no-verify`, and no new stray
  `println!` / `eprintln!` in production code.

---

## Related reading

- [Goal board API reference](./goal-board-api.md) — persistence and the goal
  JSON schema that now includes `labels`.
- [Goal target-repo routing API reference](./goal-target-repo-routing.md) —
  the sibling additive `repo` field this feature's serde pattern mirrors.
- [Goal decomposition & the goal graph](./goal-decomposition.md) — the
  parent↔child edges that sub-goal label inheritance rides on.
- [Goals tab hierarchy & differentiated priorities](./dashboard-goal-hierarchy-priority.md)
  — the Goals-tab rendering the label chips and filter join.
- [Creative-ideas API](./creative-ideas-api.md) — the `route_idea_to_goal`
  promotion path that stamps `source:creative-ideas`.
- [How to label, categorize, and filter goals](../howto/label-and-filter-goals.md)
  — operator walkthrough.
- [`simard goal label` CLI reference](./simard-cli.md#simard-goal-label).
