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
