//! Dashboard **Journal** tab handlers (issue #2606).
//!
//! Three read-only endpoints back the Journal tab, all reading the durable
//! `journal:YYYY-MM-DD` entries out of the *same* cognitive-memory store the
//! rest of the dashboard reads (via [`open_reader_client`]) — there is no
//! parallel datastore:
//!
//! * `GET  /api/journal/dates`         — the days that have an entry, newest
//!   first, for the date picker.
//! * `POST /api/journal/search`        — full-text + optional date-range search
//!   over entries, newest first.
//! * `GET  /api/journal/render/{date}` — the day's entry rendered as a
//!   jargon-free, XSS-safe HTML fragment (narrative + plain-language
//!   code-change-proposal table), or an honest note when the day has no entry.
//!
//! The Journal tab is a **distinct** tab from the in-flight Operator/Overseer
//! activity tab: it owns the `journal` slug and the `/api/journal/*` route
//! namespace, so the two never collide.

use std::path::Path;

use axum::Json;
use axum::extract::Path as AxumPath;
use axum::response::Html;
use chrono::NaiveDate;
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::error::SimardResult;
use crate::journal::{
    JournalEntry, all_entries as journal_all_entries, get_entry_by_date, html_escape,
    query_entries, render_entry_html,
};
use crate::memory_ipc::open_reader_client;

/// Length of the plain-text snippet returned with each search result.
const SNIPPET_CHARS: usize = 220;

/// `GET /api/journal/dates` — the days that have an entry, newest day first.
pub(crate) async fn journal_dates() -> Json<Value> {
    let state_root = resolve_state_root();
    match load_all_entries(&state_root) {
        Ok(entries) => Json(json!({
            "dates": entries.iter().map(date_summary).collect::<Vec<_>>(),
        })),
        Err(e) => Json(json!({ "error": e.to_string(), "dates": [] })),
    }
}

/// `POST /api/journal/search` — full-text + optional inclusive date-range search
/// over journal entries, newest day first. Body: `{query?, from?, to?}` (dates
/// as `YYYY-MM-DD`).
pub(crate) async fn journal_search(Json(body): Json<Value>) -> Json<Value> {
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let range = parse_range(&body);
    let state_root = resolve_state_root();

    let text = if query.is_empty() {
        None
    } else {
        Some(query.as_str())
    };
    match search_entries(&state_root, range, text) {
        Ok(entries) => Json(json!({
            "results": entries.iter().map(entry_summary).collect::<Vec<_>>(),
        })),
        Err(e) => Json(json!({ "error": e.to_string(), "results": [] })),
    }
}

/// `GET /api/journal/render/{date}` — the day's entry as a jargon-free,
/// XSS-safe HTML fragment, or an honest note when the day has no entry.
pub(crate) async fn journal_render(AxumPath(date): AxumPath<String>) -> Html<String> {
    let state_root = resolve_state_root();
    Html(render_journal_html(&state_root, &date))
}

