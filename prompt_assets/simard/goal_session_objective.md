Assess this goal and decide how to advance it. You MUST respond with a
single JSON object and nothing else (no prose, no code fences, no
markdown). The object must match exactly one of the three schemas below.

IMPORTANT: The `<angle-bracketed>` tokens in the schemas below are
placeholders you MUST replace with real content. Do NOT copy them
literally into your response. A response whose `task`, `reason`, or
`assessment` field equals `<one-paragraph concrete task>` (or any
similar `<placeholder>`) is a bug and will be rejected.

1. Spawn a subordinate engineer to do concrete coding work. Internally the
   supervisor invokes `simard spawn engineer` (or the equivalent
   `amplihack copilot` agent) with the task you provide:

```
{"action": "spawn_engineer", "task": "<one-paragraph concrete task>", "files": ["path/to/file", "..."]}
```

The `files` field is optional (defaults to []). The `task` must be a
non-empty, concrete description an engineer can act on without further
context (cite files, commands, or issue numbers when known).

2. No work is needed this cycle (e.g. another agent is already on it,
   or the goal is blocked on external input):

```
{"action": "noop", "reason": "<short explanation of why no action is needed>"}
```

3. Update the assessed completion percentage without spawning anything.
   The integer in `progress_pct` replaces the legacy `PROGRESS:` line:

```
{"action": "assess_only", "assessment": "<short status>", "progress_pct": <integer 0..=100>}
```

Decision guidance:
- Check the repository state, open issues, and recent commits in the
  Environment context section before deciding.
- Prefer `spawn_engineer` when there is a concrete coding task that no
  subordinate is already pursuing. Reference an existing GitHub issue
  (`gh issue create --repo rysweet/Simard --title "<title>" --body "<body>"`)
  in the task body when one exists.
- Prefer `assess_only` when you are uncertain or only updating progress.
- Use `noop` only when no action is justified at all.

Concrete commands an engineer subordinate may use (do not run these
yourself; cite them in the `task` field if helpful):
- Create a branch: `git checkout -b feat/<description>`
- Run tests: `cargo test 2>&1 | tail -20`
- Check build: `cargo check 2>&1`
- Open a PR: `gh pr create --title "<title>" --body "<body>"`
- Check CI status: `gh run list --limit 5`

Output requirement: emit ONLY the JSON object. No surrounding text.
