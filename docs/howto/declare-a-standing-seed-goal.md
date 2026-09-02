---
title: How to declare a standing (perpetual) seed goal
description: Mark a seed goal `standing = true` so it is treated as perpetual — exempt from the no-progress breaker's re-parking and issue-storm, never marked complete — and verify the live goal self-heals via reconcile_standing_markers (#4927).
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/standing-seed-goal-declaration-api.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../concepts/identity-scoped-cognition.md
  - ../howto/configure-pluggable-identity.md
  - ../howto/diagnose-a-no-progress-breaker-issue-storm.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../reference/no-progress-breaker-api.md
---

# How to declare a standing (perpetual) seed goal

Some goals never "finish" — they run every OODA cycle by design: repo-hygiene
backlogs, CI stewardship, continuous research. If such a goal is seeded as an
ordinary (convergence-required) goal, the **no-progress breaker** mistakes its
lack of a terminal state for a livelock, re-parks it each cycle, and files a
storm of `goal stuck after guided retry (UNCLEAR-CRITERIA)` issues (the #4927 /
#4930 / #4934 pattern).

Declaring the goal **standing** with a single field fixes this: a standing goal
reads as `is_perpetual()` and is exempt from the breaker's re-parking and
issue-filing, and is never marked `Completed`. This guide shows how to declare
one, and how to confirm an already-running goal self-heals.

## When to use `standing = true`

Use it for a genuinely perpetual, non-terminating goal:

- a repo-hygiene / stewardship backlog that re-derives work each cycle,
- a continuous CI-health or research goal.

**Do not** use it for a bounded goal that has a definition of done — those should
keep converging and should trip the breaker if they livelock. `standing` is an
opt-in escape from the safety breaker; apply it only to goals that are perpetual
by design.

## Option A — declare it in an identity manifest (TOML)

Add `standing = true` to the `[[identities.seed_goals]]` entry in your
`identity.toml` (see [configure pluggable identities](./configure-pluggable-identity.md)
for the file's location and structure):

```toml
[[identities.seed_goals]]
priority = 2
title = "Articulate repo-hygiene backlog"
description = "Turn observations into prioritized, target-scoped repo-hygiene goals on this identity's own board."
repo = "hyenas"
standing = true          # ← declares this goal perpetual
```

Notes:

- The field is **optional** and defaults to `false`. Existing manifests that omit
  it are unchanged — this is strictly additive.
- The manifest keeps `deny_unknown_fields`, so a typo (e.g. `standng = true`)
  fails loudly at load rather than silently leaving the goal non-perpetual. If
  Simard refuses to start after your edit, check the flag spelling.

## Option B — declare it in Rust seed goals

If you build `SeedGoal` values in code, use the `.standing()` builder:

```rust
use crate::identity::SeedGoal;

let goals = vec![
    SeedGoal::new(
        2,
        "Articulate repo-hygiene backlog",
        "Turn observations into prioritized, target-scoped repo-hygiene goals.",
        Some("hyenas".into()),
    )
    .standing(), // ← declares this goal perpetual
];
```

The `SeedGoal::new(...)` signature is unchanged (four arguments, `standing`
defaults to `false`); `.standing()` is the opt-in.

## What happens

1. **Cold start (empty board).** `seed_board_from_seed_goals` applies the durable
   standing marker (`[standing] `) to the goal's description as it creates the
   `ActiveGoal`, so `is_perpetual()` returns `true` from the first cycle.
2. **Warm board (goal already persisted).** On every cycle, right after the board
   is loaded, `reconcile_standing_markers` stamps the standing marker onto any
   already-persisted goal whose **exact id or normalized title-slug** matches a
   `standing = true` seed. This self-heals a goal that a pre-#4927 build persisted
   without the marker — no need to reseed or delete the board. The seed set is
   resolved **once per cycle** and reused for both cold seeding and this
   reconcile, so the two paths always agree.
3. **Effect.** From then on the goal is exempt from the no-progress breaker
   (no re-parking, no `ooda-stuck` issue) and is never marked `Completed`.

## Reverting a standing declaration

The declaration is **reversible** without wiping the board, but reversal must be
**explicit**. Set the seed's flag to `standing = false` (in Rust, use the
`.non_standing()` builder) — do **not** just delete the seed or drop the flag:

```toml
[[identities.seed_goals]]
priority = 2
title = "Articulate repo-hygiene backlog"
description = "…"
standing = false          # ← explicit reversal (must be present)
```

```rust
// Rust: the explicit-false builder — NOT merely omitting `.standing()`.
SeedGoal::new(2, "Articulate repo-hygiene backlog", "…", Some("hyenas".into()))
    .non_standing();
```

On the next cycle `reconcile_standing_markers` strips the leading `[standing] `
marker it previously added and the goal converges (and trips the breaker) again.
The reversal is deliberately conservative:

- It removes **only a leading `[standing] ` marker**, and **only** from a goal
  carrying the exact `source:seed` label — i.e. one this seeding path created. A
  user-created goal that merely shares the slug is never demoted.
- **Only an *explicit* `standing = false` reverses.** An **omitted** flag is
  inert: a seed that simply leaves `standing` out (the default from
  `SeedGoal::new`) never strips a marker. This is the three-state distinction —
  omitted, explicit true, and explicit false are all preserved distinctly, so an
  ordinary non-standing seed can never accidentally demote a perpetual goal.
- **Deleting a seed does not reverse anything.** A removed seed leaves its goal
  untouched (so an accidental manifest edit can't silently re-arm a safety
  breaker on a goal you meant to keep perpetual). Reversal happens only when the
  seed is still present *and* carries an explicit `standing = false`.
- **Standing *phrases* in the prose are never edited.** If a goal's description
  independently reads as standing (e.g. it literally contains "standing goal"),
  stripping the leading marker leaves it perpetual — only the sentinel prefix is
  ever removed.

See the [standing seed-goal declaration API reference](../reference/standing-seed-goal-declaration-api.md)
for the exact types and functions.

## Verify

**A running goal self-heals to standing.** After deploying the declaration, watch
one cycle of the OODA daemon (see [run the OODA daemon](./run-ooda-daemon.md)).
The cycle logs a bounded line with the reconcile counts (added/removed only) when
it stamps or reverses a goal:

```console
$ simard status --goals
p2 [not-started] [standing] Articulate repo-hygiene backlog …
```

The `[standing] ` prefix on the description confirms `is_perpetual()` is now
`true`. The goal stays `not-started`/active across idle cycles instead of
flipping to `blocked: 🔒 [OODA-SAFEGUARD] … needs human review`.

**No new issue storm.** Confirm the breaker stops filing stuck-goal issues for
this goal:

```console
$ gh issue list --repo rysweet/Simard --search "articulate-repo-hygiene UNCLEAR-CRITERIA" --state open
```

After the fix there should be no *new* entries for this goal. Existing issues
(#4927/#4930/#4934) are historical and are not auto-closed by this change.

**Ordinary goals still converge.** A goal *without* `standing = true` behaves
exactly as before: it re-parks after `NO_PROGRESS_BREAKER_THRESHOLD` (3) no-action
cycles and files an issue. The declaration changes nothing for ordinary goals.

## Troubleshooting

- **Simard won't start after the edit.** A misspelled `standing` field is
  rejected by `deny_unknown_fields`. Fix the spelling.
- **The live goal still re-parks.** Confirm the running identity manifest (not
  just this repo's copy) declares `standing = true`, and that the goal's id or
  title-slug **exactly** matches the seed — reconcile matches exactly, never
  fuzzily. As a fallback you can force a re-seed from defaults with the
  `.reseed_goals` marker (see
  [unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)).
- **An unexpected goal became standing.** Only goals whose exact id/slug matches a
  `standing` seed are marked. Check which seed matched; remove `standing = true`
  from that seed if it should converge.

## Related

- [Standing seed-goal declaration API reference](../reference/standing-seed-goal-declaration-api.md)
- [Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
- [Diagnose a no-progress breaker issue storm](./diagnose-a-no-progress-breaker-issue-storm.md)
- [Configure pluggable identities](./configure-pluggable-identity.md)
- [Unblock OODA goals stuck after a safeguard lockout](./unblock-stuck-ooda-goals.md)