/// `GET /api/journal/entry/{date}` — the day's entry as raw
/// [`JournalEntry`] JSON, or `{status:"error", error:…}` for a bad date,
/// an absent entry, or a read error.
pub(crate) async fn journal_entry(AxumPath(date): AxumPath<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let parsed = match NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => {
            return Json(json!({
                "status": "error",
                "error": "invalid date (expected YYYY-MM-DD)",
            }));
        }
    };
    match get_entry(&state_root, parsed) {
        Ok(Some(entry)) => Json(serde_json::to_value(&entry).unwrap_or_else(
            |_| json!({ "status": "error", "error": "entry could not be serialized" }),
        )),
        Ok(None) => Json(json!({ "status": "error", "error": "no journal entry for this day" })),
        Err(e) => Json(json!({ "status": "error", "error": e.to_string() })),
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (state_root injected) — the async handlers are thin shells over
// these so they are unit-testable against a hermetic store.
// ---------------------------------------------------------------------------

/// Load every stored entry (newest first) from the store at `state_root`.
fn load_all_entries(state_root: &Path) -> SimardResult<Vec<JournalEntry>> {
    let reader = open_reader_client(state_root)?;
    journal_all_entries(reader.ops())
}

/// Query entries by optional date `range` and free `text`, newest first.
fn search_entries(
    state_root: &Path,
    range: Option<(NaiveDate, NaiveDate)>,
    text: Option<&str>,
) -> SimardResult<Vec<JournalEntry>> {
    let reader = open_reader_client(state_root)?;
    query_entries(reader.ops(), range, text)
}

/// Fetch a single day's entry from the store at `state_root`.
fn get_entry(state_root: &Path, date: NaiveDate) -> SimardResult<Option<JournalEntry>> {
    let reader = open_reader_client(state_root)?;
    get_entry_by_date(reader.ops(), date)
}

/// Render the HTML fragment for the entry on `date_str` (an honest, XSS-safe
/// message on a bad date, an absent entry, or a read error).
fn render_journal_html(state_root: &Path, date_str: &str) -> String {
    let date = match NaiveDate::parse_from_str(date_str.trim(), "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => {
            return empty_fragment(None, "Choose a valid day (YYYY-MM-DD) to read its journal.");
        }
    };
    let reader = match open_reader_client(state_root) {
        Ok(r) => r,
        Err(e) => {
            return empty_fragment(
                Some(date),
                &format!("Could not open the journal store: {e}"),
            );
        }
    };
    match get_entry_by_date(reader.ops(), date) {
        Ok(Some(entry)) => render_entry_html(&entry),
        Ok(None) => empty_fragment(Some(date), "No journal entry for this day yet."),
        Err(e) => empty_fragment(
            Some(date),
            &format!("Could not read this day's journal: {e}"),
        ),
    }
}

