---
title: Verify the journal's clean narrative
description: Step-by-step how-to for confirming that the daily-journal narrative is captured from the agent's clean result-file channel, not from recipe-runner stdout — regenerating the day's journal, fetching the authenticated GET /api/journal/render/<date>, and proving the first paragraph is real prose free of the copilot launcher banner and the agent's tool-call trace.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/journal-narrative-result-channel.md
  - ../reference/journal-api.md
  - ../howto/browse-the-simard-journal.md
  - ../howto/verify-the-distillation-semantic-handoff.md
  - ../concepts/copilot-launcher-preamble-stripping.md
---

# Verify the journal's clean narrative

Use this how-to to confirm that the daily journal is captured the **clean** way:
each agent pass (the narrative **draft** and its plain-language **rewrite**)
writes its finished report to a dedicated result **file** that Simard reads, and
Simard never scrapes `recipe-runner-rs` stdout. The property you are verifying is
that the launcher banner and the agent's own tool-call trace can **no longer**
appear in the stored narrative — because stdout is never read as the result.

For the mechanism, see the
[Journal narrative result channel](../reference/journal-narrative-result-channel.md).

## Before you start

- The OODA daemon must be **running** and serving the dashboard on
  `http://127.0.0.1:8080` (adjust host/port to your deployment).
- You need the dashboard login code from `~/.simard/.dashkey`.
- Confirm the hot-reload recipes carry the file-channel contract (a stale asset
  is the one footgun this fix guards against, loudly):

  ```console
  $ grep -l narrative_output ~/.simard/prompt_assets/simard/recipes/journal-narrative.yaml
  $ grep -l plain_output     ~/.simard/prompt_assets/simard/recipes/journal-plain-language.yaml
  ```

  Both must print their path. A missing marker means the deployed recipe predates
  the fix; re-run `scripts/redeploy-local.sh` so the current recipes reach
  `~/.simard/prompt_assets/`.

## 1. Regenerate today's journal

The journal is rolling and regenerated as the day unfolds; wait for the next
journal thread tick, or force a regeneration for today's date so the entry is
produced by the current (result-file) code path. Then note the date you want to
inspect, e.g. `2026-07-07`.

## 2. Authenticate and fetch the rendered entry

Log in with the dashkey (form-urlencoded `code=<dashkey>`), keep the returned
`simard_session` cookie, and fetch the server-rendered entry:

```bash
DATE=2026-07-07
KEY="$(cat ~/.simard/.dashkey)"

# POST the login code; save the simard_session cookie.
curl -s -c /tmp/simard.cookies \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "code=${KEY}" \
  http://127.0.0.1:8080/api/login >/dev/null

# Fetch the rendered journal HTML with the session cookie.
curl -s -b /tmp/simard.cookies \
  "http://127.0.0.1:8080/api/journal/render/${DATE}" > /tmp/journal.html
```

## 3. Assert the first paragraph is clean prose

The first `<p>` of `journal-narrative` must be the real report opening (for
example, "On July 7, 2026, Simard operated in a largely self-directed decision
cycle…") and must contain **none** of the launcher/tool-trace markers. Grep for
each contaminant — every one of these must return **no match**:

```bash
! grep -F 'nested amplihack session' /tmp/journal.html \
  && ! grep -F 'launching copilot binary' /tmp/journal.html \
  && ! grep -F 'NODE_OPTIONS=' /tmp/journal.html \
  && ! grep -F 'Read draft.ctx' /tmp/journal.html \
  && ! grep -F 'lines read' /tmp/journal.html \
  && ! grep -F '●' /tmp/journal.html \
  && ! grep -F '│' /tmp/journal.html \
  && ! grep -F '└' /tmp/journal.html \
  && echo "CLEAN: no launcher/tool-trace markers in the rendered journal"
```

To eyeball the opening paragraph directly:

```bash
grep -o '<p class="journal-narrative[^>]*">[^<]*' /tmp/journal.html | head -1
```

It should begin with prose, not with `2026-…T…Z  WARN` or a `●` glyph.

## What you just proved

- The narrative was captured from the agent's **result file**, not stdout — the
  copilot launcher banner (`WARN nested amplihack session`, `INFO launching
  copilot`, `ℹ NODE_OPTIONS=…`) and the agent's tool-call trace (`● Read
  draft.ctx`, `│`, `└ N lines read`) never entered the stored text.
- If the agent had failed to write its result file, the pass would have degraded
  **loudly** to the honest offline drafter/reviewer — never to a stored stdout
  dump. See the
  [failure semantics](../reference/journal-narrative-result-channel.md#failure-semantics).

## If markers still appear

- **Stale hot-reload recipe.** Re-check the `grep -l` marker test in
  [Before you start](#before-you-start); re-run `scripts/redeploy-local.sh` and
  regenerate.
- **Old binary still deployed.** Confirm the running daemon includes this fix
  (`src/journal/recipe.rs` reads `harvest_narrative_file`, not a `RecipeEnvelope`
  from stdout) and redeploy if not.
- **Degraded-but-clean.** If the operator log shows an
  `AdapterInvocationFailed` for the journal adapter, the pass used the honest
  offline path — the text is clean but deterministic; investigate the recorded
  Overseer diagnosis rather than the render.
