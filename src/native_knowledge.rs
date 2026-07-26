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

    // Hybrid ranking (KGP-Q10): gather every retrieval signal the pack can
    // supply — GraphRAG multi-hop (KGP-Q5), vector semantic cosine (KGP-Q9), and
    // keyword coverage (KGP-Q4/Q8) — and blend them into one fused ranking
    // rather than committing to a single path. The question is embedded once
    // with the deterministic default embedder (`embed_text`) for the vector
    // signal. When only one signal is available, [`query_hybrid`] reproduces
    // that signal's native output unchanged (nothing to blend); when two or more
    // are present it fuses their normalized scores so the semantic/graph signal
    // can outweigh literal keyword overlap.
    let query_embedding = embed_text(question, DEFAULT_EMBED_DIM);
    let (answer, sources) = query_hybrid(conn, &keywords, &query_embedding, limit);
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

/// Query the articles table for matching content (test-only thin wrapper over
/// [`query_articles_scored`] that discards the coverage score). Production code
/// consumes the scored variant through [`query_hybrid`].
#[cfg(test)]
fn query_articles(conn: &Connection, keywords: &[&str], limit: usize) -> Vec<SourceInfo> {
    query_articles_scored(conn, keywords, limit)
        .into_iter()
        .map(|(_, source)| source)
        .collect()
}

