//! Conflict-tracking types for precedence resolution.

/// A single recorded conflict: which field was overridden by which identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictEntry {
    /// The field category where the conflict occurred (e.g. "prompt_asset", "base_type").
    pub field: String,
    /// The key that was in conflict (e.g. the asset id or base type name).
    pub key: String,
    /// Name of the identity whose value was kept (winner).
    pub winner: String,
    /// Name of the identity whose value was overridden (loser).
    pub loser: String,
}

/// Log of all conflicts detected during precedence resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConflictLog {
    pub entries: Vec<ConflictEntry>,
}

impl ConflictLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(
        &mut self,
        field: impl Into<String>,
        key: impl Into<String>,
        winner: impl Into<String>,
        loser: impl Into<String>,
    ) {
        self.entries.push(ConflictEntry {
            field: field.into(),
            key: key.into(),
            winner: winner.into(),
            loser: loser.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_log_new_is_empty() {
        let log = ConflictLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn conflict_log_default_is_empty() {
        let log = ConflictLog::default();
        assert!(log.is_empty());
    }

    #[test]
    fn conflict_log_record_adds_entry() {
        let mut log = ConflictLog::new();
        log.record("prompt_asset", "sys-prompt", "alpha", "beta");
        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());
        let entry = &log.entries[0];
        assert_eq!(entry.field, "prompt_asset");
        assert_eq!(entry.key, "sys-prompt");
        assert_eq!(entry.winner, "alpha");
        assert_eq!(entry.loser, "beta");
    }

    #[test]
    fn conflict_log_multiple_entries() {
        let mut log = ConflictLog::new();
        log.record("base_type", "bt-1", "a", "b");
        log.record("prompt_asset", "pa-1", "c", "d");
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn conflict_entry_equality() {
        let e1 = ConflictEntry {
            field: "f".to_string(),
            key: "k".to_string(),
            winner: "w".to_string(),
            loser: "l".to_string(),
        };
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }

    #[test]
    fn conflict_log_equality() {
        let mut log1 = ConflictLog::new();
        log1.record("f", "k", "w", "l");
        let log2 = log1.clone();
        assert_eq!(log1, log2);
    }
}
