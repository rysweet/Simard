---
title: How to run the OODA daemon
description: Procedure for installing Simard through the canonical installer rail, running the OODA and Signal services as user-level systemd units, and verifying the autonomous loop.
last_updated: 2026-07-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/actor-session-startup-purge.md
  - ../reference/simard-installer.md
  - ../reference/simard-cli.md
  - ../architecture/overview.md
  - ../howto/set-up-the-signal-channel.md
---

# How to run the OODA daemon

The supported long-running deployment path is `simard install`. The installer
places the current binary and prompt assets under `SIMARD_HOME`, writes the
`simard-ooda.service` and `simard-signal.service` user units, and restarts those
services through `systemctl --user`.

Manual daemon starts are useful for smoke tests, but they are not the canonical
deployment path for a live host.

## Prerequisites

- A Simard binary you want to deploy, from a release, `npx github:rysweet/Simard`, or a local release build.
- User-level systemd available: `systemctl --user status`.
- Runtime configuration in `$SIMARD_HOME/config.toml` when your selected provider or Signal channel needs it.
- Provider configuration in `$SIMARD_HOME/config.toml`, or user-systemd-manager
  environment imported intentionally when your selected base type requires an
  environment variable such as `ANTHROPIC_API_KEY`.

There is no Python, `pip`, or kuzu setup step.

## 1. Install and start the services

Install the release binary through npm:

```bash
npx github:rysweet/Simard install
```

Or install a locally built candidate:

```bash
cargo build --release
./target/release/simard install
```

The second command will deploy `./target/release/simard` because that is the
binary running the installer. The live service path is still
`$SIMARD_HOME/bin/simard`; systemd never points at `target/release`, a source
checkout, or `worktrees/main`.

The default install root is `~/.simard`. Override it explicitly for an isolated
primary host or test install:

```bash
./target/release/simard install --simard-home "$HOME/.simard-prod"
```

`simard install` reloads user systemd, enables both units, and restarts them
after a successful install:

```text
simard-ooda.service
simard-signal.service
```

The generated units include a deterministic tool `PATH`:
`$HOME/.local/bin:$HOME/.cargo/bin:$SIMARD_HOME/bin:/usr/local/bin:/usr/bin:/bin`.
That lets the daemon find user-installed tools such as `amplihack` without
depending on interactive shell aliases or profile files.

## 2. Verify the services

Check systemd first:

```bash
systemctl --user status simard-ooda.service --no-pager
systemctl --user status simard-signal.service --no-pager
```

Then inspect recent logs:

```bash
journalctl --user -u simard-ooda.service -n 100 --no-pager
journalctl --user -u simard-signal.service -n 100 --no-pager
```

The OODA service should emit cycle summaries. The Signal service is expected to stay running when the `[signal]` table is configured; if Signal is not configured, it exits or reports the missing configuration according to the Signal channel contract.

!!! note "Actor-session startup cleanup (#5005)"
    Every OODA daemon start clears prior-process actor sessions before
    goal-cycle work.

The cleanup makes restarts, state-root migrations, and changes to
`SIMARD_OBSERVE_ONLY` self-healing even when copied leases have future expiry
times. It affects only the transient `actor_sessions` table in
`$SIMARD_HOME/typed-ooda/outcomes.sqlite3`; durable outcomes, requests, effects,
and claims must remain intact.

The startup path fails if the ledger cannot be opened or purged instead of
running cycles against stale lease state. Do not work around startup failures
with a manual SQLite `DELETE`; correct the reported path, permission, lock, or
storage error and restart the service. See the
[actor-session startup purge reference](../reference/actor-session-startup-purge.md).

If your provider depends on environment variables, import them into the user
systemd manager before restarting services:

```bash
systemctl --user import-environment ANTHROPIC_API_KEY SIMARD_LLM_PROVIDER
systemctl --user restart simard-ooda.service simard-signal.service
```

Prefer durable provider settings in `$SIMARD_HOME/config.toml`. Do not embed
secrets in generated unit files.

## 3. Confirm the installed paths

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
ls "$SIMARD_HOME/bin/simard"
ls "$SIMARD_HOME/prompt_assets"
systemctl --user cat simard-ooda.service
systemctl --user cat simard-signal.service
```

Both unit files should use the resolved `SIMARD_HOME`:

```text
WorkingDirectory=/home/you/.simard
ExecStart=/home/you/.simard/bin/simard ooda run
ExecStart=/home/you/.simard/bin/simard signal run
Environment=PATH=/home/you/.local/bin:/home/you/.cargo/bin:/home/you/.simard/bin:/usr/local/bin:/usr/bin:/bin
```

No unit should reference `target/`, the repository checkout, or `worktrees/main`.

## 4. Run a foreground smoke test

For a one-off smoke test, run a bounded cycle directly:

```bash
"$SIMARD_HOME/bin/simard" ooda run --cycles=1 "$SIMARD_HOME"
```

This does not replace the installed systemd services. Use it to check configuration or provider credentials before rerunning `simard install`.

## 5. Prepare rollback state

The installer preserves the previous live binary under
`$SIMARD_HOME/.install-backups/` before swapping in a new one. It also prints
memory backup guidance before restarting services. When you need a state
rollback point, capture memory before deployment:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
mkdir -p "$SIMARD_HOME/.install-backups"
tar --ignore-failed-read -C "$SIMARD_HOME" \
  -czf "$SIMARD_HOME/.install-backups/memory-before-install-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" \
  cognitive goals memory config.toml
```

To restore a previous binary, stop the services, move the current binary aside, move the backup into place, and restart:

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

Use rename operations for rollback; do not copy over the live binary path.

## Configuration reference

| Setting | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root and service working directory. |
| `--simard-home PATH` | none | CLI override for `SIMARD_HOME`. |
| `--dry-run` | off | Validate and print the install plan without touching live files or invoking `systemctl`. Dry-run never executes `systemctl`. |
| `--systemd-user-dir PATH` | `$XDG_CONFIG_HOME/systemd/user` or `$HOME/.config/systemd/user` | User unit output directory; mainly for tests. |
| `--systemctl PATH` | `systemctl` | Activation command; mainly for tests with a fake executable. |
| `SIMARD_STATE_ROOT` | `$SIMARD_HOME` for installed services | Optional runtime state-root override used by runtime commands. Prefer leaving installed services on `SIMARD_HOME` unless operating a deliberate split-state setup. |

## Troubleshooting

### Installer refuses a path

Use an absolute path with no control characters or systemd percent escapes:

```bash
simard install --simard-home "$HOME/.simard"
```

Relative install roots fail closed because systemd units must be independent of the current working directory.

### Services still run an old binary

Rerun the installer from the binary you intend to deploy:

```bash
./target/release/simard install
```

Then confirm `ExecStart` points to `$SIMARD_HOME/bin/simard` and not to a worktree.

### Signal service is failing

Configure the Signal channel first, then rerun the installer or restart the service:

```bash
systemctl --user restart simard-signal.service
journalctl --user -u simard-signal.service -n 100 --no-pager
```

See [How to set up the Signal channel](./set-up-the-signal-channel.md).

## See also

- [Actor-session startup purge](../reference/actor-session-startup-purge.md)
- [Simard installer reference](../reference/simard-installer.md)
- [Simard CLI reference](../reference/simard-cli.md)
- [Daemon mode](../daemon-mode.md)
- [How to set up the Signal channel](./set-up-the-signal-channel.md)
