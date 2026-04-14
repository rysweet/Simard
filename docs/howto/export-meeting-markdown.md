---
title: How to export meeting markdown
description: Export the current meeting as a markdown file with YAML frontmatter to ~/.simard/meetings/.
last_updated: 2026-04-14
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/meeting-backend-api.md
  - ./start-a-meeting.md
  - ./use-meeting-templates.md
  - ./inspect-meeting-records.md
---

# How to export meeting markdown

The `/export` command writes a human-readable markdown snapshot of the current meeting to `~/.simard/meetings/`. Unlike the JSON transcript written on `/close`, the markdown export is designed for reading, sharing, and archiving.

## Prerequisites

- [ ] A meeting session is active (see [How to start a meeting](./start-a-meeting.md))

## 1. Export during a meeting

At any point during a meeting, type:

```text
simard> /export

⏳ Exporting meeting...
📄 Exported to /home/you/.simard/meetings/20260414T1030_discuss-memory-consolidation.md
```

The meeting continues — `/export` does not end the session.

## 2. Export file format

The exported file has two sections: YAML frontmatter and the conversation body.

```markdown
---
topic: "discuss memory consolidation"
date: "2026-04-14T10:30:00Z"
duration_minutes: 25
message_count: 18
themes: []
---

# Meeting: discuss memory consolidation

**Date:** 2026-04-14 10:30 UTC
**Duration:** 25 minutes
**Messages:** 18

---

## Conversation

**You:** Hey Simard, let's talk about the memory consolidation backlog.

**Simard:** The episodic memories from the last three engineering sessions
are piling up. I haven't promoted the useful patterns to semantic memory
yet, and I'm worried retrieval quality will degrade if we don't consolidate
soon.

**You:** What's your plan for tackling it?

**Simard:** I'd prioritize the sessions that touched the gym scoring
pipeline — those have the most reusable patterns...
```

### Frontmatter fields

| Field | Type | Description |
|-------|------|-------------|
| `topic` | string | The meeting topic (quoted in YAML to prevent injection) |
| `date` | ISO 8601 string | Session start timestamp |
| `duration_minutes` | integer | Elapsed time since session start |
| `message_count` | integer | Total messages (user + assistant) at export time |
| `themes` | string array | Always empty (`[]`) during export — theme extraction happens on `/close` |

### Conversation format

Messages are rendered as:

- **You:** for user messages
- **Simard:** for assistant messages

System messages are omitted from the export.

## 3. Export multiple times

Each `/export` creates a new file with the current timestamp. You can export at any point during the meeting to capture snapshots:

```bash
ls ~/.simard/meetings/
# 20260414T1030_discuss-memory-consolidation.md  (early snapshot)
# 20260414T1055_discuss-memory-consolidation.md  (later snapshot with more content)
```

## 4. Export vs. close

| | `/export` | `/close` |
|---|-----------|---------|
| Format | Markdown with YAML frontmatter | JSON transcript |
| Location | `~/.simard/meetings/*.md` | `~/.simard/meetings/*.json` |
| Ends session | No | Yes |
| Includes summary | No (raw conversation) | Yes (LLM-generated summary) |
| Includes memories | No | Yes (stored via bridge) |
| Includes handoff | No | Yes (written for OODA loop) |

Both are complementary. Use `/export` for a human-readable record during or after the meeting. Use `/close` to end the session and trigger the full persistence pipeline.

## 5. File permissions and location

- **Directory:** `~/.simard/meetings/` (created automatically if it doesn't exist)
- **Permissions:** `0o600` (owner read/write only) — meeting content is sensitive
- **Filename:** `{YYYYMMDD}T{HHMM}_{sanitized_topic}.md`
- **Sanitization:** Same rules as JSON transcripts — path separators stripped, special characters replaced with hyphens, max 128 characters

## 6. Export via the dashboard

The `/export` command works identically in the dashboard WebSocket chat. The file is written server-side to the same `~/.simard/meetings/` directory.

## Troubleshooting

### "Failed to export meeting"

Check that `~/.simard/meetings/` exists and is writable. The directory is created automatically, but filesystem permission issues will prevent it. Check disk space if the directory exists but writes fail.

### The export file is empty or truncated

This shouldn't happen — the export writes atomically. If you see a truncated file, check disk space (`df -h ~`) and file system health.

### Themes field is always empty

The `themes` field is always an empty array (`[]`) in markdown exports. Theme extraction only happens during `/close`, when the LLM summarizes the conversation and writes themes into the `MeetingHandoff`. This is by design — `/export` is a lightweight snapshot that avoids an LLM call.

## Related reading

- [How to start a meeting](./start-a-meeting.md) — Starting meetings from CLI or dashboard.
- [How to use meeting templates](./use-meeting-templates.md) — Structured meeting agendas.
- [How to inspect meeting records](./inspect-meeting-records.md) — Reading back JSON transcripts.
- [Meeting backend API reference](../reference/meeting-backend-api.md) — `export_markdown()` API docs.
