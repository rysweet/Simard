//! Journal reader for the TUI (issue #2606).
//!
//! Reads Simard's durable daily journal entries out of the *same* cognitive
//! memory the goal board lives in (`<state_root>/cognitive_memory.ladybug`),
//! opened read-only. Each entry is a `journal:YYYY-MM-DD` fact whose content is
//! a JSON-serialized [`JournalEntry`]; this module enumerates those facts,
//! deserializes them, and returns them newest day first for the Journal pane.
//!
//! The entry type, the layperson TUI renderer, and the search-match rule are
//! reused straight from the library ([`simard::journal`]) so the TUI and the
//! dashboard render and search identically — there is no parallel datastore and
//! no duplicated formatting.
//!
//! Read failures (missing DB, corrupt content, oversized payload) degrade
//! gracefully to an empty list: the pane shows an honest "no entries" message
//! rather than crashing.

use std::path::Path;

use simard::journal::JOURNAL_CONCEPT_PREFIX;
pub use simard::journal::{JournalEntry, entry_matches, render_entry_tui_lines};

/// Read every stored journal entry, newest day first.
///
/// Returns an empty vector on any failure (missing DB, unreadable store) so the
/// caller can render an honest empty state.
pub fn read_journal_entries(state_root: &Path) -> Vec<JournalEntry> {
    read_inner(state_root).unwrap_or_default()
}

fn read_inner(state_root: &Path) -> Option<Vec<JournalEntry>> {
    let db_path = state_root.join(crate::goals::COGNITIVE_MEMORY_DB);
    if !db_path.exists() {
        return None;
    }
    let config = lbug::SystemConfig::default().read_only(true);
    let db = lbug::Database::new(&db_path, config).ok()?;
    let conn = lbug::Connection::new(&db).ok()?;
    // Enumerate all facts and filter to journal concepts in Rust: this avoids
    // depending on a specific Cypher string-predicate dialect and matches the
    // read-only, best-effort style of the goal-board reader.
    let result = conn
        .query("MATCH (f:Fact) RETURN f.concept, f.content")
        .ok()?;

    let mut entries: Vec<JournalEntry> = Vec::new();
    // Iterate the result rows directly rather than collecting them into an
    // intermediate `Vec<Vec<Value>>` first — only the journal facts are kept,
    // so materializing every fact up front is wasted allocation.
    for row in result {
        let concept = match row.first() {
            Some(lbug::Value::String(s)) => s.as_str(),
            _ => continue,
        };
        if !concept.starts_with(JOURNAL_CONCEPT_PREFIX) {
            continue;
        }
        let content = match row.get(1) {
            Some(lbug::Value::String(s)) => s.as_str(),
            _ => continue,
        };
        if content.len() > crate::goals::MAX_PAYLOAD_BYTES {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<JournalEntry>(content) {
            entries.push(entry);
        }
    }
    // Newest day first.
    entries.sort_by_key(|e| std::cmp::Reverse(e.date));
    Some(entries)
}