/// Keyword-coverage retrieval over the articles table, returning each
/// candidate's **coverage score** alongside the source (KGP-Q10 signal input;
/// the score is discarded by the [`query_articles`] test wrapper).
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
fn query_articles_scored(
    conn: &Connection,
    keywords: &[&str],
    limit: usize,
) -> Vec<(f64, SourceInfo)> {
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
        // The coverage score is projected as a 4th column (`relevance`) so the
        // hybrid ranker can read it; it is the same expression that drives the
        // ORDER BY, so ordering is unchanged.
        let sql = format!(
            "SELECT title, COALESCE(section, '') as section, {url_col}, ({score_expr}) AS relevance FROM {table} WHERE {where_clause} ORDER BY relevance DESC, title ASC LIMIT {limit}",
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
                let relevance = row.get::<_, i64>(3).unwrap_or(0) as f64;
                Ok((
                    relevance,
                    SourceInfo {
                        title: row.get::<_, String>(0).unwrap_or_default(),
                        section: row.get::<_, String>(1).unwrap_or_default(),
                        url,
                    },
                ))
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

// ── GraphRAG multi-hop retrieval (KGP-Q5) ───────────────────────────────────

/// Maximum number of relationship hops traversed out from the keyword-matched
/// seed entities. One hop pulls in directly-linked entities; a second hop pulls
/// in their neighbours, matching the multi-hop retrieval the Python
/// `KnowledgeGraphAgent` performs over the entity/relationship graph. Kept small
/// so a densely-connected pack cannot fan out unbounded.
const MAX_GRAPH_HOPS: usize = 2;

/// An entity node in a pack's knowledge graph (the SQLite port of the upstream
/// agent-kgpacks `Entity` table: `entity_id`, `name`, `type`, `description`, and
/// an optional `url` for source citation).
#[derive(Clone)]
struct GraphEntity {
    id: String,
    name: String,
    etype: String,
    description: String,
    url: Option<String>,
}

/// A directed relationship edge between two entities (the SQLite port of the
/// upstream `ENTITY_RELATION(FROM Entity TO Entity, relation, context)`): a
/// `source_id`/`target_id` pair labelled by `relation`.
#[derive(Clone)]
struct GraphEdge {
    source: String,
    target: String,
    relation: String,
}

/// Whether a table named `table` exists in the database (case-insensitive;
/// best-effort — any error degrades to `false`). Parameter-bound, so `table` is
/// never interpolated into SQL.
fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND lower(name) = lower(?1) LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

/// Find the keyword-matched **seed** entities, ranked by keyword coverage over
/// the entity `name` (weighted like a title hit) and `description` (weighted
/// like a content hit) — the same parameterized, injection-resistant LIKE search
/// and coverage ranking used by [`query_articles`] (KGP-Q4/Q8). The entity's
/// `url` column, when present, is projected so graph answers keep the
/// source-citation guarantee (KGP-Q1); an empty url degrades to `None`.
fn seed_entities(conn: &Connection, keywords: &[&str], limit: usize) -> Vec<GraphEntity> {
    let patterns: Vec<String> = keywords.iter().map(|k| like_contains_pattern(k)).collect();
    if patterns.is_empty() {
        return Vec::new();
    }

    let where_clause = (1..=patterns.len())
        .map(|i| {
            format!(
                "(name LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' OR description LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}')"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    let score_expr = (1..=patterns.len())
        .map(|i| {
            format!(
                "(CASE WHEN name LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' THEN {TITLE_MATCH_WEIGHT} ELSE 0 END) \
                 + (CASE WHEN description LIKE ?{i} ESCAPE '{LIKE_ESCAPE_CHAR}' THEN {CONTENT_MATCH_WEIGHT} ELSE 0 END)"
            )
        })
        .collect::<Vec<_>>()
        .join(" + ");

    let url_col = if table_has_column(conn, "entities", "url") {
        "url"
    } else {
        "NULL AS url"
    };

    let sql = format!(
        "SELECT entity_id, name, COALESCE(type, '') AS type, COALESCE(description, '') AS description, {url_col} \
         FROM entities WHERE {where_clause} ORDER BY ({score_expr}) DESC, name ASC LIMIT {limit}"
    );

    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(patterns.iter()), row_to_entity)
    else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// Read a `(entity_id, name, type, description, url)` row into a [`GraphEntity`].
fn row_to_entity(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEntity> {
    let url = row
        .get::<_, Option<String>>(4)
        .unwrap_or(None)
        .filter(|u| !u.is_empty());
    Ok(GraphEntity {
        id: row.get::<_, String>(0).unwrap_or_default(),
        name: row.get::<_, String>(1).unwrap_or_default(),
        etype: row.get::<_, String>(2).unwrap_or_default(),
        description: row.get::<_, String>(3).unwrap_or_default(),
        url,
    })
}

/// Fetch every relationship edge incident to any entity id in `ids` (either as
/// source or target). Ids are bound as parameters (`?n`) and the same numbered
/// placeholders are reused in both `IN` lists, so no id is interpolated into SQL.
fn fetch_edges(conn: &Connection, ids: &[String]) -> Vec<GraphEdge> {
    if ids.is_empty() {
        return Vec::new();
    }
    let in_list = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT source_id, target_id, COALESCE(relation, '') AS relation FROM relationships \
         WHERE source_id IN ({in_list}) OR target_id IN ({in_list})"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
        Ok(GraphEdge {
            source: row.get::<_, String>(0).unwrap_or_default(),
            target: row.get::<_, String>(1).unwrap_or_default(),
            relation: row.get::<_, String>(2).unwrap_or_default(),
        })
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// Fetch entities by id (for the neighbours discovered during traversal). Ids
/// are bound as parameters.
fn fetch_entities_by_id(conn: &Connection, ids: &[String]) -> Vec<GraphEntity> {
    if ids.is_empty() {
        return Vec::new();
    }
    let in_list = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let url_col = if table_has_column(conn, "entities", "url") {
        "url"
    } else {
        "NULL AS url"
    };
    let sql = format!(
        "SELECT entity_id, name, COALESCE(type, '') AS type, COALESCE(description, '') AS description, {url_col} \
         FROM entities WHERE entity_id IN ({in_list})"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(ids.iter()), row_to_entity) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

/// GraphRAG multi-hop retrieval over a pack's `entities` + `relationships`
/// tables (KGP-Q5).
///
/// Returns `None` — so the caller falls back to the single-table article scan —
/// when the pack has no graph (`entities` or `relationships` table missing) or
/// when no seed entity matches the question. Otherwise it:
///
/// 1. selects keyword-matched **seed** entities (ranked by coverage, KGP-Q8),
/// 2. traverses relationship edges outward up to [`MAX_GRAPH_HOPS`] hops,
///    pulling in the **linked** entities (breadth-first, so nearer neighbours
///    come first), and
/// 3. builds a graph-grounded answer that names the linked entities and the
///    relationships joining them, plus the [`SourceInfo`] citations (with the
///    entity `url` when present, KGP-Q1).
///
/// This is the Rust port of the Python `KnowledgeGraphAgent.query()` graph
/// traversal that the previous single-table LIKE scan only approximated.
/// Test-only thin wrapper over [`query_graph_scored`] that discards the
/// per-entity graph-proximity score. Production code consumes the scored variant
/// through [`query_hybrid`].
#[cfg(test)]
fn query_graph(
    conn: &Connection,
    keywords: &[&str],
    limit: usize,
) -> Option<(String, Vec<SourceInfo>)> {
    let traversal = graph_traverse(conn, keywords, limit)?;
    let sources: Vec<SourceInfo> = traversal
        .order
        .iter()
        .take(limit)
        .filter_map(|id| traversal.known.get(id))
        .map(entity_to_source)
        .collect();
    let answer = build_graph_answer(&traversal.known, &traversal.order, &traversal.edges);
    Some((answer, sources))
}

/// GraphRAG retrieval that also returns each surfaced entity's **graph-proximity
/// score** (KGP-Q10 signal input).
///
/// Same traversal as [`query_graph`], but each returned [`SourceInfo`] is paired
/// with a proximity score `1 / (1 + hop_distance)` — a keyword-matched seed
/// (hop 0) scores `1.0`, a directly-linked neighbour (hop 1) `0.5`, a two-hop
/// neighbour (hop 2) `~0.33` — so the hybrid ranker ([`fuse_signals`]) can blend
/// the graph signal with the vector and keyword signals, weighting nearer
/// graph-linked entities above farther ones. Returns `None` on the same
/// conditions as [`query_graph`] (no graph tables / no seed match).
fn query_graph_scored(
    conn: &Connection,
    keywords: &[&str],
    limit: usize,
) -> Option<(String, Vec<(f64, SourceInfo)>)> {
    let traversal = graph_traverse(conn, keywords, limit)?;
    let scored: Vec<(f64, SourceInfo)> = traversal
        .order
        .iter()
        .take(limit)
        .filter_map(|id| {
            traversal.known.get(id).map(|entity| {
                let hop = traversal.hop.get(id).copied().unwrap_or(0);
                let score = 1.0 / (1.0 + hop as f64);
                (score, entity_to_source(entity))
            })
        })
        .collect();
    if scored.is_empty() {
        return None;
    }
    let answer = build_graph_answer(&traversal.known, &traversal.order, &traversal.edges);
    Some((answer, scored))
}

/// Project a [`GraphEntity`] into the wire [`SourceInfo`] shape (its `type`
/// becomes the section, defaulting to `"entity"`; its `url` carries the KGP-Q1
/// citation).
fn entity_to_source(entity: &GraphEntity) -> SourceInfo {
    SourceInfo {
        title: entity.name.clone(),
        section: if entity.etype.is_empty() {
            "entity".to_string()
        } else {
            entity.etype.clone()
        },
        url: entity.url.clone(),
    }
}

/// The result of a breadth-first graph traversal: the entities discovered
/// (`known`), the order they were discovered in (`order`, seeds-first then by
/// hop), each entity's hop distance from the seeds (`hop`), and the distinct
/// relationship edges that joined them (`edges`).
struct GraphTraversal {
    known: HashMap<String, GraphEntity>,
    order: Vec<String>,
    hop: HashMap<String, usize>,
    edges: Vec<GraphEdge>,
}

/// Seed keyword-matched entities and traverse their relationship edges
/// breadth-first up to [`MAX_GRAPH_HOPS`] hops, recording each entity's hop
/// distance. Shared by [`query_graph`] (answer + sources) and
/// [`query_graph_scored`] (adds the proximity score) so both share exactly one
/// traversal path. Returns `None` when the pack has no graph tables or no seed
/// entity matches.
fn graph_traverse(conn: &Connection, keywords: &[&str], limit: usize) -> Option<GraphTraversal> {
    if !table_exists(conn, "entities") || !table_exists(conn, "relationships") {
        return None;
    }

    let seeds = seed_entities(conn, keywords, limit);
    if seeds.is_empty() {
        return None;
    }

    // Breadth-first traversal. `order` preserves discovery order (seeds first by
    // relevance, then neighbours by hop) for deterministic, relevance-ordered
    // output; `known` de-duplicates entities; `hop` records each entity's
    // distance from the seeds; `edges` collects the distinct relationships that
    // ground the answer.
    let mut known: HashMap<String, GraphEntity> = HashMap::new();
    let mut hop: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in &seeds {
        if known.insert(e.id.clone(), e.clone()).is_none() {
            hop.insert(e.id.clone(), 0);
            order.push(e.id.clone());
        }
    }

    let mut frontier: Vec<String> = order.clone();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();

    for hop_index in 0..MAX_GRAPH_HOPS {
        if frontier.is_empty() {
            break;
        }
        let hop_edges = fetch_edges(conn, &frontier);
        let mut new_ids: Vec<String> = Vec::new();
        for edge in &hop_edges {
            let key = (
                edge.source.clone(),
                edge.target.clone(),
                edge.relation.clone(),
            );
            if seen_edges.insert(key) {
                edges.push(edge.clone());
            }
            for nid in [&edge.source, &edge.target] {
                if !known.contains_key(nid) && !new_ids.iter().any(|x| x == nid) {
                    new_ids.push(nid.clone());
                }
            }
        }

        let mut next_frontier: Vec<String> = Vec::new();
        for neighbour in fetch_entities_by_id(conn, &new_ids) {
            if !known.contains_key(&neighbour.id) {
                order.push(neighbour.id.clone());
                hop.insert(neighbour.id.clone(), hop_index + 1);
                next_frontier.push(neighbour.id.clone());
                known.insert(neighbour.id.clone(), neighbour);
            }
        }
        frontier = next_frontier;
    }

    Some(GraphTraversal {
        known,
        order,
        hop,
        edges,
    })
}

/// Compose a graph-grounded answer: the leading entities' descriptions followed
/// by the relationships joining the retrieved entities (e.g.
/// "Ownership enables Borrowing"). Descriptions are truncated on a UTF-8 char
/// boundary (KGP-Q6) so multibyte content never panics.
fn build_graph_answer(
    known: &HashMap<String, GraphEntity>,
    order: &[String],
    edges: &[GraphEdge],
) -> String {
    let mut parts: Vec<String> = Vec::new();

    for id in order.iter().take(3) {
        if let Some(entity) = known.get(id)
            && !entity.description.is_empty()
        {
            let mut desc = entity.description.clone();
            if desc.len() > 500 {
                crate::util::string_truncate::truncate_to_char_boundary(&mut desc, 500);
                desc.push_str("...");
            }
            parts.push(desc);
        }
    }

    let mut rel_sentences: Vec<String> = Vec::new();
    for edge in edges.iter().take(10) {
        let source = known
            .get(&edge.source)
            .map(|e| e.name.as_str())
            .unwrap_or(edge.source.as_str());
        let target = known
            .get(&edge.target)
            .map(|e| e.name.as_str())
            .unwrap_or(edge.target.as_str());
        let relation = if edge.relation.is_empty() {
            "is related to"
        } else {
            edge.relation.as_str()
        };
        rel_sentences.push(format!("{source} {relation} {target}"));
    }
    if !rel_sentences.is_empty() {
        parts.push(format!("Related concepts: {}.", rel_sentences.join("; ")));
    }

    if parts.is_empty() {
        // Seeds matched but carry no descriptions and no edges — still name them.
        let names: Vec<&str> = order
            .iter()
            .filter_map(|id| known.get(id))
            .map(|e| e.name.as_str())
            .collect();
        return format!(
            "Found {} related entities: {}",
            names.len(),
            names.join(", ")
        );
    }
    parts.join(" ")
}

// ── Vector semantic search (KGP-Q9) ─────────────────────────────────────────

/// Dimensionality of the default text embedder ([`embed_text`]). Chosen small
/// enough to keep query-time embedding cheap while large enough to keep hash
/// collisions between distinct tokens rare. Packs whose stored embeddings were
/// produced by this same default embedder share this dimension, so the query and
/// stored vectors are directly comparable by cosine.
const DEFAULT_EMBED_DIM: usize = 256;

/// FNV-1a hash of `bytes` — a small, dependency-free, **stable** hash so the same
/// token always maps to the same embedding bucket across a pack's build and every
/// later query (unlike `std`'s `DefaultHasher`, whose output is not guaranteed
/// stable across releases/process runs).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Deterministic **default** text embedder: a normalized feature-hashing
/// bag-of-tokens vector.
///
/// This is the port of the original agent-kgpacks *default* embedder — a
/// deterministic hash embedder, **not** a trained model (see the upstream R1
/// note: "default embeddings are deterministic-hash, not a real model"). It lets
/// the read-only client retrieve by embedding cosine (the same *method* as the
/// original) for packs whose stored embeddings came from the same default
/// embedder; real-model semantic quality is a pack-build-time concern (`KGP-B*`,
/// out of scope) and would supply its own query embedder.
///
/// Each token (whitespace-split, lower-cased, stripped of surrounding
/// non-alphanumerics, length > 2 to match the keyword filter) is hashed to a
/// bucket in `[0, dim)`; a second hash bit picks the sign (signed feature
/// hashing, which cancels rather than compounds the bias from colliding tokens).
/// The vector is then L2-normalized so cosine similarity reduces to a dot
/// product and answers of different token counts stay comparable. An empty /
/// all-short question yields an all-zero vector (norm 0), which [`query_vector`]
/// treats as "no query signal" and declines.
fn embed_text(text: &str, dim: usize) -> Vec<f64> {
    let mut v = vec![0.0f64; dim.max(1)];
    for raw in text.split_whitespace() {
        let lowered = raw.to_ascii_lowercase();
        let token = lowered.trim_matches(|c: char| !c.is_alphanumeric());
        if token.len() <= 2 {
            continue;
        }
        let h = fnv1a(token.as_bytes());
        let idx = (h % v.len() as u64) as usize;
        let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
        v[idx] += sign;
    }
    l2_normalize(&mut v);
    v
}

/// L2-normalize `v` in place. A zero vector (norm 0) is left untouched so callers
/// can detect "no signal" via an all-zero result rather than dividing by zero.
fn l2_normalize(v: &mut [f64]) {
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two equal-length vectors. Returns `0.0` when the lengths
/// differ (embeddings from different embedders/dimensions are not comparable) or
/// when either vector has zero magnitude, so a dimension mismatch degrades to
/// "not similar" instead of a wrong or panicking comparison.
fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Parse a stored embedding cell into a float vector.
///
/// Simard's SQLite pack port stores each article/section embedding as a JSON
/// array of numbers in an `embedding` TEXT column (the portable stand-in for the
/// original LadybugDB `DOUBLE[768]` typed vector). A malformed or non-array cell
/// degrades to `None` so one bad row cannot break retrieval for the rest.
fn parse_embedding(raw: &str) -> Option<Vec<f64>> {
    let parsed: Vec<f64> = serde_json::from_str(raw).ok()?;
    if parsed.is_empty() {
        return None;
    }
    Some(parsed)
}

/// Test-only thin wrapper over [`query_vector_scored`] that discards the cosine
/// score. Production code consumes the scored variant through [`query_hybrid`].
#[cfg(test)]
fn query_vector(
    conn: &Connection,
    query_embedding: &[f64],
    limit: usize,
) -> Option<Vec<SourceInfo>> {
    query_vector_scored(conn, query_embedding, limit)
        .map(|scored| scored.into_iter().map(|(_, s)| s).collect())
}

/// Vector **semantic** retrieval (KGP-Q9): rank the pack's stored article/section
/// embeddings by cosine similarity to `query_embedding` and return the top
/// `limit` as `(cosine_score, SourceInfo)`, projecting the `url` column for
/// source citations (KGP-Q1). The score is exposed so the hybrid ranker
/// ([`fuse_signals`], KGP-Q10) can blend the vector-semantic signal with the
/// graph and keyword signals; the [`query_vector`] test wrapper discards it.
///
/// This is the port of the original agent-kgpacks retrieval *method* — an
/// embedding-cosine vector query over `Section.embedding` — which the previous
/// keyword LIKE scan only approximated. Because it ranks by vector proximity
/// rather than literal token overlap, it surfaces an on-topic article that
/// shares **no** keyword with the question (which a LIKE scan misses) whenever
/// that article's embedding is near the query's. Results are ordered by cosine
/// DESC with a deterministic `title ASC` tie-break.
///
/// Returns `None` — so the caller falls back to the other signals, leaving
/// keyword-only packs unchanged — when:
///
/// * `query_embedding` carries no signal (empty / all-zero), or
/// * no scanned table (`articles`, `sections`, `nodes`, `entities`) has an
///   `embedding` column, or
/// * no stored embedding parses **and** shares the query embedding's dimension
///   (an embedder/dimension mismatch is not silently mis-ranked).
fn query_vector_scored(
    conn: &Connection,
    query_embedding: &[f64],
    limit: usize,
) -> Option<Vec<(f64, SourceInfo)>> {
    if query_embedding.iter().all(|x| *x == 0.0) {
        return None;
    }

    for table in &["articles", "sections", "nodes", "entities"] {
        if !table_exists(conn, table) || !table_has_column(conn, table, "embedding") {
            continue;
        }
        // Title column: articles/sections/nodes use `title`; the entity port
        // uses `name`. Skip a table that has neither so the row shape is known.
        let title_col = if table_has_column(conn, table, "title") {
            "title"
        } else if table_has_column(conn, table, "name") {
            "name"
        } else {
            continue;
        };
        let section_expr = if table_has_column(conn, table, "section") {
            "COALESCE(section, '')"
        } else {
            "''"
        };
        let url_col = if table_has_column(conn, table, "url") {
            "url"
        } else {
            "NULL AS url"
        };
        let sql = format!(
            "SELECT {title_col} AS title, {section_expr} AS section, {url_col}, embedding \
             FROM {table} WHERE embedding IS NOT NULL"
        );

        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |row| {
            let url = row
                .get::<_, Option<String>>(2)
                .unwrap_or(None)
                .filter(|u| !u.is_empty());
            let embedding = row.get::<_, Option<String>>(3).unwrap_or(None);
            Ok((
                SourceInfo {
                    title: row.get::<_, String>(0).unwrap_or_default(),
                    section: row.get::<_, String>(1).unwrap_or_default(),
                    url,
                },
                embedding,
            ))
        }) else {
            continue;
        };

        let mut scored: Vec<(f64, SourceInfo)> = Vec::new();
        for (source, embedding) in rows.flatten() {
            let Some(raw) = embedding else { continue };
            let Some(vec) = parse_embedding(&raw) else {
                continue;
            };
            let score = cosine_similarity(query_embedding, &vec);
            // Only positive similarity counts as a match; an orthogonal or
            // dimension-mismatched vector scores 0 and is dropped so retrieval
            // never returns an unrelated article as if it were relevant.
            if score > 0.0 {
                scored.push((score, source));
            }
        }

        if scored.is_empty() {
            // This table had an `embedding` column but nothing comparable
            // matched; try the next candidate table before giving up.
            continue;
        }

        // Highest cosine first; a deterministic `title ASC` tie-break keeps the
        // order reproducible for equal-similarity rows (mirrors query_articles).
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.cmp(&b.1.title))
        });
        scored.truncate(limit);
        return Some(scored);
    }

    None
}

// ── Hybrid ranking (KGP-Q10) ─────────────────────────────────────────────────

/// Relative weight of the **vector-semantic** signal ([`query_vector_scored`])
/// when blending retrieval signals.
///
/// Per the operator directive (issue #4321, 2026-07-20) — *"No keyword search is
/// not good enough. It needs to be the same"* GraphRAG method — retrieval parity
/// is **not** satisfied by literal keyword overlap alone. So the two GraphRAG
/// signals the original performs — vector-semantic and graph — are each weighted
/// to **outweigh** the keyword signal: a top-ranked semantic (or graph) match
/// (normalized 1.0 × [`HYBRID_VECTOR_WEIGHT`]) scores above a top-ranked
/// keyword-only match (normalized 1.0 × [`HYBRID_KEYWORD_WEIGHT`]).
const HYBRID_VECTOR_WEIGHT: f64 = 1.0;

/// Relative weight of the **graph** signal ([`query_graph_scored`]). Equal to
/// the vector weight — both are the original's GraphRAG retrieval methods — and
/// above the keyword weight (see [`HYBRID_VECTOR_WEIGHT`]).
const HYBRID_GRAPH_WEIGHT: f64 = 1.0;

/// Relative weight of the **keyword-coverage** signal
/// ([`query_articles_scored`]). Deliberately below the semantic and graph
/// weights so literal keyword overlap alone cannot outrank a genuine
/// semantic/graph match, while still contributing to (and breaking near-ties in)
/// the fused ranking.
const HYBRID_KEYWORD_WEIGHT: f64 = 0.6;

/// Blend several ranked retrieval signals into one fused ranking (KGP-Q10).
///
/// Each input signal is a `(weight, ranked_candidates)` pair, where each
/// candidate carries the signal's own raw score (cosine for vector, coverage sum
/// for keyword, hop proximity for graph). Because the raw scores of different
/// signals are not directly comparable (a cosine in `(0, 1]` vs an integer
/// coverage sum vs a `1/(1+hop)` proximity), each signal is first **normalized
/// by its own maximum** so its top candidate contributes `1.0` before the
/// per-signal `weight` is applied. A candidate's fused score is the weighted sum
/// of its normalized per-signal scores; candidates surfaced by more than one
/// signal (matched by title, case-insensitively) accumulate across them.
///
/// The result is sorted by fused score DESC with a deterministic `title ASC`
/// tie-break (mirroring the component rankers) and truncated to `limit`. When a
/// candidate appears in multiple signals its citation `url` / `section` are
/// upgraded from whichever signal first supplies a non-empty value, so a graph
/// or keyword hit does not drop a URL a sibling signal carried.
fn fuse_signals(signals: &[(f64, Vec<(f64, SourceInfo)>)], limit: usize) -> Vec<SourceInfo> {
    struct Fused {
        source: SourceInfo,
        score: f64,
    }
    let mut fused: HashMap<String, Fused> = HashMap::new();

    for (weight, candidates) in signals {
        // Normalize this signal by its own max score so heterogeneous score
        // scales (cosine vs coverage vs hop proximity) are comparable before the
        // weight is applied. A non-empty candidate list always has a positive
        // max (each retrieval path drops non-positive scores), but guard anyway.
        let max_score = candidates.iter().map(|(s, _)| *s).fold(0.0_f64, f64::max);
        if max_score <= 0.0 {
            continue;
        }
        for (score, source) in candidates {
            let contribution = weight * (score / max_score);
            let key = source.title.to_ascii_lowercase();
            fused
                .entry(key)
                .and_modify(|f| {
                    f.score += contribution;
                    if f.source.url.is_none() && source.url.is_some() {
                        f.source.url = source.url.clone();
                    }
                    if f.source.section.is_empty() && !source.section.is_empty() {
                        f.source.section = source.section.clone();
                    }
                })
                .or_insert_with(|| Fused {
                    source: source.clone(),
                    score: contribution,
                });
        }
    }

    let mut ranked: Vec<Fused> = fused.into_values().collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.source.title.cmp(&b.source.title))
    });
    ranked.truncate(limit);
    ranked.into_iter().map(|f| f.source).collect()
}

/// Hybrid GraphRAG ranker (KGP-Q10): gather every retrieval signal the pack can
/// supply and blend them into one fused answer + source ranking.
///
/// The three signals are the same the original agent-kgpacks ranker fuses:
/// graph multi-hop ([`query_graph_scored`], KGP-Q5), vector semantic cosine
/// ([`query_vector_scored`], KGP-Q9), and keyword coverage
/// ([`query_articles_scored`], KGP-Q4/Q8). Behaviour by number of *available*
/// signals:
///
/// * **0** — no signal matched: a graceful "no relevant information" answer with
///   no sources (unchanged from the prior keyword fallback).
/// * **1** — exactly one signal: its native output and answer synthesis are
///   returned **unchanged** (there is nothing to blend), so keyword-only,
///   vector-only, and graph-only packs behave exactly as before.
/// * **≥2** — the signals are fused by [`fuse_signals`] so the semantic/graph
///   signal can outrank literal keyword overlap. The answer prefers the
///   graph-grounded narration (which names the linked entities and relations)
///   when the graph signal contributed, otherwise it is synthesized from the
///   fused sources.
fn query_hybrid(
    conn: &Connection,
    keywords: &[&str],
    query_embedding: &[f64],
    limit: usize,
) -> (String, Vec<SourceInfo>) {
    let graph = query_graph_scored(conn, keywords, limit);
    let vector = query_vector_scored(conn, query_embedding, limit);
    let keyword = query_articles_scored(conn, keywords, limit);

    // Assemble the signals that actually produced candidates. `query_graph_scored`
    // and `query_vector_scored` only return `Some` when non-empty, so their mere
    // presence means they contributed.
    let mut signals: Vec<(f64, Vec<(f64, SourceInfo)>)> = Vec::new();
    if let Some((_, ref graph_scored)) = graph {
        signals.push((HYBRID_GRAPH_WEIGHT, graph_scored.clone()));
    }
    if let Some(ref vector_scored) = vector {
        signals.push((HYBRID_VECTOR_WEIGHT, vector_scored.clone()));
    }
    if !keyword.is_empty() {
        signals.push((HYBRID_KEYWORD_WEIGHT, keyword.clone()));
    }

    match signals.len() {
        0 => {
            // No retrieval signal matched — graceful empty answer (the prior
            // keyword-fallback behaviour for an unmatched question).
            let answer = build_answer(conn, keywords, &[]);
            (answer, Vec::new())
        }
        1 => {
            // Exactly one signal: reproduce that signal's native output and
            // answer synthesis unchanged — nothing to blend.
            if let Some((graph_answer, graph_scored)) = graph {
                let sources = graph_scored.into_iter().map(|(_, s)| s).collect();
                return (graph_answer, sources);
            }
            if let Some(vector_scored) = vector {
                let sources: Vec<SourceInfo> = vector_scored.into_iter().map(|(_, s)| s).collect();
                let answer = build_answer(conn, keywords, &sources);
                return (answer, sources);
            }
            let sources: Vec<SourceInfo> = keyword.into_iter().map(|(_, s)| s).collect();
            let answer = build_answer(conn, keywords, &sources);
            (answer, sources)
        }
        _ => {
            // Two or more signals: blend them into one hybrid ranking (KGP-Q10).
            let sources = fuse_signals(&signals, limit);
            let answer = match graph {
                // The graph narration names linked entities + relations, the
                // richest available synthesis; keep it when graph contributed.
                Some((graph_answer, _)) => graph_answer,
                None => build_answer(conn, keywords, &sources),
            };
            (answer, sources)
        }
    }
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

    /// Build a pack that exposes a knowledge **graph** (`entities` +
    /// `relationships` tables) modelling the upstream agent-kgpacks
    /// `Entity`/`ENTITY_RELATION` schema. Three Rust concepts are linked:
    ///
    /// ```text
    /// Ownership --enables--> Borrowing --constrained by--> Lifetimes
    /// ```
    ///
    /// Only "Ownership" mentions the word "ownership", so a query for it must
    /// traverse relationships to surface the linked "Borrowing" (hop 1) and
    /// "Lifetimes" (hop 2) — proving multi-hop GraphRAG retrieval (KGP-Q5).
    /// The `entities` table carries a `url` column so graph answers keep the
    /// source-citation guarantee (KGP-Q1); "Ownership" cites a URL, "Borrowing"
    /// has an empty (→ no citation) url, "Lifetimes" a NULL url.
    fn create_test_graph_pack(packs_dir: &Path, name: &str) -> PathBuf {
        let pack_dir = packs_dir.join(name);
        fs::create_dir_all(&pack_dir).unwrap();

        let manifest = serde_json::json!({
            "name": name,
            "description": format!("{name} knowledge pack"),
            "graph_stats": { "articles": 0, "entities": 3, "relationships": 2, "size_mb": 0.1 }
        });
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entities (entity_id TEXT, name TEXT, type TEXT, description TEXT, url TEXT);
             INSERT INTO entities VALUES ('e_own', 'Ownership', 'concept', 'Ownership is a set of rules that govern how a Rust program manages memory.', 'https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html');
             INSERT INTO entities VALUES ('e_borrow', 'Borrowing', 'concept', 'Borrowing lets you refer to a value without taking ownership of it.', '');
             INSERT INTO entities VALUES ('e_life', 'Lifetimes', 'concept', 'Lifetimes tell the compiler how long references remain valid.', NULL);
             CREATE TABLE relationships (source_id TEXT, target_id TEXT, relation TEXT, context TEXT);
             INSERT INTO relationships VALUES ('e_own', 'e_borrow', 'enables', 'ownership rules enable safe borrowing');
             INSERT INTO relationships VALUES ('e_borrow', 'e_life', 'constrained by', 'borrows are constrained by lifetimes');",
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

    // ── GraphRAG multi-hop retrieval (KGP-Q5) ───────────────────────────────

    #[test]
    fn query_graph_traverses_relationships_for_linked_entities() {
        // KGP-Q5 primary: a query that only matches "Ownership" by keyword must
        // traverse the relationship graph to surface the linked "Borrowing"
        // entity (hop 1) that the keyword never matched — proving the answer is
        // grounded in a graph join, not a single-table LIKE scan.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_graph_pack(tmp.path(), "graph-pack");
        let db_path = pack_dir.join("pack.db");

        let (answer, sources, confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();

        assert!(confidence > 0.0);
        assert!(
            sources.iter().any(|s| s.title == "Ownership"),
            "the keyword-matched seed entity must be cited; got: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.title == "Borrowing"),
            "a linked entity NOT matched by the keyword must be reached via the \
             relationship graph; got: {sources:?}"
        );
        assert!(
            answer.contains("Borrowing") && answer.contains("enables"),
            "the answer must name the linked entity and the relationship joining \
             it; got: {answer:?}"
        );
        // KGP-Q1: the seed entity's citation URL propagates through the graph path.
        let ownership = sources
            .iter()
            .find(|s| s.title == "Ownership")
            .expect("Ownership source");
        assert_eq!(
            ownership.url.as_deref(),
            Some("https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html"),
            "graph answers must keep the source-citation guarantee"
        );
    }

    #[test]
    fn query_graph_reaches_two_hop_neighbor() {
        // Multi-hop: "Lifetimes" is two relationship hops from the "Ownership"
        // seed (Ownership -> Borrowing -> Lifetimes) and shares no keyword with
        // the query, so reaching it requires MAX_GRAPH_HOPS traversal.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_graph_pack(tmp.path(), "graph-pack");
        let db_path = pack_dir.join("pack.db");

        let (answer, sources, _confidence) =
            query_pack_db(&db_path, "Tell me about ownership", 5).unwrap();

        assert!(
            sources.iter().any(|s| s.title == "Lifetimes"),
            "the two-hop neighbour must be retrieved; got: {sources:?}"
        );
        assert!(
            answer.contains("Lifetimes") && answer.contains("constrained by"),
            "the answer must include the second-hop relationship; got: {answer:?}"
        );
    }

    #[test]
    fn query_graph_empty_url_and_null_url_yield_no_citation() {
        // A present-but-empty url ("Borrowing") and a NULL url ("Lifetimes")
        // must both degrade to no citation (None), never Some("").
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_graph_pack(tmp.path(), "graph-pack");
        let db_path = pack_dir.join("pack.db");

        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();

        for title in ["Borrowing", "Lifetimes"] {
            let s = sources
                .iter()
                .find(|s| s.title == title)
                .unwrap_or_else(|| panic!("{title} must be reached via the graph"));
            assert_eq!(s.url, None, "{title} must have no citation url");
        }
    }

    #[test]
    fn query_graph_returns_none_without_graph_tables() {
        // A pack with only an `articles` table (no entities/relationships) must
        // fall back to the single-table scan — query_graph returns None.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "article-pack");
        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();

        assert!(
            query_graph(&conn, &["ownership"], 5).is_none(),
            "a pack without graph tables must not use the graph path"
        );

        // And the overall query still succeeds via the article fallback.
        let (_answer, sources, _confidence) =
            query_pack_db(&db_path, "What is ownership in Rust?", 5).unwrap();
        assert!(
            sources.iter().any(|s| s.title.contains("Ownership")),
            "the article fallback must still answer; got: {sources:?}"
        );
    }

    #[test]
    fn query_graph_ignores_pack_with_entities_but_no_relationships() {
        // Half a graph (entities but no relationships table) is not a traversable
        // graph — query_graph must return None so the caller falls back.
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("half-graph");
        fs::create_dir_all(&pack_dir).unwrap();
        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entities (entity_id TEXT, name TEXT, type TEXT, description TEXT);
             INSERT INTO entities VALUES ('e1', 'Ownership', 'concept', 'Ownership rules.');",
        )
        .unwrap();

        assert!(
            query_graph(&conn, &["ownership"], 5).is_none(),
            "a pack missing the relationships table must not use the graph path"
        );
    }

    #[test]
    fn query_graph_returns_none_when_no_seed_entity_matches() {
        // Graph tables exist but no entity matches the keywords — no seed, so no
        // traversal; the caller falls back to the article scan.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_graph_pack(tmp.path(), "graph-pack");
        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();

        assert!(
            query_graph(&conn, &["kubernetes"], 5).is_none(),
            "no matching seed entity must yield None (fall back), not an empty graph answer"
        );
    }

    #[test]
    fn native_knowledge_transport_query_graph_surfaces_linked_entity() {
        // End-to-end KGP-Q5: the linked (non-keyword) entity reached via graph
        // traversal appears in the wire response's sources.
        let tmp = TempDir::new().unwrap();
        create_test_graph_pack(tmp.path(), "graph-pack");

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "graph-pack",
                "question": "What is ownership in Rust?",
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        let result = response.result.expect("query must succeed");
        let sources = result["sources"].as_array().expect("sources array");
        assert!(
            sources
                .iter()
                .any(|s| s["title"].as_str() == Some("Borrowing")),
            "the wire response must include the graph-linked entity; got: {sources:?}"
        );
    }

    // ── KGP-Q9: vector semantic search ──────────────────────────────────────

    /// Build a pack whose `articles` table carries an `embedding` column holding
    /// explicit JSON vectors — the SQLite-port stand-in for the original
    /// agent-kgpacks `Section.embedding DOUBLE[768]`. The vectors are chosen so
    /// the two concurrency articles cluster near `[1, 0, 0, 0]` and the cooking
    /// article sits on an orthogonal axis. Crucially, the concurrency articles'
    /// title/content share **no literal token** with a semantic query such as
    /// "asynchronous coroutines scheduling", so only an embedding-cosine query —
    /// not a keyword LIKE scan — can retrieve them.
    fn create_test_vector_pack(packs_dir: &Path, name: &str) -> PathBuf {
        let pack_dir = packs_dir.join(name);
        fs::create_dir_all(&pack_dir).unwrap();

        let manifest = serde_json::json!({
            "name": name,
            "description": format!("{name} knowledge pack"),
            "graph_stats": { "articles": 3, "entities": 0, "relationships": 0, "size_mb": 0.1 }
        });
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT, embedding TEXT);
             INSERT INTO articles VALUES ('Concurrency Primitives', 'Core', 'threads mutexes locks', 'https://example.com/concurrency', '[1.0, 0.0, 0.0, 0.0]');
             INSERT INTO articles VALUES ('Parallel Execution', 'Core', 'spawning workers simultaneously', '', '[0.6, 0.8, 0.0, 0.0]');
             INSERT INTO articles VALUES ('Baking Sourdough', 'Food', 'flour water starter', 'https://example.com/bread', '[0.0, 0.0, 1.0, 0.0]');",
        )
        .unwrap();

        pack_dir
    }

    #[test]
    fn embed_text_is_deterministic_and_l2_normalized() {
        // The default embedder must be deterministic (a token maps to the same
        // bucket every run, so a pack's build-time embedding and a later query
        // embedding live in the same space) and L2-normalized (so cosine reduces
        // to a dot product and answers of different length stay comparable).
        let a = embed_text("Rust ownership and borrowing", DEFAULT_EMBED_DIM);
        let b = embed_text("Rust ownership and borrowing", DEFAULT_EMBED_DIM);
        assert_eq!(a, b, "embedding must be deterministic for identical input");

        let norm = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-9,
            "a non-empty embedding must be L2-normalized (norm={norm})"
        );

        // An empty / all-short question carries no signal → the zero vector.
        let empty = embed_text("a an to", DEFAULT_EMBED_DIM);
        assert!(
            empty.iter().all(|x| *x == 0.0),
            "a question with no >2-char tokens must embed to the zero vector"
        );
    }

    #[test]
    fn cosine_similarity_handles_identity_orthogonal_and_mismatch() {
        assert!((cosine_similarity(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-9);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        // Dimension mismatch is not comparable → 0.0, never a panic or a spurious
        // match against a differently-sized (e.g. real-model 768-dim) vector.
        assert_eq!(cosine_similarity(&[1.0, 0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn parse_embedding_reads_json_array_and_rejects_garbage() {
        assert_eq!(
            parse_embedding("[0.5, -0.25, 1.0]"),
            Some(vec![0.5, -0.25, 1.0])
        );
        assert_eq!(parse_embedding("not json"), None);
        assert_eq!(parse_embedding("[]"), None);
        assert_eq!(parse_embedding("{\"x\":1}"), None);
    }

    #[test]
    fn query_vector_retrieves_semantically_near_article_without_keyword_overlap() {
        // Parity criterion KGP-Q9 (the decisive one): an embedding-cosine query
        // retrieves the on-topic article even though it shares NO literal keyword
        // with the question — exactly what the original agent-kgpacks vector
        // retrieval does and what the keyword LIKE scan cannot.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_vector_pack(tmp.path(), "vec-pack");
        let conn = Connection::open_with_flags(
            pack_dir.join("pack.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();

        // A semantic query vector near the concurrency cluster ([1,0,0,0]).
        let query_embedding = vec![1.0, 0.1, 0.0, 0.0];
        let vector_sources =
            query_vector(&conn, &query_embedding, 5).expect("a pack with embeddings must retrieve");
        assert_eq!(
            vector_sources[0].title,
            "Concurrency Primitives",
            "cosine must rank the nearest-embedding article first; got: {:?}",
            vector_sources.iter().map(|s| &s.title).collect::<Vec<_>>()
        );
        assert!(
            vector_sources
                .iter()
                .position(|s| s.title == "Concurrency Primitives")
                .unwrap()
                < vector_sources
                    .iter()
                    .position(|s| s.title == "Parallel Execution")
                    .unwrap(),
            "the nearer concurrency article must outrank the farther one"
        );
        assert!(
            !vector_sources.iter().any(|s| s.title == "Baking Sourdough"),
            "the orthogonal (cooking) article must not be retrieved for a concurrency query"
        );

        // The SAME question's literal keywords appear in NONE of the concurrency
        // articles, so a keyword LIKE scan retrieves nothing — proving the vector
        // path surfaces knowledge the keyword scan misses.
        let keyword_sources =
            query_articles(&conn, &["asynchronous", "coroutines", "scheduling"], 5);
        assert!(
            !keyword_sources
                .iter()
                .any(|s| s.title == "Concurrency Primitives" || s.title == "Parallel Execution"),
            "a keyword scan must NOT retrieve the concurrency articles for these words; got: {:?}",
            keyword_sources.iter().map(|s| &s.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn query_vector_ranks_by_cosine_descending() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_vector_pack(tmp.path(), "vec-pack");
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();

        // Query on the concurrency axis: Concurrency [1,0,0,0] > Parallel
        // [0.6,0.8,0,0] > Baking [0,0,1,0] (orthogonal, dropped).
        let sources = query_vector(&conn, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        let titles: Vec<&str> = sources.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Concurrency Primitives", "Parallel Execution"],
            "cosine ranking must be descending and drop the orthogonal article"
        );
    }

    #[test]
    fn query_vector_respects_limit() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_vector_pack(tmp.path(), "vec-pack");
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();

        let sources = query_vector(&conn, &[1.0, 0.5, 0.0, 0.0], 1).unwrap();
        assert_eq!(sources.len(), 1, "the limit must cap the returned sources");
        assert_eq!(sources[0].title, "Concurrency Primitives");
    }

    #[test]
    fn query_vector_projects_url_citation_and_empty_url_is_no_citation() {
        // KGP-Q1 within the vector path: the nearest article's `url` is surfaced;
        // an empty url degrades to no citation (None), never Some("").
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_vector_pack(tmp.path(), "vec-pack");
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();

        let cited = query_vector(&conn, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(
            cited[0].url.as_deref(),
            Some("https://example.com/concurrency"),
            "the nearest article must surface its citation URL"
        );

        // "Parallel Execution" has an empty url → no citation.
        let parallel = query_vector(&conn, &[0.6, 0.8, 0.0, 0.0], 5).unwrap();
        let parallel = parallel
            .iter()
            .find(|s| s.title == "Parallel Execution")
            .unwrap();
        assert_eq!(
            parallel.url, None,
            "an empty url column must degrade to no citation"
        );
    }

    #[test]
    fn query_vector_returns_none_without_embedding_column() {
        // A keyword-only pack (no `embedding` column) yields None so the query
        // path falls back to the LIKE scan — keyword packs are unchanged.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "plain-pack");
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();

        assert!(
            query_vector(&conn, &[1.0, 0.0, 0.0, 0.0], 5).is_none(),
            "a pack without an embedding column must fall back (None)"
        );
    }

    #[test]
    fn query_vector_returns_none_for_zero_query_and_dimension_mismatch() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_vector_pack(tmp.path(), "vec-pack");
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();

        // No query signal → decline (fall back).
        assert!(
            query_vector(&conn, &[0.0, 0.0, 0.0, 0.0], 5).is_none(),
            "an all-zero query embedding must yield None"
        );
        // Query embedding of a different dimension than the stored 4-vectors:
        // nothing is comparable, so retrieval declines rather than mis-ranking.
        assert!(
            query_vector(&conn, &[1.0, 0.0], 5).is_none(),
            "a dimension-mismatched query embedding must yield None"
        );
    }

    #[test]
    fn native_knowledge_transport_query_uses_vector_search_when_embeddings_present() {
        // End-to-end KGP-Q9: a pack whose articles carry embeddings produced by
        // the default embedder is retrieved by embedding cosine through the RPC
        // transport. The pack has no graph tables; the question's keywords also
        // match the target article, so under the hybrid ranker (KGP-Q10) the
        // vector and keyword signals both fire and are blended — the
        // vector-nearest article ("Garbage Collection") still ranks first.
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("embed-pack");
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "embed-pack",
                "description": "embedded knowledge pack",
                "graph_stats": { "articles": 2, "entities": 0, "relationships": 0, "size_mb": 0.1 }
            }))
            .unwrap(),
        )
        .unwrap();

        // Store each article's embedding = embed_text(content) so a question that
        // is semantically about the same content lands nearest to it.
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT, embedding TEXT);",
        )
        .unwrap();
        let rows = [
            (
                "Garbage Collection",
                "Automatic memory reclamation traces reachable objects at runtime",
                "https://example.com/gc",
            ),
            (
                "Manual Allocation",
                "The programmer frees pointers explicitly with malloc",
                "https://example.com/malloc",
            ),
        ];
        for (title, content, url) in rows {
            let emb = serde_json::to_string(&embed_text(content, DEFAULT_EMBED_DIM)).unwrap();
            conn.execute(
                "INSERT INTO articles (title, section, content, url, embedding) VALUES (?1, 'Core', ?2, ?3, ?4)",
                rusqlite::params![title, content, url, emb],
            )
            .unwrap();
        }
        drop(conn);

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "embed-pack",
                "question": "How does automatic memory reclamation work at runtime?",
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        let result = response.result.expect("query must succeed");
        let sources = result["sources"].as_array().expect("sources array");
        assert_eq!(
            sources.first().and_then(|s| s["title"].as_str()),
            Some("Garbage Collection"),
            "vector search must rank the semantically-closest article first; got: {sources:?}"
        );
        assert!(result["confidence"].as_f64().unwrap() > 0.0);
    }

    // ── KGP-Q10: hybrid ranking ─────────────────────────────────────────────

    /// Build a pack that exposes **all three** retrieval signals so the hybrid
    /// ranker has something to blend:
    ///
    /// * `articles` with an `embedding` column — the vector-semantic signal
    ///   (KGP-Q9) and the keyword-coverage signal (KGP-Q8) both draw from here;
    /// * `entities` + `relationships` — the GraphRAG signal (KGP-Q5).
    ///
    /// The rows are chosen so the signals *disagree*, which is the only way to
    /// prove a genuine blend:
    ///
    /// * `Vector Near` — embedding `[1,0,0,0]` (nearest a query on that axis) but
    ///   its title/content ("photons lattices") share **no** literal keyword
    ///   with the query, so ONLY the vector signal surfaces it.
    /// * `Keyword Rich` — embedding `[0,0,1,0]` (orthogonal to the query, dropped
    ///   by the vector path) but its content is exactly the query words, so ONLY
    ///   the keyword signal surfaces it, with maximum coverage.
    /// * entity `Alpha Concept` — keyword-matched graph **seed** (hop 0), linked
    ///   to `Linked Concept` (hop 1) which the keyword never matches — so the
    ///   graph signal contributes both a seed and a graph-only neighbour.
    ///
    /// A query on the `[1,0,0,0]` axis for the words "alpha beta gamma" therefore
    /// makes the semantic (`Vector Near`) and graph (`Alpha Concept`) signals
    /// outrank the keyword-only `Keyword Rich`, and surfaces the graph-only
    /// `Linked Concept` the keyword scan alone would miss.
    fn create_test_hybrid_pack(packs_dir: &Path, name: &str) -> PathBuf {
        let pack_dir = packs_dir.join(name);
        fs::create_dir_all(&pack_dir).unwrap();

        let manifest = serde_json::json!({
            "name": name,
            "description": format!("{name} knowledge pack"),
            "graph_stats": { "articles": 2, "entities": 2, "relationships": 1, "size_mb": 0.1 }
        });
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let db_path = pack_dir.join("pack.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT, embedding TEXT);
             INSERT INTO articles VALUES ('Vector Near', 'Core', 'photons lattices', 'https://example.com/vn', '[1.0, 0.0, 0.0, 0.0]');
             INSERT INTO articles VALUES ('Keyword Rich', 'Core', 'alpha beta gamma', 'https://example.com/kr', '[0.0, 0.0, 1.0, 0.0]');
             CREATE TABLE entities (entity_id TEXT, name TEXT, type TEXT, description TEXT, url TEXT);
             INSERT INTO entities VALUES ('e_alpha', 'Alpha Concept', 'concept', 'about alpha', 'https://example.com/alpha');
             INSERT INTO entities VALUES ('e_link', 'Linked Concept', 'concept', 'graph-only neighbour', '');
             CREATE TABLE relationships (source_id TEXT, target_id TEXT, relation TEXT, context TEXT);
             INSERT INTO relationships VALUES ('e_alpha', 'e_link', 'relates to', 'alpha relates to the linked concept');",
        )
        .unwrap();

        pack_dir
    }

    #[test]
    fn fuse_signals_normalizes_and_weights_signals() {
        // Pure unit test of the fusion math (no DB): two signals whose raw score
        // scales differ (a keyword coverage of 3 vs a cosine of 0.9) are each
        // normalized by their own max before weighting, and a source appearing in
        // both signals accumulates. A source's citation url is carried over from
        // whichever signal supplies one.
        let src = |title: &str, url: Option<&str>| SourceInfo {
            title: title.to_string(),
            section: "s".to_string(),
            url: url.map(|u| u.to_string()),
        };
        // Signal A (weight 1.0): "shared" top (score 0.9), "vec_only" (0.45).
        let signal_a = (
            1.0,
            vec![
                (0.9, src("shared", None)),
                (0.45, src("vec_only", Some("https://example.com/v"))),
            ],
        );
        // Signal B (weight 0.6): "kw_only" top (coverage 3), "shared" (coverage 3).
        let signal_b = (
            0.6,
            vec![
                (3.0, src("kw_only", None)),
                (3.0, src("shared", Some("https://example.com/s"))),
            ],
        );

        let fused = fuse_signals(&[signal_a, signal_b], 10);
        let titles: Vec<&str> = fused.iter().map(|s| s.title.as_str()).collect();
        // shared: 1.0*(0.9/0.9) + 0.6*(3/3) = 1.6  (highest — in both signals)
        // vec_only: 1.0*(0.45/0.9)          = 0.5
        // kw_only: 0.6*(3/3)                = 0.6
        // → shared > kw_only > vec_only
        assert_eq!(
            titles,
            vec!["shared", "kw_only", "vec_only"],
            "fused order must reflect normalized, weighted, accumulated scores"
        );
        // The shared source had no url in signal A but a url in signal B; the
        // fused result must carry the citation over.
        let shared = fused.iter().find(|s| s.title == "shared").unwrap();
        assert_eq!(shared.url.as_deref(), Some("https://example.com/s"));
    }

    #[test]
    fn query_hybrid_blends_semantic_graph_and_keyword() {
        // Decisive KGP-Q10 test: on one fixture exposing vector + graph + keyword
        // signals that DISAGREE, the fused ranking places the semantic and graph
        // matches above the keyword-only match, and surfaces the graph-only
        // neighbour the keyword scan alone would miss — "not any single signal
        // alone".
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_hybrid_pack(tmp.path(), "hybrid-pack");
        let conn = Connection::open_with_flags(
            pack_dir.join("pack.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();

        let keywords = ["alpha", "beta", "gamma"];
        // Query embedding on the `[1,0,0,0]` axis → nearest "Vector Near".
        let query_embedding = [1.0, 0.0, 0.0, 0.0];
        let (_answer, sources) = query_hybrid(&conn, &keywords, &query_embedding, 10);
        let titles: Vec<&str> = sources.iter().map(|s| s.title.as_str()).collect();
        let pos = |t: &str| titles.iter().position(|x| *x == t);

        // All three signals contributed.
        assert!(
            pos("Vector Near").is_some(),
            "the vector-semantic signal must surface 'Vector Near'; got {titles:?}"
        );
        assert!(
            pos("Alpha Concept").is_some(),
            "the graph seed must be surfaced; got {titles:?}"
        );
        assert!(
            pos("Keyword Rich").is_some(),
            "the keyword signal must surface 'Keyword Rich'; got {titles:?}"
        );
        assert!(
            pos("Linked Concept").is_some(),
            "the graph-only neighbour (no keyword match) must be surfaced by the blend; got {titles:?}"
        );

        // Semantic and graph signals outweigh literal keyword overlap: the
        // keyword-only 'Keyword Rich' ranks BELOW both the semantic 'Vector Near'
        // and the graph seed 'Alpha Concept'.
        assert!(
            pos("Vector Near") < pos("Keyword Rich"),
            "the semantic match must outrank the keyword-only match; got {titles:?}"
        );
        assert!(
            pos("Alpha Concept") < pos("Keyword Rich"),
            "the graph seed must outrank the keyword-only match; got {titles:?}"
        );

        // Contrast: the keyword signal ALONE ranks 'Keyword Rich' first and never
        // surfaces the semantic-only / graph-only sources — proving the hybrid
        // ranker is not just the keyword scan.
        let keyword_only = query_articles(&conn, &keywords, 10);
        assert_eq!(
            keyword_only.first().map(|s| s.title.as_str()),
            Some("Keyword Rich"),
            "the keyword scan alone must rank 'Keyword Rich' first"
        );
        assert!(
            !keyword_only.iter().any(|s| s.title == "Vector Near"),
            "the keyword scan alone must NOT surface the semantic-only article"
        );
    }

    #[test]
    fn query_hybrid_semantic_outranks_keyword_only_with_two_signals() {
        // Focused two-signal case (vector vs keyword, no graph): a semantic-only
        // article outranks a keyword-only article because the vector weight
        // exceeds the keyword weight — the core KGP-Q10 guarantee.
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("vk-pack");
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "vk-pack",
                "description": "vector+keyword pack",
                "graph_stats": { "articles": 2, "entities": 0, "relationships": 0, "size_mb": 0.1 }
            }))
            .unwrap(),
        )
        .unwrap();
        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT, embedding TEXT);
             INSERT INTO articles VALUES ('Semantic Only', 'Core', 'photons lattices', '', '[1.0, 0.0, 0.0, 0.0]');
             INSERT INTO articles VALUES ('Keyword Only', 'Core', 'alpha beta gamma', '', '[0.0, 0.0, 1.0, 0.0]');",
        )
        .unwrap();

        let (_answer, sources) = query_hybrid(
            &conn,
            &["alpha", "beta", "gamma"],
            &[1.0, 0.0, 0.0, 0.0],
            10,
        );
        assert_eq!(
            sources.first().map(|s| s.title.as_str()),
            Some("Semantic Only"),
            "the semantic match must outrank the keyword-only match under hybrid ranking"
        );
        assert!(
            sources.iter().any(|s| s.title == "Keyword Only"),
            "the keyword match must still appear, just ranked below the semantic one"
        );
    }

    #[test]
    fn query_hybrid_single_signal_matches_component_ranking() {
        // Backward-compatibility guard: when a pack exposes only ONE signal there
        // is nothing to blend, so the hybrid ranker must reproduce that
        // component's native ordering exactly. A keyword-only pack must match
        // `query_articles`.
        let tmp = TempDir::new().unwrap();
        let pack_dir = create_test_pack(tmp.path(), "plain-pack");
        let conn = Connection::open_with_flags(
            pack_dir.join("pack.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();

        let keywords = ["ownership", "borrowing", "references"];
        let query_embedding = embed_text("ownership borrowing references", DEFAULT_EMBED_DIM);
        let (_answer, hybrid_sources) = query_hybrid(&conn, &keywords, &query_embedding, 5);
        let keyword_sources = query_articles(&conn, &keywords, 5);

        let hybrid_titles: Vec<&str> = hybrid_sources.iter().map(|s| s.title.as_str()).collect();
        let keyword_titles: Vec<&str> = keyword_sources.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            hybrid_titles, keyword_titles,
            "a single-signal (keyword-only) pack must rank identically to query_articles"
        );
    }

    #[test]
    fn native_knowledge_transport_query_hybrid_ranks_semantic_over_keyword() {
        // End-to-end KGP-Q10 through the RPC transport: a pack where the
        // vector-nearest article shares NO keyword with the question, and a
        // different article has full keyword overlap but a far embedding. The
        // hybrid ranker must place the semantic match first in the wire response,
        // where a pure keyword scan would rank the keyword article first.
        //
        // Embeddings are decoupled from the article text (as real precomputed
        // embeddings are): the semantic article stores `embed_text(question)`
        // (256-dim, so it matches the query the handler computes) while its
        // surface text shares no query token; the keyword article stores an
        // embedding of unrelated words.
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("hybrid-wire-pack");
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join("manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "hybrid-wire-pack",
                "description": "hybrid ranked pack",
                "graph_stats": { "articles": 2, "entities": 0, "relationships": 0, "size_mb": 0.1 }
            }))
            .unwrap(),
        )
        .unwrap();

        let question = "asynchronous scheduling pipeline";
        let semantic_embedding =
            serde_json::to_string(&embed_text(question, DEFAULT_EMBED_DIM)).unwrap();
        let far_embedding =
            serde_json::to_string(&embed_text("baking bread flour dessert", DEFAULT_EMBED_DIM))
                .unwrap();

        let conn = Connection::open(pack_dir.join("pack.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (title TEXT, section TEXT, content TEXT, url TEXT, embedding TEXT);",
        )
        .unwrap();
        // 'Event Loops': embedding == embed_text(question) so cosine == 1.0, but
        // its text shares no query keyword.
        conn.execute(
            "INSERT INTO articles (title, section, content, url, embedding) VALUES ('Event Loops', 'Core', 'reactor threads mutexes locks', 'https://example.com/el', ?1)",
            rusqlite::params![semantic_embedding],
        )
        .unwrap();
        // 'Async Scheduling Pipeline': full keyword overlap, far embedding.
        conn.execute(
            "INSERT INTO articles (title, section, content, url, embedding) VALUES ('Async Scheduling Pipeline', 'Core', 'asynchronous scheduling pipeline overview', 'https://example.com/asp', ?1)",
            rusqlite::params![far_embedding],
        )
        .unwrap();
        drop(conn);

        let mut transport = NativeRpcTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::rpc::RpcRequest {
            id: crate::rpc::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "hybrid-wire-pack",
                "question": question,
                "limit": 5,
            }),
        };
        let response = crate::rpc::RpcTransport::call(&transport, request).unwrap();
        let result = response.result.expect("query must succeed");
        let sources = result["sources"].as_array().expect("sources array");
        assert_eq!(
            sources.first().and_then(|s| s["title"].as_str()),
            Some("Event Loops"),
            "hybrid ranking must place the semantic match first over the keyword-overlap article; got: {sources:?}"
        );
        // Both articles are present — the keyword one is ranked, just lower.
        assert!(
            sources
                .iter()
                .any(|s| s["title"].as_str() == Some("Async Scheduling Pipeline")),
            "the keyword-overlap article must still be surfaced (ranked below); got: {sources:?}"
        );

        // Confirm a pure keyword scan disagrees (ranks the keyword article first
        // and never surfaces the semantic-only article), so the hybrid ordering
        // is genuinely a blend, not the keyword scan.
        let conn = Connection::open_with_flags(
            pack_dir.join("pack.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let keyword_only = query_articles(&conn, &["asynchronous", "scheduling", "pipeline"], 5);
        assert_eq!(
            keyword_only.first().map(|s| s.title.as_str()),
            Some("Async Scheduling Pipeline"),
            "the keyword scan alone must rank the keyword-overlap article first"
        );
        assert!(
            !keyword_only.iter().any(|s| s.title == "Event Loops"),
            "the keyword scan alone must NOT surface the semantic-only article"
        );
    }
}
