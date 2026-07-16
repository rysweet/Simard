---
title: Ecosystem-roster path resolution
description: >
  How the ecosystem-observe rail resolves the stewarded roster
  (ecosystem_repos.toml) install-first — from ~/.simard/prompt_assets before the
  in-tree checkout — so the rail wires live on a deployed daemon whose repo_root
  is a source (or stale) checkout. Documents resolve_ecosystem_roster_path, the
  resolution ladder it shares with the recipe resolver, the fail-visible /
  fail-open wiring contract in build_ecosystem_observer, and how to verify it.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ../design/ecosystem-observe.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ./recipe-brain-api.md
  - ../fail-open-audit.md
  - ./state-root-resolution.md
---

# Ecosystem-roster path resolution

> **Status: current.** This page is the contract for how the
> `ecosystem-observe` rail finds its stewarded roster
> (`ecosystem_repos.toml`). The rail resolves the roster **install-first**:
> `~/.simard/prompt_assets/simard/ecosystem_repos.toml` (the deployed install
> location) is preferred over `<repo_root>/prompt_assets/simard/ecosystem_repos.toml`
> (the in-tree checkout). This mirrors, exactly, how the rail already resolves
> its recipe (`resolve_observe_recipe_path`), so both prompt-assets live under
> one resolution ladder.

The rail is otherwise unchanged: it loads the roster, applies the Overseer
cadence, spawns `recipe-runner-rs` on `ecosystem-observe.yaml`, and forwards the
agent's opaque result. See [Ecosystem Observe](../design/ecosystem-observe.md)
for the end-to-end chain. This page documents **only** the roster path
resolution and the wiring behavior around it.

---

## Why install-first

A deployed Simard daemon runs with `WorkingDirectory=~/.simard`, and the
prompt-assets tree it actually reads from is the **installed** one at
`~/.simard/prompt_assets/`. The daemon's `repo_root`, however, points at a
**source checkout** — which on a deployed host may be a *stale* deploy directory
that predates the roster file entirely (for example `…/Simard-deploy-4049`).

The recipe path already accounts for this: `resolve_observe_recipe_path` checks
the `~/.simard` install location first and only falls back to `repo_root`. The
roster load historically did **not** — it resolved the roster solely as
`repo_root.join("prompt_assets/simard/ecosystem_repos.toml")`. On a deployed
daemon that path does not exist, so the rail failed closed on **every** tick
with:

```
WARN simard::ecosystem_observe: [simard] ecosystem-observe NOT wired: failed to load stewarded roster
  error=… read ecosystem roster failed: No such file or directory (os error 2)
  roster_path=…/Simard-deploy-4049/prompt_assets/simard/ecosystem_repos.toml
```

The feature was effectively dead in production. Giving the roster the **same
install-first resolution as the recipe** fixes the inconsistency: the roster is
found at its installed location, and the rail wires live.

---

## Resolution ladder

`resolve_ecosystem_roster_path(repo_root, home_override)` returns the first
existing candidate, checked in this order:

1. **Install location** —
   `<home>/.simard/prompt_assets/simard/ecosystem_repos.toml`, where `<home>` is
   `home_override` when provided, otherwise `dirs::home_dir()`. Used when it
   `is_file()`.
2. **In-tree checkout** —
   `<repo_root>/prompt_assets/simard/ecosystem_repos.toml`. Used when it
   `is_file()` and the install candidate was absent.
3. **None** — neither candidate is a file. The rail treats this as
   "roster unavailable" (fail-open: the observation pass is skipped, and a
   warning naming **both** attempted paths is logged; the daemon never panics).

This is the same ladder the recipe resolver uses, only with the roster's
relative path (`prompt_assets/simard/ecosystem_repos.toml`) instead of the
recipe's (`prompt_assets/simard/recipes/ecosystem-observe.yaml`).

| Candidate | Path | Chosen when |
|---|---|---|
| Install (preferred) | `~/.simard/prompt_assets/simard/ecosystem_repos.toml` | file exists |
| In-tree (fallback) | `<repo_root>/prompt_assets/simard/ecosystem_repos.toml` | install absent, file exists |
| None | — | neither exists |

> The resolver **probes the filesystem only** — no shell, no `Command`, no
> network. It never reads or parses the roster contents; parsing remains the
> job of `load_ecosystem_roster`. It never panics.

