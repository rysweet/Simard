# Meeting Handoff Schema

> Reference for the `MeetingHandoff` JSON artifact produced when a meeting
> closes. Consumer code lives in `src/meeting_facilitator/handoff/mod.rs`.

## Schema versions

| Version | Introduced | Description |
|---------|-----------|-------------|
| v1 | Initial | Original schema — no `schema_version` field. Consumers use `#[serde(default)]` to fill missing fields. |
| v2 | #1987 | Added `schema_version`, `goal`, `next_actor`, `applied_templates`, `history_truncated_count`, `partial_reason`. All new fields use `#[serde(default)]` so v1 files deserialize unchanged. |

## v2 fields

### `schema_version: u32`

Schema version tag. Written as `2` on all new handoffs. Missing from v1
files — deserialization defaults to `1` via
`#[serde(default = "default_handoff_schema_version_v1")]`.

### `goal: Option<String>`

The meeting's overarching objective, distinct from the short `topic`.
Set at the REPL via `/goal <text>`. Falls back to the first user message
if unset by the operator. `None` in v1 handoffs.

### `next_actor: Option<NextActor>`

Structured routing hint for which actor should consume the handoff next.
Complements the free-form `next_owner` string.

```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NextActor {
    Operator,
    Engineer,
    OodaCurate,
    External,
}
```

**JSON example:**

```json
"next_actor": { "kind": "engineer" }
```

`None` when no routing preference was set. v1 handoffs deserialize as
`None`.

### `applied_templates: Vec<AppliedTemplate>`

Templates applied during the meeting via `/template <name>`. Already
present on `MeetingSummary`; promoted to the handoff so downstream
consumers see them without parsing the bundle.

Each entry carries:
- `name: String` — template name (e.g. `"standup"`, `"retro"`)
- `agenda: String` — full agenda markdown
- `applied_at: String` — RFC 3339 timestamp

Empty `[]` in v1 handoffs.

### `history_truncated_count: usize`

Number of conversation messages dropped because the backend hit the
`MAX_HISTORY` cap (currently 500). Lets downstream consumers gauge
transcript completeness. `0` in v1 handoffs.

### `partial_reason: Option<String>`

Wire-string form of `PartialReason` from the close pipeline. Populated
when the close was partial (e.g. `"summary_timeout"`,
`"close_timeout"`, `"persistence_error"`). `None` for a clean close and
for v1 handoffs.

Valid wire strings (see `src/meeting_backend/close_guard.rs`):
- `close_timeout`
- `agent_close_timeout`
- `summary_timeout`
- `summary_empty`
- `bridge_timeout`
- `persistence_error`

## Backward compatibility

All v2 fields use `#[serde(default)]`, so:

1. **v1 → v2 read**: Older JSON files missing the new fields deserialize
   cleanly with default values (`schema_version=1`, others empty/`None`/`0`).
2. **v2 → v1 consumer**: Consumers that don't know about the new fields
   ignore them (serde's default behaviour for unknown fields).
3. **No migration tool needed** — `#[serde(default)]` *is* the migration.

## Deprecation timeline

v1 handoffs will continue to deserialize indefinitely. A future issue may
add a `MeetingHandoff::v1_to_v2()` in-place upgrader if batch migration
becomes desirable, but it is not required.

## REPL commands

| Command | Field | Description |
|---------|-------|-------------|
| `/goal <text>` | `goal` | Set the meeting's overarching objective |
| `/owner <name>` | `next_owner` | Name the next agent/persona/human expected to action this handoff |

## Related issues

- #1982 — parent epic (enhance meeting experience)
- #1987 — this schema bump
- #1985 — bundle consumption (depends on v2 schema)
- #1984 — resume support (independent)
- #1951 — sub-issues 3, 6, 7 (coordinated via this schema bump)
