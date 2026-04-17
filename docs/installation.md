# Installation

Simard ships as a single Rust binary. Pick whichever install path fits your workflow; all four produce the same binary.

## Requirements

- **Linux / macOS / Windows** (x86_64 or arm64).
- **[GitHub CLI](https://cli.github.com/)** authenticated with access to `rysweet/Simard`. The repository is currently **private**, so `npx`, `install`, and the raw `curl` release URL below all require a valid `gh auth` session (or a `GH_TOKEN` / `GITHUB_TOKEN` in the environment). Unauthenticated `curl` against the release URL will return `404 Not Found`.
- Optional: **`amplihack`** on `PATH` if you plan to use the `copilot-sdk` base type or run the gym bridge. See [amplihack-comparison.md](amplihack-comparison.md) for what still depends on amplihack.

## Option 1 — `npx` (easiest)

```bash
# Run Simard directly — downloads the latest release into ~/.simard/bin
npx github:rysweet/Simard meeting repl

# Install the binary locally without running it
npx github:rysweet/Simard install
```

The npx wrapper (`bin.js`) resolves the right platform asset from GitHub Releases using `gh release download`. See [reference/npx-npm-install.md](reference/npx-npm-install.md) for details.

## Option 2 — GitHub Releases

Because the repo is private, use `gh release download` (authenticated) rather than raw `curl`:

```bash
gh release download --repo rysweet/Simard --pattern 'simard-linux-x86_64.tar.gz' --output - | tar xz
chmod +x simard
sudo mv simard /usr/local/bin/
```

Replace `linux-x86_64` with `linux-aarch64`, `macos-x86_64`, `macos-aarch64`, or `windows-x86_64` as needed. Once the repo is made public, `curl -L https://github.com/rysweet/Simard/releases/latest/download/simard-linux-x86_64.tar.gz | tar xz` will also work.

## Option 3 — From source

```bash
git clone https://github.com/rysweet/Simard.git
cd Simard
cargo build --release
# Binary at target/release/simard
```

## Option 4 — Cargo

```bash
cargo install --git https://github.com/rysweet/Simard.git
```

## Verifying the install

```bash
simard --version
simard gym list | head
```

## Environment variables

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | RustyClawd base type. |
| `CLAUDE_CODE_BIN` | Path to `claude` (default: `claude`). |
| `MS_AGENT_FRAMEWORK_BIN` | Path to MS Agent Framework binary. |
| `SIMARD_COPILOT_GH_ACCOUNT` | GitHub account for copilot auth (e.g., `rysweet_microsoft`). |
| `SIMARD_COMMIT_GH_ACCOUNT` | GitHub account for git commits (e.g., `rysweet`). |

## Upgrading

```bash
simard update    # pulls the latest release into ~/.simard/bin
```

## Next

- [Quickstart](quickstart.md)
- [CLI reference](reference/simard-cli.md)
- [Troubleshooting](troubleshooting.md)
