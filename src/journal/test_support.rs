//! Shared, offline test doubles for the journal tests (issue #2606).
//!
//! Everything here is `#[cfg(test)]`: an in-memory [`FakeMemory`] backend that
//! implements just enough of [`CognitiveMemoryOps`] for the journal store
//! (caller-key dedup + substring search), plus injectable fake clock / episode
//! / PR sources so the whole pipeline runs with no network and no wall clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::journal::generate::{GlossaryReviewer, JournalReviewer};
use crate::journal::providers::{EpisodeSource, JournalClock, PrListSource};
use crate::journal::types::PrSummary;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

/// In-memory cognitive-memory backend for journal tests.
///
/// Implements `store_fact_with_caller_key` with caller-key dedup (one live fact
/// per key — the same contract the real library backend provides) and
/// `search_facts` with a case-insensitive substring match, which is all the
/// [`JournalStore`](crate::journal::store::JournalStore) needs.
#[derive(Default)]
pub(crate) struct FakeMemory {
    facts: Mutex<Vec<CognitiveFact>>,
    episodes: Mutex<Vec<CognitiveEpisode>>,
    seq: AtomicUsize,
}

impl FakeMemory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Seed an episode so [`list_all_episodes`](CognitiveMemoryOps::list_all_episodes)
    /// (and thus the journal thread's episode source) has material to narrate.
    pub(crate) fn add_episode(&self, ep: CognitiveEpisode) {
        self.episodes.lock().expect("episodes lock").push(ep);
    }

    fn mk_fact(
        &self,
        node_id: String,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> CognitiveFact {
        CognitiveFact {
            node_id,
            concept: concept.to_string(),
            content: content.to_string(),
            confidence,
            source_id: source_id.to_string(),
            tags: tags.to_vec(),
            usage_count: 0,
            last_accessed_at: None,
        }
    }
}

impl CognitiveMemoryOps for FakeMemory {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen".into())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("wrk".into())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        content: &str,
        source: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let node_id = format!("epi-{id}");
        self.episodes
            .lock()
            .expect("episodes lock")
            .push(CognitiveEpisode {
                node_id: node_id.clone(),
                content: content.to_string(),
                source_label: source.to_string(),
                temporal_index: id as i64,
                compressed: false,
                created_at: None,
            });
        Ok(node_id)
    }
    fn list_all_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        let episodes = self.episodes.lock().expect("episodes lock");
        // Newest-first, mirroring the real backend's ordering.
        let mut out: Vec<CognitiveEpisode> = episodes.iter().rev().cloned().collect();
        out.truncate(limit as usize);
        Ok(out)
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let node_id = format!("fact-{id}");
        let fact = self.mk_fact(
            node_id.clone(),
            concept,
            content,
            confidence,
            tags,
            source_id,
        );
        self.facts.lock().expect("facts lock").push(fact);
        Ok(node_id)
    }
    fn store_fact_with_caller_key(
        &self,
        caller_key: &str,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        // One live fact per caller key: replace any prior fact with this key.
        let mut facts = self.facts.lock().expect("facts lock");
        facts.retain(|f| f.node_id != caller_key);
        let fact = self.mk_fact(
            caller_key.to_string(),
            concept,
            content,
            confidence,
            tags,
            source_id,
        );
        facts.push(fact);
        Ok(caller_key.to_string())
    }
    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let needle = query.to_lowercase();
        let facts = self.facts.lock().expect("facts lock");
        let mut out: Vec<CognitiveFact> = facts
            .iter()
            .filter(|f| f.confidence >= min_confidence)
            .filter(|f| {
                needle.is_empty()
                    || f.concept.to_lowercase().contains(&needle)
                    || f.content.to_lowercase().contains(&needle)
                    || f.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();
        out.truncate(limit as usize);
        Ok(out)
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc".into())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro".into())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

/// A clock that always reports a fixed day.
pub(crate) struct FixedClock(pub(crate) NaiveDate);

impl JournalClock for FixedClock {
    fn today(&self) -> NaiveDate {
        self.0
    }
}

/// An [`EpisodeSource`] that returns a canned list regardless of date.
pub(crate) struct FixedEpisodes(pub(crate) Vec<CognitiveEpisode>);

impl EpisodeSource for FixedEpisodes {
    fn episodes_for_date(&self, _date: NaiveDate) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(self.0.clone())
    }
}

/// A [`PrListSource`] that returns a canned list regardless of date.
pub(crate) struct FixedPrs(pub(crate) Vec<PrSummary>);

impl PrListSource for FixedPrs {
    fn prs_for_date(&self, _date: NaiveDate) -> SimardResult<Vec<PrSummary>> {
        Ok(self.0.clone())
    }
}

/// A reviewer that counts how many times it ran, wrapping the real
/// [`GlossaryReviewer`], so tests can prove the mandatory review pass fired.
/// The counter is shared (via `Arc`) so a test can read it after the reviewer
/// has been moved into a [`JournalGenerator`].
pub(crate) struct CountingReviewer {
    inner: GlossaryReviewer,
    calls: Arc<AtomicUsize>,
}

impl CountingReviewer {
    pub(crate) fn new(calls: Arc<AtomicUsize>) -> Self {
        Self {
            inner: GlossaryReviewer,
            calls,
        }
    }
}

impl JournalReviewer for CountingReviewer {
    fn review(&self, draft: &str) -> String {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.review(draft)
    }
}

/// Build a [`CognitiveEpisode`] with `content` for a test.
pub(crate) fn episode(content: &str) -> CognitiveEpisode {
    CognitiveEpisode {
        node_id: format!("ep-{}", content.len()),
        content: content.to_string(),
        source_label: "test".to_string(),
        temporal_index: 0,
        compressed: false,
        created_at: None,
    }
}

/// Build a [`CognitiveEpisode`] with `content` and an explicit `temporal_index`
/// (issue #2606: episodes carry timestamps and render chronologically).
pub(crate) fn episode_at(content: &str, temporal_index: i64) -> CognitiveEpisode {
    CognitiveEpisode {
        node_id: format!("ep-{temporal_index}"),
        content: content.to_string(),
        source_label: "test".to_string(),
        temporal_index,
        compressed: false,
        created_at: None,
    }
}

/// Build a [`PrSummary`] for a test.
pub(crate) fn pr(number: u64, plain_summary: &str, outcome: &str) -> PrSummary {
    PrSummary {
        number,
        plain_summary: plain_summary.to_string(),
        outcome: outcome.to_string(),
    }
}

/// A fixed calendar day used across the journal tests.
pub(crate) fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 5).expect("valid date")
}
