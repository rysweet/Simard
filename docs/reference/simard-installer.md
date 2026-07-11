---
title: Simard installer reference
description: Contract for the canonical `simard install` deployment path, including SIMARD_HOME layout, prompt assets, user systemd units, atomic replacement, rollback artifacts, update integration, and dry-run/test controls.
last_updated: 2026-07-09
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/run-ooda-daemon.md
  - ./simard-cli.md
  - ./npx-npm-install.md
  - ../howto/set-up-the-signal-channel.md
---

# Simard installer reference

`simard install` is the canonical deployment rail for a running Simard host. It
installs the currently executing Simard binary, installs matching prompt assets,
writes user-level systemd units for the OODA and Signal services, and activates
those services through `systemctl --user`.

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
```

| Option | Purpose |
| --- | --- |
| `--simard-home PATH` | Install root. Overrides `SIMARD_HOME`. Must be an absolute, non-empty path. |
| `--dry-run` | Validate inputs and print the install plan without replacing live files or invoking `systemctl`. |
| `--systemd-user-dir PATH` | User unit directory override. Intended for tests and isolated hosts. Defaults to `$XDG_CONFIG_HOME/systemd/user` or `$HOME/.config/systemd/user`. |
| `--systemctl PATH` | `systemctl` executable override. Intended for hermetic tests with a fake command. Defaults to `systemctl`. |

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root when `--simard-home` is not supplied. |
| `XDG_CONFIG_HOME` | `$HOME/.config` | Base directory for the default user systemd unit directory. |
| `SIMARD_INSTALL_PROMPT_ASSETS_ROOT` | Auto-discovered | Preferred prompt asset source root for packaged installs. Must contain `simard/ooda_orient.md` and `simard/recipes/ooda-orient.yaml`. |
| `SIMARD_PROMPT_ASSET_ROOT` | Auto-discovered | Compatibility prompt asset source root with the same expected directory shape as `SIMARD_INSTALL_PROMPT_ASSETS_ROOT`. |
| `SIMARD_PROMPT_ASSETS_DIR` | Auto-discovered | Compatibility prompt asset source. May point either at a root containing `simard/` or directly at the `simard/` asset directory; direct `simard/` values are normalized to their parent root. |

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
```

| Path | Owner | Notes |
| --- | --- | --- |
| `$SIMARD_HOME/bin/simard` | Installer | Live Simard binary used by the systemd units. Replaced only by atomic rename. |
| `$SIMARD_HOME/prompt_assets/` | Installer | Prompt assets that match the installed binary. Installed as a staged tree, then swapped into place through the directory strategy below. |
| `$SIMARD_HOME/.install.lock` | Installer | Per-`SIMARD_HOME` install lock. A second installer for the same home fails instead of racing live replacements. |
| `$SIMARD_HOME/.install-staging/` | Installer | Private staging area for the current install attempt. |
| `$SIMARD_HOME/.install-backups/` | Installer/operator | Previous binary backups and operator-created memory snapshots. |
| `$SIMARD_HOME/cognitive/` | Runtime | Cognitive memory store. The installer does not delete or rewrite it. |
| `$SIMARD_HOME/config.toml` | Operator/runtime | Runtime configuration. The installer does not rewrite operator config. |

The installer creates staging and backup directories with owner-only
permissions on Unix hosts.

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
11. Print rollback guidance, including memory backup guidance, before any service restart.
12. Reload, enable, and restart the user services unless `--dry-run` was supplied.

The live binary must never be overwritten by copying into the final path. The
only live-binary replacement operation is a rename from a fully staged file.

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

Service activation remains intentional on rerun: `daemon-reload`, `enable`, and
`restart` are issued after a successful non-dry-run install so the running
services use the installed binary and assets.

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
- any non-zero `systemctl --user` command

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
