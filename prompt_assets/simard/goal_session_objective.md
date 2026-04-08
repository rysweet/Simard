Assess this goal's current status by:
1. Check the repository state, open issues, and recent commits to understand where things stand.
2. Decide whether this goal needs an amplihack coding session to make progress.
3. If work is needed: create a GitHub issue describing the specific task, then launch `simard engineer` or `amplihack copilot` to handle it.
4. If the goal is already progressing or blocked, report the status without launching new work.

End your response with a PROGRESS line indicating your assessed completion percentage (0-100), e.g.: PROGRESS: 45

Concrete commands you can use:
- Create a GitHub issue: `gh issue create --repo rysweet/Simard --title "<title>" --body "<body>"`
- Create a branch: `git checkout -b feat/<description>`
- Launch an amplihack coding session: `amplihack copilot` then type your task
- Run tests: `cargo test 2>&1 | tail -20`
- Check build: `cargo check 2>&1`
- Open a PR: `gh pr create --title "<title>" --body "<body>"`
- Check CI status: `gh run list --limit 5`
