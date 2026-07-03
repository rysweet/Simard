//! Serialization of [`StatusSnapshot`] — the `simard status --json` payload and
//! the `GET /api/status/snapshot` HTTP body are the same bytes.

use super::StatusSnapshot;

/// Serialize a snapshot to compact JSON.
pub fn to_string(snapshot: &StatusSnapshot) -> serde_json::Result<String> {
    serde_json::to_string(snapshot)
}

/// Serialize a snapshot to pretty (human-diffable) JSON.
pub fn to_string_pretty(snapshot: &StatusSnapshot) -> serde_json::Result<String> {
    serde_json::to_string_pretty(snapshot)
}

/// Parse a snapshot from JSON. Tolerant of missing fields (they default) and
/// ignores unknown ones, so the schema can grow additively.
pub fn from_str(json: &str) -> serde_json::Result<StatusSnapshot> {
    serde_json::from_str(json)
}
