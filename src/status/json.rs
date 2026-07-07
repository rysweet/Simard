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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_compact_json() {
        let snap = StatusSnapshot::empty();
        let encoded = to_string(&snap).unwrap();
        assert_eq!(from_str(&encoded).unwrap(), snap);
    }

    #[test]
    fn pretty_json_is_multiline_and_parseable() {
        let snap = StatusSnapshot::empty();
        let pretty = to_string_pretty(&snap).unwrap();
        assert!(pretty.contains('\n'), "pretty JSON should be multi-line");
        assert_eq!(from_str(&pretty).unwrap(), snap);
    }

    #[test]
    fn from_str_defaults_missing_sections() {
        // Only the two required top-level fields; every section defaults to absent.
        let json = r#"{"schema_version":1,"generated_at":"2026-07-06T12:00:00Z"}"#;
        let snap = from_str(json).unwrap();
        assert_eq!(snap.schema_version, 1);
        assert_eq!(snap.generated_at, "2026-07-06T12:00:00Z");
        assert!(!snap.daemon.is_present());
        assert!(!snap.overseer.is_present());
    }

    #[test]
    fn from_str_ignores_unknown_fields() {
        let json = r#"{"schema_version":1,"generated_at":"x","totally_unknown":true}"#;
        assert!(from_str(json).is_ok());
    }
}
