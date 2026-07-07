//! Free-form labels (tags) on goals — the single, deterministic authority for
//! label normalization, add/remove, the AND filter, and the `source:*`
//! provenance constants (issue
//! [#2743](https://github.com/rysweet/Simard/issues/2743)).
//!
//! This is a **self-contained, zero-I/O brick**: it owns the *only* place the
//! `source:*` provenance strings are spelled (so they cannot drift across the
//! many goal-creation sites) and the four pure label operations every operator
//! surface reuses. Every function here is pure and independently unit-tested; no
//! other module re-implements add/remove/match or re-spells a `source:*` token.
//!
//! **Determinism boundary.** Label CRUD and provenance stamping are
//! deterministic structured-data operations — plain code, never text inference.
//! Simard performs no semantic/topical auto-tagging from goal text; if that is
//! ever added it must be an agentic step, not a keyword matcher (out of scope).

/// Provenance: a goal promoted from a **creative idea**
/// (`creative_ideas::routing::route_idea_to_goal`). The headline case of
/// #2743 — stamping this makes "which goals came from creative ideas?"
/// answerable at a glance.
pub const SOURCE_CREATIVE_IDEAS: &str = "source:creative-ideas";
/// Provenance: an operator-added goal (`simard goal add` or the dashboard
/// create form).
pub const SOURCE_OPERATOR: &str = "source:operator";
/// Provenance: a goal materialized by the OODA loop (also the
/// unrecognized-backlog-source fallback in [`source_for_backlog`]).
pub const SOURCE_OODA: &str = "source:ooda";
/// Provenance: a goal contributed by the Overseer.
pub const SOURCE_OVERSEER: &str = "source:overseer";
/// Provenance: a goal derived from a meeting decision / meeting goal-curation.
pub const SOURCE_MEETING: &str = "source:meeting";
/// Provenance: a goal from the default seed board / seed store.
pub const SOURCE_SEED: &str = "source:seed";
/// Provenance: a sub-goal produced by decomposing a parent goal. A child
/// additionally inherits the parent's full label set, so a child of a
/// `source:creative-ideas` goal stays discoverable as creative-ideas-originated.
pub const SOURCE_DECOMPOSITION: &str = "source:decomposition";

/// The reserved provenance namespace prefix. Every `SOURCE_*` constant begins
/// with this. Labels in this namespace are stamped **only** by code at the
/// goal-creation sites; operator input may *filter* by them but must never
/// forge (add) or strip (remove) one — see [`is_source_tag`], enforced at the
/// operator-CLI mutation boundary. Keeping this authoritative means a
/// `source:*` chip on the dashboard always reflects a genuine, code-stamped
/// origin (security hardening H1, issue #2743).
pub const SOURCE_PREFIX: &str = "source:";

/// Maximum length, in Unicode scalar values, of an operator-supplied tag.
/// Tags are short human labels, so a modest cap keeps the persisted board
/// snapshot bounded (storage/DoS hardening H2) and the TSV/console/dashboard
/// output well-formed. Every `SOURCE_*` provenance constant is far shorter.
pub const MAX_TAG_LEN: usize = 128;

/// `true` iff `tag` sits in the reserved `source:*` provenance namespace.
/// Used at the operator-CLI mutation boundary to reject hand-forged or
/// hand-removed provenance labels, so provenance stays code-authoritative.
pub fn is_source_tag(tag: &str) -> bool {
    tag.starts_with(SOURCE_PREFIX)
}

