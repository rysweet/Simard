# Governed-Repo Roster Charter for Simard

## Status

- **Created**: 2026-07-22
- **State**: RATIFIED — adopted as the goal's machine-checkable done-gate via
  escalation triage (`rewrite-done-gate`). The disambiguation (§1), the
  measurable done-criteria (§2), and the deterministic next-target procedure
  (§3) are in force. This charter is prose + acceptance criteria only; it does
  not itself change any Rust or CI behavior.
- **Governs goal slug**: `move-the-governed-repo-roster-out-of-framework-a8f57a50`
  (the recurring blocked goal this charter's `rewrite-done-gate` triage
  re-points here).
- **Done when**: the single acceptance test in §2 passes — specifically, a PR
  that satisfies all three roster properties is observed **MERGED** and the
  guard test module `governed_repo_roster` is green.
- **Related identity model**:
  [`docs/concepts/identity-scoped-cognition.md`](../docs/concepts/identity-scoped-cognition.md),
  `src/identity/manifest.rs` (`IdentityManifest.target_repos`, `seed_goals`).

This file is the **canonical written charter** for the recurring
"move the governed-repo roster out of framework code and into Simard's identity"
goal. It exists so that any future operator, engineer agent, or OODA cycle that
picks up the goal gets a single answer to three questions that were previously
unanswered:

1. **What does the goal mean, precisely?** (§1)
2. **When is it DONE, measured how?** (§2)
3. **What is the concrete next step if it is not yet done?** (§3)

## Why this charter exists now

The goal cycled and was marked blocked. Restated in plain English: Simard could
not automatically tell when this goal was finished. Its finish line was a
multi-part prose wish — "move the roster into the identity, make it mutable,
make it survive a deploy, and remove the code coupling" — with **nothing a
done-gate could certify**. Each cycle re-read the same open-ended description,
found no observable finish condition, and either restarted or wedged. On the
most recent attempt the assigned worker got stuck on a dead worktree that still
held a cognitive-store lock, so the safeguard marked the goal blocked.

Root cause, in one sentence: **an unmeasurable multi-part finish line combined
with a worker wedged on a stale worktree holding a cognitive-store lock.** The
fix is to replace the prose wish with one machine-checkable acceptance test and
clear the wedge so a fresh worker can act — exactly what this charter does.

## 1. What the goal means (disambiguation)

Today the roster of repositories Simard stewards lives in
`prompt_assets/simard/ecosystem_repos.toml` — a **git-tracked framework file**
with Rust bound to it (`src/overseer/ecosystem_observe.rs` load/parse), and
every self-deploy re-installs `prompt_assets/` from the repo (see
`~/.simard/.install-backups/prompt_assets.*.bak`), clobbering any runtime edit.
So Simard cannot durably steward her own roster.

The goal is to move the roster **out of framework code and into Simard's
identity** as agentically-curated, mutable, deploy-durable state — the way the
example identities carry their own data (a Gastronome identity carries
menus/events; Simard carries the set of repos she stewards). The identity system
already models this: `IdentityManifest` carries per-identity `target_repos` and
`seed_goals` (`src/identity/manifest.rs`), and identity-scoped cognition exists
(`docs/concepts/identity-scoped-cognition.md`).

**In scope.** Seed the roster from the identity; own it as mutable
identity-scoped state Simard curates agentically; keep it across self-deploys;
provide a *generic* identity-scoped-mutable-data mechanism (repos for Simard,
menus for Gastronome) rather than a hardcoded "ecosystem roster" concept in
code; re-point `ecosystem_observe` and the prose that references the roster at
the identity-curated source; keep exactly one source of truth.

**Out of scope.** Changing which repos Simard stewards; re-designing the OODA
loop; any change to the escalation seam or CI hard gates.

## 2. Measurable done-criteria (the machine-checkable acceptance test)

The goal is **DONE** when a single PR is observed **MERGED** on `rysweet/Simard`
that makes **all four** of the following hold, each observable from a file or
from command output — and the guard test module stays green:

```bash
# The single machine-checkable acceptance gate for this goal.
cargo test -p simard governed_repo_roster
```

1. **Seeded from the identity.** With a fresh Simard identity and **no**
   git-tracked `ecosystem_repos.toml` present, the resolved roster equals the
   identity's declared repos (`IdentityManifest.target_repos` / a Simard
   identity package). *Observable:* a hermetic test constructs an identity with
   a known `target_repos`, resolves the roster, and asserts equality — no read
   of a framework file.

2. **Mutable by Simard at runtime.** Simard can add or remove a stewarded repo
   through her own agentic reasoning at runtime, and the change persists to
   identity-scoped state (SIMARD_HOME identity state or identity-scoped
   cognitive memory), not to a git-tracked file. *Observable:* a test adds a
   repo at runtime, re-resolves the roster, and asserts the addition is present;
   removing it and re-resolving asserts it is gone.

3. **Survives a self-deploy without being reset.** Running the `install` /
   self-deploy path does **not** overwrite the runtime roster. *Observable:* a
   test seeds a runtime-added repo, simulates the install/prompt_assets
   re-install path, re-resolves the roster, and asserts the runtime addition
   still present (no clobber).

4. **Exactly one source of truth.** The framework-level committed
   `ecosystem_repos.toml` coupling is removed or superseded, and
   `ecosystem_observe`, the agentic merge-queue-reasoning scope, and the prose
   in `engineer_system.md` / `ecosystem-map.md` all read the identity-curated
   roster. *Observable:* a repo-wide check finds no remaining code path that
   loads the roster from the committed framework file as the source of truth
   (the old file may remain only as a one-time seed/migration, not as the live
   source).

This replaces the previous unmeasurable finish line ("move the roster into the
identity …") with one gate a done-gate can certify: **the module
`governed_repo_roster` is green and the delivering PR is MERGED.**

## 3. Deterministic next-target procedure

If §2 is not yet satisfied, the next step is deterministic — do the smallest
increment that turns one un-satisfied bullet green, in this order:

1. Add an identity-scoped roster accessor that resolves from
   `IdentityManifest.target_repos` (bullet 1), behind the guard test module.
2. Add a runtime mutation path that persists to identity-scoped state
   (bullet 2).
3. Make the install / prompt_assets re-install path skip the runtime roster
   (bullet 3).
4. Re-point `ecosystem_observe` + prose to the identity source and retire the
   framework-file coupling to a one-time seed (bullet 4).

Land each increment as an additive, CI-green, crusty-reviewed, merge-ready PR;
notify on merge. When all four bullets are green and the delivering PR is
MERGED, the goal is complete.
