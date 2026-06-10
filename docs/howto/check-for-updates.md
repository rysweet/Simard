---
title: Check for updates and control update notifications
description: "How to see when a new Simard version is available, suppress the check, and upgrade."
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/update-check.md
  - ../safe-self-update.md
  - ./monitor-simard-with-tui.md
---

# Check for updates and control update notifications

Simard checks GitHub Releases for a newer version on every launch.
This guide explains what you see, how to act on it, and how to
suppress it.

## 1. What the update notice looks like

### CLI

When you run any `simard` command and a newer version exists, you see
a yellow notice on stderr:

```
simard: update available v0.19.0 → v0.20.0
  https://github.com/rysweet/Simard/releases/tag/v0.20.0
  Run `simard self-update` to upgrade.
```

The notice appears after the command starts — it never delays startup.
If the network check takes longer than 5 seconds, no notice is shown.

### TUI

In `simard-tui`, the notice appears in the footer bar:

```
Update available: v0.19.0 → v0.20.0  https://...  Run `simard self-update` to upgrade.  | Alt+1‥6: tabs | ←/→: cycle | q: quit
```

The footer turns yellow when an update is available. It appears 1–5
seconds after launch and stays visible for the rest of the session.

## 2. Upgrade

If the notice says "Run `simard self-update` to upgrade":

```bash
simard self-update
```

This downloads the pre-built binary for your platform and replaces the
current binary.

If the notice says "(no pre-built binary for this platform — build
from source)", build from the repository:

```bash
git pull origin main
cargo build --release
cp target/release/simard ~/.simard/bin/simard
```

For autonomous daemon upgrades with safety rails, use
`simard safe-update` — see [Safe self-update](../safe-self-update.md).

## 3. Disable the update check

Set the environment variable before launching:

```bash
export SIMARD_NO_UPDATE_CHECK=1
```

Or for a single invocation:

```bash
SIMARD_NO_UPDATE_CHECK=1 simard engineer run --goal=my-goal
```

This suppresses the check in both CLI and TUI modes. No network
request is made.

**When to disable:**

- Air-gapped or restricted-network environments where the HTTP
  request would fail or be flagged.
- CI/CD pipelines where the notice adds noise.
- Scripted automation where even stderr output is undesirable.

## 4. Verify the check works

Run a command that takes at least a few seconds and look for the
notice on stderr. Fast commands like `simard --version` may exit
before the background check completes — use a longer-running command:

```bash
simard status 2>&1 | head -10
```

If you are already on the latest version, no notice appears. To
confirm the check ran silently (no newer version), there is no visible
output — the check only produces output when an update is available.

To verify the check is *disabled*:

```bash
SIMARD_NO_UPDATE_CHECK=1 simard --version
```

No notice appears regardless of available updates.

## 5. How it works (brief)

1. Spawns a background thread on startup.
2. The thread calls `gh api repos/rysweet/Simard/releases/latest`
   (falls back to `curl` if `gh` fails).
3. Compares the release tag against the running binary's version.
4. Prereleases (e.g., `1.0.0-rc1`) are treated as older than the
   corresponding release (`1.0.0`), so you are not prompted to
   "upgrade" to a prerelease.
5. If newer, prints the notice (CLI) or sends it to the TUI via
   an internal channel.

The check never blocks, never writes to stdout, and never
auto-executes anything.

## See also

- [Update check reference](../reference/update-check.md) — full API
  and behavior specification.
- [Safe self-update](../safe-self-update.md) — autonomous daemon
  upgrade flow with rollback.
- [Monitor with TUI](./monitor-simard-with-tui.md) — TUI usage guide.