---

## API

Module-private helper in `src/overseer/ecosystem_observe.rs`, alongside
`resolve_observe_recipe_path`:

```rust
/// The stewarded-roster filename. A compile-time constant — never derived
/// from env, argv, or file contents (path-traversal invariant).
const ECOSYSTEM_ROSTER_FILENAME: &str = "ecosystem_repos.toml";

/// Resolve the `ecosystem_repos.toml` roster path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/<name>` (installed / hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/<name>` (in-tree)
///
/// Mirrors `resolve_observe_recipe_path`. `home_override` keeps tests hermetic
/// against the ambient `~/.simard`; production passes `None`.
fn resolve_ecosystem_roster_path(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> Option<PathBuf>;
```

### Parameters

| Parameter | Meaning |
|---|---|
| `repo_root` | The daemon's source checkout root. Used only for the in-tree fallback. |
| `home_override` | Test seam. `Some(dir)` resolves the install candidate under `dir/.simard/…` instead of the real home; production passes `None`, which uses `dirs::home_dir()`. |

### Return value

`Some(path)` — the first existing roster file (install-first). `None` — neither
candidate is a file. The resolver performs no I/O beyond `is_file()` probes and
never panics.

---

## Wiring contract (`build_ecosystem_observer`)

`build_ecosystem_observer` (in `src/overseer/wiring.rs`) resolves the roster via
`resolve_ecosystem_roster_path(repo_root, None)` and preserves the rail's
established **fail-visible / fail-open** posture:

| Resolver result | Behavior |
|---|---|
| `Some(path)` | Call `load_ecosystem_roster(&path)`. On parse `Err`, emit the existing fail-visible `WARN` (logging the resolved `path`) and return `None`. |
| `None` | Emit the fail-visible `WARN` `"[simard] ecosystem-observe NOT wired: failed to load stewarded roster"` on target `simard::ecosystem_observe`, logging **both attempted candidate paths** (the `~/.simard` install location and the `<repo_root>` in-tree location), and return `None`. |

Both misses return `None` — the rail is left unwired and the observation pass is
skipped for that build; the daemon **never panics** and **never silently
degrades**. The resolver miss and the loader error are distinct log lines so the
failure mode is unambiguous in the journal.

Because the resolver returns a bare `Option<PathBuf>` (it collapses both
candidates to the winner or `None`), the `None` branch reconstructs the two
candidate paths for the warning — using `ECOSYSTEM_ROSTER_FILENAME` and the same
`prompt_assets/simard` relative path — so the journal names **every** location
that was probed. This is deliberately more fail-visible than logging a single
representative path: an operator diagnosing a dead rail sees both the install
location and the in-tree location in one line.

Only the roster **path selection** changed. The loader
(`load_ecosystem_roster`), the recipe runner spawn
(`SpawnEcosystemRecipeRunner`), the cadence gates, and the OBSERVE→BRIEF recipe
semantics are untouched.

---

## Configuration

There is **no new environment variable.** The roster is data, configured by
placing/editing `ecosystem_repos.toml` at one of the two resolved locations.

| Deployment | Where the roster lives | How it resolves |
|---|---|---|
| Deployed daemon (`WorkingDirectory=~/.simard`) | `~/.simard/prompt_assets/simard/ecosystem_repos.toml` | Install candidate (preferred) |
| Local source checkout / dev | `<repo_root>/prompt_assets/simard/ecosystem_repos.toml` | In-tree fallback |

Editing the roster (adding or removing a stewarded repo) is still a one-line
data change with no code change — see
[Ecosystem Observe → Roster](../design/ecosystem-observe.md#1-roster--the-single-source-of-truth).
On a deployed daemon, edit the **installed** copy under `~/.simard`; the next
Overseer tick that builds the rail picks it up.

---

## Examples

### Deployed daemon (install-first)

The roster is installed under `~/.simard`; `repo_root` is a source checkout that
may not contain it. The rail resolves the installed copy and wires live — no
`NOT wired` warning:

```
$ ls ~/.simard/prompt_assets/simard/ecosystem_repos.toml
/home/azureuser/.simard/prompt_assets/simard/ecosystem_repos.toml

# Next Overseer tick — no "ecosystem-observe NOT wired" warning; the agentic
# ecosystem-observe recipe runs and surfaces problems from stewarded repos.
```

### Local development (in-tree fallback)

Running from a source checkout with no installed `~/.simard` copy, the rail
falls back to the in-tree roster:

```
<repo_root>/prompt_assets/simard/ecosystem_repos.toml   ← resolved (fallback)
```

### Neither present (fail-open)

If neither candidate exists, the rail logs the fail-visible warning naming
**both** attempted paths and skips the observation pass. The daemon keeps
running:

```
WARN simard::ecosystem_observe: [simard] ecosystem-observe NOT wired: failed to load stewarded roster
  install_candidate=~/.simard/prompt_assets/simard/ecosystem_repos.toml
  in_tree_candidate=<repo_root>/prompt_assets/simard/ecosystem_repos.toml
```

---

## Verifying resolution

On a deployed host, confirm the installed roster exists and that the journal no
longer carries the `NOT wired: failed to load stewarded roster` line after the
next tick:

```bash
# 1. Confirm the installed roster is present.
test -f ~/.simard/prompt_assets/simard/ecosystem_repos.toml && echo "roster present"

# 2. Watch the daemon journal for a clean wire (absence of the failure line).
journalctl --user -u simard -f | grep -i ecosystem-observe
#   Expect NO "NOT wired: failed to load stewarded roster".
#   Expect the ecosystem-observe recipe to run on cadence.
```

See [Watch Overseer activity](../howto/watch-overseer-activity.md) for the
broader journal-tailing workflow.

---

## Testing

The resolver is covered by hermetic unit tests in
`src/overseer/ecosystem_observe.rs` that use `tempfile::tempdir()` plus
`home_override`, so they never touch the real `~/.simard`:

| Test | Setup | Expectation |
|---|---|---|
| Prefers install | Roster file under `home_override/.simard/prompt_assets/simard/ecosystem_repos.toml` (and/or in-tree) | Returns the `~/.simard` path |
| Falls back to in-tree | Roster only under `repo_root/prompt_assets/simard/…` | Returns the `repo_root` path |
| None when absent | Neither location has the file | Returns `None` |
| Regression (bug #2419) | `repo_root` **lacks** the roster but `home_override/.simard/…` has it | Returns the install path — proves the rail wires from the installed location even when `repo_root` is stale/source-only |

The regression test pins the reported production failure so it cannot recur: a
`repo_root` without the roster must still resolve the installed copy.

Gates: full `cargo test` (not `--lib` only) and
`cargo clippy --all-targets -- -D warnings` pass. No `--no-verify`, no `--admin`.

---

## Security notes

- **Constant filename.** `ECOSYSTEM_ROSTER_FILENAME` is a compile-time constant;
  the roster filename is never constructed from env, argv, or file contents
  (path-traversal prevention).
- **No untrusted input reaches the resolver.** Production passes
  `home_override = None`; the override exists solely so tests avoid the ambient
  home. `repo_root` and `~/.simard` are both operator-controlled.
- **Pure filesystem probe.** The resolver runs no shell, no `Command`, and no
  network — this posture is preserved from the recipe resolver.
- **Logs are paths only.** The fail-visible warnings log resolved-or-attempted
  paths (both candidates on a total miss), never roster contents or credentials
  (no info disclosure / log injection).
- **Never panics.** Missing or inaccessible paths return `None`, keeping the
  daemon available (fail-open, DoS-resistant).
- **TOCTOU/symlink between `is_file()` and load** is a LOW/accepted residual:
  both roots are operator-controlled on the single-user host, consistent with
  the [state-root resolution](./state-root-resolution.md) threat model.

---

## See also

- [Ecosystem Observe](../design/ecosystem-observe.md) — the full agentic
  OBSERVE→BRIEF chain and the roster's role as an allowlist. This page covers
  only the roster path resolution.
- [State-root resolution](./state-root-resolution.md) — the sibling
  install/override resolution ladder and its shared threat model.
- [Fail-open audit](../fail-open-audit.md) — the fail-visible / fail-open
  posture this rail preserves.
- [Watch Overseer activity](../howto/watch-overseer-activity.md) — how to
  confirm the rail wires on the deployed daemon.
