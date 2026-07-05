//! Durable journal persistence over cognitive memory (issue #2606).
//!
//! Journal entries live in the **same** cognitive-memory store as the rest of
//! Simard's knowledge — there is no parallel datastore. Each day's entry is a
//! single semantic fact:
//!
//! * `caller_key` / `concept` = `"journal:YYYY-MM-DD"` — so regenerating a
//!   day's entry as the day rolls forward **supersedes** the prior one instead
//!   of piling up duplicates (the caller-key dedup is idempotent).
//! * `content` = the JSON-serialised [`JournalEntry`].
//! * tag `"journal"` — so the store can enumerate journal facts.
//!
//! [`JournalStore::query`] is the single retrieval path (by date range + free
//! text) that backs both the dashboard and the TUI, so the two never diverge.

use std::sync::Arc;

use chrono::NaiveDate;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::journal::types::JournalEntry;

/// Concept/caller-key prefix for a journal fact.
pub const JOURNAL_CONCEPT_PREFIX: &str = "journal:";
/// The tag every journal fact carries.
pub const JOURNAL_TAG: &str = "journal";
/// Broad search token used to enumerate journal facts from the backend before
/// the exact `journal:`-concept filter is applied in Rust.
const JOURNAL_SEARCH_TOKEN: &str = "journal";
/// Source label recorded on journal facts.
const JOURNAL_SOURCE: &str = "journal-generator";
/// Upper bound on how many journal facts an enumeration pulls back.
const ENUMERATION_LIMIT: u32 = 10_000;

/// The stable `caller_key` (and `concept`) for a given day's journal fact.
#[must_use]
pub fn journal_caller_key(date: NaiveDate) -> String {
    format!("{JOURNAL_CONCEPT_PREFIX}{}", date.format("%Y-%m-%d"))
}

/// Parse the date out of a `"journal:YYYY-MM-DD"` concept, if it is one.
fn date_from_concept(concept: &str) -> Option<NaiveDate> {
    let rest = concept.strip_prefix(JOURNAL_CONCEPT_PREFIX)?;
    NaiveDate::parse_from_str(rest, "%Y-%m-%d").ok()
}

/// Durable store for journal entries, backed by cognitive memory.
///
/// Cloneable and cheap to pass around — it holds only an `Arc` to the shared
/// backend, so a freshly-constructed [`JournalStore`] over the same backend
/// sees every previously-saved entry (entries survive process restarts because
/// the backend is persistent).
#[derive(Clone)]
pub struct JournalStore {
    mem: Arc<dyn CognitiveMemoryOps>,
}

impl JournalStore {
    /// Wrap a cognitive-memory backend.
    pub fn new(mem: Arc<dyn CognitiveMemoryOps>) -> Self {
        Self { mem }
    }

    /// Persist (or roll-forward) the entry for its day.
    ///
    /// Uses the day's stable caller key so repeated saves for the same date
    /// supersede rather than accumulate (idempotent rolling update). Returns
    /// the backend node id.
    pub fn save(&self, entry: &JournalEntry) -> SimardResult<String> {
        let key = journal_caller_key(entry.date);
        // Our own plain-data type — serialization cannot fail in practice, so
        // an `expect` here mirrors the codebase convention for known-safe
        // serialization (e.g. tab-meta JSON).
        let content = serde_json::to_string(entry).expect("JournalEntry is JSON-serializable");
        let node_id = self.mem.store_fact_with_caller_key(
            &key,
            &key,
            &content,
            1.0,
            &[JOURNAL_TAG.to_string()],
            JOURNAL_SOURCE,
        )?;
        tracing::debug!(
            target: "simard::journal",
            date = %entry.date,
            quiet_day = entry.quiet_day,
            prs = entry.prs.len(),
            "journal entry saved"
        );
        Ok(node_id)
    }

    /// Fetch the entry for an exact day, if one exists.
    pub fn get_by_date(&self, date: NaiveDate) -> SimardResult<Option<JournalEntry>> {
        let key = journal_caller_key(date);
        let facts = self.mem.search_facts(&key, 64, 0.0)?;
        for fact in facts {
            if fact.concept == key {
                return Ok(Some(parse_entry(&fact.concept, &fact.content)?));
            }
        }
        Ok(None)
    }

    /// Every stored entry, newest day first.
    ///
    /// Enumeration is lenient: any candidate fact whose content is not a valid
    /// [`JournalEntry`] is skipped (it is simply not a journal record), so a
    /// broad backend search that returns unrelated facts cannot break browsing.
    pub fn all_entries(&self) -> SimardResult<Vec<JournalEntry>> {
        let facts = self
            .mem
            .search_facts(JOURNAL_SEARCH_TOKEN, ENUMERATION_LIMIT, 0.0)?;
        let mut entries: Vec<JournalEntry> = facts
            .iter()
            .filter(|f| date_from_concept(&f.concept).is_some())
            .filter_map(|f| serde_json::from_str::<JournalEntry>(&f.content).ok())
            .collect();
        // Newest day first.
        entries.sort_by_key(|e| std::cmp::Reverse(e.date));
        Ok(entries)
    }

    /// The dates that have an entry, newest first (for a date picker / list).
    pub fn dates(&self) -> SimardResult<Vec<NaiveDate>> {
        Ok(self.all_entries()?.into_iter().map(|e| e.date).collect())
    }

    /// The single query API backing both dashboard and TUI: filter by an
    /// inclusive date `range` and/or a free-`text` search, newest day first.
    ///
    /// * `range = None` — no date bound.
    /// * `text = None` or empty — no text bound.
    ///
    /// Text search is a case-insensitive substring over the narrative, the date,
    /// and each code-change-proposal summary/outcome/number, so an operator can
    /// find an entry by what it says as well as by when it happened.
    pub fn query(
        &self,
        range: Option<(NaiveDate, NaiveDate)>,
        text: Option<&str>,
    ) -> SimardResult<Vec<JournalEntry>> {
        let mut entries = self.all_entries()?;
        if let Some((from, to)) = range {
            entries.retain(|e| e.date >= from && e.date <= to);
        }
        if let Some(t) = text {
            let needle = t.to_lowercase();
            if !needle.is_empty() {
                entries.retain(|e| entry_matches_text(e, &needle));
            }
        }
        Ok(entries)
    }
}

/// Deserialize a journal fact's content, mapping a parse failure to a typed,
/// fail-loud error (a matching `journal:` concept that will not parse is real
/// corruption, not an absent entry).
fn parse_entry(concept: &str, content: &str) -> SimardResult<JournalEntry> {
    serde_json::from_str::<JournalEntry>(content).map_err(|e| SimardError::InvalidJournalRecord {
        field: concept.to_string(),
        reason: format!("stored journal entry is not valid JSON: {e}"),
    })
}

/// Case-insensitive substring match of `needle` (already lowercased) over the
/// searchable text of an entry.
fn entry_matches_text(entry: &JournalEntry, needle: &str) -> bool {
    if entry.narrative.to_lowercase().contains(needle) {
        return true;
    }
    if entry.date.format("%Y-%m-%d").to_string().contains(needle) {
        return true;
    }
    entry.prs.iter().any(|pr| {
        pr.plain_summary.to_lowercase().contains(needle)
            || pr.outcome.to_lowercase().contains(needle)
            || pr.number.to_string().contains(needle)
    })
}
