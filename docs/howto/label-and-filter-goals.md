---
title: How to label, categorize, and filter goals
description: Operator walkthrough for adding, removing, and listing free-form labels (tags) on Simard goals, reading the automatic source:* provenance tags (including source:creative-ideas), and filtering the board by tag from the CLI, the dashboard, and the TUI.
last_updated: 2026-07-06
owner: simard
doc_type: howto
status: howto
related:
  - ../reference/goal-labels.md
  - ../reference/simard-cli.md
  - ../reference/creative-ideas-api.md
  - ../reference/dashboard-goal-hierarchy-priority.md
  - ./monitor-simard-with-tui.md
  - ./configure-creative-ideas-thread.md
---

# How to label, categorize, and filter goals

Every Simard goal carries a list of free-form **labels** (tags) so you can
categorize goals, filter the board, and trace each goal to its origin. Two
things happen for you automatically:

- **Provenance is stamped at creation.** Every goal gets exactly one
  `source:*` tag when it is created — most importantly, a goal promoted from a
  **creative idea** is stamped `source:creative-ideas`, so you can answer
  "which goals came from creative ideas?" at a glance.
- **Everything is back-compatible.** Goals that existed before labels were
  added simply show **no** labels (an empty list) — nothing to migrate.

This guide shows how to read and edit labels and how to filter by tag from the
CLI, the dashboard, and the TUI. For the full contract see the
[Goal labels reference](../reference/goal-labels.md).

## Prerequisites

- [ ] You can reach the `simard goal` CLI (it resolves the same state root as
  the daemon: `$SIMARD_STATE_ROOT`, else `$HOME/.simard`).
- [ ] For dashboard steps, the operator dashboard is running and you have the
  dashboard key (`~/.simard/.dashkey`).

---

## 1. See a goal's labels

```bash
simard goal label enhance-simard-meeting-experience list
```

Prints the goal's tags to stdout, one per line, or `(none)` if it has none:

```text
source:meeting
area:meeting
```

You can also see every goal's labels as a trailing `LABELS` column on the board
listing:

```bash
simard goal list
```

```text
active goals: 2 / 5
ID                              PRIORITY  STATUS       ASSIGNED  DESCRIPTION                          LABELS
promote-idea-live-tag-filter    p2        not-started  -         Ship the live tag filter …           source:creative-ideas,area:dashboard
enhance-simard-meeting-experience  p1     in-progress  -         Enhance Simard meeting experience    source:meeting
```

> The `LABELS` column is appended **after** the existing columns, so scripts
> that parse the first five tab-separated fields keep working unchanged.

---

## 2. Add a label

```bash
simard goal label enhance-simard-meeting-experience add area:meeting
```

- The tag is trimmed; an empty-after-trim tag is rejected with a clear error
  and a non-zero exit.
- **Idempotent.** Adding a tag the goal already has is a successful no-op —
  safe to rerun in scripts.
- Tags are matched **exactly** and are **case-sensitive**: `area:meeting` and
  `Area:Meeting` are different tags.

Confirm it landed:

```bash
simard goal label enhance-simard-meeting-experience list
```

---

## 3. Remove a label

```bash
simard goal label promote-idea-live-tag-filter remove area:dashboard
```

Removing a tag the goal does **not** have is a no-op that still exits `0` (with
a short note on stderr), so this too is safe to rerun.

> Provenance tags (`source:*`) are ordinary tags — you *can* remove one, but
> doing so erases the goal's recorded origin. Prefer to leave `source:*` tags
> in place and add your own `area:…` / topical tags alongside them.

---

## 4. Filter the board by tag

Show only goals that carry a tag:

```bash
simard goal list --tag source:creative-ideas
```

```text
active goals: 1 / 7 (filtered by tag)
ID                            PRIORITY  STATUS       ASSIGNED  DESCRIPTION                 LABELS
promote-idea-live-tag-filter  p2        not-started  -         Ship the live tag filter …  source:creative-ideas,area:dashboard
```

The header annotates the filtered count (`1 / 7 (filtered by tag)`).

`--tag` is **repeatable** and combines with **AND** — a goal must carry *all*
requested tags to appear:

```bash
# creative-ideas goals that are ALSO about the dashboard:
simard goal list --tag source:creative-ideas --tag area:dashboard
```

---

## 5. Find every goal that came from a creative idea

This is the headline use case. Creative-idea promotions are stamped
`source:creative-ideas` automatically, so:

```bash
simard goal list --tag source:creative-ideas
```

