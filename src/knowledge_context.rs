//! Planning context enrichment via knowledge graph packs.
//!
//! Before the engineer loop begins planning, this module inspects the
//! objective text, determines which knowledge packs are relevant, and
//! queries the top packs to produce a [`PlanningContext`] that can be
//! injected into the planning prompt.

use std::collections::HashSet;

use crate::error::SimardResult;
use crate::knowledge_client::{KnowledgeClient, KnowledgePackInfo, KnowledgeQueryResult};

/// Maximum number of packs to query per objective.
const MAX_PACKS_PER_OBJECTIVE: usize = 3;

/// Default result limit per pack query.
const DEFAULT_QUERY_LIMIT: u32 = 5;

/// Knowledge gathered from packs to enrich the planning phase.
#[derive(Clone, Debug)]
pub struct PlanningContext {
    /// Query results from the most relevant packs.
    pub relevant_knowledge: Vec<KnowledgeQueryResult>,
    /// Names of packs that contributed knowledge.
    pub pack_sources: Vec<String>,
}

impl PlanningContext {
    /// True when no knowledge was gathered (all queries failed or no packs matched).
    pub fn is_empty(&self) -> bool {
        self.relevant_knowledge.is_empty()
    }
}

/// Enrich the planning phase by querying relevant knowledge packs.
///
/// The function:
/// 1. Lists available packs from the knowledge.
/// 2. Scores each pack by keyword overlap with the objective.
/// 3. Queries the top [`MAX_PACKS_PER_OBJECTIVE`] packs.
/// 4. Returns the aggregated results as a [`PlanningContext`].
///
/// If the knowledge is unavailable or no packs match, an empty context is returned
/// rather than hiding errors — knowledge enrichment failures propagate per PHILOSOPHY.md.
pub fn enrich_planning_context(
    objective: &str,
    knowledge: &KnowledgeClient,
) -> SimardResult<PlanningContext> {
    let packs = knowledge.list_packs()?;

    if packs.is_empty() {
        return Ok(PlanningContext {
            relevant_knowledge: vec![],
            pack_sources: vec![],
        });
    }

    let mut scored: Vec<(usize, &KnowledgePackInfo)> = packs
        .iter()
        .map(|pack| (relevance_score(objective, pack), pack))
        .filter(|(score, _)| *score > 0)
        .collect();

    // Sort descending by score.
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));
    scored.truncate(MAX_PACKS_PER_OBJECTIVE);

    let mut relevant_knowledge = Vec::new();
    let mut pack_sources = Vec::new();

    for (_score, pack) in &scored {
        match knowledge.query(&pack.name, objective, DEFAULT_QUERY_LIMIT) {
            Ok(result) if result.confidence > 0.0 => {
                pack_sources.push(pack.name.clone());
                relevant_knowledge.push(result);
            }
            Ok(_) => {
                // Low confidence -- skip this pack.
            }
            Err(_) => {
                // Query failed -- skip gracefully.
            }
        }
    }

    Ok(PlanningContext {
        relevant_knowledge,
        pack_sources,
    })
}

/// Minimum length of an objective token considered for relevance scoring.
/// One-character tokens (`a`, `1`, stray operators split off punctuation) carry
/// no topical signal and would match too indiscriminately. It also lower-bounds
/// the *stem* a plural-strip may produce in [`token_matches_pack`], so a regular
/// plural never collapses onto a one-character fragment.
const MIN_TOKEN_LEN: usize = 2;

