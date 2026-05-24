# Meeting Handoff Schema v2

> Reference for the `MeetingHandoff` JSON artifact produced when a meeting closes.
> Schema version: **2** (issue #1987). Backward-compatible with v1.

## Overview

The meeting handoff artifact is the primary machine-readable output of a
closed meeting session. It is consumed by:

- **Engineer loop** — routes action items into the work queue.
- **OODA curation** — feeds decisions/questions into the observe pipeline.
- **`act-on-decisions`** — files GitHub issues from action items.
- **Dashboard** — renders meeting summaries for operator review.

## JSON Shape

```jsonc
{
  // ── Identity ────────────────────────────────────────────────
  "meeting_id":   "20260115T100000Z-sprint-planning",  // sortable id
  "topic":        "Sprint Planning",
  "started_at":   "2026-01-15T10:00:00Z",              // RFC 3339
  "closed_at":    "2026-01-15T11:00:00Z",              // RFC 3339

  // ── Schema version (v2, issue #1987) ────────────────────────
  "schema_version": 2,       // 1 for legacy; absent → defaults to 1

  // ── Core content ────────────────────────────────────────────
  "decisions": [
    {
      "description": "Adopt TDD for new modules",
      "rationale":   "Better quality",
      "participants": ["alice"]
    }
  ],
  "action_items": [
    {
      "description":     "Set up CI",
      "owner":           "bob",
      "priority":        1,
      "due_description": "Friday",
      "linked_issue":    null
    }
  ],
  "open_questions": [
    { "text": "What is our SLO target?", "explicit": true }
  ],

  // ── Enrichment ──────────────────────────────────────────────
  "participants":  ["alice", "bob", "operator", "simard"],
  "themes":        ["testing", "performance"],
  "transcript":    ["summary text"],
  "transcript_path": "/home/user/.simard/meetings/.../transcript.json",

  // ── Routing (v1 + v2) ──────────────────────────────────────
  "next_owner": "engineer",          // free-form (v1, kept for compat)
  "next_actor": "engineer",          // typed enum (v2, see NextActor)

  // ── Meeting objective (v2) ─────────────────────────────────
  "goal": "Ship handoff schema v2",  // set via /goal command; null if unset

  // ── Applied templates (v2) ─────────────────────────────────
  "applied_templates": [
    {
      "name": "standup",
      "agenda": "## Standup\n- Yesterday...",
      "applied_at": "2026-01-15T10:01:00Z"
    }
  ],

  // ── History metadata (v2) ──────────────────────────────────
  "history_truncated_count": 0,      // turns dropped by MAX_HISTORY cap

  // ── Close status (v2) ─────────────────────────────────────
  "partial_reason": null,            // null = clean close; string = reason

  // ── Artifacts (v1.5+) ─────────────────────────────────────
  "artifacts": [
    {
      "kind": "transcript",
      "uri_or_path": "/path/to/transcript.json",
      "description": "Meeting transcript JSON"
    },
    {
      "kind": "bundle",
      "uri_or_path": "/path/to/bundle/",
      "description": "Per-meeting handoff bundle directory"
    }
  ],

  // ── Processing state ───────────────────────────────────────
  "processed":     false,
  "duration_secs": 3600
}
```

## New Fields in v2

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema_version` | `u32` | `1` | Schema version tag. `2` for the enriched schema. |
| `goal` | `string?` | `null` | Meeting's overarching objective (set via `/goal`). |
| `next_actor` | `NextActor?` | `null` | Typed routing tag superseding `next_owner`. |
| `applied_templates` | `AppliedTemplate[]` | `[]` | Templates applied during the meeting. |
| `history_truncated_count` | `usize` | `0` | Conversation turns dropped by history cap. |
| `partial_reason` | `string?` | `null` | Machine-readable partial-close reason. |

## `NextActor` Enum

The `next_actor` field uses a discriminated enum serialized as a
`snake_case` string (simple variants) or `{"variant": value}` (data
variants):

| Variant | Wire value | Description |
|---------|-----------|-------------|
| `Engineer` | `"engineer"` | The Simard engineer loop. |
| `OodaCurate` | `"ooda_curate"` | The OODA curation pipeline. |
| `ActOnDecisions` | `"act_on_decisions"` | The `act-on-decisions` CLI. |
| `Human(name)` | `{"human": "alice"}` | A specific human. |
| `Other(name)` | `{"other": "custom-agent"}` | Arbitrary persona. |

## Backward Compatibility

All v2 fields use `#[serde(default)]`. A v1 JSON (missing the new fields)
deserializes cleanly into a `MeetingHandoff` with:

- `schema_version` = `1`
- `goal` = `None`
- `next_actor` = `None`
- `applied_templates` = `[]`
- `history_truncated_count` = `0`
- `partial_reason` = `None`

No migration step is required. Consumers should check `schema_version`
before relying on v2-only fields.

## REPL Commands

| Command | Effect |
|---------|--------|
| `/goal <text>` | Sets the meeting's `goal` field. |
| `/owner <name>` | Sets `next_owner` and derives `next_actor`. |

## Related Issues

- **#1987** — Structured handoff schema v2 (this document).
- **#1982** — Parent epic: enhance Simard meeting experience.
- **#1985** — Per-meeting bundle consumer (depends on v2 schema).
- **#1954** — Prior `next_owner` + artifacts work (v1.5).
