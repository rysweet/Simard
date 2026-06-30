---
title: Goal target-repo routing API reference
description: Rust API reference for the per-goal target repository field, the repo-slug resolver, and the spawn-time wiring that routes engineer worktrees (and their PRs) into the correct ecosystem repo.
last_updated: 2026-06-22
owner: simard
doc_type: reference
status: reference
related:
  - ./goal-board-api.md
  - ./goal-coverage-allocation.md
  - ./engineer-worktree-isolation.md
  - ./spawn-agent-for-goal.md
  - ../howto/route-a-goal-to-its-target-repo.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../ecosystem-map.md
---

# Goal target-repo routing API reference

> **Issue [#2359](https://github.com/rysweet/Simard/issues/2359).** Engineers
> spawned for a goal now branch their worktree off the goal's **target
> repository** instead of always off the Simard daemon's own checkout.

Simard is an autonomous OODA daemon that stewards an ecosystem of repos under
`~/src/` (`amplihack-rs`, `RustyClawd`, `agent-kgpacks`,
`amplihack-memory-lib`, …). Each active goal may target one of those repos.
Before #2359, `dispatch_spawn_engineer` always allocated the engineer worktree
from `std::env::current_dir()` — the daemon's own working directory — so every
engineer edited Simard's source and opened PRs against `rysweet/Simard`
regardless of which ecosystem repo the goal was actually about.

This reference documents the three pieces that fix that:

1. The [`ActiveGoal::repo`](#activegoalrepo-field) field — an explicit,
   serde-back-compatible target-repo slug on every goal.
2. The [repo-slug resolver](#repo-slug-resolver) — maps a slug to a validated
   local git-repository path, failing loud when the repo is absent.
3. The [spawn-time wiring](#spawn-time-wiring) — resolves the goal's repo and
   passes that path to `EngineerWorktree::allocate`, so the worktree and every
   `gh` PR command operate against the correct repo.

---

## `ActiveGoal::repo` field

`ActiveGoal` (in `src/goal_curation/types.rs`) carries the target repo as an
optional slug:

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

    /// Ecosystem repository this goal targets, as a repo **slug** (the
    /// directory name under `~/src/`, e.g. `"amplihack-rs"`).
    ///
    /// `None` (the default) means the goal targets the daemon's own repo
    /// ("Simard"). Resolved to a concrete path by
    /// [`resolve_goal_repo`](#resolve_goal_repo) at spawn time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}
```

### Semantics

| `repo` value | Meaning | Resolves to |
|--------------|---------|-------------|
| `None` (field absent) | Goal targets the daemon's own repo | daemon `current_dir()` |
| `Some("Simard")` (case-insensitive) | Explicitly the daemon repo | daemon `current_dir()` |
| `Some("amplihack-rs")` | Goal targets the `amplihack-rs` ecosystem repo | `~/src/amplihack-rs` |
| `Some("<other-slug>")` | Goal targets `<other-slug>` | `~/src/<other-slug>` |

### Serde back-compatibility

The field is `#[serde(default, skip_serializing_if = "Option::is_none")]`:

- **Reading old snapshots:** goal-board JSON written before #2359 has no
  `repo` key. `#[serde(default)]` deserializes those goals with `repo = None`,
  so existing `goal-board:snapshot` facts and any legacy `goal_records.json`
  load unchanged.
- **Writing repo-less goals:** `skip_serializing_if = "Option::is_none"` omits
  the key entirely when `repo` is `None`. A board of repo-less goals
  serializes **byte-identically** to its pre-#2359 form — no spurious
  migration, no diff churn in the snapshot fact.

### `ActiveGoal::new`

Because `ActiveGoal` has no `Default` derive and is constructed at ~40 literal
sites, a constructor centralises field defaults and keeps those sites compiling
when fields are added:

```rust
impl ActiveGoal {
    /// Construct an active goal with the standard defaults
    /// (`status = NotStarted`, no assignment, no wip, no repo).
    pub fn new(id: impl Into<String>, description: impl Into<String>, priority: u32) -> Self;

    /// Builder-style setter for the target repo slug.
    pub fn with_repo(mut self, repo: Option<String>) -> Self;
}
```

`with_repo` is a thin builder convenience over the public `repo` field — it
keeps seed-board construction readable (`ActiveGoal::new(..).with_repo(slug)`);
callers that already hold a `&mut ActiveGoal` may set `.repo` directly instead.
Existing fields and helpers (`concise_label`, etc.) are unchanged, and `repo`
is **not** included in `concise_label` output.

---

## Repo-slug resolver

Module: `simard::ooda_actions::advance_goal::repo_resolver`

The resolver turns a goal's `repo` slug into a validated, absolute path to a
local git repository. It is the **single** authority for goal→path mapping;
nothing else may synthesise a repo path.

### `resolve_goal_repo`

```rust
pub fn resolve_goal_repo(repo: Option<&str>) -> SimardResult<PathBuf>
```

Resolves a goal's target repo to a local git-repository path.

**Resolution order:**

1. **Daemon repo.** If `repo` is `None`, or `Some(slug)` where `slug`
   case-insensitively equals `"simard"`, return `std::env::current_dir()`
   (the daemon's own checkout). No `~/src/` lookup is performed. If
   `current_dir()` itself fails (rare — e.g. the working directory was
   deleted), its `io::Error` is mapped to a `SimardError` and returned as
   `Err`; the resolver never panics.
2. **Ecosystem repo.** Otherwise, read the search root from `$HOME` (a missing
   `HOME` is an `Err`, never a guessed path), validate the slug
   ([`validate_repo_slug`](#validate_repo_slug)), join it under the search
   root `$HOME/src/<slug>`, canonicalize it, confirm it is contained under
   `$HOME/src/`, and confirm it
   [is a git repository](#git-repository-validation). Return the canonical
   path on success.

**Return contract:**

| Outcome | Result |
|---------|--------|
| `None` / `"Simard"` | `Ok(current_dir())` |
| `None` / `"Simard"` but `current_dir()` fails | `Err` — *daemon repo unresolved* |
| Valid slug, repo present & is a git repo | `Ok(<canonical ~/src/slug>)` |
| Slug fails validation (charset, `..`, leading `-`/`.`, length) | `Err(SimardError::…)` describing the rejected slug |
| `HOME` unset (ecosystem lookup) | `Err` — *search root unresolved* |
| Slug valid but `~/src/<slug>` does not exist | `Err` — *missing target repo* |
| Path exists but is **not** a git repo | `Err` — *not a git repository* |
| Canonical path escapes `$HOME/src/` (symlink, traversal) | `Err` — *containment violation* |

> **No silent fallback.** When a targeted repo cannot be resolved, the
> resolver returns `Err`. It **never** falls back to the Simard checkout —
> that silent fallback was the #2359 defect (engineers editing the wrong repo
> and pushing PRs to `rysweet/Simard`). Callers surface the error as a
> [`Blocked` outcome](#spawn-time-wiring) so the operator sees exactly why the
> goal stalled.

### Search root

The search root is `$HOME/src/`, derived from the `HOME` environment variable
at call time (not a hard-coded `/home/azureuser`). Tests override `HOME` to
point at a temp directory so they never touch the real ecosystem repos.

### `validate_repo_slug`

```rust
pub fn validate_repo_slug(slug: &str) -> SimardResult<()>
```

Validates an operator-supplied repo slug before it is used to build a
filesystem path. Modeled on `validate_goal_id`. Rejects anything that could
escape the search root or inject into a git/shell argument:

| Rule | Rejected example |
|------|------------------|
| Charset `^[A-Za-z0-9._-]{1,64}$` | `amplihack rs`, `repo/sub`, `repo$x` |
| No path traversal | `..`, `../etc`, `a/../b` |
| No leading `-` (argv-injection) | `-amplihack-rs`, `--force` |
| No leading `.` (hidden / `.`/`..`) | `.git`, `.` |
| Length 1–64 | empty string, 65-char slug |

The slug is treated as **untrusted input** — it arrives from the
`simard goal add --repo` CLI, the `POST /api/goals` dashboard handler, and
meeting/curation handoffs. Validation runs at every ingress and again inside
`resolve_goal_repo` (defense in depth).

### Git-repository validation

A path counts as a git repository if **either**:

- it contains a `.git` entry — a directory (normal clone) **or** a file (a git
  *worktree* / submodule gitlink); **or**
- `git -C <path> rev-parse --is-inside-work-tree` exits `0`.

The `git` invocation reuses the existing hardened `git_capture` wrapper
(`env_clear()` plus an explicit allowlist), so `GIT_DIR`, `LD_PRELOAD`, and
similar env/argv injection vectors cannot influence the check.

---

## Spawn-time wiring

Module: `simard::ooda_actions::advance_goal::spawn`

`dispatch_spawn_engineer` resolves the goal's target repo **before** allocating
the worktree and uses the resolved path as `parent_repo`:

```rust
// Before #2359 (the bug):
let parent_repo = std::env::current_dir()?;   // always the Simard repo

// After #2359:
let parent_repo = match resolve_goal_repo(goal.repo.as_deref()) {
    Ok(path) => path,
    Err(e) => {
        // Fail loud: mark the goal Blocked rather than silently editing Simard.
        return blocked_outcome(action, goal_id, format!(
            "target repo for goal '{goal_id}' could not be resolved: {e}"
        ));
    }
};

let worktree = EngineerWorktree::allocate(&parent_repo, &state_root, goal_id)?;
```

### What changes for the engineer

- **Worktree location.** `EngineerWorktree::allocate(&parent_repo, …)` branches
  `engineer/<goal-id>-<suffix>` off `<parent_repo>`'s `main`. The worktree
  still lives under `<state_root>/engineer-worktrees/`, but its git history,
  remote, and `gh` repo context are the **target** repo's.
- **PRs.** The engineer subprocess runs `gh pr create` / `gh pr comment` from
  inside the worktree, so PRs are opened against the target repo's remote
  (e.g. `rysweet/amplihack-rs`) — not `rysweet/Simard`. No `gh --repo` override
  is needed; the worktree's remote is authoritative.
- **Task prompt.** The objective handed to `spawn_agent_for_goal` names the
  resolved repo so the agent's plan and any explicit `gh` commands target it.

### Interaction with in-flight de-duplication

Repo resolution does **not** change the existing per-goal de-dup. The order in
`dispatch_spawn_engineer` is unchanged:

1. `assigned_to` board check — skip if a live subordinate is already recorded.
2. `find_live_engineer_for_goal` worktree scan — skip if a live worktree is
   already pursuing this goal (catches engineers the board check missed).
3. **Resolve repo** → allocate worktree → spawn.

A `Blocked` outcome from an unresolved repo records the reason on the goal but
does **not** create a worktree or an assignment, so the next cycle re-evaluates
the goal cleanly once the operator clones the missing repo (or corrects the
slug).

### Blocked outcome shape

When `resolve_goal_repo` errors, the action outcome is `success = false` with a
human-readable summary, and the goal's `status` is set to
`GoalProgress::Blocked(reason)`. The reason is a plain operator-set block — it
carries **no** `OODA-SAFEGUARD` sentinel, so `simard goal unblock-all` does
**not** clear it (that command is scoped to brain-failure markers only). Clear
it with `simard goal unblock <goal-id>` after the repo is available, or correct
the slug with the dashboard / CLI.

---

## Seed goals carry repos

`DEFAULT_SEED_GOALS` (in `src/goal_curation/operations.rs`) is the single
source of truth for both `seed_default_board` (a `GoalBoard`) and
`seed_default_goals` (a `GoalStore`). Its tuple gains a fourth element — the
target-repo slug:

```rust
/// Each tuple: (priority, title, description, repo_slug).
/// `None` repo_slug ⇒ the daemon's own repo ("Simard").
pub const DEFAULT_SEED_GOALS: [(u32, &str, &str, Option<&str>); 5] = [
    (
        1,
        "Improve amplihack test coverage",
        "Increase test coverage across the amplihack ecosystem to catch regressions early",
        Some("amplihack-rs"),                 // ← routed to the amplihack-rs repo
    ),
    (
        2,
        "Enhance Simard meeting experience",
        "Improve the interactive meeting facilitator with better UX and richer handoffs",
        None,                                 // Simard's own repo
    ),
    // … remaining seed goals default to None (Simard) …
];
```

Both consumers destructure the 4-tuple and thread the slug through:
`seed_default_board` sets `ActiveGoal::repo`, and `seed_default_goals` records
the same target on its `GoalRecord`s. The
`improve-amplihack-test-coverage` seed goal now resolves to `~/src/amplihack-rs`
and its engineer opens PRs against `rysweet/amplihack-rs`.

> Any other ecosystem-targeted seed goal must set its slug here. Seed goals
> that genuinely improve Simard itself keep `None`.

---

## Operator ingress

### CLI — `simard goal add`

```text
simard goal add <priority> <description> [--repo <slug>]
```

`--repo <slug>` is optional. When present, the slug is validated with
`validate_repo_slug` and stored on the new goal's `repo` field; when absent the
goal targets Simard (`repo = None`). See the
[CLI reference](./simard-cli.md#simard-goal-add) for the full contract.

### Dashboard — `POST /api/goals`

The create-goal handler accepts an optional `repo` string for **active** goals:

```json
{ "description": "Raise amplihack-rs branch coverage to 80%",
  "priority": 2,
  "status": "active",
  "repo": "amplihack-rs" }
```

The handler performs **shape-only** validation (`validate_repo_slug`) at the
ingress; the existence/git-repo check happens later in `resolve_goal_repo` at
spawn time, so a goal can be created for a repo that will be cloned shortly.
`GET /api/goals` echoes `repo` for any goal that has one (omitted when `None`,
matching the serde contract). The repo field rides the existing dashboard
auth; it adds no new auth surface.

---

## Errors

All resolver errors are `SimardError` variants surfaced verbatim in the
`Blocked` reason and the cycle report:

| Condition | Operator-visible message (shape) |
|-----------|----------------------------------|
| Slug rejected by `validate_repo_slug` | `invalid repo slug '<slug>': <rule>` |
| `~/src/<slug>` missing | `target repo '<slug>' not found at <path>; clone it under ~/src/ or correct the goal's repo` |
| Path exists, not a git repo | `'<path>' is not a git repository` |
| Containment violation | `resolved repo path '<path>' escapes ~/src/` |
| `HOME` unset | `cannot resolve ~/src/: HOME is not set` |
| `current_dir()` fails (daemon repo) | `cannot resolve daemon repo: <io error>` |

---

## Related reading

- [Goal board API reference](./goal-board-api.md) — persistence and the goal
  JSON schema that now includes `repo`.
- [Goal coverage allocation](./goal-coverage-allocation.md) — the companion
  #2359 fix that guarantees every incomplete goal gets an engineer.
- [Engineer worktree isolation](./engineer-worktree-isolation.md) — how
  `EngineerWorktree::allocate` branches off `parent_repo`.
- [How to route a goal to its target repo](../howto/route-a-goal-to-its-target-repo.md)
  — operator walkthrough and troubleshooting.
- [Ecosystem map](../ecosystem-map.md) — the repos under `~/src/` Simard
  stewards.