/// Trim surrounding whitespace from `raw`. Returns `None` when the tag is empty
/// after trimming — the low-level normalization every label op shares.
///
/// Deliberately does **not** lowercase: labels are matched by exact,
/// case-sensitive equality, so forcing a case would silently rewrite operator
/// input. Richer operator-input validation (length/charset) lives in
/// [`validate_tag`]; this primitive stays minimal for internal callers that
/// stamp trusted `source:*` constants.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Validate an **operator-supplied** tag before it enters a label set or a
/// `--tag` filter: [`normalize_tag`] (trim + reject empty), then enforce the
/// [`MAX_TAG_LEN`] length cap and reject interior control characters (which
/// would corrupt TSV/console output and the dashboard). Returns the normalized
/// tag or a human-readable rejection reason.
///
/// This is the boundary guard for operator text. It deliberately does **not**
/// touch the reserved `source:*` namespace, because filtering *by* provenance
/// is a first-class use; the forge/strip guard is applied separately at the
/// mutation sites via [`is_source_tag`].
pub fn validate_tag(raw: &str) -> Result<String, String> {
    let tag = normalize_tag(raw)
        .ok_or_else(|| "tag cannot be empty (whitespace-only after trimming)".to_string())?;
    let len = tag.chars().count();
    if len > MAX_TAG_LEN {
        return Err(format!("tag is too long ({len} chars; max {MAX_TAG_LEN})"));
    }
    if tag.chars().any(char::is_control) {
        return Err("tag must not contain control characters".to_string());
    }
    Ok(tag)
}

/// Add `raw` (after [`normalize_tag`]) to `labels` if not already present.
///
/// Idempotent and **order-preserving**: a duplicate is a no-op, and a genuinely
/// new tag is appended at the end (so displayed order is first-applied order).
/// A tag that normalizes to `None` (empty after trim) is never added. Returns
/// `true` iff a label was actually added.
pub fn add_label(labels: &mut Vec<String>, raw: &str) -> bool {
    match normalize_tag(raw) {
        Some(tag) if !labels.iter().any(|l| l == &tag) => {
            labels.push(tag);
            true
        }
        _ => false,
    }
}

/// Remove `raw` (after [`normalize_tag`]) from `labels`. Removing a tag that is
/// not present — or a tag that normalizes to `None` — is a **no-op**. Returns
/// `true` iff a label was actually removed.
pub fn remove_label(labels: &mut Vec<String>, raw: &str) -> bool {
    let Some(tag) = normalize_tag(raw) else {
        return false;
    };
    let before = labels.len();
    labels.retain(|l| l != &tag);
    labels.len() != before
}

/// `true` iff `labels` contains **every** tag in `wanted` (logical AND),
/// matching the repeatable `--tag` CLI flag. An empty `wanted` slice matches
/// every goal (an unfiltered listing). Matching is exact and case-sensitive.
pub fn matches_all_tags(labels: &[String], wanted: &[String]) -> bool {
    wanted.iter().all(|w| labels.iter().any(|l| l == w))
}

/// Map a backlog item's coarse `source` string to the `source:*` label to stamp
/// when the item is promoted to an active goal (its first label-bearing
/// materialization).
///
/// Production backlog `source` strings are structured `prefix:…` tokens
/// (`operator:demote`, `meeting:{topic}`, `overseer:{repo}`), so this matches on
/// the **prefix**, with [`SOURCE_OODA`] as the fallback for anything
/// unrecognized.
pub fn source_for_backlog(backlog_source: &str) -> &'static str {
    if backlog_source.starts_with("operator:") {
        SOURCE_OPERATOR
    } else if backlog_source.starts_with("meeting:") {
        SOURCE_MEETING
    } else if backlog_source.starts_with("overseer:") {
        SOURCE_OVERSEER
    } else {
        SOURCE_OODA
    }
}

