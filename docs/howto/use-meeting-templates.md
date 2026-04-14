---
title: How to use meeting templates
description: Start structured meetings with pre-built templates for standups, 1-on-1s, retrospectives, and planning sessions.
last_updated: 2026-04-14
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/meeting-backend-api.md
  - ./start-a-meeting.md
  - ./export-meeting-markdown.md
---

# How to use meeting templates

Templates give meetings a structured starting point. Instead of an open-ended conversation, a template injects a focused agenda that guides both you and Simard through the meeting.

## Prerequisites

- [ ] You are in the repository root
- [ ] A meeting session is active (see [How to start a meeting](./start-a-meeting.md))

## 1. List available templates

During any meeting, type:

```text
simard> /template

Available templates: standup, 1on1, retro, planning
Use /template <name> to apply one.
```

## 2. Apply a template

```text
simard> /template standup

⏳ Applying template: standup

📋 Standup Meeting
━━━━━━━━━━━━━━━━━━
1. What did you accomplish since our last standup?
2. What are you working on today?
3. Any blockers or things you need help with?
4. Any updates to share with the team?

Template applied. The agenda has been added to the meeting context.
```

The template text is injected into the meeting's conversation context. Simard sees the agenda and structures her responses accordingly.

## 3. Available templates

### `standup`

Quick daily check-in format:

1. What did you accomplish since our last standup?
2. What are you working on today?
3. Any blockers or things you need help with?
4. Any updates to share with the team?

### `1on1`

Operator/Simard one-on-one check-in:

1. How are things going overall?
2. Progress on current goals and priorities
3. Any concerns or challenges to discuss?
4. Feedback — what's working well, what could improve?
5. Action items and next steps

### `retro`

Sprint or milestone retrospective:

1. What went well this sprint/milestone?
2. What didn't go well or could be improved?
3. What specific actions should we take?
4. Any process changes to try next time?

### `planning`

Sprint or milestone planning session:

1. Review and prioritize the backlog
2. What's the scope for this sprint/milestone?
3. Dependencies and risks to watch
4. Resource allocation and assignments
5. Definition of done for key items

## 4. Templates and conversation flow

Applying a template does not reset the conversation. If you've already been chatting with Simard and then apply a template mid-meeting, the agenda is appended to the existing context. Simard acknowledges the template and shifts focus to the agenda items.

You can apply multiple templates in one meeting. Each adds its agenda to the context. This is unusual but not harmful — Simard handles it by addressing both agendas.

## 5. Templates via the dashboard

The same `/template` command works in the dashboard WebSocket chat. Type it in the chat input — the backend handles it identically to the CLI.

## Troubleshooting

### "Unknown template: xyz"

Only the 4 built-in templates are recognized. Template names are case-sensitive and must match exactly: `standup`, `1on1`, `retro`, `planning`.

### Template didn't change Simard's behavior

The template adds structured context, but Simard still responds naturally. If the conversation drifts from the agenda, redirect with a specific question like "Let's move to item 3 — any blockers?"

## Related reading

- [How to start a meeting](./start-a-meeting.md) — Starting meetings from CLI or dashboard.
- [Meeting backend API reference](../reference/meeting-backend-api.md) — `get_template()` and `template_names()` API docs.
- [How to export meeting markdown](./export-meeting-markdown.md) — Save the templated meeting as markdown.