lists exactly the goals that originated in the creative-ideas thread. Sub-goals
produced by decomposing such a goal **inherit** the `source:creative-ideas`
tag (and additionally carry `source:decomposition`), so they stay discoverable
too. See [How to configure the creative-ideas thread](./configure-creative-ideas-thread.md)
and the [creative-ideas API](../reference/creative-ideas-api.md).

Over the dashboard API:

```bash
DASHKEY="$(cat ~/.simard/.dashkey)"
curl -s -u "operator:$DASHKEY" http://localhost:8080/api/goals \
  | jq '.active[] | select((.labels // []) | index("source:creative-ideas")) | {id, labels}'
```

The `// []` guard handles goals with no `labels` key (unlabelled goals omit the
field).

---

## 6. Filter on the dashboard

Open the **Goals** tab. Each goal shows its labels as chips next to the
existing status / priority rendering. Use the tab's **tag filter** to narrow
the board to a single tag — for example pick `source:creative-ideas` to see
only the creative-ideas-originated goals. Filtering happens client-side over
the live goal data, so it is instant and adds no new permissions.

---

## 7. Filter in the TUI

Launch the TUI and open the Goals tab:

```bash
simard-tui
```

Each goal renders its labels as badges, and the tab offers a tag filter that
reads the same live goal data. See
[How to monitor Simard with the TUI](./monitor-simard-with-tui.md).

---

## Understanding automatic provenance tags

You never set `source:*` tags by hand — Simard stamps exactly one when a goal
is created:

| A goal created by… | is stamped |
|--------------------|------------|
| a promoted **creative idea** | `source:creative-ideas` |
| `simard goal add` / the dashboard create form | `source:operator` |
| the OODA loop / the Overseer | `source:ooda` / `source:overseer` |
| a meeting decision / meeting goal-curation | `source:meeting` |
| the default seed board | `source:seed` |
| **decomposing** a parent goal | the parent's labels **+** `source:decomposition` |

In-place lifecycle moves — unblocking, reprioritizing, progress updates,
demoting — do **not** change a goal's `source:*` tag. (Promoting a backlog item
to active is its *first* materialization, so it **is** stamped — a demoted goal
comes back as `source:operator`.) See the
[provenance table](../reference/goal-labels.md#provenance-auto-tagging) for the
exact creation sites.

---

## Troubleshooting

### `simard goal label … add` rejected the tag

The tag was empty after trimming (e.g. you passed `""` or only whitespace).
Provide a non-empty tag. Simard imposes no other validation — tags are opaque
tokens.

### A tag I added does not match my filter

Matching is **exact and case-sensitive**. `simard goal list --tag Research`
will not match a goal tagged `research`. Re-check the exact spelling with
`simard goal label <goal-id> list`.

### `label add`/`remove` exits non-zero

The most common cause is an unknown `<goal-id>` — it is not on the active
board. Confirm the id with `simard goal list`. A non-zero exit can also come
from a persistence failure; the daemon does not need to be paused (label
mutations use the same flock-guarded write path as `simard goal add`).

### An old goal shows no labels

Goals created before this feature have an **empty** label list by design (the
change is additive with no migration). They still get provenance stamped only
if they are re-created; existing goals are not back-filled. Add your own tags
with `simard goal label <goal-id> add <tag>`.

---

## Verify end-to-end

```bash
# 1. Add a tag and confirm the round-trip.
simard goal label enhance-simard-meeting-experience add area:meeting
simard goal label enhance-simard-meeting-experience list          # shows area:meeting

# 2. Filter finds it.
simard goal list --tag area:meeting                               # goal appears, count annotated

# 3. Remove is idempotent.
simard goal label enhance-simard-meeting-experience remove area:meeting
simard goal label enhance-simard-meeting-experience remove area:meeting   # no-op, exit 0

# 4. Creative-ideas provenance is queryable.
simard goal list --tag source:creative-ideas
```

---

## Related reading

- [Goal labels / tags API reference](../reference/goal-labels.md) — data model,
  the `goal_curation::labels` brick, provenance, and serde back-compat.
- [`simard goal label` CLI reference](../reference/simard-cli.md#simard-goal-label).
- [Creative-ideas API](../reference/creative-ideas-api.md) — the promotion path
  that stamps `source:creative-ideas`.
- [Goals tab hierarchy & priorities](../reference/dashboard-goal-hierarchy-priority.md)
  — the Goals tab the label chips and filter render into.
- [How to monitor Simard with the TUI](./monitor-simard-with-tui.md).
