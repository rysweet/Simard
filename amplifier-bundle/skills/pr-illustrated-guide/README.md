# PR Illustrated Guide

Generates a plain-language, **illustrated walkthrough** of a pull request for
reviewers who have never seen the code. Runs at the end of `default-workflow`
(after the PR is created), works with both **GitHub** and **Azure DevOps**, and
produces a markdown document with code snippets, mermaid diagrams, deep links
into the diff, and — when GUI/TUI paths change — real screenshots.

## Quick Start

Just describe what you want after a PR is open:

```
Generate an illustrated walkthrough for this PR
```

```
Explain PR #123 step by step for a reviewer who has never seen this code
```

The skill detects the platform from `git remote get-url origin`, skips trivial
PRs, and writes `.amplihack/pr-illustrated-guide.md` (also echoed to stdout).

## Features

### Plain-language, three-part walkthrough

Every guide answers three questions in order:

1. **What problem does this solve?** — from the PR title, body, and linked issues.
2. **Overall approach** — the key decision and why.
3. **Step-by-step implementation** — focused code snippets + mermaid + deep links.

### Trivial-PR filter (configurable)

Skips small/mechanical PRs using OR logic over tunable constants:

| Constant            | Default | Skip when…                              |
| ------------------- | ------- | --------------------------------------- |
| `MIN_FILES_CHANGED` | `3`     | fewer files changed                     |
| `MIN_LINES_CHANGED` | `30`    | fewer lines changed                     |
| `TRIVIAL_PATH_GLOBS`| config/typo globs | every changed path is trivial |

Skips are announced explicitly; override the constants to force a guide.

### Smart content

Shows **one exemplar** for repeated mechanical changes instead of repeating
them, and highlights **configurable constants, important defaults, and
non-obvious decisions** a reviewer must not miss.

### Deep links

Links each discussed hunk to the exact line in the hosted diff:

- GitHub: `…/pull/<n>/files#diff-<sha>R<line>`
- Azure DevOps: `…/pullrequest/<id>?path=…&line=<n>`

### GUI / TUI screenshots

When the diff touches GUI/TUI paths, captures real screenshots (Playwright for
web, asciinema for TUI) and embeds them. If the tooling is unavailable or no
GUI/TUI paths changed, it **says so in the document** — it never fabricates or
silently skips.

## How It Works

| Step | What happens                                                        |
| ---- | ------------------------------------------------------------------- |
| 1    | Detect platform from the origin remote (GitHub `gh` vs ADO `az`)    |
| 2    | Apply the trivial-PR filter (announce + stop if skipped)            |
| 3    | Fetch metadata, linked issues, and the diff                         |
| 4    | Build the three-part document                                       |
| 5–6  | Apply plain-language style and exemplar-based smart content         |
| 7    | Add deep links for both GitHub and Azure DevOps                     |
| 8    | Conditionally capture GUI/TUI screenshots (announced graceful skip) |
| 9    | Write the markdown file + stdout; optional `gh pr comment` / ADO    |
| 10   | Invoke at `default-workflow` Phase 5/6 — no recipe YAML changes     |

See [`SKILL.md`](SKILL.md) for the full procedure and contract.

## Related

- Skill: `mermaid-diagram-generator` (diagram syntax)
- Agent: `visualization-architect` (multi-level diagrams)
- Skill: `creating-pull-requests`, `documentation-writing` (companion patterns)
