---
title: Identity-scoped cognition API reference
description: TOML and Rust reference for identity-scoped cognition (#3125) — the [[identities.seed_goals]] and target_repos identity.toml surface, the seed-goal override at the OODA seeding site (seed_default_board / seed_default_goals / DEFAULT_SEED_GOALS), the read-only observe-only Act phase (posture_permits_spawn deterministic rail + agentic observe-and-propose branch, layered above observe_only_dispatch_refusal), target-scope resolution, fail-closed error behavior, and operator diagnostics.
last_updated: 2026-07-08
owner: simard
doc_type: reference
related:
  - ../concepts/identity-scoped-cognition.md
  - ../concepts/write-authority-posture.md
  - ./write-authority-posture-api.md
  - ./pluggable-identity-api.md
---

# Identity-scoped cognition API reference

!!! warning "Implementation status — this page describes the PLANNED cognition surface (tracking #3125)"
    The `[[identities.seed_goals]]` / `target_repos` TOML fields, the identity
    seed-goal override at the OODA seeding site, and the read-only observe-only Act
    phase are **not yet implemented**; they are the target design tracked in
    [#3125](https://github.com/rysweet/Simard/issues/3125). **What ships today** is
    the env-driven write floor documented under
    [write-authority posture](./write-authority-posture-api.md): `SIMARD_OBSERVE_ONLY=1`
    → `read_only_guard` → `observe_only_dispatch_refusal` inside
    `dispatch_spawn_engineer`. The **shipped** seeding primitives this page extends —
    `DEFAULT_SEED_GOALS`, `seed_default_board`, `seed_default_goals`, the
    `.reseed_goals` marker — exist today and are named exactly as below. Because every
    pluggable-identity TOML struct uses `deny_unknown_fields`, a `[[identities.seed_goals]]`
    or `target_repos` key is a hard parse error against the **current** schema until
    this lands.

Modules: `simard::identity::toml_types`, `simard::identity::manifest`,
`simard::goal_curation`, `simard::goals::seed`, `simard::ooda_loop::cycle`,
`simard::ooda_actions::advance_goal::spawn`

For the rationale and the two-level (write chokepoint vs. cognition) model, see
[Identity-scoped cognition](../concepts/identity-scoped-cognition.md).

---

## `identity.toml` surface

```toml
[[identities]]
name = "crocutus"
default_mode = "engineer"

# Target repos for goals/observations (scope). Optional: defaults to the union
# of the `repo` slugs declared on `seed_goals` below.
target_repos = ["hyenas"]

# Identity seed goals. When present they OVERRIDE Simard's DEFAULT_SEED_GOALS.
[[identities.seed_goals]]
priority = 1
title = "Observe hyenas repo health"
description = "Read the hyenas Azure DevOps repos and assess branch hygiene, CODEOWNERS, LICENSE, dependabot coverage, and large blobs. OBSERVE ONLY."
repo = "hyenas"

# The read-only switch for the observe-only Act phase (write-authority posture).
[identities.authority]
posture = "read-only"          # "read-only" | "scoped-write" | "full"
```

Both new structs are deserialized with `deny_unknown_fields`, matching every
other [pluggable-identity TOML type](./pluggable-identity-api.md); an unexpected
key is a hard `IdentityTomlParseError`.

### `[[identities.seed_goals]]` — `TomlSeedGoal`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `priority` | int (`u32`) | *required* | Seed priority, mirroring the `DEFAULT_SEED_GOALS` tuple's first element. |
| `title` | string | *required* | Goal title (the `id_source` for `goal_slug`). |
| `description` | string | *required* | Goal description shown on the board. |
| `repo` | string | *optional* | Target-repo slug. `None`/omitted means the identity's own repo; a slug scopes the goal to an ecosystem/target repo, exactly like `ActiveGoal.repo`. |

### `target_repos`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `target_repos` | `[string]` | union of `seed_goals[].repo` | The identity's target repo set. Goals and observations are scoped here, never to `rysweet/Simard`. |

!!! note "Relationship to `[identities.authority]`"
    The read-only *switch* is the existing (planned, #3067)
    [`[identities.authority] posture`](./write-authority-posture-api.md#identitytoml-surface)
    field, **not** a new one. The task framing `write_authority = "read-only" | "read-write"`
    maps onto `posture`: `read-write` = `full` (the default, Simard unchanged),
    `read-only` = `read-only`. Reusing `posture` keeps one authority concept rather
    than two parallel fields.

---

## Domain types

Module: `simard::identity::manifest`

```rust
/// One identity-declared seed goal. Mirrors the DEFAULT_SEED_GOALS tuple shape
/// (priority, title, description, Option<repo>) so seeding stays one shape.
pub struct SeedGoal {
    pub priority: u32,
    pub title: String,
    pub description: String,
    pub repo: Option<String>,
}
```

`IdentityManifest` gains two fields, both defaulting empty so built-in and
`full` identities are unchanged:

```rust
pub struct IdentityManifest {
    // ... existing fields (name, version, prompt_assets, components,
    // supported_base_types, required_capabilities, default_mode,
    // memory_policy, contract) ...
    pub seed_goals: Vec<SeedGoal>,   // empty => use DEFAULT_SEED_GOALS
    pub target_repos: Vec<String>,   // empty => union of seed_goals[].repo
}
```

An empty `seed_goals` is the backward-compatible default: seeding falls through
to `DEFAULT_SEED_GOALS`, so Simard's five defaults are untouched.

---

## Seed-goal override

Module: `simard::goal_curation` / `simard::goals::seed`

The single source of truth for Simard's defaults is unchanged:

```rust
// simard::goal_curation — SHIPPED, unchanged by this feature.
pub const DEFAULT_SEED_GOALS: [(u32, &str, &str, Option<&str>); 5] = [ /* ... */ ];

pub fn seed_default_board(board: &mut GoalBoard) -> usize;              // GoalBoard
pub fn seed_default_goals(store: &dyn GoalStore) -> SimardResult<Vec<GoalRecord>>; // GoalStore
```

The override is a **resolver at the seeding site**, not a rewrite of the seeding
functions. The OODA cold-start seeding block in `simard::ooda_loop::cycle`
consults the resolved identity first:

```rust
// Planned (#3125): identity seed goals override the baked-in defaults.
let seeds: Vec<SeedGoal> = if !identity.seed_goals.is_empty() {
    identity.seed_goals.clone()                 // identity-scoped override
} else {
    default_seed_goals()                        // DEFAULT_SEED_GOALS, unchanged
};
```

Precedence at the seeding site (in order):

1. **`.reseed_goals` marker** — forces a fresh seed, ignoring the cognitive-memory
   board (shipped behavior, unchanged).
2. **Non-empty loaded board** from cognitive memory — kept (shipped behavior).
3. **Seed** — identity `seed_goals` if present, else `DEFAULT_SEED_GOALS`.

Seeding only fires when the board is empty, so the override never disturbs an
identity that already has a live board.

---

## Read-only observe-only Act phase

Module: `simard::ooda_actions::advance_goal::spawn`

### Deterministic rail — `posture_permits_spawn`

A pure, default-DENY predicate gates the dispatch branch. It is the thin
deterministic rail that the agentic Act cognition runs behind:

```rust
/// Returns true only for a definitively writing posture (`full` / `scoped-write`).
/// read-only => false. An UNRESOLVED posture under an identity => false
/// (fail-closed). No identity resolves deterministically to `full` => true,
/// so Simard is unaffected.
pub fn posture_permits_spawn(authority: Option<&IdentityAuthority>) -> bool;
```

When `posture_permits_spawn` is false, the Act phase takes the **observe-only
branch** and never calls `dispatch_spawn_engineer`. There are **no wall-clock
timeouts** on the agentic observe/propose step, and **no fallback** to
dispatching if resolution is uncertain.

### Agentic branch — observe and propose

The observe-only branch is agentic: a reasoner/prompt decides what to observe on
the identity's `target_repos` and what goals to propose, then records
observations and writes the proposed goals **to the identity's own board**. This
follows the existing `dispatch_spawn_engineer` pattern of an agentic brain
decision (`OodaBrain::decide_engineer_lifecycle`) paired with a deterministic
safeguard — here the safeguard is `posture_permits_spawn` rather than the
3-strikes counter.

### Layering with the shipped write floor

`observe_only_dispatch_refusal` (shipped, #3071) remains **inside**
`dispatch_spawn_engineer` as the last-line write-primitive floor:

```rust
// SHIPPED — write-primitive chokepoint, kept as defense-in-depth.
fn observe_only_dispatch_refusal(action: &PlannedAction, goal_id: &str) -> Option<ActionOutcome>;
// returns Some(success=true refusal) when read_only_guard::observe_only_enabled().
```

The cognition-level `posture_permits_spawn` rail means a correctly configured
read-only identity **never reaches** that refusal for a write-bearing action —
the two compose as defense in depth (cognition prevents the doomed decision; the
chokepoint guarantees no write if anything slips past).

---

## Target-scope resolution

Observations and proposed goals are scoped to the identity's target set:

- **Explicit** `target_repos` when present.
- Otherwise the **union** of `repo` slugs on `seed_goals`.
- Repo resolution reuses the existing goal-to-repo mapping (the same path used
  by `ActiveGoal.repo` and `DEFAULT_SEED_GOALS`' `Option<repo>` slug), so a
  scoped goal is filed against the target repo, never `rysweet/Simard`.

A read-only identity with an empty target set proposes nothing and dispatches
nothing (fail-closed): it does not fall back to the daemon's own repo.

---

## Error and fail-closed behavior

| Condition | Behavior |
|-----------|----------|
| Unknown field in `[[identities.seed_goals]]` or `target_repos` | `IdentityTomlParseError` (`deny_unknown_fields`) |
| `seed_goals` present | **Override** — replaces `DEFAULT_SEED_GOALS` at seeding (no merge) |
| `seed_goals` empty / absent | `DEFAULT_SEED_GOALS` used unchanged (Simard default) |
| Posture read-only | Act takes observe-only branch; `dispatch_spawn_engineer` **not called** |
| Posture `full` / no identity | Engineer-dispatching Act phase, unchanged |
| Posture **unresolved** under an identity | `posture_permits_spawn` = false → **no spawn** (fail-closed) |
| Read-only identity reaches `dispatch_spawn_engineer` anyway | `observe_only_dispatch_refusal` refuses, fail-closed (shipped floor) |

No branch degrades to a silent no-op or a silent fallback; read-only always
fails **closed**.

---

## Operator diagnostics

Following the repo's `[simard]` eprintln operator-diagnostic convention, the
seeding site and Act phase surface which path they took, e.g.:

```
[simard] OODA start: seeded 2 identity seed goal(s) (crocutus) — overriding defaults
[simard] OODA start: seeded 5 default goal(s)
[simard] Act: read-only posture (crocutus) — observe-only branch, proposed 3 goal(s), dispatched 0 engineer(s)
```

The default (no identity) line is the existing
`[simard] OODA start: seeded {n} default goal(s)` message — unchanged.

---

## See also

- [Identity-scoped cognition](../concepts/identity-scoped-cognition.md) — the
  design rationale and the write-chokepoint-vs-cognition model.
- [Write-authority posture API reference](./write-authority-posture-api.md) — the
  `[identities.authority] posture` field this feature reuses as the read-only switch.
- [Pluggable identity API reference](./pluggable-identity-api.md) — the
  `identity.toml` loader and TOML types these fields extend.
- [Deploy Crocutus as a read-only observer](../tutorials/deploy-crocutus-read-only-observer.md)
  — the end-to-end worked example.
