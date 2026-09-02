---
title: Simard installer reference
description: Contract for the canonical `simard install` deployment path, including SIMARD_HOME layout, the owned PATH entrypoint and orphan reconciliation, prompt assets, user systemd units, atomic replacement, the post-deploy version-parity gate, rollback artifacts, update integration, and dry-run/test controls.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/run-ooda-daemon.md
  - ../howto/verify-path-entrypoint-parity.md
  - ./simard-cli.md
  - ./self-deploy-api.md
  - ./npx-npm-install.md
  - ../howto/set-up-the-signal-channel.md
---

# Simard installer reference

`simard install` is the canonical deployment rail for a running Simard host. It
installs the currently executing Simard binary, installs matching prompt assets,
writes user-level systemd units for the OODA and Signal services, activates
those services through `systemctl --user`, and **owns the `simard` entrypoint on
your PATH** so that after any install the `simard` your shell resolves is the
binary that was just deployed.

## PATH-entrypoint ownership guarantee

Every `simard install` — including every `scripts/redeploy-local.sh` deploy —
guarantees that:

1. `~/.local/bin/simard` is a symlink owned by the installer that points at the
   versioned live binary `~/.simard/bin/simard`.
2. No **stale, installer-owned** `simard` copy shadows it earlier on PATH. Known
   orphan locations (at minimum `~/.local/bin/simard` and `~/.cargo/bin/simard`)
   are reconciled on every deploy: an installer-owned entrypoint is repaired,
   and a verified-ours orphan is removed.
3. The PATH-resolved `simard` **is** the just-installed binary — not merely the
   same version string. Parity is asserted two ways: the PATH-resolved path
   canonicalizes (`readlink -f`) to `~/.simard/bin/simard` (**path identity**),
   and its `--version` equals the just-installed binary's version.
   `scripts/redeploy-local.sh` and the `entrypoint_parity` probe of
   `simard self-health` both fail loudly if either is ever violated.

   > **Why path identity, not just the version string?** After reconciliation the
   > entrypoint is a symlink to `~/.simard/bin/simard`, so a version-string
   > comparison alone is near-tautological — it only catches skew when a foreign
   > shadow reports a *different* version, and cannot catch a same-version
   > developer rebuild (the common `redeploy-local.sh` case) that a stale
   > *file* is still shadowing. Asserting path identity catches the stale-file
   > bug directly, independent of the version string.

This closes the historical failure mode where a deploy rebuilt and reinstalled
`~/.simard/bin/simard` (the version the systemd daemon runs) but left an older
`~/.local/bin/simard` — first on the daemon's rendered `PATH` — shadowing it, so
the operator's `simard status` / `goal list` silently ran a version-skewed CLI.

> **The systemd `PATH` renders `~/.local/bin` first**
> (`{home}/.local/bin:{home}/.cargo/bin:{simard_home}/bin:...`). Owning
> `~/.local/bin/simard` is therefore what guarantees the *first* `simard` on
> PATH is the freshly installed one.

