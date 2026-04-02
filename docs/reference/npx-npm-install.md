# npx / npm Install

Install and run Simard via npm's `npx` command. This is the easiest way to get started.

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

# Install the binary locally for faster subsequent runs
npx github:rysweet/Simard install
```

## How It Works

1. `npx` downloads the package from the GitHub repo
2. `bin.js` detects your platform (linux/darwin/win32, x86_64/aarch64)
3. Downloads the matching release binary via `gh release download`
4. Falls back to `curl` for public access if `gh` is unavailable
5. Caches the binary in `~/.simard/bin/` for subsequent runs

## Self-Management Commands

| Command | Description |
|---------|-------------|
| `simard install` | Download and install the binary to `~/.simard/bin` |
| `simard update` | Self-update to the latest release |