/// Score a pack's relevance to an objective by **whole-word, singular/plural-
/// folded** keyword overlap.
///
/// The objective and the pack text (`name` + `description`) are each tokenized
/// into lowercase alphanumeric words. The score is the number of **distinct**
/// objective tokens (length >= [`MIN_TOKEN_LEN`]) that match a whole word in the
/// pack's word set, where a match holds if the token equals a pack word *or*
/// differs from one only by a regular English inflection (see
/// [`token_matches_pack`]).
///
/// Three properties matter for recall quality, extending the word-boundary
/// policy already adopted by [`crate::memory_consolidation::classifier`] and
/// [`crate::fact_reliability`]:
///
///   * **Whole-word, not substring.** A short token no longer matches when it is
///     merely *embedded* in an unrelated pack word — `go` must not match
///     `category`/`algorithm`, `test` must not match `latest`, `own` must not
///     match `download`. Substring hits inflated the relevance of unrelated packs
///     and could crowd a genuinely relevant pack out of the
///     [`MAX_PACKS_PER_OBJECTIVE`] cut, injecting off-topic knowledge into the
///     planning prompt.
///   * **Distinct tokens, not repetitions.** A word repeated in the objective
///     counts once, so a verbose objective that restates one term cannot inflate
///     a pack's score and distort the ranking.
///   * **Singular/plural-folded, not exact-only.** A genuinely relevant pack is
///     no longer missed when the objective and the pack name/description differ
///     only by a regular inflection — `container` vs `containers`, `image` vs
///     `images`, `library` vs `libraries`. Exact-only matching silently dropped
///     these near-hits, deflating a relevant pack's score (or zeroing it) and
///     costing recall. Folding is deliberately conservative — only regular
///     plural (`-s`/`-es`) and `-y`/`-ies` variants, each requiring the counter-
///     part to actually exist in the pack — so it adds recall without the
///     substring over-matching the whole-word rule above just removed.
fn relevance_score(objective: &str, pack: &KnowledgePackInfo) -> usize {
    let pack_words = word_set(&format!("{} {}", pack.name, pack.description));

    let objective_tokens: HashSet<String> = objective
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= MIN_TOKEN_LEN)
        .map(str::to_lowercase)
        .collect();

    objective_tokens
        .iter()
        .filter(|token| token_matches_pack(token, &pack_words))
        .count()
}

/// `true` when a lowercased objective `token` matches a whole word in
/// `pack_words`, exactly or via a regular English inflection.
///
/// The token matches if it is present verbatim, or if one of its conservative
/// **lemma variants** is — the variants being the regular plural forms (`+s`,
/// `+es`, and the singular obtained by stripping a trailing `-s`/`-es`) and the
/// `-y`↔`-ies` pair. Each generated variant must clear [`MIN_TOKEN_LEN`] before
/// it is considered, so a short token cannot fold onto a one-character fragment,
/// and — crucially — the match only fires when the variant is *actually a word
/// in the pack*. That keeps folding from re-introducing the substring
/// over-matching the whole-word rule removed: `class`/`focus`/`status` still
/// match themselves directly and only spuriously match a pack word if the pack
/// literally contains their (non-word) stripped stem, which real pack text does
/// not. Case is already folded by the caller; the variants therefore operate on
/// lowercase ASCII inflectional endings only.
fn token_matches_pack(token: &str, pack_words: &HashSet<String>) -> bool {
    if pack_words.contains(token) {
        return true;
    }

    // Regular plural, additive: singular objective token ↔ plural pack word.
    if pack_words.contains(&format!("{token}s")) || pack_words.contains(&format!("{token}es")) {
        return true;
    }

    // Regular plural, subtractive: plural objective token ↔ singular pack word.
    // Strip the longest applicable ending first (`-es` before `-s`) so a base is
    // not over-generated, and only when the stem still clears MIN_TOKEN_LEN.
    let stem_es = token
        .strip_suffix("es")
        .filter(|s| s.len() >= MIN_TOKEN_LEN);
    let stem_s = token.strip_suffix('s').filter(|s| s.len() >= MIN_TOKEN_LEN);
    if stem_es.is_some_and(|s| pack_words.contains(s))
        || stem_s.is_some_and(|s| pack_words.contains(s))
    {
        return true;
    }

    // `-y` ↔ `-ies` (category/categories, library/libraries, query/queries).
    if token
        .strip_suffix("ies")
        .filter(|s| s.len() >= MIN_TOKEN_LEN)
        .is_some_and(|s| pack_words.contains(&format!("{s}y")))
    {
        return true;
    }
    if token.len() > MIN_TOKEN_LEN
        && token
            .strip_suffix('y')
            .is_some_and(|s| pack_words.contains(&format!("{s}ies")))
    {
        return true;
    }

    false
}