/// An honest, XSS-safe "no entry" / error fragment matching the entry markup
/// so the tab renders cleanly. `message` is escaped.
fn empty_fragment(date: Option<NaiveDate>, message: &str) -> String {
    let heading = date
        .map(|d| {
            format!(
                "<h2 class=\"journal-date\">{}</h2>",
                html_escape(&d.format("%Y-%m-%d").to_string())
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"journal-entry\">{heading}<p class=\"journal-empty\">{}</p></div>",
        html_escape(message)
    )
}

/// Compact per-day summary for the date picker.
fn date_summary(entry: &JournalEntry) -> Value {
    json!({
        "date": entry.date.format("%Y-%m-%d").to_string(),
        "quiet_day": entry.quiet_day,
        "pr_count": entry.prs.len(),
        "merged": entry.merged_pr_count(),
    })
}

/// Search-result summary: the per-day fields plus a short narrative snippet.
fn entry_summary(entry: &JournalEntry) -> Value {
    // Single pass: build the snippet, then peek one char past it to decide on
    // the ellipsis — avoids re-walking the whole narrative with `chars().count()`
    // (O(n)) just to test whether it exceeds the snippet length.
    let mut chars = entry.narrative.chars();
    let head: String = chars.by_ref().take(SNIPPET_CHARS).collect();
    let snippet = if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    };
    json!({
        "date": entry.date.format("%Y-%m-%d").to_string(),
        "quiet_day": entry.quiet_day,
        "pr_count": entry.prs.len(),
        "merged": entry.merged_pr_count(),
        "snippet": snippet,
    })
}

/// Parse an optional inclusive `{from, to}` date range from the search body.
/// A range is used only when both bounds parse; a partial/invalid range is
/// ignored (the search still runs, unbounded by date).
fn parse_range(body: &Value) -> Option<(NaiveDate, NaiveDate)> {
    let parse = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
    };
    match (parse("from"), parse("to")) {
        (Some(from), Some(to)) if from <= to => Some((from, to)),
        (Some(from), Some(to)) => Some((to, from)), // tolerate swapped bounds
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::Path as AxumPath;
    use chrono::{NaiveDate, Utc};

    use super::*;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::journal::types::{JournalEntry, PrSummary};
    use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
    use crate::test_support::HermeticState;

    /// Registers a shared in-process writer for the hermetic state root (the
    /// tier-0 path the dashboard reads through) and clears it on drop.
    struct MemGuard {
        writer: Arc<dyn CognitiveMemoryOps>,
    }

    impl MemGuard {
        fn register(state: &HermeticState) -> Self {
            let writer: Arc<dyn CognitiveMemoryOps> =
                Arc::new(LibraryCognitiveMemory::open(state.state_root()).expect("open store"));
            register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
            Self { writer }
        }
        fn ops(&self) -> &dyn CognitiveMemoryOps {
            self.writer.as_ref()
        }
    }

    impl Drop for MemGuard {
        fn drop(&mut self) {
            clear_in_process_writer();
        }
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn entry_with_prs(date: NaiveDate) -> JournalEntry {
        JournalEntry {
            date,
            generated_at: Utc::now(),
            narrative: "Today I helped fix the login page.\n\n\
                <script>alert('x')</script> lingered in a memory."
                .to_string(),
            draft: String::new(),
            prs: vec![
                PrSummary {
                    number: 12,
                    plain_summary: "Made login <b>safer</b>".to_string(),
                    outcome: "merged".to_string(),
                },
                PrSummary {
                    number: 15,
                    plain_summary: "Sped up the dashboard".to_string(),
                    outcome: "open".to_string(),
                },
            ],
            quiet_day: false,
        }
    }

    fn quiet_entry(date: NaiveDate) -> JournalEntry {
        JournalEntry {
            date,
            generated_at: Utc::now(),
            narrative: format!("{date} was a quiet day. Nothing remarkable happened."),
            draft: String::new(),
            prs: vec![],
            quiet_day: true,
        }
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn render_route_renders_entry_with_pr_table_xss_safe() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        crate::journal::save_entry(mem.ops(), &entry_with_prs(ymd(2026, 7, 5))).expect("save");

        let Html(html) = journal_render(AxumPath("2026-07-05".to_string())).await;

        // Narrative + PR table with both rows and the plain-language summary.
        assert!(
            html.contains("helped fix the login page"),
            "narrative present"
        );
        assert!(
            html.contains("<table class=\"journal-prs\""),
            "PR table present: {html}"
        );
        assert!(html.contains("#12") && html.contains("#15"), "both PR rows");
        assert!(html.contains("Sped up the dashboard"), "PR summary present");
        assert!(html.contains("merged"), "outcome present");

        // XSS-safe: untrusted markup in narrative and PR summary is escaped.
        assert!(!html.contains("<script>"), "raw <script> must not survive");
        assert!(html.contains("&lt;script&gt;"), "narrative script escaped");
        assert!(!html.contains("<b>safer</b>"), "raw <b> must not survive");
        assert!(
            html.contains("&lt;b&gt;safer&lt;/b&gt;"),
            "PR markup escaped"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn render_route_empty_day_is_honest() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        crate::journal::save_entry(mem.ops(), &quiet_entry(ymd(2026, 7, 6))).expect("save");

        // A stored quiet day renders the honest quiet narrative, no PR table.
        let Html(html) = journal_render(AxumPath("2026-07-06".to_string())).await;
        assert!(html.to_lowercase().contains("quiet"), "quiet day narrated");
        assert!(
            html.contains("No code changes were proposed"),
            "honest no-changes"
        );
        assert!(!html.contains("<table"), "no PR table on a quiet day");

        // A day with NO entry at all is honest too (and never fabricated).
        let Html(none_html) = journal_render(AxumPath("2020-01-01".to_string())).await;
        assert!(
            none_html.contains("No journal entry for this day yet"),
            "absent day is honest: {none_html}"
        );
        assert!(!none_html.contains("<table"), "no fabricated table");
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn dates_and_search_browse_and_find_entries() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        crate::journal::save_entry(mem.ops(), &entry_with_prs(ymd(2026, 7, 5))).expect("save");
        crate::journal::save_entry(mem.ops(), &quiet_entry(ymd(2026, 7, 6))).expect("save");

        // Dates: newest day first.
        let Json(dates) = journal_dates().await;
        let list = dates["dates"].as_array().expect("dates array");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["date"], "2026-07-06", "newest first");
        assert_eq!(list[1]["date"], "2026-07-05");
        assert_eq!(list[1]["pr_count"], 2);
        assert_eq!(list[1]["merged"], 1);

        // Search matches PR-summary text and returns a snippet.
        let Json(found) = journal_search(Json(json!({ "query": "login" }))).await;
        let results = found["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["date"], "2026-07-05");
        assert!(
            results[0]["snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("login"),
            "snippet carries the match"
        );

        // Search matches the quiet-day narrative too, case-insensitively.
        let Json(quiet) = journal_search(Json(json!({ "query": "QUIET" }))).await;
        assert_eq!(quiet["results"].as_array().expect("arr").len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn entry_route_returns_json_or_honest_error() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        crate::journal::save_entry(mem.ops(), &entry_with_prs(ymd(2026, 7, 5))).expect("save");

        let Json(entry) = journal_entry(AxumPath("2026-07-05".to_string())).await;
        assert_eq!(entry["date"], "2026-07-05");
        assert_eq!(entry["prs"].as_array().expect("prs").len(), 2);
        assert_eq!(entry["prs"][0]["number"], 12);

        // Absent day and bad date are honest errors, never fabricated entries.
        let Json(missing) = journal_entry(AxumPath("2020-01-01".to_string())).await;
        assert_eq!(missing["status"], "error");
        let Json(bad) = journal_entry(AxumPath("not-a-date".to_string())).await;
        assert_eq!(bad["status"], "error");
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn dates_collapse_duplicate_day_facts_newest_wins() {
        use crate::journal::store::{JOURNAL_TAG, journal_caller_key};

        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let d = ymd(2026, 7, 15);
        let key = journal_caller_key(d);

        // Forge the live-store corruption directly through the real backend:
        // two distinct `journal:2026-07-15` facts for the SAME day. `store_fact`
        // (no caller key) appends without dedup, so both persist — exactly the
        // state that made the date picker list the day twice.
        let older = JournalEntry {
            date: d,
            generated_at: "2026-07-15T21:20:47Z".parse().expect("ts"),
            narrative: "older generation".to_string(),
            draft: String::new(),
            prs: vec![PrSummary {
                number: 1,
                plain_summary: "a".to_string(),
                outcome: "open".to_string(),
            }],
            quiet_day: false,
        };
        let mut newer = older.clone();
        newer.generated_at = "2026-07-15T22:04:00Z".parse().expect("ts");
        newer.narrative = "newer generation".to_string();
        newer.prs = vec![
            PrSummary {
                number: 1,
                plain_summary: "a".to_string(),
                outcome: "merged".to_string(),
            },
            PrSummary {
                number: 2,
                plain_summary: "b".to_string(),
                outcome: "open".to_string(),
            },
        ];
        for e in [&newer, &older] {
            let content = serde_json::to_string(e).expect("serialize");
            mem.ops()
                .store_fact(
                    &key,
                    &content,
                    1.0,
                    &[JOURNAL_TAG.to_string()],
                    "journal-generator",
                )
                .expect("inject duplicate journal fact");
        }

        // The Journal tab date picker must show the day exactly ONCE, carrying
        // the newest generation's counts (2 PRs, 1 merged) — never the day
        // twice with conflicting numbers.
        let Json(dates) = journal_dates().await;
        let list = dates["dates"].as_array().expect("dates array");
        assert_eq!(
            list.len(),
            1,
            "duplicate-day facts must collapse to one date, got: {list:?}"
        );
        assert_eq!(list[0]["date"], "2026-07-15");
        assert_eq!(list[0]["pr_count"], 2, "newest generation's PR count");
        assert_eq!(list[0]["merged"], 1, "newest generation's merged count");

        // Search agrees (no duplicate day).
        let Json(found) = journal_search(Json(json!({}))).await;
        let results = found["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1, "search must not list the day twice");
        assert_eq!(results[0]["pr_count"], 2);
    }

    #[test]
    fn parse_range_tolerates_swapped_bounds_and_ignores_partial() {
        let swapped = parse_range(&json!({"from": "2026-07-06", "to": "2026-07-01"}));
        assert_eq!(swapped, Some((ymd(2026, 7, 1), ymd(2026, 7, 6))));
        assert_eq!(parse_range(&json!({"from": "2026-07-01"})), None);
        assert_eq!(parse_range(&json!({"to": "bogus"})), None);
    }
}
