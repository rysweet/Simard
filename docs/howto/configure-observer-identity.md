---
title: How to configure a read-only observer identity
description: Give an identity a read-only write-authority posture, target repo set, and its own seed goals so its OODA Act phase observes and proposes goals instead of dispatching write-bearing engineers — using the Crocutus/hyenas observer as the worked example.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./configure-pluggable-identity.md
  - ./route-a-goal-to-its-target-repo.md
  - ./grant-engineer-write-permissions.md
  - ../reference/identity-posture-observer.md
  - ../concepts/identity-write-authority.md
---

# How to configure a read-only observer identity

An **observer** identity drives the OODA daemon in read-only mode: its Act phase
records observations about a set of **target** repositories and proposes
repo-hygiene goals to its own board, but it **never** dispatches a write-bearing
engineer and **never** writes to any target repo. This is enforced at the
cognition level, not only at the write primitive — see
[Identity write-authority](../concepts/identity-write-authority.md) for the why.

This guide uses the **Crocutus** identity observing the **hyenas** Azure DevOps
repos as the worked example. Crocutus consumes this feature through
configuration only — all logic lives in Simard.

## Prerequisites

- Simard binary built (`cargo build --quiet`).
- A working [pluggable identity](./configure-pluggable-identity.md) setup
  (an `identity.toml` under a prompt-root-relative identity directory).
- The target repos are reachable for read (clone/`gh`/`az repos` access).

## Step 1 — Declare a read-only posture

Add `write_authority = "read-only"` to your `[[identities]]` entry. The default
is `"read-write"`, so you must set this explicitly to opt into observe-only.

```toml
[package]
name = "crocutus-identity"
version = "0.1.0"
description = "Read-only observer of the hyenas repos"

[[identities]]
name = "crocutus-observer"
default_mode = "curator"
write_authority = "read-only"
```