/// Tokenize `text` into a set of distinct lowercase alphanumeric words for
/// whole-word membership tests. Empty tokens (produced by leading/trailing or
/// repeated separators) are dropped.
fn word_set(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{RPC_ERROR_METHOD_NOT_FOUND, RpcErrorPayload};
    use crate::rpc_transport::InMemoryRpcTransport;

    fn mock_transport() -> InMemoryRpcTransport {
        InMemoryRpcTransport::new("knowledge-ctx-test", |method, params| match method {
            "bridge.health" => Ok(serde_json::json!({
                "server_name": "simard-knowledge",
                "healthy": true,
            })),
            "knowledge.list_packs" => Ok(serde_json::json!({
                "packs": [
                    {
                        "name": "rust-expert",
                        "description": "Rust programming language ownership borrowing",
                        "article_count": 120,
                        "section_count": 450,
                    },
                    {
                        "name": "python-expert",
                        "description": "Python programming language stdlib",
                        "article_count": 200,
                        "section_count": 800,
                    },
                    {
                        "name": "docker-expert",
                        "description": "Docker containers images",
                        "article_count": 80,
                        "section_count": 300,
                    },
                ]
            })),
            "knowledge.query" => {
                let pack = params
                    .get("pack_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({
                    "answer": format!("Knowledge from {pack}"),
                    "sources": [{
                        "title": format!("{pack} article"),
                        "section": "Overview",
                    }],
                    "confidence": 0.8,
                }))
            }
            _ => Err(RpcErrorPayload {
                code: RPC_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            }),
        })
    }

    fn failing_transport() -> InMemoryRpcTransport {
        InMemoryRpcTransport::new("knowledge-fail", |method, _params| {
            Err(RpcErrorPayload {
                code: -32603,
                message: format!("knowledge down: {method}"),
            })
        })
    }

    #[test]
    fn enrich_picks_relevant_packs() {
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("Fix Rust ownership bug", &knowledge).unwrap();
        assert!(ctx.pack_sources.contains(&"rust-expert".to_string()));
        assert!(!ctx.relevant_knowledge.is_empty());
    }

    #[test]
    fn enrich_returns_error_when_knowledge_unavailable() {
        let knowledge = KnowledgeClient::new(Box::new(failing_transport()));
        let result = enrich_planning_context("anything", &knowledge);
        assert!(
            result.is_err(),
            "should propagate knowledge error, not silently degrade"
        );
    }

    #[test]
    fn enrich_returns_empty_for_unrelated_objective() {
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("xyzzy plugh", &knowledge).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn relevance_score_matches_pack_name() {
        let pack = KnowledgePackInfo {
            name: "rust-expert".to_string(),
            description: "Rust programming language".to_string(),
            article_count: 100,
            section_count: 400,
            ..Default::default()
        };
        let score = relevance_score("Fix Rust ownership issue", &pack);
        assert!(score >= 1, "expected match on 'rust', got {score}");
    }

    #[test]
    fn relevance_score_zero_for_no_match() {
        let pack = KnowledgePackInfo {
            name: "docker-expert".to_string(),
            description: "Docker containers".to_string(),
            article_count: 80,
            section_count: 300,
            ..Default::default()
        };
        let score = relevance_score("Fix Rust ownership issue", &pack);
        assert_eq!(score, 0);
    }

    #[test]
    fn relevance_score_requires_whole_word_match() {
        // A short objective token must NOT match when it is merely *embedded* in
        // an unrelated pack word: `go` inside `category`/`algorithm`, `test`
        // inside `latest`. A raw-substring scan spuriously matched these and
        // inflated an unrelated pack's relevance.
        let pack = KnowledgePackInfo {
            name: "algorithms-expert".to_string(),
            description: "Sorting category latest algorithm".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("go test", &pack),
            0,
            "'go'/'test' embedded in 'category'/'algorithm'/'latest' must not match"
        );
        // The same tokens DO match when present as whole words.
        let go_pack = KnowledgePackInfo {
            name: "go-expert".to_string(),
            description: "Go test tooling".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(relevance_score("go test", &go_pack), 2);
    }

    #[test]
    fn relevance_score_counts_distinct_tokens_only() {
        // A word repeated in the objective must count once, so a verbose
        // objective that restates one term cannot inflate a pack's score.
        let pack = KnowledgePackInfo {
            name: "rust-expert".to_string(),
            description: "Rust programming language".to_string(),
            article_count: 100,
            section_count: 400,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("rust rust rust programming", &pack),
            2,
            "distinct tokens {{rust, programming}} => 2, not 4"
        );
    }

    #[test]
    fn relevance_score_folds_regular_plural_both_directions() {
        // A genuinely relevant pack must not be missed when the objective and
        // the pack differ only by a regular plural inflection. Exact-only
        // matching scored just `docker` here (1); singular/plural folding also
        // credits `container`↔`containers` and `image`↔`images`.
        let pack = KnowledgePackInfo {
            name: "docker-expert".to_string(),
            description: "Docker containers images".to_string(),
            article_count: 80,
            section_count: 300,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("fix docker container image caching", &pack),
            3,
            "docker + container(s) + image(s) all fold to a match"
        );
        // Plural objective against a singular pack word folds the same way.
        let singular_pack = KnowledgePackInfo {
            name: "container-expert".to_string(),
            description: "Container runtime".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("debug containers", &singular_pack),
            1,
            "plural objective 'containers' folds onto singular pack word 'container'"
        );
    }

    #[test]
    fn relevance_score_folds_y_ies_variants() {
        // The `-y` ↔ `-ies` pair (library/libraries, category/categories) is a
        // common technical inflection that exact-only matching missed.
        let pack = KnowledgePackInfo {
            name: "python-expert".to_string(),
            description: "Python libraries categories".to_string(),
            article_count: 200,
            section_count: 800,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("pick a python library by category", &pack),
            3,
            "python + library↔libraries + category↔categories"
        );
    }

    #[test]
    fn relevance_score_folding_does_not_mangle_s_ending_words() {
        // Folding must not re-introduce over-matching: words that merely END in
        // `s`/`es` but are not plurals (`class`, `focus`, `status`) still match
        // themselves directly, and a token whose stripped stem is not a real
        // pack word does not spuriously match. `class` (stem `clas`) must not
        // match a pack that only contains the unrelated word `clang`.
        let self_match_pack = KnowledgePackInfo {
            name: "language-expert".to_string(),
            description: "class focus status".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("class focus status", &self_match_pack),
            3,
            "non-plural s-words still match themselves exactly"
        );
        let unrelated_pack = KnowledgePackInfo {
            name: "clang-expert".to_string(),
            description: "clang tooling".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("class hierarchy", &unrelated_pack),
            0,
            "'class' must not fold onto the unrelated pack word 'clang'"
        );
    }

    #[test]
    fn relevance_score_folding_preserves_whole_word_rule() {
        // Folding is additive over the whole-word rule, never a regression of
        // it: a token embedded in an unrelated pack word still must not match,
        // even though both share a plural relationship elsewhere.
        let pack = KnowledgePackInfo {
            name: "algorithms-expert".to_string(),
            description: "Sorting category latest algorithm".to_string(),
            article_count: 10,
            section_count: 20,
            ..Default::default()
        };
        assert_eq!(
            relevance_score("go test", &pack),
            0,
            "'go'/'test' embedded in pack words must not match even with folding"
        );
    }

    #[test]
    fn planning_context_is_empty_when_no_results() {
        let ctx = PlanningContext {
            relevant_knowledge: vec![],
            pack_sources: vec![],
        };
        assert!(ctx.is_empty());
    }

    #[test]
    fn max_packs_capped() {
        // Even with many matching packs, we cap at MAX_PACKS_PER_OBJECTIVE.
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context(
            "Rust Python Docker containers programming language",
            &knowledge,
        )
        .unwrap();
        assert!(ctx.pack_sources.len() <= MAX_PACKS_PER_OBJECTIVE);
    }
}
