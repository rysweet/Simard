---
title: How to route a goal to its target ecosystem repo
description: Operator walkthrough for targeting an active goal at a specific ecosystem repo under ~/src/ so its engineer branches the worktree and opens PRs in the correct repository.
last_updated: 2026-06-22
owner: simard
doc_type: howto
status: howto
related:
  - ../reference/goal-target-repo-routing.md
  - ../reference/goal-coverage-allocation.md
  - ../reference/simard-cli.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../ecosystem-map.md
---

# How to route a goal to its target ecosystem repo

By default a Simard goal targets the **daemon's own repo** (`Simard`). When a
goal is really about an ecosystem repo Simard stewards — `amplihack-rs`,
`RustyClawd`, `agent-kgpacks`, `amplihack-memory-lib`, … — set the goal's
**target repo** so its engineer branches the worktree off that repo and opens
PRs against *that* remote instead of `rysweet/Simard`.

This guide covers the three ways to set a goal's repo, how the daemon resolves
the slug to a path, and how to recover when a targeted repo is missing.

## Prerequisites

- [ ] The target repo is cloned (or will be cloned) under `~/src/<slug>`. The
  slug is exactly the directory name, e.g. `~/src/amplihack-rs` ⇒ slug
  `amplihack-rs`.
- [ ] You can reach the `simard goal` CLI or the operator dashboard.
- [ ] For the daemon to actually spawn engineers there, the resolved path must
  be a real git repo (a `.git` directory **or** a git-worktree gitlink file).

---

## 1. Add a repo-targeted goal from the CLI

```bash
simard goal add 2 "Raise amplihack-rs branch coverage to 80%" --repo amplihack-rs
```

- `--repo <slug>` is optional. Omit it and the goal targets Simard.
- The slug is validated immediately (charset `^[A-Za-z0-9._-]{1,64}$`, no
  `..`, no leading `-` or `.`). An invalid slug is rejected before the goal is
  written.
- The new goal's `repo` field is persisted on the goal-board snapshot.

Confirm it landed:

```bash
simard goal list
```

The goal appears on the active board. Its engineer — spawned on a subsequent
OODA cycle — will branch `engineer/<goal-id>-<suffix>` off `~/src/amplihack-rs`
and open its PR against `rysweet/amplihack-rs`.

> **Existence is checked at spawn time, not add time.** You may add the goal
> before cloning the repo; the daemon validates the path when it tries to spawn
> the engineer (see [troubleshooting](#troubleshooting) below).

---

## 2. Add a repo-targeted goal from the dashboard

`POST /api/goals` accepts an optional `repo` string for active goals:

```bash
DASHKEY="$(cat ~/.simard/.dashkey)"
curl -s -u "operator:$DASHKEY" \
  -X POST http://localhost:8080/api/goals \
  -H 'content-type: application/json' \
  -d '{
        "description": "Raise amplihack-rs branch coverage to 80%",
        "priority": 2,
        "status": "active",
        "repo": "amplihack-rs"
      }'
```

`GET /api/goals` echoes the `repo` field for any goal that has one (it is
omitted for goals that target Simard).

---

## 3. Seed goals are pre-routed

The default seed board already routes ecosystem-targeted goals. The
`improve-amplihack-test-coverage` seed goal carries `repo = "amplihack-rs"`, so
a freshly seeded daemon sends its amplihack test-coverage engineer to
`~/src/amplihack-rs` automatically. Seed goals that improve Simard itself keep
the default (`None`).

You normally do not edit seed goals as an operator; to reseed see
[How to recover a corrupted or missing goal board](./recover-goal-board.md).

---

## How resolution works

When the daemon spawns an engineer for a goal, it resolves the goal's `repo`
slug to a local path:

| `repo` | Resolves to |
|--------|-------------|
| absent / `None` | the daemon's own checkout (`current_dir()`) |
| `"Simard"` (any case) | the daemon's own checkout |
| `"amplihack-rs"` | `~/src/amplihack-rs` (validated as a git repo) |
| `"<slug>"` | `~/src/<slug>` (validated as a git repo) |

`~/src/` is derived from `$HOME`. The resolved path is canonicalized and must
stay inside `~/src/`. If the repo is missing or is not a git repo, the daemon
**marks the goal `Blocked`** rather than silently editing Simard — see the
[routing reference](../reference/goal-target-repo-routing.md#errors) for the
exact error shapes.

---

## Troubleshooting

### Goal went `Blocked` with "target repo … not found"

The slug is valid but `~/src/<slug>` does not exist. Clone the repo and unblock
the goal:

```bash
git clone git@github.com:rysweet/amplihack-rs.git ~/src/amplihack-rs
simard goal unblock <goal-id>
```

On the next cycle the resolver succeeds and an engineer is spawned in the
correct repo.

> Use `simard goal unblock <goal-id>` (single-goal). `simard goal unblock-all`
> only clears the `🔒 [OODA-SAFEGUARD]` brain-failure marker and will **not**
> clear a repo-routing block.

### Goal went `Blocked` with "is not a git repository"

`~/src/<slug>` exists but has no `.git` entry. You probably created an empty
directory or the clone failed. Remove it and re-clone:

```bash
rm -rf ~/src/<slug>
git clone <remote-url> ~/src/<slug>
simard goal unblock <goal-id>
```

### Goal went `Blocked` with "invalid repo slug"

The slug contained an illegal character, a path separator, `..`, or a leading
`-`/`.`. Correct it — re-add the goal with a valid `--repo` value, or fix it via
the dashboard. The slug must be the bare directory name under `~/src/`.

### Engineer still opened a PR against `rysweet/Simard`

Confirm the goal actually has a `repo` set:

```bash
simard goal list            # or:  curl … /api/goals | jq '.active[] | {id, repo}'
```

If `repo` is absent the goal targets Simard by design. Add `--repo <slug>` (or
remove and re-add the goal with the slug). If `repo` is set correctly but the
PR still landed in Simard, check the daemon log for a coverage/spawn line and a
resolver error around that goal id.

---

## Verify end-to-end

1. Add a repo-targeted goal (step 1 or 2).
2. Watch the daemon log for the spawn line naming the resolved repo path:
   ```
   [simard] spawn_engineer for goal '<id>': worktree allocated in /home/<you>/src/amplihack-rs
   ```
3. Confirm the PR opened by the engineer is against the target repo:
   ```bash
   gh pr list --repo rysweet/amplihack-rs
   ```

---

## Related reading

- [Goal target-repo routing API reference](../reference/goal-target-repo-routing.md)
- [Goal coverage allocation](../reference/goal-coverage-allocation.md) — ensures
  every incomplete goal (repo-targeted or not) gets an engineer each cycle.
- [How OODA spawns engineer agents](./spawn-engineers-from-ooda-daemon.md)
- [Ecosystem map](../ecosystem-map.md)