| Value | Effect |
|-------|--------|
| `"read-write"` (default) | Full authority. Act phase dispatches engineers (Simard's normal behavior). |
| `"read-only"` | Observe-only. Act phase records observations + proposes goals; engineer dispatch is hard-blocked. |

!!! note "The operating mode does not gate observe-only"
    The observe-only Act branch is selected by `write_authority`, **not** by
    `default_mode`. Use whichever mode best fits your persona (`curator` reads
    naturally for an observer).

## Step 2 — List the target repos

`targets` is the identity's **observe scope** — the repos it may look at. It is
never used as a write scope.

```toml
[[identities]]
name = "crocutus-observer"
default_mode = "curator"
write_authority = "read-only"
targets = [
  "hyenas/infra",
  "hyenas/services",
  "hyenas/webapp",
]
```

`targets` slugs are the identity's observe scope and the allowed scope for its
read-only seed goals (every read-only seed goal's `repo` must be one of them —
Step 3). They are not used as a write scope, and no engineer is dispatched
against them, so an observer never writes to a target repo regardless of slug
shape.

## Step 3 — Declare target-scoped seed goals

Seed goals declared by an identity **replace** Simard's five baked-in
`DEFAULT_SEED_GOALS`. For a read-only identity, **every** seed goal must name a
`repo` that is within `targets` — this is what keeps the observer pointed at the
hyenas and never at `rysweet/Simard`.

```toml
[[identities.seed_goals]]
priority = 90
title = "Observe branch hygiene"
description = "OBSERVE ONLY: report stale/unmerged branches and missing branch protection."
repo = "hyenas/infra"

[[identities.seed_goals]]
priority = 80
title = "Observe governance files"
description = "OBSERVE ONLY: report missing CODEOWNERS and LICENSE files."
repo = "hyenas/services"

[[identities.seed_goals]]
priority = 70
title = "Observe dependency & blob hygiene"
description = "OBSERVE ONLY: report absent dependabot config and large binary blobs in history."
repo = "hyenas/webapp"
```

| Field | Required? | Notes |
|-------|-----------|-------|
| `priority` | yes | Integer; higher runs first. |
| `title` | yes | Short goal title. |
| `description` | yes | Prefix with `OBSERVE ONLY:` by convention. |
| `repo` | yes (read-only) | Must be one of the `targets` slugs. Missing or out-of-`targets` ⇒ hard error. |

!!! warning "Fail-closed scoping"
    A read-only seed goal with no `repo`, or a `repo` outside `targets`, is a
    **hard `IdentityTomlParseError`** at load time. Simard will **not** silently
    scope it to its own repo. Fix the config — do not rely on a default.

## Step 4 — (Advanced) The observe reasoner

The observe-only Act phase runs behind the `ObserveOnlyBrain` trait — the seam
where *what* to observe and *which* goals to propose is decided. The wired
baseline is a deterministic floor (`DeterministicObserveBrain`): it records that
it inspected the identity's `targets` and proposes no new goals beyond the seed
goals already placed on the board at boot. This mirrors the deterministic-floor
pattern used elsewhere in the OODA brain.

A prompt-backed `ObserveOnlyBrain` (an LLM reasoner that inspects live repo
health and proposes goals) is a forward-looking extension: the trait makes it
pluggable without touching the deterministic no-spawn rail. Until one is wired,
declare the goals you want observed directly as `seed_goals` (Step 3) — that is
the supported way to steer Crocutus today.

## Step 5 — Run the daemon under the identity

Point the daemon at the identity via the existing environment plumbing
(`SIMARD_IDENTITY_PATH` / `SIMARD_IDENTITY`). Resolution happens **once at
boot**:

```bash
export SIMARD_IDENTITY_PATH="$PWD/.simard/identity"
export SIMARD_IDENTITY="crocutus-observer"
simard daemon run
```

Confirm the posture in the boot diagnostics (`[simard]` eprintln convention):

```text
[simard] OODA daemon: identity write posture = read-only (targets: hyenas/infra, hyenas/services, hyenas/webapp; identity seed goals: 3) (issue #3125)
[simard] OODA start: seeded 3 identity goal(s) — overriding defaults, target-scoped observe-only (issue #3125)
```

If you instead see `posture = read-write` or the five Simard defaults being
seeded, the identity did not resolve — re-check `SIMARD_IDENTITY_PATH`,
`SIMARD_IDENTITY`, and that the identity name matches.

## Step 6 — Verify observe-only behavior

Watch the daemon through one or more cycles and confirm:

1. **Goals are the identity's, target-scoped.** The board shows your three
   hyenas goals, each with its `repo` slug — not Simard's defaults.
2. **No engineers are dispatched.** The Act phase records observations and may
   append proposed goals, but the deterministic rail refuses any spawn:

   ```text
   [simard] spawn_engineer BLOCKED for goal 'observe-branch-hygiene': identity posture is read-only (deny-by-default: only read-write may dispatch engineers) — no write-bearing engineer dispatched (issue #3125)
   ```

   These skips are **success no-ops** — they do not trip the 3-strikes
   brain-failure safeguard or block the goal.
3. **No target-repo writes.** No worktrees, branches, or PRs are created against
   any `targets` repo.

## Fail-closed behavior reference

| Condition | Behavior |
|-----------|----------|
| `write_authority` absent | Defaults to `read-write` (Simard unchanged). |
| `write_authority = "read-only"` | Observe-only Act phase; engineer dispatch hard-blocked. |
| Identity present but posture unresolvable | Fails **closed** to read-only / no-spawn. |
| Read-only seed goal missing `repo` / `repo` ∉ `targets` | Hard `IdentityTomlParseError` at load. |
| Invalid `write_authority` string | Hard `IdentityTomlParseError`. |
| No identity (`SIMARD_IDENTITY_PATH` unset) | Simard defaults: five seed goals + engineer-dispatching Act phase. |

## Full example: crocutus `identity.toml`

```toml
[package]
name = "crocutus-identity"
version = "0.1.0"
description = "Read-only observer of the hyenas repos"

[[identities]]
name = "crocutus-observer"
default_mode = "curator"
write_authority = "read-only"
targets = ["hyenas/infra", "hyenas/services", "hyenas/webapp"]

[[identities.seed_goals]]
priority = 90
title = "Observe branch hygiene"
description = "OBSERVE ONLY: report stale/unmerged branches and missing branch protection."
repo = "hyenas/infra"

[[identities.seed_goals]]
priority = 80
title = "Observe governance files"
description = "OBSERVE ONLY: report missing CODEOWNERS and LICENSE files."
repo = "hyenas/services"

[[identities.seed_goals]]
priority = 70
title = "Observe dependency & blob hygiene"
description = "OBSERVE ONLY: report absent dependabot config and large binary blobs in history."
repo = "hyenas/webapp"
```

## Related

- Reference: [Identity write-authority & observer posture API](../reference/identity-posture-observer.md)
- Concept: [Identity write-authority — cognition-level observer posture](../concepts/identity-write-authority.md)
- [How to configure pluggable identities](./configure-pluggable-identity.md)
- [Route a goal to its target repo](./route-a-goal-to-its-target-repo.md)