The reconciliation is deliberately **conservative**: the installer only ever
replaces or removes a `simard` it can prove is its own. A foreign file at a known
path is never modified or deleted — it is surfaced loudly instead (see
[Orphan reconciliation](#owned-path-entrypoint-and-orphan-reconciliation)). The
one bounded exception is a non-symlink regular file whose `--version` banner
starts with `simard `, which the installer treats as an ours-copy; see
[the `OursMarker` residual](#clearly-ours-classification-two-tier-fail-closed).

The operator rule is: do not deploy Simard by copying a binary over
`~/.simard/bin/simard`, swapping files from a worktree, or relying on an ad-hoc
`cargo build` path as the live service path. Build or download whatever binary
you intend to deploy, then run that binary's installer:

```bash
./target/release/simard install
```

For release installs through npm:

```bash
npx github:rysweet/Simard install
```

## Command

```text
simard install [--simard-home PATH] [--dry-run] [--systemd-user-dir PATH] [--systemctl PATH]
               [--entrypoint-dir PATH] [--orphan-dir PATH ...]
```

| Option | Purpose |
| --- | --- |
| `--simard-home PATH` | Install root. Overrides `SIMARD_HOME`. Must be an absolute, non-empty path. |
| `--dry-run` | Validate inputs and print the install plan (including the entrypoint symlink and orphan reconciliation plan) without replacing live files, touching orphans, or invoking `systemctl`. |
| `--systemd-user-dir PATH` | User unit directory override. Intended for tests and isolated hosts. Defaults to `$XDG_CONFIG_HOME/systemd/user` or `$HOME/.config/systemd/user`. |
| `--systemctl PATH` | `systemctl` executable override. Intended for hermetic tests with a fake command. Defaults to `systemctl`. |
| `--entrypoint-dir PATH` | Directory that will hold the owned `simard` symlink. Must be an absolute path. Intended for temp-HOME tests. Defaults to `$HOME/.local/bin`. |
| `--orphan-dir PATH` | Additional directory to scan for a stale, verified-ours `simard` orphan. Repeatable. Intended for temp-HOME tests. Defaults to `[$HOME/.cargo/bin]`. |

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root when `--simard-home` is not supplied. |
| `XDG_CONFIG_HOME` | `$HOME/.config` | Base directory for the default user systemd unit directory. |
| `SIMARD_INSTALL_PROMPT_ASSETS_ROOT` | Auto-discovered | Preferred prompt asset source root for packaged installs. Must contain `simard/ooda_orient.md` and `simard/recipes/ooda-orient.yaml`. |
| `SIMARD_PROMPT_ASSET_ROOT` | Auto-discovered | Compatibility prompt asset source root with the same expected directory shape as `SIMARD_INSTALL_PROMPT_ASSETS_ROOT`. |
| `SIMARD_PROMPT_ASSETS_DIR` | Auto-discovered | Compatibility prompt asset source. May point either at a root containing `simard/` or directly at the `simard/` asset directory; direct `simard/` values are normalized to their parent root. |
| `SIMARD_ENTRYPOINT_DIR` | `$HOME/.local/bin` | Directory that holds the owned `simard` symlink when `--entrypoint-dir` is not supplied. |
| `SIMARD_ORPHAN_DIRS` | `$HOME/.cargo/bin` | `:`-separated list of extra directories scanned for a stale, verified-ours `simard` orphan when `--orphan-dir` is not supplied. |

Precedence for the entrypoint directory is:

1. `--entrypoint-dir PATH`
2. `SIMARD_ENTRYPOINT_DIR`
3. `$HOME/.local/bin`

Precedence for the orphan directory list is:

1. `--orphan-dir PATH` (repeatable)
2. `SIMARD_ORPHAN_DIRS` (`:`-separated)
3. `[$HOME/.cargo/bin]`

The resolved entrypoint directory is always scanned for reconciliation in
addition to the orphan list, and the entrypoint path itself is excluded from the
orphan-removal set (it is *repaired*, not deleted). All entrypoint and orphan
paths are validated with the same fail-closed rules as `SIMARD_HOME`: empty,
relative, whitespace, control-character, newline, and unsafe-escape paths are
rejected before any live mutation.

Precedence for the install root is:

1. `--simard-home PATH`
2. `SIMARD_HOME`
3. `$HOME/.simard`

Precedence for prompt asset source discovery is:

1. `SIMARD_INSTALL_PROMPT_ASSETS_ROOT`
2. `SIMARD_PROMPT_ASSET_ROOT`
3. `SIMARD_PROMPT_ASSETS_DIR`
4. `prompt_assets` under the current working directory
5. `prompt_assets` under the compiled Cargo manifest directory

The selected prompt asset source must contain both required files:

```text
<source-root>/
`- simard/
   |- ooda_orient.md
   `- recipes/
      `- ooda-orient.yaml
```

If no candidate has that shape, `simard install` exits non-zero before staging or service activation and reports the checked environment variables and fallback roots.

All installer paths are validated before any live mutation. Empty paths, relative paths, control characters, newlines, carriage returns, and unsafe systemd percent escapes are rejected. Rejection is fail-closed: the installer exits non-zero and does not activate services.

## Installed layout

For the default home, the installer owns this layout:

```text
~/.simard/
|- bin/
|  `- simard
|- prompt_assets/
|- cognitive/
|- config.toml
|- .install.lock
|- .install-staging/
`- .install-backups/

~/.local/bin/
`- simard -> ~/.simard/bin/simard   # owned PATH entrypoint (symlink)
```

| Path | Owner | Notes |
| --- | --- | --- |
| `$SIMARD_HOME/bin/simard` | Installer | Live Simard binary used by the systemd units. Replaced only by atomic rename. |
| `~/.local/bin/simard` | Installer | Owned PATH entrypoint. A symlink to `$SIMARD_HOME/bin/simard`, created atomically (temp symlink + rename) and repaired idempotently on every install. This is the `simard` your interactive shell resolves. |
| `$SIMARD_HOME/prompt_assets/` | Installer | Prompt assets that match the installed binary. Installed as a staged tree, then swapped into place through the directory strategy below. |
| `$SIMARD_HOME/.install.lock` | Installer | Per-`SIMARD_HOME` install lock. A second installer for the same home fails instead of racing live replacements. |
| `$SIMARD_HOME/.install-staging/` | Installer | Private staging area for the current install attempt. |
| `$SIMARD_HOME/.install-backups/` | Installer/operator | Previous binary backups and operator-created memory snapshots. |
| `$SIMARD_HOME/cognitive/` | Runtime | Cognitive memory store. The installer does not delete or rewrite it. |
| `$SIMARD_HOME/config.toml` | Operator/runtime | Runtime configuration. The installer does not rewrite operator config. |

The installer creates staging and backup directories with owner-only
permissions on Unix hosts. The entrypoint directory (`~/.local/bin`) is created
if it does not already exist, but its permissions are left as-is because it is a
shared user PATH directory, not a private installer directory.

> The owned entrypoint is a **symlink**, not a copy. A symlink stays correct
> across future in-place replacements of the versioned binary (the install swaps
> `~/.simard/bin/simard` by atomic rename; the symlink target path never
> changes), is trivially idempotent, and is self-identifying as installer-owned
> because its canonicalized target resolves under `~/.simard/bin/`.

## Owned PATH entrypoint and orphan reconciliation

Entrypoint reconciliation runs **unconditionally on every install** — including
when the live `~/.simard/bin/simard` already matches the running binary and the
binary swap is skipped. This makes the guarantee self-healing: an operator (or a
stray `cargo install`) that reintroduces a stale `simard` on PATH has it
reconciled on the very next deploy.

### The owned entrypoint

The installer ensures `~/.local/bin/simard` is a symlink to
`~/.simard/bin/simard`:

- If it is already that exact symlink, it is left in place.
- Otherwise a fresh symlink is written **atomically**: the installer creates a
  uniquely named temporary symlink in the same directory and `rename`s it over
  the entrypoint path. The same-directory temp + rename prevents a partially
  written or swapped-out entrypoint and closes the symlink-swap race window.

### "Clearly ours" classification (two-tier, fail-closed)

Before the installer replaces or removes any `simard` at a known path, it
classifies the existing file. Classification is a total, fail-closed function —
anything it cannot positively prove is ours is treated as `Foreign` and left
untouched:

| Class | Condition | Action |
| --- | --- | --- |
| `Absent` | Nothing at the path. | Create the owned symlink (entrypoint) / nothing to prune (orphan). |
| `OursSymlink` | A symlink whose canonicalized target is inside `~/.simard/bin/`. | Entrypoint: keep/repair. Orphan: remove. |
| `OursMarker` | A regular file that, run as `<path> --version`, prints a line starting with `simard ` (our identifying marker). | Entrypoint: replace with the owned symlink. Orphan: remove. |
| `Foreign` | Anything else: an unrelated file, an unreadable path, a broken symlink, a symlink pointing outside `~/.simard/bin/`, a binary whose `--version` fails, exits non-zero, or does not start with `simard `. | **Never modified or deleted.** Surfaced loudly (see below). |

The marker probe is safe by construction: it runs `Command::new(path).arg("--version")`
— argv-only, no shell, no inherited stdin, bounded output — so it cannot be a
shell-injection vector. Any exec failure, non-zero exit, or non-UTF-8 / non-matching
output classifies the file as `Foreign`.

> **Residual of the `OursMarker` heuristic.** The `--version`-prefix marker is a
> heuristic, not a proof of provenance. A non-symlink regular file is the only
> case where the installer cannot cryptographically prove the file is its own,
> so it falls back to the identifying banner. The consequence is a narrow but
> real edge: an **unrelated** third-party binary that happens to be named
> `simard` **and** prints a `--version` line starting with `simard ` would be
> classified `OursMarker` and thus replaced (at the entrypoint) or removed (as
> an orphan). This is the one case where the "a foreign file is never modified
> or deleted" guarantee is bounded by the marker's specificity rather than
> absolute. `OursSymlink` files carry no such ambiguity. Operators who keep an
> unrelated tool named `simard` on PATH should not place it at a reconciled
> path (`~/.local/bin` or `~/.cargo/bin`).

### Orphan pruning

For each directory in the resolved orphan set (default `~/.cargo/bin`, plus the
entrypoint directory), a `simard` classified `OursSymlink` or `OursMarker` is
removed so it can never shadow the owned entrypoint. A `Foreign` orphan is left
in place.

### Foreign shadows are surfaced, never clobbered

If a `Foreign` `simard` occupies the entrypoint path itself, the installer does
**not** clobber it — it skips the atomic replace, records the path as a
`foreign_shadow` on the install outcome, and emits a loud operator diagnostic:

```text
[simard] WARNING: a foreign 'simard' occupies the PATH entrypoint and was left untouched:
[simard]   /home/you/.local/bin/simard
[simard]   (not an installer-owned symlink and its --version did not start with "simard ")
[simard]   the owned entrypoint could not be installed; resolve this file manually
```

The same fault is reported by the `entrypoint_parity` probe of
`simard self-health`. This is what protects a user's unrelated `~/.local/bin/simard`
from ever being deleted by a Simard deploy, and what the temp-HOME
"planted foreign file survives" regression test asserts.

### Idempotency

Reconciliation converges: a first install creates exactly one owned entrypoint
symlink and prunes ours-orphans; a second, identical install finds the entrypoint
already correct, leaves it in place, and finds no ours-orphans to prune. Repeated
installs never accumulate more than one entrypoint.

### Non-unix targets

The owned entrypoint is a Unix symlink, so the entire reconciliation step is
gated behind `#[cfg(unix)]`. On non-unix targets the reconciler compiles to a
`#[cfg(not(unix))]` no-op: no entrypoint is created, no orphans are pruned, and
the `entrypoint_path` / `foreign_shadows` outcome fields are empty. Simard's
supported deployment surface (user systemd units, `~/.local/bin` PATH ordering)
is Unix-only, so this is a compile-time exclusion rather than a runtime
degradation.

## User systemd units

`simard install` writes two user-level systemd units:

| Unit | Command | Working directory |
| --- | --- | --- |
| `simard-ooda.service` | `$SIMARD_HOME/bin/simard ooda run` | `$SIMARD_HOME` |
| `simard-signal.service` | `$SIMARD_HOME/bin/simard signal run` | `$SIMARD_HOME` |

The units never reference a source checkout, worktree, `target/`, or
`worktrees/main`. `WorkingDirectory` is always the resolved `SIMARD_HOME`, so
service behavior is independent of the directory where the installer was
launched.

After successful staging and file replacement, activation runs:

```bash
systemctl --user daemon-reload
systemctl --user enable simard-ooda.service
systemctl --user enable simard-signal.service
systemctl --user restart simard-ooda.service
systemctl --user restart simard-signal.service
```

If any activation command fails, `simard install` exits non-zero and reports
the failed command. It must not hide `systemctl` errors.

### Service environment

User systemd services do not reliably inherit the shell environment used to run
the installer. Provider selection and credentials must come from durable Simard
configuration or from user-manager environment imported intentionally, not from
implicit shell state.

Recommended provider settings belong in `$SIMARD_HOME/config.toml`. If a
provider still requires an environment variable such as `ANTHROPIC_API_KEY`,
import it into the user systemd manager before activation:

```bash
systemctl --user import-environment ANTHROPIC_API_KEY SIMARD_LLM_PROVIDER
systemctl --user restart simard-ooda.service simard-signal.service
```

Generated unit files must not embed secrets.

## Install flow

The installer is ordered so a failed preflight or staging step cannot leave
services pointed at a half-written binary or partial prompt tree.

1. Resolve and validate `SIMARD_HOME` and the user systemd unit directory.
2. Resolve the current executable: the binary running `simard install` is the binary being deployed.
3. Render both systemd unit files in memory and validate their paths.
4. Resolve the `systemctl` executable when activation is enabled.
5. Acquire the per-`SIMARD_HOME` install lock for the live transaction.
6. Stage the new binary and prompt assets under `.install-staging/`.
7. Preserve the previous live binary under `.install-backups/` when one exists and differs from the new binary.
8. Atomically rename the staged binary into `$SIMARD_HOME/bin/simard`.
9. Replace `$SIMARD_HOME/prompt_assets/` with the staged asset tree using the prompt-assets swap strategy below.
10. Atomically rename staged unit files into the user systemd directory.
11. **Reconcile the owned PATH entrypoint and orphans** (unconditional): repair `~/.local/bin/simard` to the owned symlink, prune verified-ours orphans in the entrypoint and orphan directories, and record any foreign shadow. This step runs on every install, even when the binary swap in steps 7–8 was skipped because the live binary already matched.
12. Print rollback guidance, including memory backup guidance, before any service restart.
13. Reload, enable, and restart the user services unless `--dry-run` was supplied.

The live binary must never be overwritten by copying into the final path. The
only live-binary replacement operation is a rename from a fully staged file. The
owned entrypoint symlink is likewise only ever written by a same-directory temp
symlink + rename, never by an in-place unlink-then-create.

### Prompt-assets swap strategy

Directory replacement uses an explicit transaction rather than a recursive copy
over the live tree:

1. Stage the complete asset tree under `.install-staging/<transaction>/prompt_assets`.
2. Rename the live `$SIMARD_HOME/prompt_assets` to a backup path under
   `.install-backups/` when it exists.
3. Rename the staged tree into `$SIMARD_HOME/prompt_assets`.
4. Leave the previous tree available for operator rollback until cleanup.

Recursive copy-over-live is not acceptable.

## `simard update` integration

`simard update` remains a separate self-update command today. The planned
installer integration is for `simard update` to download and verify the release
asset, then hand that verified binary and its prompt assets to the same installer
transaction described here. That keeps release upgrades on the same staging,
backup, systemd activation, and rollback rail as source-built installs.

Until that integration lands, docs that describe `simard update` as replacing
the current binary are describing the legacy self-update rail, not this host
installer.

## Idempotency

The command is safe to rerun:

```bash
simard install
simard install
```

Repeated runs converge on the same layout and unit contents. If the live
binary already matches the current executable, the installer leaves it in place
instead of creating unnecessary backup churn. If unit files or prompt assets
differ, they are replaced through the same staging and rename flow.

Entrypoint reconciliation is idempotent independently of the binary swap: a
rerun that skips the binary swap still verifies (and repairs, if needed) the
owned `~/.local/bin/simard` symlink and re-prunes any ours-orphans, so a stale
`simard` reintroduced onto PATH between deploys is always cleaned up on the next
install. A second identical install leaves exactly one entrypoint symlink and
removes nothing new.

Service activation remains intentional on rerun: `daemon-reload`, `enable`, and
`restart` are issued after a successful non-dry-run install so the running
services use the installed binary and assets.

## Install outcome

A successful `simard install` reports an `InstallOutcome` describing the
transaction. The entrypoint-ownership fields are additive:

| Field | Meaning |
| --- | --- |
| `simard_home` | Resolved install root. |
| `binary_path` | Live versioned binary path (`$SIMARD_HOME/bin/simard`). |
| `prompt_assets_path` | Installed prompt-asset tree. |
| `ooda_unit_path` / `signal_unit_path` | Installed / decommissioned unit paths. |
| `prior_binary_backup` | Preserved previous binary, when the binary was swapped. |
| `activated` | Whether services were activated (`false` for `--dry-run`). |
| `entrypoint_path` | The owned PATH entrypoint that was created or verified (`~/.local/bin/simard`). |
| `foreign_shadows` | Paths where a foreign `simard` was found and deliberately left untouched. Empty on a clean host. A non-empty list means the owned entrypoint could not fully take over PATH and requires operator attention. |

## Post-deploy version-parity gate

After a non-dry-run install, the deploy asserts **entrypoint parity**: the
`simard` resolved on `PATH` must be the installed binary. Parity has two
components, checked in order:

- **Path identity** — the PATH-resolved `simard`, canonicalized with `readlink -f`,
  must equal `$SIMARD_HOME/bin/simard`. This is the primary check: it catches a
  stale *file* shadowing the entrypoint even when it happens to report the same
  version (a developer rebuild at the same `CARGO_PKG_VERSION`).
- **Version equality** — the PATH-resolved `simard --version` must equal the
  installed binary's own `simard --version` string (`simard <CARGO_PKG_VERSION>`),
  captured during the install transaction. This is distinct from the git-commit
  `VersionAdvancedProbe` used by the self-deploy orchestrator.

Parity is enforced in two independent places:

- **`scripts/redeploy-local.sh`** re-hashes the shell command cache, asserts path
  identity, then compares versions, exiting non-zero with `[simard]` diagnostics
  on any skew. See [`redeploy-local.sh` parity gate](#redeploy-localsh-parity-gate).
- **`simard self-health`** includes an `entrypoint_parity` probe that reports a
  fault on a path mismatch, a version skew, or a foreign shadow. See the
  [self-deploy API reference](./self-deploy-api.md#entrypointparityprobe).

### `redeploy-local.sh` parity gate

`scripts/redeploy-local.sh` builds the release binary, delegates to
`simard install`, and then — unless `DRY_RUN=1` — runs a parity gate. The gate
asserts **path identity first**, then version equality, because path identity is
what catches a same-version stale file (the common developer-rebuild case):

```text
[redeploy] verifying PATH-entrypoint parity ...
[redeploy] PATH-resolved:  /home/you/.local/bin/simard -> /home/you/.simard/bin/simard
[redeploy] installed:      simard 0.35.0
[redeploy] PATH-version:   simard 0.35.0
[redeploy] parity OK (path identity + version match)
```

The gate performs, in order:

1. `hash -r` to drop any shell command-cache entry from earlier in the session.
2. Resolve `command -v simard` and `readlink -f` it; the canonical target **must**
   equal `$SIMARD_HOME/bin/simard`. A mismatch (a stale file or a foreign shadow
   ahead on PATH) fails the gate even when the two `--version` strings are
   identical.
3. Compare the installed `--version` against the PATH-resolved `simard --version`.

On a path-identity failure (the same-version stale-file case a version-only
check would miss):

```text
[simard] FATAL: PATH-entrypoint parity check failed after install
[simard]   PATH-resolved: /home/you/.local/bin/simard -> /home/you/.local/bin/simard
[simard]   expected:      -> /home/you/.simard/bin/simard
[simard]   the 'simard' on PATH is not the installed entrypoint (stale file or foreign shadow)
```

On a version skew:

```text
[simard] FATAL: version parity check failed after install
[simard]   installed:     simard 0.35.0  (/home/you/.simard/bin/simard)
[simard]   PATH-resolved: simard 0.31.0  (/home/you/.local/bin/simard)
[simard]   a stale 'simard' is still shadowing the freshly installed binary on PATH
```

The gate is skipped only under `DRY_RUN=1`, because a dry run performs no live
install. When the preceding `simard install` recorded a non-empty
`foreign_shadows` list, the gate treats that as a hard failure regardless of the
version strings, since a foreign shadow means the owned entrypoint could not take
over PATH.



## Rollback artifacts and memory backup guidance

`simard install` preserves the previous live binary before swapping in a new
one:

```text
$SIMARD_HOME/.install-backups/simard.<transaction-id>.bak
```

The backup is first written to a sibling temporary file and then renamed into
that final path, so operators never see a partially written final backup.

The installer does not rewrite cognitive memory, but operators should snapshot
memory before a service swap when they need a rollback point for state as well
as code. The installer will print this guidance before restarting services:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
mkdir -p "$SIMARD_HOME/.install-backups"
tar --ignore-failed-read -C "$SIMARD_HOME" \
  -czf "$SIMARD_HOME/.install-backups/memory-before-install-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" \
  cognitive goals memory config.toml
```

Manual binary rollback uses rename operations, not copy-over-live:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
BACKUP="$(ls -t "$SIMARD_HOME"/.install-backups/simard.*.bak | head -n 1)"

systemctl --user stop simard-ooda.service simard-signal.service
mv "$SIMARD_HOME/bin/simard" "$SIMARD_HOME/.install-backups/simard.failed.$(date -u +%Y%m%dT%H%M%SZ)"
mv "$BACKUP" "$SIMARD_HOME/bin/simard"
chmod 755 "$SIMARD_HOME/bin/simard"
systemctl --user daemon-reload
systemctl --user restart simard-ooda.service simard-signal.service
```

Full automated rollback is not part of `simard install`; the installer provides
the preserved binary and the operator-visible state backup guidance needed to
roll back deliberately.

## Dry-run and hermetic tests

Use `--dry-run` to validate the install plan without mutating live files or
invoking `systemctl`:

```bash
simard install --dry-run --simard-home "$HOME/.simard"
```

Integration tests and CI should use temporary homes, temporary unit directories,
and a fake `systemctl` executable:

```bash
tmp="$(mktemp -d)"
mkdir -p "$tmp/bin" "$tmp/systemd/user"

cat > "$tmp/bin/systemctl" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$SIMARD_FAKE_SYSTEMCTL_LOG"
exit 0
SH
chmod 755 "$tmp/bin/systemctl"

SIMARD_FAKE_SYSTEMCTL_LOG="$tmp/systemctl.log" \
simard install \
  --simard-home "$tmp/home" \
  --systemd-user-dir "$tmp/systemd/user" \
  --systemctl "$tmp/bin/systemctl"
```

The resulting temp tree should contain:

```text
$tmp/home/bin/simard
$tmp/home/prompt_assets/
$tmp/systemd/user/simard-ooda.service
$tmp/systemd/user/simard-signal.service
$tmp/systemctl.log
```

The generated unit files should use `$tmp/home` as `WorkingDirectory` and must not contain the repository checkout path.

## Failure contract

`simard install` must fail closed. It exits non-zero and skips service
activation when any required step fails, including:

- invalid `SIMARD_HOME` or override paths
- missing or non-executable `systemctl` when activation is enabled
- failure to read the current executable
- failure to acquire the per-`SIMARD_HOME` install lock
- failure to stage the binary, prompt assets, or units
- failure to preserve the previous binary
- failure to atomically rename a staged live file into place
- failure to write the owned entrypoint symlink (for example, the entrypoint directory cannot be created or is not writable), except when the entrypoint path is occupied by a foreign file, which is a surfaced-warning outcome rather than a hard failure (see below)
- any non-zero `systemctl --user` command

A **foreign shadow** at the entrypoint path is a distinct, softer outcome: the
install completes, but the outcome's `foreign_shadows` list is non-empty, a loud
`[simard]` warning is printed, and `simard self-health` reports an
`entrypoint_parity` fault. The installer never deletes the foreign file to force
ownership. Reconciliation never removes a file it cannot prove is installer-owned,
so a bug or a poisoned override cannot cause the installer to delete an operator's
unrelated binary.

Dry-run mode never invokes `systemctl`, even if `--systemctl` points at a real
executable. Executable existence checks are deferred when activation is
disabled.

## Troubleshooting

### Confirm what is installed

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
"$SIMARD_HOME/bin/simard" status
ls "$SIMARD_HOME/prompt_assets"
```

### Inspect service state

```bash
systemctl --user status simard-ooda.service
systemctl --user status simard-signal.service
journalctl --user -u simard-ooda.service -n 100 --no-pager
journalctl --user -u simard-signal.service -n 100 --no-pager
```

### Verify unit paths

```bash
systemctl --user cat simard-ooda.service
systemctl --user cat simard-signal.service
```

Both units should show:

```text
WorkingDirectory=/home/you/.simard
ExecStart=/home/you/.simard/bin/simard ...
```

If a unit references a source checkout, a worktree, `target/`, or `worktrees/main`, rerun `simard install` from the binary you intend to deploy.

### Verify PATH-entrypoint parity

Confirm the `simard` your shell resolves is the freshly installed binary and
that nothing stale shadows it:

```bash
which simard                     # expect: ~/.local/bin/simard
readlink -f "$(command -v simard)"   # expect: ~/.simard/bin/simard
simard --version                 # expect: same version as ~/.simard/bin/simard --version
"$HOME/.simard/bin/simard" --version
simard self-health --json | jq .probes.entrypoint_parity
```

If `simard self-health` reports an `entrypoint_parity` fault, or
`redeploy-local.sh` exits non-zero on the parity gate, see the runbook
[Verify and repair PATH-entrypoint parity](../howto/verify-path-entrypoint-parity.md).
