//! Native Rust implementation of the knowledge knowledge.
//!
//! Replaces `python/simard_knowledge_client.py` with in-process Rust logic.
//! Reads knowledge pack manifests from disk and queries pack databases via
//! rusqlite, eliminating the Python subprocess dependency.
//!
//! Parity with the Python agent-kgpacks contract is tracked by a measurable
//! criteria checklist in `Specs/agent-kgpacks-rs-parity.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rpc::RpcErrorPayload;
use crate::rpc_transport::native::NativeRpcTransport;

const ERROR_INTERNAL: i32 = -32603;

/// Manifest metadata for a knowledge pack, matching the Python PackRegistry shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PackManifest {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    graph_stats: GraphStats,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct GraphStats {
    #[serde(default)]
    articles: u32,
    #[serde(default)]
    entities: u32,
    #[serde(default)]
    relationships: u32,
    #[serde(default)]
    size_mb: f64,
}

/// Discovered pack on disk.
#[derive(Clone, Debug)]
struct DiscoveredPack {
    name: String,
    description: String,
    article_count: u32,
    section_count: u32,
    db_path: PathBuf,
}

/// Discover all packs in the packs directory.
fn discover_packs(packs_dir: &Path) -> Vec<DiscoveredPack> {
    let entries = match std::fs::read_dir(packs_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut packs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("manifest.json");
        let db_path = path.join("pack.db");

        // Try to read manifest; fall back to directory-name based metadata.
        let (name, description, article_count, section_count) =
            if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<PackManifest>(&content) {
                    (
                        manifest.name,
                        manifest.description,
                        manifest.graph_stats.articles,
                        manifest.graph_stats.entities,
                    )
                } else {
                    let dir_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    (dir_name, String::new(), 0, 0)
                }
            } else {
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                (dir_name, String::new(), 0, 0)
            };

        packs.push(DiscoveredPack {
            name,
            description,
            article_count,
            section_count,
            db_path,
        });
    }

    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

/// Open a pack database read-only and answer `question` against it.
///
/// This is now a **test-only** convenience wrapper: production queries go
/// through [`ConnCache`] + [`query_open_pack`] so the connection is reused
/// (KGP-T3). The path-based unit tests keep exercising the full retrieval path
/// via this helper.
#[cfg(test)]
fn query_pack_db(
    db_path: &Path,
    question: &str,
    limit: usize,
) -> Result<(String, Vec<SourceInfo>, f64), String> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("cannot open pack database: {e}"))?;
    query_open_pack(&conn, question, limit)
}

/// Query a pack's SQLite database for entities matching the question, against an
/// **already-open** connection.
///
/// This is a simplified version of the Python KnowledgeGraphAgent.query().
/// It searches across article titles and content for relevant matches.
///
/// Split out of [`query_pack_db`] (KGP-T3) so a cached, reused connection
/// (see [`ConnCache`]) and a freshly-opened one share exactly one query path.
/// `query_pack_db` owns opening the read-only connection; this function owns the
/// keyword extraction + retrieval + answer synthesis so neither the reuse cache
/// nor the path-based unit tests duplicate that logic.
fn query_open_pack(
    conn: &Connection,
    question: &str,
    limit: usize,
) -> Result<(String, Vec<SourceInfo>, f64), String> {
    // Search for articles/sections matching keywords from the question.
    //
    // Keep only DISTINCT keywords (case-folded). The relevance ranking in
    // [`query_articles`] scores keyword COVERAGE, so a query word repeated
    // across the question ("rust ... rust") would otherwise count twice and
    // over-reward an article that merely mentions that one word — the same
    // double-counting the distinct-token discipline of `knowledge_context`
    // (#4241) removes at pack-selection level. SQLite's default `LIKE` is ASCII
    // case-insensitive, so folding the dedup key with `to_ascii_lowercase`
    // matches how the keyword is later compared. First-seen surface form and
    // order are preserved for the readable answer message (`keywords.join`).
    let mut seen = std::collections::HashSet::new();
    let keywords: Vec<&str> = question
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .filter(|w| seen.insert(w.to_ascii_lowercase()))
        .collect();

    if keywords.is_empty() {
        return Ok((
            "Please provide a more specific question.".to_string(),
            Vec::new(),
            0.1,
        ));
    }

    // Try to query articles table — pack databases may have varying schemas.
    let sources = query_articles(conn, &keywords, limit);
    let answer = build_answer(conn, &keywords, &sources);
    let confidence = estimate_confidence(&answer, &sources);

    Ok((answer, sources, confidence))
}

#[derive(Clone, Debug)]
struct SourceInfo {
    title: String,
    section: String,
    url: Option<String>,
}

