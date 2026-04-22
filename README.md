# Simard

An autonomous engineer who drives and curates agentic coding systems.

Named after [Suzanne Simard](https://en.wikipedia.org/wiki/Suzanne_Simard), the scientist who discovered how trees communicate through underground fungal networks.

## What is Simard?

Simard is a terminal-native engineering agent built in Rust. She operates like a disciplined software engineer: she understands codebases, works through tasks in explicit sessions, preserves useful memory, evaluates herself against benchmarks, and improves through structured review loops.

### Relationship to Python amplihack

Simard is the **Rust successor to [Python amplihack](https://github.com/rysweet/amplihack)**. The intent is for Simard to become a drop-in replacement: the same agentic-coding capabilities, delivered as a single static Rust binary, with no Python runtime or `pip install` step required.

That goal is **not yet fully met**. Today Simard implements the core engineer/meeting/goal-curation/improvement-curation/gym/OODA loops natively in Rust, but does not yet cover the full amplihack command surface (37 slash commands) or skill catalog (117 skills). Parity work is tracked in the [parity matrix](docs/reference/parity-matrix.md) and in issues [#896](https://github.com/rysweet/Simard/issues/896), [#897](https://github.com/rysweet/Simard/issues/897), and [#898](https://github.com/rysweet/Simard/issues/898).

If you are a current Python amplihack user: see [Migrating from Python amplihack](#migrating-from-python-amplihack) below for the command map.

## Install

### With npx (easiest)

Requires [GitHub CLI](https://cli.github.com/) authenticated with repo access.

```bash
# Run Simard directly
npx github:rysweet/Simard meeting repl

# Install the binary locally (~/.simard/bin)
npx github:rysweet/Simard install
```

### From GitHub Releases

```bash
# Download the latest release binary
curl -L https://github.com/rysweet/Simard/releases/latest/download/simard-linux-x86_64.tar.gz | tar xz
chmod +x simard
sudo mv simard /usr/local/bin/
```

### From Source

```bash
git clone https://github.com/rysweet/Simard.git
cd Simard
cargo build --release
# Binary at target/release/simard
```

### With Cargo

```bash
cargo install --git https://github.com/rysweet/Simard.git
```

## Migrating from Python amplihack

Simard's CLI is grouped by product mode rather than slash commands. Common amplihack workflows map as follows. Items marked **TBD** are not yet implemented natively in Simard — see the [parity audit (#898)](https://github.com/rysweet/Simard/issues/898) for status and to request prioritization.

| Python amplihack                  | Simard equivalent                                     | Status            |
| --------------------------------- | ----------------------------------------------------- | ----------------- |
| `/dev <task>`                     | `simard engineer terminal <topology> <objective>`     | parity            |
| `/improve <area>`                 | `simard improvement-curation run <base> <topo> <obj>` | parity            |
| `/analyze <target>`               | `simard engineer read <topology>` + meeting review    | partial           |
| `/investigation <topic>`          | `simard meeting repl <topic>`                         | partial           |
| `/customize`                      | direct edits to `prompt_assets/` + recompile          | partial           |
| skills auto-routing (117 skills)  | curated set of native Rust subcommands                | partial — see #898 |
| `amplihack install`               | `simard install` or `npx github:rysweet/Simard install` | parity          |

If your workflow is not covered above, please open an issue referencing #898 so it lands in the parity matrix.

## Quick Start

```bash
# Run an engineering session
simard engineer run single-process /path/to/repo "improve test coverage"

# Have a meeting with Simard
simard meeting repl "weekly sync"

# List gym benchmarks
simard gym list

# Run a benchmark
simard gym run repo-exploration-local
```

## CLI Commands

### Engineer Mode
```bash
simard engineer run <topology> <workspace-root> <objective>
simard engineer terminal <topology> <objective>        # interactive PTY
simard engineer copilot-submit <topology>              # submit to copilot
simard engineer read <topology>                        # read last session
```

### Meeting Mode
```bash
simard meeting run <base-type> <topology> <objective>
simard meeting repl <topic>                            # interactive REPL
simard meeting read <base-type> <topology>             # read last meeting
```

### Goal Curation
```bash
simard goal-curation run <base-type> <topology> <objective>
simard goal-curation read <base-type> <topology>
```

### Gym Benchmarks
```bash
simard gym list                        # list all scenarios
simard gym run <scenario-id>           # run a scenario
simard gym compare <scenario-id>       # compare results
simard gym run-suite <suite-id>        # run a suite
```

### Self-Management
```bash
simard update                          # self-update to the latest release
simard install                         # install binary to ~/.simard/bin
```

### Other Commands
```bash
simard improvement-curation run <base-type> <topology> <objective>
simard review run <base-type> <topology> <objective>
simard bootstrap run <identity> <base-type> <topology> <objective>
```

## Base Types

Simard delegates work to agent runtimes through base types:

| Base Type | Description | Status |
|-----------|-------------|--------|
| `rusty-clawd` | RustyClawd SDK — LLM + tool calling pipeline | Real (needs `ANTHROPIC_API_KEY`) |
| `copilot-sdk` | amplihack copilot via PTY terminal | Real (needs `amplihack copilot`) |
| `claude-agent-sdk` | Claude Code CLI as subprocess agent | Real (needs `claude` binary) |
| `ms-agent-framework` | Microsoft Agent Framework | Real (needs `ms-agent-framework` or `python -m microsoft_agent_framework`) |
| `local-harness` | Test adapter for development | Always available |
| `terminal-shell` | Local PTY shell execution | Always available |

## Architecture

```mermaid
graph TB
    subgraph CLI["Operator CLI"]
        cmd_eng[engineer]
        cmd_meet[meeting]
        cmd_ooda[ooda run]
        cmd_dash[dashboard serve]
        cmd_gym[gym]
        cmd_review[review]
        cmd_goal[goal-curation]
        cmd_imp[improvement-curation]
    end

    subgraph Daemon["OODA Daemon (autonomous loop)"]
        observe["Observe<br/>issues · gym scores · handoffs · memory"]
        orient["Orient<br/>rank priorities"]
        decide["Decide<br/>select actions"]
        act["Act<br/>dispatch work"]
        review_step["Review & Curate"]
        observe --> orient --> decide --> act --> review_step --> observe
    end

    subgraph Actions["Action Dispatch"]
        adv_goal["Advance Goal<br/>(subordinate LLM turn)"]
        run_eng["Launch Session<br/>(PTY engineer)"]
        run_imp["Run Improvement<br/>(self-improve cycle)"]
        run_gym["Run Gym Eval<br/>(benchmark suite)"]
        consol["Consolidate Memory"]
        research["Research Query"]
        build_skill["Build Skill"]
    end

    subgraph WorkLoops["Work Loops"]
        eng_loop["Engineer Loop<br/>inspect → select → execute → verify"]
        meet_repl["Meeting REPL<br/>decisions · action items · handoff"]
        self_imp["Self-Improve Cycle<br/>eval → analyze → improve → reeval"]
    end

    subgraph Runtime["Agent Runtime"]
        bootstrap["Bootstrap<br/>config · identity · assembly"]
        session["Session Builder<br/>ports · lifecycle"]
        identity["Identity Manifests<br/>roles · capabilities · precedence"]
    end

    subgraph BaseTypes["Agent Base Types"]
        rustyclawd["RustyClawd"]
        copilot["Copilot"]
        claude["Claude SDK"]
        ms_agent["MS Agent"]
        harness["Test Harness"]
    end

    subgraph Bridges["Python Bridges (subprocess)"]
        mem_bridge["Memory Bridge<br/>cognitive memory"]
        know_bridge["Knowledge Bridge<br/>RAG · packs"]
        gym_bridge["Gym Bridge<br/>scenario eval"]
    end

    subgraph Memory["Cognitive Memory"]
        sensory["Sensory"]
        working["Working"]
        episodic["Episodic"]
        semantic["Semantic"]
        procedural["Procedural"]
        prospective["Prospective"]
    end

    subgraph Storage["Persistent State"]
        goals_store["Goal Board<br/>active · backlog"]
        improvements_store["Improvement Log"]
        metrics_store["Self-Metrics<br/>JSONL"]
        cost_store["Cost Tracking<br/>JSONL"]
        handoff_files["Handoff Files"]
    end

    subgraph Dashboard["Web Dashboard :8080"]
        dash_ui["Status · Issues · Metrics<br/>Costs · Processes · Logs"]
    end

    cmd_ooda --> Daemon
    cmd_eng --> eng_loop
    cmd_meet --> meet_repl
    cmd_dash --> Dashboard
    cmd_gym --> gym_bridge

    act --> Actions
    adv_goal --> session
    run_eng --> eng_loop
    run_imp --> self_imp
    run_gym --> gym_bridge
    consol --> mem_bridge
    research --> know_bridge

    eng_loop --> session
    meet_repl --> session
    self_imp --> gym_bridge

    session --> bootstrap --> identity
    session --> BaseTypes

    mem_bridge --> Memory
    meet_repl -.->|handoff| handoff_files
    eng_loop -.->|reads| handoff_files
    Daemon -.->|reads/writes| goals_store
    Daemon -.->|writes| metrics_store

    style Daemon fill:#2d4a3e,stroke:#4a8,color:#fff
    style CLI fill:#1a3a5c,stroke:#48a,color:#fff
    style Memory fill:#3a2d4a,stroke:#84a,color:#fff
    style Dashboard fill:#4a3a1a,stroke:#a84,color:#fff
```


## Configuration

| Environment Variable | Purpose |
|---------------------|---------|
| `ANTHROPIC_API_KEY` | API key for RustyClawd base type |
| `CLAUDE_CODE_BIN` | Path to claude binary (default: `claude`) |
| `MS_AGENT_FRAMEWORK_BIN` | Path to MS Agent Framework binary |
| `SIMARD_COPILOT_GH_ACCOUNT` | GitHub account for copilot auth (e.g., `rysweet_microsoft`) |
| `SIMARD_COMMIT_GH_ACCOUNT` | GitHub account for git commits (e.g., `rysweet`) |

## Development

```bash
# Run tests
cargo test

# Run clippy
cargo clippy --all-targets

# Format
cargo fmt --all

# Run a gym benchmark
cargo run -- gym run repo-exploration-local
```

## License

Private repository. See [rysweet/Simard](https://github.com/rysweet/Simard).
<!-- workflow smoke test - safe to merge -->
