use super::parsing::*;
use crate::error::SimardError;

// ── parse_bracketed_list ──────────────────────────────────────────

#[test]
fn bracketed_list_empty() {
    let result = parse_bracketed_list("f", "[]").unwrap();
    assert!(result.is_empty());
}

#[test]
fn bracketed_list_single_item() {
    let result = parse_bracketed_list("f", "[hello]").unwrap();
    assert_eq!(result, vec!["hello"]);
}

#[test]
fn bracketed_list_multiple_items() {
    let result = parse_bracketed_list("f", "[a|b|c]").unwrap();
    assert_eq!(result, vec!["a", "b", "c"]);
}

#[test]
fn bracketed_list_trims_whitespace() {
    let result = parse_bracketed_list("f", "[ foo | bar | baz ]").unwrap();
    assert_eq!(result, vec!["foo", "bar", "baz"]);
}

#[test]
fn bracketed_list_nested_brackets() {
    let result = parse_bracketed_list("f", "[outer [inner]|second]").unwrap();
    assert_eq!(result, vec!["outer [inner]", "second"]);
}

#[test]
fn bracketed_list_rejects_missing_brackets() {
    let err = parse_bracketed_list("f", "no brackets").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "f".to_string(),
            reason: "value must use bracketed list syntax".to_string(),
        }
    );
}

#[test]
fn bracketed_list_rejects_empty_item() {
    let err = parse_bracketed_list("f", "[a||b]").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "f".to_string(),
            reason: "list contains an empty item".to_string(),
        }
    );
}

#[test]
fn bracketed_list_rejects_trailing_separator() {
    let err = parse_bracketed_list("f", "[a|b|]").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "f".to_string(),
            reason: "list contains an empty item".to_string(),
        }
    );
}

#[test]
fn bracketed_list_rejects_unexpected_close_bracket() {
    // `[a]b]` → outer brackets stripped to `a]b`, then inner `]` at depth 0
    let err = parse_bracketed_list("f", "[a]b]").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "f".to_string(),
            reason: "list contains an unexpected closing bracket".to_string(),
        }
    );
}

#[test]
fn bracketed_list_rejects_unterminated_bracket() {
    let err = parse_bracketed_list("f", "[a [b|c]").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "f".to_string(),
            reason: "list contains an unterminated bracket".to_string(),
        }
    );
}

#[test]
fn bracketed_list_handles_surrounding_whitespace() {
    let result = parse_bracketed_list("f", "  [x]  ").unwrap();
    assert_eq!(result, vec!["x"]);
}

// ── parse_non_negative_count ──────────────────────────────────────

#[test]
fn non_negative_count_zero() {
    assert_eq!(parse_non_negative_count("f", "0").unwrap(), 0);
}

#[test]
fn non_negative_count_positive() {
    assert_eq!(parse_non_negative_count("f", "42").unwrap(), 42);
}

#[test]
fn non_negative_count_trims_whitespace() {
    assert_eq!(parse_non_negative_count("f", "  7  ").unwrap(), 7);
}

#[test]
fn non_negative_count_rejects_non_numeric() {
    let err = parse_non_negative_count("f", "abc").unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidImprovementRecord { field, .. } if field == "f"
    ));
}

#[test]
fn non_negative_count_rejects_negative() {
    let err = parse_non_negative_count("f", "-1").unwrap_err();
    assert!(matches!(err, SimardError::InvalidImprovementRecord { .. }));
}

// ── parse_persisted_record_pairs ──────────────────────────────────

#[test]
fn record_pairs_single_pair() {
    let pairs = parse_persisted_record_pairs("key=value").unwrap();
    assert_eq!(pairs, vec![("key", "value")]);
}

#[test]
fn record_pairs_multiple_pairs() {
    let pairs = parse_persisted_record_pairs("a=1 b=2").unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0], ("a", "1"));
    assert_eq!(pairs[1], ("b", "2"));
}