/// Whether `table` has a column named `col` (case-insensitive; best-effort —
/// any error, e.g. a missing table, degrades to `false`).
///
/// `table` is always drawn from the fixed allowlist in [`query_articles`], so
/// interpolating it into the `PRAGMA` (which cannot bind an identifier
/// parameter) introduces no injection surface.
fn table_has_column(conn: &Connection, table: &str, col: &str) -> bool {
    let sql = format!("PRAGMA table_info({table})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    // PRAGMA table_info column 1 (`name`) is the column name.
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    rows.flatten().any(|name| name.eq_ignore_ascii_case(col))
}

/// Relevance weight of a keyword found in an article's **title**. A title hit is
/// a stronger topical signal than a passing mention in the body, so it is scored
/// above [`CONTENT_MATCH_WEIGHT`] when ranking candidates.
const TITLE_MATCH_WEIGHT: i64 = 2;

/// Relevance weight of a keyword found only in an article's **content** body.
const CONTENT_MATCH_WEIGHT: i64 = 1;

/// Escape character used with `LIKE ... ESCAPE` so a keyword's own `%` / `_`
/// characters are matched literally rather than acting as SQL `LIKE` wildcards.
const LIKE_ESCAPE_CHAR: char = '\\';

/// Wrap a keyword as a bound `LIKE` pattern (`%keyword%`) that matches the
/// keyword as a **literal substring**.
///
/// The returned string is passed to SQLite as a *bound parameter* (never
/// interpolated into SQL), so it cannot alter the statement. In addition, the
/// keyword's own `LIKE` metacharacters — `%` (any run), `_` (any single char),
/// and the escape character itself — are escaped so they are matched literally.
/// Callers therefore pair the bound value with `LIKE ?n ESCAPE '\'`. Without
/// this, a question word such as `100%` or `a_b` would silently widen the match
/// (`%` = wildcard) instead of searching for the literal token the user asked
/// about.
fn like_contains_pattern(keyword: &str) -> String {
    let mut escaped = String::with_capacity(keyword.len() + 2);
    escaped.push('%');
    for c in keyword.chars() {
        if c == LIKE_ESCAPE_CHAR || c == '%' || c == '_' {
            escaped.push(LIKE_ESCAPE_CHAR);
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

/// Query the articles table for matching content.
///
/// Candidate articles are gathered by a LIKE-based keyword search (any keyword
/// hit qualifies — recall breadth) and then **ranked by keyword coverage** so
/// the most on-topic article survives the `limit` cut. Each keyword contributes
/// [`TITLE_MATCH_WEIGHT`] when it appears in the title and
/// [`CONTENT_MATCH_WEIGHT`] when it appears in the body; the per-article score is
/// the sum across the distinct query keywords (the caller de-duplicates keywords
/// case-insensitively, so a repeated query word cannot double-count), and rows
/// are returned highest-score-first with a deterministic `title ASC` tie-break
/// among equal-score matches (`ORDER BY … DESC, title ASC LIMIT`).
///
/// This fixes a recall-quality defect: without the `ORDER BY`, SQLite returned
/// matching rows in arbitrary storage (rowid) order, so an article matching a
/// single keyword purely because it was inserted earlier could crowd a genuinely
/// on-topic article — one matching every keyword — out of the `limit` results,
/// starving the reasoner's planning context of the most relevant knowledge. The
/// coverage ranking mirrors the whole-word / keyword-coverage relevance policy
/// already adopted by [`crate::knowledge_context`] and
/// [`crate::fact_reliability`]. (The LIKE probes stay substring-based to preserve
/// recall breadth for stemmed/compound forms; ranking, not membership, is what
/// governs which candidates survive the cut.)
///
/// **Parameterized search (KGP-Q4).** Each keyword is passed to SQLite as a
/// *bound* `LIKE` parameter (`?n`) via [`like_contains_pattern`] rather than
/// being interpolated into the SQL text. The statement's placeholders are the
/// only keyword-derived tokens in the SQL, so a keyword containing `'`, `%`, or
/// `_` cannot alter the query: quotes are handled by binding, and the pattern's
/// own `%`/`_` are escaped (`LIKE ?n ESCAPE '\'`) so they match literally
/// instead of acting as wildcards. Each keyword's placeholder is referenced
/// from both the `WHERE` membership clause and the `ORDER BY` coverage score, so
/// each keyword is bound exactly once.
///
/// When the matched table carries a `url` column, each returned [`SourceInfo`]
/// is populated with the article's source URL so answers trace back to a
/// specific source article — the agent-kgpacks citation guarantee. Packs whose
/// schema has no `url` column degrade gracefully to `url: None` (unchanged
/// behaviour).
fn query_articles(conn: &Connection, keywords: &[&str], limit: usize) -> Vec<SourceInfo> {
    // Build a LIKE-based search (pack databases don't always have FTS). Each
    // keyword becomes one bound `%keyword%` pattern (KGP-Q4): the value is bound
    // as parameter `?n` — never interpolated — and its own LIKE metacharacters
    // are escaped so the search is a literal-substring probe that cannot alter
    // the SQL. The same `?n` is reused by both the WHERE clause and the ORDER BY
    // relevance score below.
    let patterns: Vec<String> = keywords.iter().map(|k| like_contains_pattern(k)).collect();

    if patterns.is_empty() {
        return Vec::new();
    }

    let where_clause = (1..=patterns.len())
        .map(|i| {
            format!("(title LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' OR content LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}')")
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    // Relevance score: reward keyword COVERAGE, weighting a title hit above a
    // content-only hit. Ranking the candidates by this score (DESC) before the
    // LIMIT keeps the most on-topic article instead of dropping it for an
    // arbitrary earlier-in-table row that matched a single keyword. A
    // deterministic `title ASC` tie-break (applied in the ORDER BY below) then
    // fixes the order of equal-score matches so recall is reproducible run to
    // run rather than falling back to SQLite's arbitrary storage order.
    let score_expr = (1..=patterns.len())
        .map(|i| {
            format!(
                "(CASE WHEN title LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' THEN {TITLE_MATCH_WEIGHT} ELSE 0 END) \
                 + (CASE WHEN content LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' THEN {CONTENT_MATCH_WEIGHT} ELSE 0 END)"
            )
        })
        .collect::<Vec<_>>()
        .join(" + ");

    // Try "articles" table first, then "nodes"/"entities" as fallback.
    for table in &["articles", "nodes", "entities"] {
        // Select the real `url` column when the schema has one; otherwise
        // project a literal NULL so the row shape (and the reader closure
        // below) stays uniform across pack schemas.
        let url_col = if table_has_column(conn, table, "url") {
            "url"
        } else {
            "NULL AS url"
        };
        let sql = format!(
            "SELECT title, COALESCE(section, '') as section, {url_col} FROM {table} WHERE {where_clause} ORDER BY ({score_expr}) DESC, title ASC LIMIT {limit}",
            table = table,
            where_clause = where_clause,
            score_expr = score_expr,
            limit = limit,
        );

        if let Ok(mut stmt) = conn.prepare(&sql) {
            let mut sources = Vec::new();
            if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(patterns.iter()), |row| {
                let url = row
                    .get::<_, Option<String>>(2)
                    .unwrap_or(None)
                    // Treat a present-but-empty URL as "no citation".
                    .filter(|u| !u.is_empty());
                Ok(SourceInfo {
                    title: row.get::<_, String>(0).unwrap_or_default(),
                    section: row.get::<_, String>(1).unwrap_or_default(),
                    url,
                })
            }) {
                for row in rows.flatten() {
                    sources.push(row);
                }
            }
            if !sources.is_empty() {
                return sources;
            }
        }
    }

    Vec::new()
}

/// Build an answer string from matched content.
fn build_answer(conn: &Connection, keywords: &[&str], sources: &[SourceInfo]) -> String {
    if sources.is_empty() {
        return format!(
            "No relevant information found for the query involving: {}",
            keywords.join(", ")
        );
    }

    // Try to extract content snippets from matched articles.
    let mut snippets = Vec::new();
    for source in sources.iter().take(3) {
        for table in &["articles", "nodes", "entities"] {
            let sql = format!(
                "SELECT content FROM {table} WHERE title = ?1 LIMIT 1",
                table = table,
            );
            if let Ok(mut stmt) = conn.prepare(&sql)
                && let Ok(content) = stmt.query_row([&source.title], |row| row.get::<_, String>(0))
            {
                let truncated = if content.len() > 500 {
                    // Char-boundary-safe: `&content[..500]` panics when byte 500
                    // splits a multi-byte UTF-8 sequence, and knowledge-article
                    // content read from SQLite routinely contains non-ASCII text.
                    let mut t = content;
                    crate::util::string_truncate::truncate_to_char_boundary(&mut t, 500);
                    t.push_str("...");
                    t
                } else {
                    content
                };
                snippets.push(truncated);
                break;
            }
        }
    }

    if snippets.is_empty() {
        format!(
            "Found {} relevant sources for: {}",
            sources.len(),
            keywords.join(", ")
        )
    } else {
        snippets.join("\n\n")
    }
}

/// Port of Python's _estimate_confidence heuristic.
fn estimate_confidence(answer: &str, sources: &[SourceInfo]) -> f64 {
    if sources.is_empty() {
        return 0.3;
    }
    if answer.is_empty() {
        return 0.1;
    }

    let source_score = (sources.len() as f64 / 5.0).min(1.0);
    let length_score = (answer.len() as f64 / 200.0).min(1.0);
    let raw = 0.3 + 0.4 * source_score + 0.3 * length_score;
    (raw * 100.0).round() / 100.0
}

/// Per-pack cache of a **live, reused** read-only SQLite connection (KGP-T3).
///
/// Previously the knowledge handler cached only each pack's database *path* and
/// re-opened a fresh [`Connection`] on every `knowledge.query` — re-parsing the
/// schema and paying the file-open cost per request. This caches the open
/// connection itself so repeated queries against the same pack reuse one handle.
///
/// rusqlite's [`Connection`] is `Send` but **not** `Sync`, so each cached
/// connection is wrapped in its own [`Mutex`]: queries against one pack serialize
/// on that per-pack mutex (a read-only connection, so this is cheap), while
/// queries against *different* packs proceed independently. The newtype also
/// keeps the nested `Arc<Mutex<HashMap<_, Arc<Mutex<Connection>>>>>` off the
/// call sites.
#[derive(Default)]
struct ConnCache {
    inner: Arc<Mutex<HashMap<String, Arc<Mutex<Connection>>>>>,
}

impl ConnCache {
    fn new() -> Self {
        Self::default()
    }

    /// Return the cached read-only connection for `pack_name`, opening a new one
    /// on first use. `resolve_path` is invoked **only on a cache miss** to locate
    /// (and validate) the pack's database, so the hot path (a warm cache) avoids
    /// re-discovering packs on disk. Subsequent calls with the same `pack_name`
    /// return the *same* [`Connection`] handle (proven by `Arc::ptr_eq` in the
    /// reuse test), which is the observable meaning of connection reuse.
    fn get_or_open<F>(
        &self,
        pack_name: &str,
        resolve_path: F,
    ) -> Result<Arc<Mutex<Connection>>, String>
    where
        F: FnOnce() -> Result<PathBuf, String>,
    {
        let mut cache = self.inner.lock().unwrap();
        if let Some(conn) = cache.get(pack_name) {
            return Ok(conn.clone());
        }
        let db_path = resolve_path()?;
        let conn =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("cannot open pack database: {e}"))?;
        let handle = Arc::new(Mutex::new(conn));
        cache.insert(pack_name.to_string(), handle.clone());
        Ok(handle)
    }
}

/// Register all knowledge knowledge method handlers on a NativeRpcTransport.
pub fn register_knowledge_handlers(transport: &mut NativeRpcTransport, packs_dir: PathBuf) {
    let packs_dir_list = packs_dir.clone();
    let packs_dir_info = packs_dir.clone();
    let packs_dir_query = packs_dir;

    // Shared cache of live read-only connections, one per pack, so repeated
    // queries reuse an open connection instead of re-opening the database
    // (KGP-T3).
    let conn_cache = ConnCache::new();

    // knowledge.list_packs
    transport.register(
        "knowledge.list_packs",
        Arc::new(move |_params: &Value| {
            let packs = discover_packs(&packs_dir_list);
            let pack_infos: Vec<Value> = packs
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "description": p.description,
                        "article_count": p.article_count,
                        "section_count": p.section_count,
                    })
                })
                .collect();
            Ok(serde_json::json!({ "packs": pack_infos }))
        }),
    );

    // knowledge.pack_info
    transport.register(
        "knowledge.pack_info",
        Arc::new(move |params: &Value| {
            let pack_name = params
                .get("pack_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if pack_name.is_empty() {
                return Err(RpcErrorPayload {
                    code: ERROR_INTERNAL,
                    message: "pack_name is required".to_string(),
                });
            }

            let packs = discover_packs(&packs_dir_info);
            let pack = packs.iter().find(|p| p.name == pack_name);
            match pack {
                Some(p) => Ok(serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "article_count": p.article_count,
                    "section_count": p.section_count,
                })),
                None => Err(RpcErrorPayload {
                    code: ERROR_INTERNAL,
                    message: format!("pack '{pack_name}' not found"),
                }),
            }
        }),
    );

    // knowledge.query
    transport.register(
        "knowledge.query",
        Arc::new(move |params: &Value| {
            let pack_name = params
                .get("pack_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let question = params
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

            if pack_name.is_empty() {
                return Err(RpcErrorPayload {
                    code: ERROR_INTERNAL,
                    message: "pack_name is required".to_string(),
                });
            }

            if question.is_empty() {
                return Ok(serde_json::json!({
                    "answer": "Please provide a question.",
                    "sources": [],
                    "confidence": 0.0,
                }));
            }

            // Resolve (on cache miss) and reuse a live read-only connection for
            // this pack. `resolve_path` runs only when the pack is not yet
            // cached: it discovers the pack on disk and validates its database
            // exists, preserving the prior "not found" / "no database" errors.
            let handle = conn_cache.get_or_open(pack_name, || {
                let packs = discover_packs(&packs_dir_query);
                let pack = packs
                    .iter()
                    .find(|p| p.name == pack_name)
                    .ok_or_else(|| format!("pack '{pack_name}' not found"))?;
                let db_path = pack.db_path.clone();
                if !db_path.exists() {
                    return Err(format!(
                        "pack '{pack_name}' has no database at {}",
                        db_path.display()
                    ));
                }
                Ok(db_path)
            });
            let handle = match handle {
                Ok(h) => h,
                Err(message) => {
                    return Err(RpcErrorPayload {
                        code: ERROR_INTERNAL,
                        message,
                    });
                }
            };

            let limit = limit.min(100);
            let result = {
                let conn = handle.lock().unwrap();
                query_open_pack(&conn, question, limit)
            };
            match result {
                Ok((answer, sources, confidence)) => {
                    let source_values: Vec<Value> = sources
                        .iter()
                        .take(limit)
                        .map(|s| {
                            let mut obj = serde_json::json!({
                                "title": s.title,
                                "section": s.section,
                            });
                            if let Some(url) = &s.url {
                                obj["url"] = serde_json::json!(url);
                            }
                            obj
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "answer": answer,
                        "sources": source_values,
                        "confidence": confidence,
                    }))
                }
                Err(e) => Err(RpcErrorPayload {
                    code: ERROR_INTERNAL,
                    message: e,
                }),
            }
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_pack(packs_dir: &Path, name: &str) -> PathBuf {
        let pack_dir = packs_dir.join(name);
        fs::create_dir_all(&pack_dir).unwrap();

        // Write manifest
        let manifest = serde_json::json!({
            "name": name,
            "description": format!("{name} knowledge pack"),
            "graph_stats": {
                "articles": 10,
                "entities": 25,
                "relationships": 30,
                "size_mb": 1.5
            }
        });
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        // Create a SQLite database with test data
        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT);
             INSERT INTO articles VALUES ('Ownership in Rust', 'Basics', 'Ownership is a set of rules that govern how a Rust program manages memory.');
             INSERT INTO articles VALUES ('Borrowing', 'References', 'References allow you to refer to a value without taking ownership of it.');
             INSERT INTO articles VALUES ('Lifetimes', 'Advanced', 'Lifetimes are a way of telling the compiler how long references are valid.');",
        )
        .unwrap();

        pack_dir
    }

    /// Like [`create_test_pack`] but the `articles` schema carries a `url`
    /// column, mirroring a real agent-kgpacks pack whose articles cite a
    /// source URL. Used to prove the native knowledge reader surfaces those citations.
    fn create_test_pack_with_urls(packs_dir: &Path, name: &str) -> PathBuf {
        let pack_dir = packs_dir.join(name);
        fs::create_dir_all(&pack_dir).unwrap();

        let manifest = serde_json::json!({
            "name": name,
            "description": format!("{name} knowledge pack"),
            "graph_stats": { "articles": 2, "entities": 4, "relationships": 3, "size_mb": 0.1 }
        });
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT);
             INSERT INTO articles VALUES ('Ownership in Rust', 'Basics', 'Ownership is a set of rules that govern how a Rust program manages memory.', 'https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html');
             INSERT INTO articles VALUES ('Borrowing', 'References', 'References let you refer to a value without taking ownership.', '');",
        )
        .unwrap();

        pack_dir
    }

    #[test]
    fn discover_packs_finds_packs_with_manifests() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "rust-expert");
        create_test_pack(tmp.path(), "python-expert");

        let packs = discover_packs(tmp.path());
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].name, "python-expert");
        assert_eq!(packs[1].name, "rust-expert");
        assert_eq!(packs[1].article_count, 10);
        assert_eq!(packs[1].section_count, 25);
    }

    #[test]
    fn discover_packs_returns_empty_for_missing_dir() {
        let packs = discover_packs(Path::new("/nonexistent/path"));
        assert!(packs.is_empty());
    }

    #[test]
    fn query_pack_db_finds_matching_articles() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "test-pack");
        let db_path = pack_dir.join("pack.db");

        let (answer, sources, confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();
        assert!(!answer.is_empty());
        assert!(!sources.is_empty());
        assert!(confidence > 0.0);
        assert!(sources.iter().any(|s| s.title.contains("Ownership")));
    }

    #[test]
    fn query_pack_db_returns_low_confidence_for_no_matches() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "test-pack");
        let db_path = pack_dir.join("pack.db");

        let (answer, sources, confidence) =
            query_pack_db(&db_path, "quantum entanglement physics", 5).unwrap();
        assert!(sources.is_empty() || confidence <= 0.5);
        let _ = answer; // may be a "not found" message
    }

    #[test]
    fn query_pack_db_handles_empty_question_keywords() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "test-pack");
        let db_path = pack_dir.join("pack.db");

        let (answer, sources, confidence) = query_pack_db(&db_path, "a", 5).unwrap();
        // All single-char keywords are filtered out
        assert!(confidence <= 0.2);
        assert!(sources.is_empty());
        let _ = answer;
    }

    #[test]
    fn estimate_confidence_matches_python_heuristics() {
        // No sources → 0.3
        assert!((estimate_confidence("some answer", &[]) - 0.3).abs() < 0.01);

        // No answer → 0.1
        assert!(
            (estimate_confidence(
                "",
                &[SourceInfo {
                    title: "t".into(),
                    section: "".into(),
                    url: None,
                }]
            ) - 0.1)
                .abs()
                < 0.01
        );

        // Both present → > 0.3
        let sources = vec![SourceInfo {
            title: "Article".into(),
            section: "Section".into(),
            url: None,
        }];
        let conf = estimate_confidence("A reasonable answer with some content", &sources);
        assert!(conf > 0.3);
    }

    #[test]
    fn native_knowledge_transport_list_packs() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.list_packs".to_string(),
            params: serde_json::json!({}),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        let packs = result["packs"].as_array().unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0]["name"], "test-pack");
    }

    #[test]
    fn native_knowledge_transport_query() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "test-pack",
                "question": "What is ownership?",
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(!result["answer"].as_str().unwrap().is_empty());
        assert!(result["confidence"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn query_pack_db_returns_source_urls_when_present() {
        // Parity criterion KGP-Q1: a pack whose articles carry a `url` column
        // yields source citations with that URL, so answers trace back to a
        // specific source article (the agent-kgpacks guarantee).
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack_with_urls(tmp.path(), "url-pack");
        let db_path = pack_dir.join("pack.db");

        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();

        let cited = sources
            .iter()
            .find(|s| s.title.contains("Ownership"))
            .expect("the Ownership article must match");
        assert_eq!(
            cited.url.as_deref(),
            Some("https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html"),
            "a matched article with a url column must surface its citation URL"
        );
    }

    #[test]
    fn query_pack_db_treats_empty_url_as_no_citation() {
        // A present-but-empty url column value is not a usable citation and
        // must degrade to `None`, not `Some("")`.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack_with_urls(tmp.path(), "url-pack");
        let db_path = pack_dir.join("pack.db");

        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "References borrowing", 5).unwrap();

        let borrowing = sources
            .iter()
            .find(|s| s.title.contains("Borrowing"))
            .expect("the Borrowing article must match");
        assert_eq!(
            borrowing.url, None,
            "an empty url value must be reported as no citation (None)"
        );
    }

    #[test]
    fn query_pack_db_omits_urls_when_column_absent() {
        // Backward compatibility: packs whose schema has no `url` column keep
        // the prior behaviour (url: None) rather than erroring.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "no-url-pack");
        let db_path = pack_dir.join("pack.db");

        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();

        assert!(
            !sources.is_empty(),
            "the query must still match without urls"
        );
        assert!(
            sources.iter().all(|s| s.url.is_none()),
            "a urlless pack schema must yield no citation URLs, not an error"
        );
    }

    #[test]
    fn native_knowledge_transport_query_surfaces_source_url() {
        // End-to-end: the citation URL propagates through the RPC handler into
        // the `sources[].url` field of the wire response.
        let tmp = TempDir::new().unwrap();
        create_test_pack_with_urls(tmp.path(), "url-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "url-pack",
                "question": "What is ownership in Rust?",
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        let result = response.result.expect("query must succeed");
        let sources = result["sources"].as_array().expect("sources array");
        assert!(
            sources.iter().any(|s| s["url"].as_str()
                == Some("https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html")),
            "the wire response must include the article's source citation URL; got: {sources:?}"
        );
    }

    #[test]
    fn native_knowledge_transport_pack_info() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.pack_info".to_string(),
            params: serde_json::json!({"pack_name": "test-pack"}),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["name"], "test-pack");
        assert_eq!(result["article_count"], 10);
    }

    #[test]
    fn native_knowledge_transport_pack_not_found() {
        let tmp = TempDir::new().unwrap();

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.pack_info".to_string(),
            params: serde_json::json!({"pack_name": "nonexistent"}),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        assert!(response.error.is_some());
        assert!(response.error.unwrap().message.contains("not found"));
    }

    #[test]
    fn native_knowledge_transport_empty_question() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "test-pack",
                "question": "",
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["confidence"], 0.0);
    }

    /// Build an in-memory `articles` table whose rows are inserted in the given
    /// order, so a test can prove that ranking — not storage (rowid) order —
    /// governs which candidates survive the `limit` cut.
    fn ranking_conn(rows: &[(&str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE articles (title TEXT, section TEXT, content TEXT);")
            .unwrap();
        for (title, content) in rows {
            conn.execute(
                "INSERT INTO articles (title, section, content) VALUES (?1, 'Sec', ?2)",
                rusqlite::params![title, content],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn query_articles_ranks_most_relevant_first() {
        // Three single-keyword articles are inserted BEFORE the one article that
        // covers every keyword. Under the old arbitrary rowid order the
        // single-keyword rows would lead; coverage ranking must surface the
        // full-coverage article first.
        let conn = ranking_conn(&[
            ("Notes A", "this mentions rust only"),
            ("Notes B", "this mentions ownership only"),
            ("Notes C", "this mentions memory only"),
            (
                "Rust Ownership Memory Guide",
                "covers rust ownership and memory management",
            ),
        ]);
        let sources = query_articles(&conn, &["rust", "ownership", "memory"], 5);
        assert!(!sources.is_empty(), "expected keyword matches");
        assert_eq!(
            sources[0].title,
            "Rust Ownership Memory Guide",
            "the article covering every keyword must rank first, got: {:?}",
            sources.iter().map(|s| &s.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn query_articles_limit_keeps_most_relevant() {
        // The recall-quality regression this fix targets: with `limit = 1`, an
        // article matching a single keyword purely because it was inserted first
        // must NOT crowd out the article matching every keyword. Without the
        // ORDER BY, rowid order returned "Notes A" and dropped the guide.
        let conn = ranking_conn(&[
            ("Notes A", "this mentions rust only"),
            ("Notes B", "this mentions ownership only"),
            ("Notes C", "this mentions memory only"),
            (
                "Rust Ownership Memory Guide",
                "covers rust ownership and memory management",
            ),
        ]);
        let sources = query_articles(&conn, &["rust", "ownership", "memory"], 1);
        assert_eq!(sources.len(), 1, "limit must be honoured");
        assert_eq!(
            sources[0].title, "Rust Ownership Memory Guide",
            "the single kept result must be the most on-topic article, not an \
             arbitrary earlier-in-table single-keyword row"
        );
    }

    #[test]
    fn query_articles_prefers_title_over_content_match() {
        // Two articles match the same single keyword: one in the body only, one
        // in the title. A title hit is the stronger topical signal and must rank
        // above a passing content mention (TITLE_MATCH_WEIGHT > CONTENT_MATCH_WEIGHT).
        let conn = ranking_conn(&[
            ("General Notes", "a passage about ownership rules"),
            ("Ownership Deep Dive", "deep dive into the model"),
        ]);
        let sources = query_articles(&conn, &["ownership"], 5);
        assert_eq!(sources.len(), 2, "both articles match the keyword");
        assert_eq!(
            sources[0].title, "Ownership Deep Dive",
            "a title match must outrank a content-only match"
        );
    }

    #[test]
    fn query_articles_breaks_score_ties_by_title_ascending() {
        // Three articles match the single keyword equally (same score). Inserted
        // in reverse-alphabetical order, they must still come back title
        // ASCending rather than in SQLite's arbitrary storage order — so recall
        // is deterministic and reproducible run to run.
        let conn = ranking_conn(&[
            ("Zebra ownership", "x"),
            ("Mango ownership", "y"),
            ("Apple ownership", "z"),
        ]);
        let titles: Vec<String> = query_articles(&conn, &["ownership"], 5)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            vec![
                "Apple ownership".to_string(),
                "Mango ownership".to_string(),
                "Zebra ownership".to_string(),
            ],
            "equal-score matches must order by title ASC deterministically"
        );
    }

    #[test]
    fn like_contains_pattern_escapes_metacharacters() {
        // KGP-Q4: the bound `%keyword%` pattern must escape the keyword's own
        // LIKE metacharacters (`%`, `_`, and the escape char) so they match
        // literally. Single quotes are handled by parameter binding, not here.
        assert_eq!(like_contains_pattern("rust"), "%rust%");
        assert_eq!(like_contains_pattern("100%"), "%100\\%%");
        assert_eq!(like_contains_pattern("a_b"), "%a\\_b%");
        assert_eq!(like_contains_pattern("c\\d"), "%c\\\\d%");
        assert_eq!(like_contains_pattern("it's"), "%it's%");
    }

    #[test]
    fn query_articles_treats_like_wildcards_as_literal() {
        // KGP-Q4: a keyword's own LIKE metacharacters must match literally, not
        // act as wildcards. "100%" must match only the article containing the
        // literal token "100%" — a bare `%` wildcard would match every row.
        let conn = ranking_conn(&[
            ("Coverage Report", "we reached 100% coverage today"),
            ("Unrelated", "nothing about the metric here"),
        ]);
        let titles: Vec<String> = query_articles(&conn, &["100%"], 5)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            vec!["Coverage Report".to_string()],
            "`%` in a keyword must match literally, not as a wildcard: {titles:?}"
        );

        // "_" must likewise be literal: "a_b" must not match "axb" (which the
        // single-character `_` wildcard would).
        let conn = ranking_conn(&[
            ("Underscore", "the token a_b appears here"),
            ("SingleChar", "the token axb appears here"),
        ]);
        let titles: Vec<String> = query_articles(&conn, &["a_b"], 5)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            vec!["Underscore".to_string()],
            "`_` in a keyword must match literally, not as a single-char wildcard: {titles:?}"
        );
    }

    #[test]
    fn query_articles_binds_keywords_and_resists_injection() {
        // KGP-Q4: a keyword containing a single quote and SQL syntax must be
        // bound as a parameter — treated as a literal search string that can
        // neither break the statement nor mutate the database.
        let conn = ranking_conn(&[
            ("Contraction", "the compiler says it's fine"),
            ("Other", "unrelated content"),
        ]);

        // A quote-containing keyword still finds its literal match (no SQL error).
        let titles: Vec<String> = query_articles(&conn, &["it's"], 5)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            vec!["Contraction".to_string()],
            "a quoted keyword must match literally: {titles:?}"
        );

        // A classic injection payload must return zero rows (no literal match)
        // and must NOT drop the table — proving the value is bound, not
        // interpolated into the SQL text.
        let sources = query_articles(&conn, &["'; DROP TABLE articles; --"], 5);
        assert!(
            sources.is_empty(),
            "an injection-shaped keyword must not match any row"
        );
        let still_there: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .expect("articles table must still exist after an injection attempt");
        assert_eq!(
            still_there, 2,
            "no rows may be lost to a SQL-injection keyword — it must be bound, not executed"
        );
    }

    #[test]
    fn query_pack_db_dedups_repeated_keywords() {
        // A query word repeated across the question must not double-count in the
        // coverage ranking. 'Rust Intro' matches only "rust" (title, score 2);
        // 'Ownership Guide' matches "ownership" (title) and "borrowing" (body)
        // for score 3. With DISTINCT keywords the guide outranks the intro; if
        // "rust rust" double-counted, the intro would score 4 and wrongly lead.
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("dedup-pack");
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string(&serde_json::json!({
                "name": "dedup-pack",
                "description": "dedup-pack knowledge pack",
            }))
            .unwrap(),
        )
        .unwrap();
        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT);
             INSERT INTO articles VALUES ('Rust Intro', 'A', 'a short introduction');
             INSERT INTO articles VALUES ('Ownership Guide', 'B', 'covers borrowing rules');",
        )
        .unwrap();
        drop(conn);

        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "rust rust ownership borrowing", 5).unwrap();
        assert_eq!(
            sources.first().map(|s| s.title.as_str()),
            Some("Ownership Guide"),
            "repeated 'rust' must not double-count and vault the single-keyword \
             article above the higher-coverage one; got: {:?}",
            sources.iter().map(|s| &s.title).collect::<Vec<_>>()
        );
    }

    // ── KGP-T3: connection reuse ────────────────────────────────────────────

    #[test]
    fn conn_cache_reuses_open_connection_across_queries() {
        // Parity criterion KGP-T3: a second query to the same pack must reuse
        // the *same* open connection rather than re-opening the database. The
        // observable proof is `Arc::ptr_eq` on the two returned handles, plus
        // the resolver closure being invoked only on the first (miss) call.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "reuse-pack");
        let db_path = pack_dir.join("pack.db");

        let cache = ConnCache::new();

        let first = cache
            .get_or_open("reuse-pack", || Ok(db_path.clone()))
            .expect("first open must succeed");

        // The second call must NOT invoke the resolver (warm-cache hot path).
        let second = cache
            .get_or_open("reuse-pack", || {
                panic!("resolver must not run on a cache hit — the connection is reused")
            })
            .expect("second open must reuse the cached connection");

        assert!(
            Arc::ptr_eq(&first, &second),
            "the second query to one pack must reuse the same open Connection"
        );

        // The reused connection still answers a real query.
        let (_answer, sources, _confidence) = {
            let conn = second.lock().unwrap();
            query_open_pack(&conn, "What is ownership in Rust?", 5).unwrap()
        };
        assert!(
            sources.iter().any(|s| s.title.contains("Ownership")),
            "the reused connection must still return matching sources"
        );
    }

    #[test]
    fn conn_cache_keeps_distinct_connections_per_pack() {
        // Different packs must get independent connections, not a shared handle.
        let tmp = TempDir::new().unwrap();
        let pack_a = create_test_pack(tmp.path(), "pack-a").join("pack.db");
        let pack_b = create_test_pack(tmp.path(), "pack-b").join("pack.db");

        let cache = ConnCache::new();
        let a = cache.get_or_open("pack-a", || Ok(pack_a.clone())).unwrap();
        let b = cache.get_or_open("pack-b", || Ok(pack_b.clone())).unwrap();

        assert!(
            !Arc::ptr_eq(&a, &b),
            "two distinct packs must not share one connection handle"
        );
    }

    #[test]
    fn conn_cache_propagates_resolve_error_without_caching() {
        // A failed path resolution must surface the error and leave nothing
        // cached, so a later successful resolution can still open the pack.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "later-pack");
        let db_path = pack_dir.join("pack.db");

        let cache = ConnCache::new();
        let err = cache
            .get_or_open("later-pack", || {
                Err("pack 'later-pack' not found".to_string())
            })
            .unwrap_err();
        assert!(err.contains("not found"));

        // A subsequent successful resolve still opens and caches the pack.
        let ok = cache.get_or_open("later-pack", || Ok(db_path.clone()));
        assert!(ok.is_ok(), "a failed resolve must not poison the cache");
    }

    #[test]
    fn native_knowledge_transport_repeated_query_reuses_connection() {
        // End-to-end KGP-T3: two knowledge.query calls to one pack through the
        // RPC transport both succeed against the reused cached connection.
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let make_request = || crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "test-pack",
                "question": "What is ownership?",
                "limit": 5,
            }),
        };

        for _ in 0..2 {
            let response = crate::rpc::RpcTransport::call(&transport, make_request()).unwrap();
            let result = response.result.expect("each repeated query must succeed");
            assert!(!result["answer"].as_str().unwrap().is_empty());
            assert!(result["confidence"].as_f64().unwrap() > 0.0);
        }
    }
}