/// Plain-English provenance label for a goal, derived from the first `source:*`
/// tag in `labels` (issue #2922). Used when rendering a live overlay
/// `goal-store:record` as a backlog `source`, so the raw `source:creative-ideas`
/// token never leaks to the operator surface. Falls back to `"From goals"` when
/// no recognized `source:*` tag is present (e.g. legacy/seed records).
pub fn human_source_label(labels: &[String]) -> &'static str {
    for label in labels {
        match label.as_str() {
            SOURCE_CREATIVE_IDEAS => return "From creative ideas",
            SOURCE_OPERATOR => return "From an operator",
            SOURCE_OODA => return "From the OODA loop",
            SOURCE_OVERSEER => return "From the overseer",
            SOURCE_MEETING => return "From a meeting",
            SOURCE_SEED => return "From the seed board",
            SOURCE_DECOMPOSITION => return "From goal decomposition",
            _ => {}
        }
    }
    "From goals"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_rejects_empty() {
        assert_eq!(
            normalize_tag("  area:dashboard  "),
            Some("area:dashboard".to_string())
        );
        assert_eq!(normalize_tag("research"), Some("research".to_string()));
        assert_eq!(normalize_tag(""), None);
        assert_eq!(normalize_tag("   "), None);
        assert_eq!(normalize_tag("\t\n"), None);
    }

    #[test]
    fn normalize_does_not_lowercase() {
        // Opaque tokens: case is preserved, not folded.
        assert_eq!(
            normalize_tag("Area:Meeting"),
            Some("Area:Meeting".to_string())
        );
        assert_eq!(normalize_tag("Research"), Some("Research".to_string()));
    }

    #[test]
    fn is_source_tag_detects_reserved_namespace() {
        assert!(is_source_tag(SOURCE_OPERATOR));
        assert!(is_source_tag(SOURCE_CREATIVE_IDEAS));
        assert!(is_source_tag("source:anything"));
        // Non-provenance labels are not in the reserved namespace.
        assert!(!is_source_tag("area:dashboard"));
        assert!(!is_source_tag("research"));
        // Prefix must be at the very start.
        assert!(!is_source_tag("x-source:operator"));
    }

    #[test]
    fn validate_tag_trims_and_rejects_empty() {
        assert_eq!(
            validate_tag("  area:dashboard  "),
            Ok("area:dashboard".to_string())
        );
        assert!(validate_tag("").is_err());
        assert!(validate_tag("   ").is_err());
    }

    #[test]
    fn validate_tag_enforces_length_cap() {
        // Exactly at the cap is accepted; one over is rejected.
        let at_cap = "a".repeat(MAX_TAG_LEN);
        assert_eq!(validate_tag(&at_cap), Ok(at_cap.clone()));
        let over = "a".repeat(MAX_TAG_LEN + 1);
        let err = validate_tag(&over).expect_err("over-long tag must be rejected");
        assert!(err.contains("too long"), "got: {err}");
    }

    #[test]
    fn validate_tag_rejects_control_characters() {
        // Interior control chars (ANSI escape, tab, newline, NUL) are rejected;
        // surrounding whitespace is still just trimmed.
        assert!(validate_tag("area:\u{1b}[31mred").is_err());
        assert!(validate_tag("area\tdashboard").is_err());
        assert!(validate_tag("area\ndashboard").is_err());
        assert!(validate_tag("area\u{0}dashboard").is_err());
        assert_eq!(validate_tag("  area:ok  "), Ok("area:ok".to_string()));
    }

    #[test]
    fn validate_tag_allows_source_namespace_for_filtering() {
        // validate_tag does NOT reject the reserved namespace — filtering *by*
        // provenance (goal list --tag source:*) is a first-class use. The
        // forge/strip guard is is_source_tag, applied only at mutation sites.
        assert_eq!(
            validate_tag("source:creative-ideas"),
            Ok("source:creative-ideas".to_string())
        );
    }

    #[test]
    fn add_label_is_idempotent_and_order_preserving() {
        let mut labels = Vec::new();
        assert!(add_label(&mut labels, "source:creative-ideas"));
        assert!(add_label(&mut labels, "area:dashboard"));
        // Re-adding an existing tag is a no-op and returns false.
        assert!(!add_label(&mut labels, "source:creative-ideas"));
        // Trimming means a whitespace-padded duplicate is still a duplicate.
        assert!(!add_label(&mut labels, "  area:dashboard  "));
        assert_eq!(labels, vec!["source:creative-ideas", "area:dashboard"]);
    }

    #[test]
    fn add_label_trims_before_storing() {
        let mut labels = Vec::new();
        assert!(add_label(&mut labels, "  research  "));
        assert_eq!(labels, vec!["research"]);
    }

    #[test]
    fn add_label_rejects_empty_after_trim() {
        let mut labels = Vec::new();
        assert!(!add_label(&mut labels, "   "));
        assert!(labels.is_empty());
    }

    #[test]
    fn remove_label_removes_present_and_noops_absent() {
        let mut labels = vec!["source:operator".to_string(), "area:api".to_string()];
        assert!(remove_label(&mut labels, "area:api"));
        assert_eq!(labels, vec!["source:operator"]);
        // Removing an absent tag is a no-op that returns false.
        assert!(!remove_label(&mut labels, "area:api"));
        // Trims before matching.
        assert!(remove_label(&mut labels, "  source:operator  "));
        assert!(labels.is_empty());
        // Empty-after-trim is a no-op.
        assert!(!remove_label(&mut labels, "  "));
    }

    #[test]
    fn matches_all_tags_is_and_with_empty_matching_everything() {
        let labels = vec![
            "source:creative-ideas".to_string(),
            "area:dashboard".to_string(),
        ];
        // Empty filter matches everything.
        assert!(matches_all_tags(&labels, &[]));
        // Single present tag matches.
        assert!(matches_all_tags(&labels, &["area:dashboard".to_string()]));
        // AND: both present -> match.
        assert!(matches_all_tags(
            &labels,
            &[
                "source:creative-ideas".to_string(),
                "area:dashboard".to_string()
            ],
        ));
        // AND: one missing -> no match.
        assert!(!matches_all_tags(
            &labels,
            &[
                "source:creative-ideas".to_string(),
                "area:missing".to_string()
            ],
        ));
        // Empty labels only match an empty filter.
        assert!(matches_all_tags(&[], &[]));
        assert!(!matches_all_tags(&[], &["research".to_string()]));
    }

    #[test]
    fn matches_all_tags_is_case_sensitive() {
        let labels = vec!["research".to_string()];
        assert!(matches_all_tags(&labels, &["research".to_string()]));
        assert!(!matches_all_tags(&labels, &["Research".to_string()]));
    }

    #[test]
    fn source_for_backlog_maps_prefixes_with_ooda_fallback() {
        assert_eq!(source_for_backlog("operator:demote"), SOURCE_OPERATOR);
        assert_eq!(
            source_for_backlog("meeting:dashboard owner=alice"),
            SOURCE_MEETING
        );
        assert_eq!(source_for_backlog("overseer:amplihack-rs"), SOURCE_OVERSEER);
        // Unrecognized sources fall back to OODA.
        assert_eq!(source_for_backlog("stewardship:repo#12"), SOURCE_OODA);
        assert_eq!(source_for_backlog("decompose-parent"), SOURCE_OODA);
        assert_eq!(source_for_backlog(""), SOURCE_OODA);
    }

    #[test]
    fn provenance_constants_are_stable_source_tokens() {
        // Pin the durable wire tokens — these appear in persisted goal JSON and
        // must never drift.
        assert_eq!(SOURCE_CREATIVE_IDEAS, "source:creative-ideas");
        assert_eq!(SOURCE_OPERATOR, "source:operator");
        assert_eq!(SOURCE_OODA, "source:ooda");
        assert_eq!(SOURCE_OVERSEER, "source:overseer");
        assert_eq!(SOURCE_MEETING, "source:meeting");
        assert_eq!(SOURCE_SEED, "source:seed");
        assert_eq!(SOURCE_DECOMPOSITION, "source:decomposition");
    }
}
