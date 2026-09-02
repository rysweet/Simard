# npx / npm Install

Install and run Simard via npm's `npx` command. For a long-running host, the
canonical path is `simard install`: `npx` obtains the release binary, then the
installer deploys that binary, prompt assets, and user systemd units under
`SIMARD_HOME`.

## Prerequisites

- [Node.js](https://nodejs.org/) 18+
- [GitHub CLI](https://cli.github.com/) authenticated with access to `rysweet/Simard`

## Usage

```bash
# Run any Simard command directly
npx github:rysweet/Simard <command> [args...]

# Examples
npx github:rysweet/Simard meeting repl "weekly sync"
npx github:rysweet/Simard engineer run single-process /path/to/repo "task"

# Install the binary/assets and restart the user services
npx github:rysweet/Simard install
```

## How It Works

1. `npx` downloads the package from the GitHub repo
2. `bin.js` detects your platform (linux/darwin/win32, x86_64/aarch64)
3. Downloads the matching release binary via `gh release download`
4. Falls back to `curl` for public access if `gh` is unavailable
5. Runs the `simard install` transaction, which stages the binary and prompt assets under `SIMARD_HOME`
6. Writes `simard-ooda.service` and `simard-signal.service`
7. Reloads, enables, and restarts the user services through `systemctl --user`

## Self-Management Commands

| Command | Description |
|---------|-------------|
| `simard install` | Install binary/assets to `SIMARD_HOME`, write user systemd units, preserve the previous binary, and restart OODA/Signal services |
| `simard update` | Self-update to the latest release; planned to delegate release deployment into the installer transaction |

See [Simard installer reference](./simard-installer.md) for the contract,
including `--simard-home`, `--dry-run`, rollback artifacts, and hermetic test
overrides.
