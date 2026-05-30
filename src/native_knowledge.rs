//! Native Rust implementation of the knowledge bridge.
//!
//! Replaces `python/simard_knowledge_bridge.py` with in-process Rust logic.
//! Reads knowledge pack manifests from disk and queries pack databases via
//! rusqlite, eliminating the Python subprocess dependency.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::BridgeErrorPayload;
use crate::bridge_subprocess::native::NativeBridgeTransport;

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

/// Query a pack's SQLite database for entities matching the question.
///
/// This is a simplified version of the Python KnowledgeGraphAgent.query().
/// It searches across article titles and content for relevant matches.
fn query_pack_db(
    db_path: &Path,
    question: &str,
    limit: usize,
) -> Result<(String, Vec<SourceInfo>, f64), String> {
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("cannot open pack database: {e}"))?;

    // Search for articles/sections matching keywords from the question.
    let keywords: Vec<&str> = question
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .collect();

    if keywords.is_empty() {
        return Ok((
            "Please provide a more specific question.".to_string(),
            Vec::new(),
            0.1,
        ));
    }

    // Try to query articles table — pack databases may have varying schemas.
    let sources = query_articles(&conn, &keywords, limit);
    let answer = build_answer(&conn, &keywords, &sources);
    let confidence = estimate_confidence(&answer, &sources);

    Ok((answer, sources, confidence))
}

#[derive(Clone, Debug)]
struct SourceInfo {
    title: String,
    section: String,
    url: Option<String>,
}

/// Query the articles table for matching content.
fn query_articles(conn: &Connection, keywords: &[&str], limit: usize) -> Vec<SourceInfo> {
    // Build a LIKE-based search (pack databases don't always have FTS).
    let like_clauses: Vec<String> = keywords
        .iter()
        .map(|k| {
            format!(
                "(title LIKE '%{kw}%' OR content LIKE '%{kw}%')",
                kw = k.replace('\'', "''")
            )
        })
        .collect();

    if like_clauses.is_empty() {
        return Vec::new();
    }

    // Try "articles" table first, then "nodes"/"entities" as fallback.
    for table in &["articles", "nodes", "entities"] {
        let sql = format!(
            "SELECT title, COALESCE(section, '') as section FROM {table} WHERE {clauses} LIMIT {limit}",
            table = table,
            clauses = like_clauses.join(" OR "),
            limit = limit,
        );

        if let Ok(mut stmt) = conn.prepare(&sql) {
            let mut sources = Vec::new();
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok(SourceInfo {
                    title: row.get::<_, String>(0).unwrap_or_default(),
                    section: row.get::<_, String>(1).unwrap_or_default(),
                    url: None,
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
                    format!("{}...", &content[..500])
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

/// Register all knowledge bridge method handlers on a NativeBridgeTransport.
pub fn register_knowledge_handlers(transport: &mut NativeBridgeTransport, packs_dir: PathBuf) {
    let packs_dir_list = packs_dir.clone();
    let packs_dir_info = packs_dir.clone();
    let packs_dir_query = packs_dir;

    // Shared connection cache to avoid re-opening databases on every query.
    let conn_cache: Arc<Mutex<HashMap<String, PathBuf>>> = Arc::new(Mutex::new(HashMap::new()));

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
                return Err(BridgeErrorPayload {
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
                None => Err(BridgeErrorPayload {
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
                return Err(BridgeErrorPayload {
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

            // Find the pack's database path.
            let db_path = {
                let cache = conn_cache.lock().unwrap();
                cache.get(pack_name).cloned()
            };

            let db_path = match db_path {
                Some(p) => p,
                None => {
                    let packs = discover_packs(&packs_dir_query);
                    let pack = packs.iter().find(|p| p.name == pack_name);
                    match pack {
                        Some(p) => {
                            let path = p.db_path.clone();
                            let mut cache = conn_cache.lock().unwrap();
                            cache.insert(pack_name.to_string(), path.clone());
                            path
                        }
                        None => {
                            return Err(BridgeErrorPayload {
                                code: ERROR_INTERNAL,
                                message: format!("pack '{pack_name}' not found"),
                            });
                        }
                    }
                }
            };

            if !db_path.exists() {
                return Err(BridgeErrorPayload {
                    code: ERROR_INTERNAL,
                    message: format!(
                        "pack '{pack_name}' has no database at {}",
                        db_path.display()
                    ),
                });
            }

            let limit = limit.min(100);
            match query_pack_db(&db_path, question, limit) {
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
                Err(e) => Err(BridgeErrorPayload {
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

        let mut transport = NativeBridgeTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "knowledge.list_packs".to_string(),
            params: serde_json::json!({}),
        };
        let response = crate::bridge::BridgeTransport::call(&transport, request).unwrap();
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

        let mut transport = NativeBridgeTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "test-pack",
                "question": "What is ownership?",
                "limit": 5,
            }),
        };
        let response = crate::bridge::BridgeTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(!result["answer"].as_str().unwrap().is_empty());
        assert!(result["confidence"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn native_knowledge_transport_pack_info() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeBridgeTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "knowledge.pack_info".to_string(),
            params: serde_json::json!({"pack_name": "test-pack"}),
        };
        let response = crate::bridge::BridgeTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["name"], "test-pack");
        assert_eq!(result["article_count"], 10);
    }

    #[test]
    fn native_knowledge_transport_pack_not_found() {
        let tmp = TempDir::new().unwrap();

        let mut transport = NativeBridgeTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "knowledge.pack_info".to_string(),
            params: serde_json::json!({"pack_name": "nonexistent"}),
        };
        let response = crate::bridge::BridgeTransport::call(&transport, request).unwrap();
        assert!(response.error.is_some());
        assert!(response.error.unwrap().message.contains("not found"));
    }

    #[test]
    fn native_knowledge_transport_empty_question() {
        let tmp = TempDir::new().unwrap();
        create_test_pack(tmp.path(), "test-pack");

        let mut transport = NativeBridgeTransport::new("simard-knowledge");
        register_knowledge_handlers(&mut transport, tmp.path().to_path_buf());

        let request = crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "knowledge.query".to_string(),
            params: serde_json::json!({
                "pack_name": "test-pack",
                "question": "",
                "limit": 5,
            }),
        };
        let response = crate::bridge::BridgeTransport::call(&transport, request).unwrap();
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result["confidence"], 0.0);
    }
}