#[test]
fn record_pairs_pipe_separated() {
    let pairs = parse_persisted_record_pairs("a=1|b=2").unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0], ("a", "1"));
    assert_eq!(pairs[1], ("b", "2"));
}

#[test]
fn record_pairs_strips_curation_prefix() {
    let pairs = parse_persisted_record_pairs("improvement-curation-record|key=val").unwrap();
    assert_eq!(pairs, vec![("key", "val")]);
}

#[test]
fn record_pairs_strips_curation_prefix_without_pipe() {
    let pairs = parse_persisted_record_pairs("improvement-curation-record key=val").unwrap();
    assert_eq!(pairs, vec![("key", "val")]);
}

#[test]
fn record_pairs_bracketed_value() {
    let pairs = parse_persisted_record_pairs("items=[a|b|c]").unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, "items");
    assert_eq!(pairs[0].1, "[a|b|c]");
}

#[test]
fn record_pairs_rejects_empty() {
    let err = parse_persisted_record_pairs("").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record cannot be empty".to_string(),
        }
    );
}

#[test]
fn record_pairs_rejects_no_equals() {
    let err = parse_persisted_record_pairs("just-a-key").unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidImprovementRecord { field, reason }
            if field == "record" && reason.contains("expected key=value")
    ));
}

#[test]
fn record_pairs_rejects_empty_value() {
    let err = parse_persisted_record_pairs("key=").unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidImprovementRecord { reason, .. }
            if reason == "value cannot be empty"
    ));
}

#[test]
fn record_pairs_whitespace_only() {
    let err = parse_persisted_record_pairs("   ").unwrap_err();
    assert_eq!(
        err,
        SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record cannot be empty".to_string(),
        }
    );
}

// ── read_bracketed_value ──────────────────────────────────────────

#[test]
fn read_bracketed_value_simple() {
    let (value, cursor) = read_bracketed_value("[abc]", 0).unwrap();
    assert_eq!(value, "[abc]");
    assert_eq!(cursor, 5);
}

#[test]
fn read_bracketed_value_nested() {
    let (value, cursor) = read_bracketed_value("[a [b] c]", 0).unwrap();
    assert_eq!(value, "[a [b] c]");
    assert_eq!(cursor, 9);
}

#[test]
fn read_bracketed_value_with_offset() {
    let input = "key=[val]rest";
    let (value, cursor) = read_bracketed_value(input, 4).unwrap();
    assert_eq!(value, "[val]");
    assert_eq!(cursor, 9);
}

#[test]
fn read_bracketed_value_rejects_unterminated() {
    let err = read_bracketed_value("[abc", 0).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidImprovementRecord { reason, .. }
            if reason.contains("unterminated")
    ));
}

#[test]
fn read_bracketed_value_rejects_unexpected_close() {
    let err = read_bracketed_value("]", 0).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidImprovementRecord { reason, .. }
            if reason.contains("unexpected closing bracket")
    ));
}

// ── helper functions ─────────────────────────────────────────────

#[test]
fn skip_record_separators_skips_pipes_and_whitespace() {
    assert_eq!(skip_record_separators("| | abc", 0), 4);
}

#[test]
fn skip_record_separators_noop_on_alpha() {
    assert_eq!(skip_record_separators("abc", 0), 0);
}

#[test]
fn skip_spaces_skips_only_whitespace() {
    assert_eq!(skip_spaces("   abc", 0), 3);
}

#[test]
fn looks_like_field_start_true_for_key_eq() {
    assert!(looks_like_field_start("key=value", 0));
}

#[test]
fn looks_like_field_start_false_for_plain_text() {
    assert!(!looks_like_field_start("just text", 0));
}

#[test]
fn looks_like_field_start_false_for_pipe() {
    assert!(!looks_like_field_start("|next", 0));
}

#[test]
fn looks_like_field_start_false_at_end() {
    assert!(!looks_like_field_start("key", 0));
}

#[test]
fn looks_like_field_start_with_hyphenated_key() {
    assert!(looks_like_field_start("my-key=value", 0));
}
