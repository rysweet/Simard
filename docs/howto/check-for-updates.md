---
title: Check for updates and control update notifications
description: "How to see when a new Simard version is available, respond to the upgrade prompt, run non-interactively, and suppress the check entirely."
last_updated: 2026-07-09
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/update-check.md
  - ../concepts/update-check-design.md
  - ../safe-self-update.md
  - ./monitor-simard-with-tui.md
---

# Check for updates and control update notifications

Simard checks GitHub Releases for a newer version at launch and, if one
exists, prints a one-line notice. In an interactive terminal it can also
offer a one-key upgrade prompt. This guide shows what you see, how to
respond, and how to control or suppress the behavior.

The check is non-blocking and fail-open: it never delays startup, and a
network or GitHub API failure never blocks or crashes Simard (#2250).

## 1. What the update notice looks like

### CLI

When you run any `simard` command (including `simard meeting`) and a
newer version exists, you see a notice on stderr:

```
simard: update available 0.26.0 -> 0.27.0
  https://github.com/rysweet/Simard/releases/tag/v0.27.0
```

The first line is always the exact form
`simard: update available X.Y.Z -> A.B.C`. The notice appears after the
command starts — it never delays startup — and it goes to stderr, so it
stays out of `simard <command> | jq` pipelines.

### TUI

In `simard-tui`, the notice appears in the footer bar instead:

```
Update available: 0.26.0 -> 0.27.0  https://github.com/rysweet/Simard/releases/tag/v0.27.0  | Alt+1–9: tabs | Tab/Shift+Tab: cycle | ←/→: prev/next | q: quit
```

The `Update available: …` prefix is prepended to the normal footer
keybindings and highlighted while an update is available; it stays
visible for the rest of the session. The version format matches the CLI
banner (ASCII `->`, no `v`). The TUI never shows an interactive prompt.

## 2. Respond to the upgrade prompt

In an interactive terminal, the CLI banner is followed by:

```
Upgrade now? [y/N] 
```

- Press **`y`** then Enter → Simard prints
  `Run \`simard update\` to upgrade.` It does **not** install
  anything in place; upgrading stays an explicit, separate step.
- Press **Enter** (or anything else) → treated as **No**.
- Do nothing → after ~10 seconds the prompt times out and is treated as
  **No**, so an unattended launch is never blocked.

The prompt is skipped automatically when stdin/stderr is not a terminal
(for example, output redirected to a file or a pipe) and in the TUI.

## 3. Upgrade

When you are ready to upgrade:

```bash
simard update
```

Today this downloads the pre-built binary for your platform, verifies it, and
replaces the current binary. The planned installer integration is for
`simard update` to hand the verified release binary and matching prompt assets
to the same staging, backup, systemd activation, and rollback transaction as
`simard install`.

If no pre-built binary exists for your platform, the installer-based source
fallback is:

```bash
git pull origin main
cargo build --release
./target/release/simard install
```

The source-built fallback uses the installer. Do not copy the built binary over
`~/.simard/bin/simard`; the installer transaction stages the binary, preserves
the previous one for rollback, updates prompt assets, and restarts the user
services safely.

For autonomous daemon upgrades with safety rails (drain → snapshot →
pre-test → swap → validate → rollback), use `simard safe-update` — see
[Safe self-update](../safe-self-update.md).

## 4. Run non-interactively (banner, no prompt)

To keep the informational banner but skip the `[y/N]` prompt — useful in
scripts, cron, or CI where you still want update visibility in logs:

```bash
export SIMARD_NONINTERACTIVE=1
```

Or for a single invocation:

```bash
SIMARD_NONINTERACTIVE=1 simard status
```

With `SIMARD_NONINTERACTIVE=1`, the update banner still prints, but the
prompt is never shown.

## 5. Disable the update check entirely

To skip the check completely — no cache read, no network request, no
banner, no prompt:

```bash
export SIMARD_NO_UPDATE_CHECK=1
```

Or for a single invocation:

```bash
SIMARD_NO_UPDATE_CHECK=1 simard engineer run --goal=my-goal
```

This short-circuits both CLI and TUI modes. `SIMARD_NO_UPDATE_CHECK`
takes precedence over `SIMARD_NONINTERACTIVE`: if it is set to `1`,
nothing runs regardless of the interactive setting.

**When to disable:**

- Air-gapped or restricted-network environments where the HTTPS request
  would fail or be flagged.
- CI/CD pipelines where even the banner adds noise.
- Scripted automation where any stderr output is undesirable.

## 6. Understand the 24-hour cache

To avoid a request on every launch, the result is cached for 24 hours at:

```
~/.simard/update_cache.json
```

(under Simard's state root, so `SIMARD_STATE_ROOT` moves it along with
the rest of Simard's state). While the cache is fresh (< 24h), Simard
reuses the stored result and makes **no** network request. After 24
hours the next launch refreshes it.

To force an immediate re-check, delete the cache and launch again:

```bash
rm -f ~/.simard/update_cache.json
simard status
```

The cache file is written atomically with `0600` permissions inside a
`0700` directory, and a corrupt or missing cache is treated as a
harmless miss (the check just re-fetches).

## 7. Verify the check works

Run a command that stays alive for a moment and watch stderr. Very fast
commands (`simard --version`) may exit before the background check
completes:

```bash
simard status 2>&1 | head -10
```

If you are already on the latest version, no notice appears — the check
only emits output when a strictly newer version exists.

To confirm the check is **disabled**:

```bash
SIMARD_NO_UPDATE_CHECK=1 simard --version
```

No notice appears regardless of available updates.

## 8. How it works (brief)

1. At launch, Simard spawns a background thread (fire-and-forget for the
   CLI, channel-delivered for the TUI).
2. If the on-disk cache is fresh (< 24h), it reuses that result and skips
   the network.
3. Otherwise it fetches
   `https://api.github.com/repos/rysweet/Simard/releases/latest`
   in-process via `ureq` (no `gh`/`curl` subprocess), with a 5-second
   timeout and no authentication.
4. It compares the release tag against the running binary's version
   using the `semver` crate. Pre-releases (e.g. `1.0.0-rc1`) count as
   *older* than the matching release (`1.0.0`), so you are never prompted
   to "upgrade" to a pre-release.
5. If a strictly newer version exists, it prints the banner (CLI) or
   sends it to the footer (TUI), and offers the prompt when interactive.

The check never blocks, never writes to stdout, and never installs
anything on its own. Any failure is logged (via tracing) and otherwise
ignored — it will not crash or delay Simard.

## See also

- [Update check reference](../reference/update-check.md) — full API and
  behavior specification.
- [Update check design](../concepts/update-check-design.md) — the design
  rationale.
- [Safe self-update](../safe-self-update.md) — autonomous daemon upgrade
  flow with rollback.
- [Monitor with TUI](./monitor-simard-with-tui.md) — TUI usage guide.
