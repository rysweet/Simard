---
name: pr-illustrated-guide
version: 1.0.0
description: |
  Generates a plain-language illustrated walkthrough for non-trivial pull
  requests at the end of default-workflow. Detects the hosting platform
  (GitHub or Azure DevOps) from the git remote, skips trivial PRs with
  configurable thresholds, and produces a reviewer-oriented markdown document
  that explains the problem, the approach, and the step-by-step implementation
  with code snippets, mermaid diagrams, deep links to the diff, and — when GUI
  or TUI paths change — embedded screenshots.
  Use when: a non-trivial PR has just been opened and an unfamiliar reviewer
  needs a clear, complete, jargon-free explanation of what changed and why.
invokes:
  skills:
    - mermaid-diagram-generator # For mermaid syntax generation and formatting
  agents:
    - visualization-architect # For multi-level architecture / flow diagrams
---

# PR Illustrated Guide Skill

## Purpose

Turn a pull request into a **plain-language, illustrated walkthrough** aimed at
a reviewer who has never seen the code before. The guide answers three
questions in order — _what problem does this solve?_, _what approach was
taken?_, and _how was it implemented, step by step?_ — and illustrates the
answers with code snippets, mermaid diagrams, deep links into the diff, and
(when relevant) real screenshots of GUI/TUI changes.

The skill is **declarative**: it documents a procedure an agent follows using
the `gh` / `az` CLIs and the repository tools it already has. It writes a
markdown file and echoes it to stdout. Posting that markdown to the PR is an
explicit, optional final step — never automatic.

## When to Run

- At the **end of `default-workflow`** (Phase 5/6, after the PR is created),
  on demand, or whenever someone asks for "a walkthrough of this PR".
