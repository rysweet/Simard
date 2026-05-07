---
title: How to start a meeting with Simard
description: Start a conversational meeting with Simard from the CLI or dashboard. Simard maintains conversation context, remembers past meetings, and persists outcomes on close.
last_updated: 2026-04-12
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/meeting-backend-api.md
  - ../architecture/unified-meeting-backend.md
  - ./carry-meeting-decisions-into-engineer-sessions.md
  - ./use-meeting-templates.md
  - ./export-meeting-markdown.md
---

# How to start a meeting with Simard

Meetings are natural conversations with Simard — no structured input format required. Both the CLI and dashboard provide the same meeting experience: Simard remembers the full conversation, knows her current goals and past meetings, and persists everything on close.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally (for CLI meetings)
- [ ] Dashboard is running (for WebSocket meetings)

## 1. Start a meeting from the CLI

```bash
cargo run --quiet -- meeting run local-harness single-process \
  "discuss the next Simard milestone" \
  "$STATE_ROOT"
```

`$STATE_ROOT` is the shared state directory (e.g., `target/state-root`). See the [local session tutorial](../tutorials/run-your-first-local-session.md) for setup.

Simard greets you and the conversation begins:

```text
Simard meeting session started
Topic: discuss the next Simard milestone
Type /help for commands, or just start talking.

simard> Hey Simard, what's been on your mind since our last meeting?

Simard: I've been thinking about the memory consolidation pipeline. The episodic
memories from the last three engineering sessions are piling up and I haven't
promoted the useful patterns to semantic memory yet. I'm worried that if I don't
consolidate soon, the retrieval quality on future sessions will degrade.

Also — the gym scores on the SecurityAudit scenarios are still below where I
want them. I've been stuck at 62% on the "detect-hardcoded-secrets" benchmark
and I think the issue is that my code scanning prompts are too generic...
```

Just talk. Simard maintains the full conversation history and uses it on every turn.

## 2. Start a meeting from the dashboard

Open the operator dashboard and click **Chat**. The WebSocket connection creates a meeting session automatically. Type in the chat input — the experience is identical to the CLI.

## 3. Available commands

Five slash commands are recognized during a meeting:

| Command   | What it does |
|-----------|-------------|
| `/help`   | Lists these commands |
| `/status` | Shows topic, duration, and message count |
| `/template [name]` | Lists available templates, or applies one by name |
| `/export` | Exports the meeting as a markdown file to `~/.simard/meetings/` |
| `/close` (or `/done`)  | Ends the meeting, persists transcript, and generates a summary |

Everything else is natural conversation.

### Meeting templates

Use `/template` to list the 4 built-in templates, or `/template standup` to apply one immediately. The template sets a structured agenda as initial meeting context:

| Template | Purpose |
|----------|---------|
| `standup` | Quick daily check-in — blockers, progress, plans |
| `1on1` | Operator/Simard check-in — goals, concerns, growth |
| `retro` | Sprint retrospective — went well, improve, actions |
| `planning` | Sprint/milestone planning — scope, priorities, risks |

See [How to use meeting templates](./use-meeting-templates.md) for full details.

### Markdown export

Type `/export` at any point during a meeting to write a markdown snapshot to `~/.simard/meetings/`. The file includes YAML frontmatter (topic, date, duration, message count) and the full conversation history. See [How to export meeting markdown](./export-meeting-markdown.md).

## 4. What Simard knows during a meeting

Simard's meeting context is built from three sources at session start:

1. **Personality and role** — Simard's conversational style, her operator (Ryan Sweet), her ecosystem stewardship responsibilities across all 10 repos.
2. **Live context** — Current top-5 goals, active projects, recent session outcomes, and research tracker updates from monitored developers.
3. **Cognitive memory** — Episodic memories from recent meetings, semantic knowledge relevant to the topic, and prospective plans.

This context is refreshed at session start. Within the session, Simard has access to the full conversation history (last 30 messages verbatim, earlier messages summarized).

## 5. Close the meeting

When you're done, type `/close`:

```text
simard> /close

Simard: Here's my summary of what we covered:

We discussed the memory consolidation backlog and agreed to prioritize it this
week. The gym SecurityAudit scores need attention — I'll focus on making the
code scanning prompts more specific to each benchmark scenario. We also decided
to defer the dashboard auth improvements until after the consolidation work
ships.

Meeting saved to ~/.simard/meetings/20260412T1430_discuss-the-next-simard-milestone.json
Meeting handoff written for OODA integration.
Memories stored: 1 episodic, 2 semantic, 1 prospective.
```

On close, three things are persisted:

1. **Full transcript** → `~/.simard/meetings/{timestamp}_{topic}.json`
2. **Meeting handoff** → `target/meeting_handoffs/meeting_handoff.json` (consumed by the OODA loop)
3. **Cognitive memories** — episodic record of the meeting, semantic extraction of decisions, prospective memory of agreed next steps

## 6. Review past meetings

Read back the latest meeting transcript:

```bash
cargo run --quiet -- meeting read local-harness single-process "$STATE_ROOT"
```

Or inspect the JSON directly:

```bash
ls ~/.simard/meetings/
cat ~/.simard/meetings/20260412T1430_discuss-the-next-simard-milestone.json | python3 -m json.tool
```

The JSON transcript contains:

```json
{
  "version": 1,
  "topic": "discuss the next Simard milestone",
  "started_at": "2026-04-12T14:30:00Z",
  "ended_at": "2026-04-12T15:15:00Z",
  "messages": [
    {
      "role": "user",
      "content": "Hey Simard, what's been on your mind since our last meeting?",
      "timestamp": "2026-04-12T14:30:12Z"
    },
    {
      "role": "assistant",
      "content": "I've been thinking about the memory consolidation pipeline...",
      "timestamp": "2026-04-12T14:30:18Z"
    }
  ],
  "summary": "Discussed memory consolidation priorities and gym SecurityAudit scores...",
  "message_count": 24,
  "duration_seconds": 2700
}
```

## 7. Carry meeting outcomes into engineer sessions

Meeting handoffs still integrate with the OODA loop and engineer sessions. See [How to carry meeting decisions into engineer sessions](./carry-meeting-decisions-into-engineer-sessions.md) for the full flow.

The key difference: decisions and action items are no longer extracted from structured `decision:` / `next-step:` prefixes. Instead, the `/close` summary captures them from the natural conversation and writes them into the handoff artifact.

## Troubleshooting

### Simard doesn't remember what we discussed earlier in the meeting

This should not happen — conversation history is maintained for the full session (up to 500 messages). If Simard loses context:

- Check that you're in the same meeting session (not a new one)
- Sessions longer than 30 messages use a rolling summary for earlier turns — some detail may be compressed

### The meeting transcript file wasn't created

Check that `~/.simard/meetings/` exists and is writable. The directory is created automatically on first close, but filesystem permission issues will prevent it. The close operation logs errors at `WARN` level without crashing.

### Dashboard chat feels different from CLI

It shouldn't. Both use the same `MeetingBackend`. If you notice differences, file a bug — the contract is that both interfaces provide identical Simard meeting behavior.

## Related reading

- [Unified meeting backend architecture](../architecture/unified-meeting-backend.md) — How the backend is structured.
- [Meeting backend API reference](../reference/meeting-backend-api.md) — Rust API for `MeetingBackend`.
- [Simard CLI reference](../reference/simard-cli.md) — Full command tree.
- [OODA meeting handoff integration](../architecture/ooda-meeting-handoff-integration.md) — How meeting outcomes feed into the OODA loop.