- Skipped automatically for trivial PRs (see [Step 2](#step-2--trivial-pr-filter)).

## Invoked Capabilities

| Capability                  | Type  | Used for                                                       |
| --------------------------- | ----- | -------------------------------------------------------------- |
| `mermaid-diagram-generator` | skill | Producing valid mermaid for flow / sequence / architecture art |
| `visualization-architect`   | agent | Designing multi-level diagrams when the change is large        |

## Procedure

The skill executes ten ordered steps. Steps 2 and 8 can short-circuit (skip the
PR, or skip screenshots) — both emit an **explicit, announced** notice rather
than failing or degrading silently.

### Step 1 — Platform detection

Detect the hosting platform from the origin remote so the rest of the procedure
uses the correct CLI:

```bash
remote_url="$(git remote get-url origin)"
case "$remote_url" in
  *github.com*)                       platform="github" ;;   # use gh
  *dev.azure.com*|*visualstudio.com*) platform="azure"  ;;   # use az + azure-devops ext
  *)                                  platform="unknown" ;;  # announce + stop
esac
```

- **GitHub** → drive everything through the `gh` CLI.
- **Azure DevOps** → drive everything through `az` with the `azure-devops`
  extension (`az extension add --name azure-devops` if not already present).
- **Unknown remote** → emit a clear notice (`Unsupported remote: <url>`) and stop;
  never guess.

### Step 2 — Trivial-PR filter

Skip PRs that are too small to be worth a full walkthrough. The filter uses
**OR logic**: any single trivial signal causes a skip.

**Configurable constants (all overridable):**

| Constant            | Default                                                     | Meaning                                        |
| ------------------- | ---------------------------------------------------------- | ---------------------------------------------- |
| `MIN_FILES_CHANGED` | `3`                                                        | Skip if fewer than this many files changed     |
| `MIN_LINES_CHANGED` | `30`                                                       | Skip if fewer than this many total lines changed |
| `TRIVIAL_PATH_GLOBS`| `*.lock`, `*.cfg`, `*.toml` (config-only), `*.md` (typo-only), `*.txt`, `*.ini` | Skip if **every** changed path matches a trivial glob |

**Skip rule:**

```
skip = (files_changed < MIN_FILES_CHANGED)
    OR (lines_changed  < MIN_LINES_CHANGED)
    OR (every changed path matches TRIVIAL_PATH_GLOBS)
```

**Sourcing the counts:**

- GitHub: `gh pr diff <pr> --name-only` (file list) and `git diff --stat <base>...<head>`
  (line counts), or `gh pr view <pr> --json files` for `additions`/`deletions`.
- Azure DevOps: resolve both endpoints of the diff, then count —
  `az repos pr show --id <pr> --query '{base:lastMergeTargetCommit.commitId, head:lastMergeSourceCommit.commitId}'`
  followed by `git diff --stat <base>...<head>` for counts.

When a PR is skipped, emit an **explicit skip notice** and stop — for example:

```
[pr-illustrated-guide] Skipped: 2 files / 11 lines changed
(below MIN_FILES_CHANGED=3 / MIN_LINES_CHANGED=30). No guide generated.
```

To force a guide on a small PR, override the constants (e.g.
`MIN_FILES_CHANGED=0 MIN_LINES_CHANGED=0`).

### Step 3 — Metadata & diff fetch

Gather everything the document needs: title, body, linked issues, and the diff.

**GitHub:**

```bash
gh pr view <pr> --json number,title,body,headRefName,baseRefName,url,files,additions,deletions,closingIssuesReferences
gh pr diff <pr>                       # full unified diff
# linked issues come from `closingIssuesReferences` (the issues this PR closes);
# also parse any extra "Closes #NN" / "Fixes #NN" mentioned in the body.
```

**Azure DevOps:**

```bash
az repos pr show --id <pr> \
  --query '{title:title, description:description, source:sourceRefName, target:targetRefName, url:url}'
az repos pr work-item list --id <pr>                # linked work items
# work-item detail (for the "problem" section): az boards work-item show --id <id>
# diff: git diff <target>...<source>, or the ADO REST iterations/changes API.
```

### Step 4 — Three-part document structure

Generate a markdown document with exactly three top-level parts:

1. **What problem does this solve?** — Synthesized from the PR title, body, and
   linked issues/work items. Plain, concrete framing of the user-facing or
   developer-facing problem.
2. **Overall approach** — The strategy in two or three short paragraphs: the
   key decision, why this path over alternatives, and the shape of the change.
3. **Step-by-step implementation** — Ordered walkthrough of the substantive
   changes, each with:
   - a short prose explanation,
   - a focused **code snippet** (the few lines that matter, not the whole file),
   - a **deep link** to that hunk in the diff (see [Step 7](#step-7--deep-links)),
   - a **mermaid diagram** where a flow, sequence, or architecture relationship
     makes the change clearer.

**Delegate all mermaid** to the `mermaid-diagram-generator` skill for valid
syntax; for large or multi-level diagrams, hand off to the
`visualization-architect` agent.

### Step 5 — Style

- **Plain language, jargon-free.** Write for a competent engineer who is
  unfamiliar with _this_ codebase. Expand or avoid acronyms and internal terms.
- **Explanatory, not a changelog.** Explain the _why_, not just the _what_.
- **Complete on the critical elements.** Cover every change that affects
  behavior, security, data, or the public surface. It is fine to summarize
  repetitive or mechanical changes (see [Step 6](#step-6--smart-content)).
- **Easy to scan.** Short paragraphs, meaningful headings, code fences with
  language hints.

### Step 6 — Smart content

- **Show exemplars, not repetition.** When the same mechanical change is applied
  across many files (e.g. a renamed symbol in 40 places), show **one**
  representative example and state how many other sites received the identical
  change, rather than reproducing each one.
- **Highlight the parts a reviewer must not miss:**
  - **Configurable constants** and their defaults (call out tunable thresholds,
    timeouts, limits).
  - **Important defaults** and any behavior change to existing defaults.
  - **Non-obvious decisions** — anything a reviewer would otherwise have to
    reverse-engineer (concurrency choices, error-handling strategy, security
    trade-offs, backward-compatibility shims).

### Step 7 — Deep links

Link each discussed hunk directly to the line(s) in the hosted diff so reviewers
can jump straight to the code.

- **GitHub** — `…/pull/<n>/files#diff-<file-sha>R<line>` (the `diff-<sha>` anchor
  is the SHA-256 of the file path; `R<line>` targets the right/after side, `L<line>`
  the left/before side). When the per-file SHA is impractical to compute, link to
  the exact line via a commit-pinned blob permalink — `…/blob/<head-sha>/<path>#L<line>`
  — which still deep-links to the precise line without the diff hash.
- **Azure DevOps** — `…/pullrequest/<id>?_a=files&path=<urlencoded-path>&line=<n>&lineEnd=<n>&lineStartColumn=1&lineEndColumn=1&type=2`.

Build links from the platform base URL discovered in Step 1 + the PR id from
Step 3.

### Step 8 — GUI / TUI screenshots (conditional)

Only when the diff touches **GUI or TUI** code paths, capture and embed real
screenshots:

- **Web GUI** → primary tool is **Playwright**: launch the app, navigate to the
  changed view, capture a PNG, save it beside the document, and embed it with a
  caption.
- **TUI** → use `asciinema` / terminal capture (optional) to record the changed
  flow.

This step **degrades visibly, never silently**. Skip — with an explicit note in
the document — when:

- no GUI/TUI paths are touched (`Screenshots: not applicable — no GUI/TUI paths changed`), or
- the tool is unavailable / there is no display
  (`Screenshots unavailable: Playwright not installed` or `no GUI/TUI environment`).

Determine "GUI/TUI paths" by matching changed paths against project conventions
(e.g. web/front-end directories, `*.tsx`/`*.vue`/`*.svelte`, or known TUI/dashboard
modules). Keep this match list documented and overridable.

### Step 9 — Output

- Write the document to a markdown file at the canonical path
  **`.amplihack/pr-illustrated-guide.md`** and **echo it to stdout**. When running
  inside a session, also drop a copy in the session artifacts directory (an
  additional copy, never a replacement for the canonical path).
- **Optional posting** (consumer's choice, never automatic):
  - GitHub: `gh pr comment <pr> --body-file .amplihack/pr-illustrated-guide.md`
  - Azure DevOps: post a PR thread via the threads API, e.g.
    `az devops invoke --area git --resource pullRequestThreads --route-parameters project=<proj> repositoryId=<repo> pullRequestId=<pr> --http-method POST --in-file thread.json`
    (`az repos pr update` changes PR status/metadata — it does **not** add comments).
- Embedded screenshots are saved next to the document and referenced with
  relative paths.

### Step 10 — Integration hook

This skill is designed to be invoked at **`default-workflow` Phase 5/6**, right
after the PR is created. It is purely additive documentation:

- **No recipe YAML changes are required** to ship the skill. A consumer enables
  it by invoking `Skill(skill="pr-illustrated-guide")` after PR creation.
- The skill never blocks the workflow: a skip (Step 2) or a missing screenshot
  tool (Step 8) results in an announced notice, not a failure.

## Output Contract

| Output           | Location                                   | Notes                                  |
| ---------------- | ------------------------------------------ | -------------------------------------- |
| Walkthrough doc  | `.amplihack/pr-illustrated-guide.md` + stdout | Always, unless the PR is skipped       |
| Screenshots      | next to the doc (relative paths)           | Only when GUI/TUI paths change         |
| Skip notice      | stdout                                      | When Step 2 filters the PR             |
| PR comment       | the PR thread                               | Only when the consumer opts in         |

## Failure & Degradation Policy

- **Unsupported remote** (Step 1) → announce and stop.
- **Trivial PR** (Step 2) → announce skip and stop. Override via the constants.
- **Screenshot tool missing / no display / no GUI-TUI paths** (Step 8) → embed
  an explicit note in the document and continue. Never fabricate screenshots.

## Scope

A single declarative skill (markdown only): `SKILL.md` plus `README.md`. No Rust
or core runtime changes. It composes existing CLIs (`gh`, `az`, `git`), the
`mermaid-diagram-generator` skill, and the `visualization-architect` agent.
