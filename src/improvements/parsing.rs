use crate::error::{SimardError, SimardResult};

pub(super) fn parse_bracketed_list(field: &str, raw: &str) -> SimardResult<Vec<String>> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value must use bracketed list syntax".to_string(),
        });
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0usize;
    let chars = inner.chars().peekable();
    for ch in chars {
        match ch {
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                if bracket_depth == 0 {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: field.to_string(),
                        reason: "list contains an unexpected closing bracket".to_string(),
                    });
                }
                bracket_depth -= 1;
                current.push(ch);
            }
            '|' if bracket_depth == 0 => {
                let item = current.trim();
                if item.is_empty() {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: field.to_string(),
                        reason: "list contains an empty item".to_string(),
                    });
                }
                items.push(item.to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if bracket_depth != 0 {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "list contains an unterminated bracket".to_string(),
        });
    }

    let item = current.trim();
    if item.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "list contains an empty item".to_string(),
        });
    }
    items.push(item.to_string());
    Ok(items)
}

pub(super) fn parse_non_negative_count(field: &str, raw: &str) -> SimardResult<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value must be a non-negative integer or bracketed list".to_string(),
        })
}

pub(super) fn parse_persisted_record_pairs(raw: &str) -> SimardResult<Vec<(&str, &str)>> {
    let mut trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record cannot be empty".to_string(),
        });
    }
    if let Some(stripped) = trimmed.strip_prefix("improvement-curation-record") {
        trimmed = stripped.trim_start();
        if let Some(stripped) = trimmed.strip_prefix('|') {
            trimmed = stripped.trim_start();
        }
    }

    let mut pairs = Vec::new();
    let mut cursor = 0;
    while cursor < trimmed.len() {
        cursor = skip_record_separators(trimmed, cursor);
        if cursor >= trimmed.len() {
            break;
        }

        let key_start = cursor;
        while cursor < trimmed.len() {
            let Some(ch) = trimmed[cursor..].chars().next() else {
                break;
            };
            if ch == '=' || ch.is_whitespace() || ch == '|' {
                break;
            }
            cursor += ch.len_utf8();
        }
        if cursor >= trimmed.len() || !trimmed[cursor..].starts_with('=') {
            return Err(SimardError::InvalidImprovementRecord {
                field: "record".to_string(),
                reason: format!(
                    "expected key=value segment near '{}'",
                    trimmed[key_start..].trim()
                ),
            });
        }

        let field = trimmed[key_start..cursor].trim();
        cursor += 1;
        let value_start = cursor;
        let value;
        if trimmed[value_start..].starts_with('[') {
            let (parsed, next_cursor) = read_bracketed_value(trimmed, value_start)?;
            value = parsed;
            cursor = next_cursor;
        } else {
            while cursor < trimmed.len() {
                let Some(ch) = trimmed[cursor..].chars().next() else {
                    break;
                };
                if ch == '|' {
                    break;
                }
                if ch.is_whitespace() {
                    let next_cursor = skip_spaces(trimmed, cursor);
                    if looks_like_field_start(trimmed, next_cursor) {
                        break;
                    }
                    cursor = next_cursor;
                    continue;
                }
                cursor += ch.len_utf8();
            }
            value = trimmed[value_start..cursor].trim();
        }

        if field.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: "record".to_string(),
                reason: "persisted improvement record contains an empty field name".to_string(),
            });
        }
        if value.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: field.to_string(),
                reason: "value cannot be empty".to_string(),
            });
        }
        pairs.push((field, value));
    }

    if pairs.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record contained no key=value fields".to_string(),
        });
    }

    Ok(pairs)
}

pub(super) fn read_bracketed_value(raw: &str, start: usize) -> SimardResult<(&str, usize)> {
    let mut cursor = start;
    let mut depth = 0usize;
    while cursor < raw.len() {
        let Some(ch) = raw[cursor..].chars().next() else {
            break;
        };
        match ch {
            '[' => depth += 1,
            ']' => {
                depth =
                    depth
                        .checked_sub(1)
                        .ok_or_else(|| SimardError::InvalidImprovementRecord {
                            field: "record".to_string(),
                            reason:
                                "persisted improvement record has an unexpected closing bracket"
                                    .to_string(),
                        })?;
                if depth == 0 {
                    cursor += ch.len_utf8();
                    return Ok((&raw[start..cursor], cursor));
                }
            }
            _ => {}
        }
        cursor += ch.len_utf8();
    }

    Err(SimardError::InvalidImprovementRecord {
        field: "record".to_string(),
        reason: "persisted improvement record has an unterminated bracketed list".to_string(),
    })
}

pub(super) fn skip_record_separators(raw: &str, mut cursor: usize) -> usize {
    while cursor < raw.len() {
        let Some(ch) = raw[cursor..].chars().next() else {
            break;
        };
        if ch == '|' || ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        break;
    }
    cursor
}

pub(super) fn skip_spaces(raw: &str, mut cursor: usize) -> usize {
    while cursor < raw.len() {
        let Some(ch) = raw[cursor..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        break;
    }
    cursor
}

pub(super) fn looks_like_field_start(raw: &str, cursor: usize) -> bool {
    let tail = &raw[cursor..];
    let mut seen_any = false;
    for ch in tail.chars() {
        if ch == '=' {
            return seen_any;
        }
        if ch == '|' || ch.is_whitespace() {
            return false;
        }
        if !ch.is_ascii_alphanumeric() && ch != '-' {
            return false;
        }
        seen_any = true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_list_nested_brackets_preserved() {
        let result = parse_bracketed_list("f", "[outer [inner] value]").unwrap();
        assert_eq!(result, vec!["outer [inner] value"]);
    }

    #[test]
    fn bracketed_list_pipe_inside_nested_brackets_not_split() {
        let result = parse_bracketed_list("f", "[a [x|y] | b]").unwrap();
        assert_eq!(result, vec!["a [x|y]", "b"]);
    }

    #[test]
    fn bracketed_list_rejects_missing_brackets() {
        let result = parse_bracketed_list("test-field", "no brackets here");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            SimardError::InvalidImprovementRecord { field, reason } => {
                assert_eq!(field, "test-field");
                assert!(reason.contains("bracketed list syntax"));
            }
            other => panic!("expected InvalidImprovementRecord, got {other:?}"),
        }
    }

    #[test]
    fn bracketed_list_rejects_empty_item() {
        let result = parse_bracketed_list("f", "[a||b]");
        assert!(result.is_err());
    }

    #[test]
    fn bracketed_list_rejects_unbalanced_open_bracket() {
        let result = parse_bracketed_list("f", "[a [b]");
        assert!(result.is_err());
    }

    #[test]
    fn bracketed_list_rejects_extra_close_bracket() {
        let result = parse_bracketed_list("f", "[a ] b]");
        assert!(result.is_err());
    }

    #[test]
    fn non_negative_count_valid() {
        assert_eq!(parse_non_negative_count("f", "42").unwrap(), 42);
    }

    #[test]
    fn non_negative_count_zero() {
        assert_eq!(parse_non_negative_count("f", "0").unwrap(), 0);
    }

    #[test]
    fn non_negative_count_trims_whitespace() {
        assert_eq!(parse_non_negative_count("f", "  7  ").unwrap(), 7);
    }

    #[test]
    fn non_negative_count_rejects_negative() {
        assert!(parse_non_negative_count("f", "-1").is_err());
    }

    #[test]
    fn non_negative_count_rejects_non_numeric() {
        assert!(parse_non_negative_count("f", "abc").is_err());
    }

    #[test]
    fn record_pairs_simple() {
        let pairs = parse_persisted_record_pairs("key=value").unwrap();
        assert_eq!(pairs, vec![("key", "value")]);
    }

    #[test]
    fn record_pairs_multiple_pipe_separated() {
        let pairs = parse_persisted_record_pairs("a=1 | b=2 | c=3").unwrap();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("a", "1"));
        assert_eq!(pairs[1], ("b", "2"));
        assert_eq!(pairs[2], ("c", "3"));
    }

    #[test]
    fn record_pairs_strips_improvement_curation_prefix() {
        let pairs =
            parse_persisted_record_pairs("improvement-curation-record | key=value").unwrap();
        assert_eq!(pairs, vec![("key", "value")]);
    }

    #[test]
    fn record_pairs_bracketed_value() {
        let pairs = parse_persisted_record_pairs("items=[a|b|c]").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "items");
        assert_eq!(pairs[0].1, "[a|b|c]");
    }

    #[test]
    fn record_pairs_rejects_empty_input() {
        assert!(parse_persisted_record_pairs("").is_err());
    }

    #[test]
    fn record_pairs_rejects_whitespace_only() {
        assert!(parse_persisted_record_pairs("   ").is_err());
    }

    #[test]
    fn read_bracketed_value_simple() {
        let (val, cursor) = read_bracketed_value("[abc]", 0).unwrap();
        assert_eq!(val, "[abc]");
        assert_eq!(cursor, 5);
    }

    #[test]
    fn read_bracketed_value_nested() {
        let (val, cursor) = read_bracketed_value("[a [b] c]", 0).unwrap();
        assert_eq!(val, "[a [b] c]");
        assert_eq!(cursor, 9);
    }

    #[test]
    fn read_bracketed_value_unterminated() {
        assert!(read_bracketed_value("[abc", 0).is_err());
    }

    #[test]
    fn skip_record_separators_skips_pipes_and_spaces() {
        assert_eq!(skip_record_separators(" | | abc", 0), 5);
    }

    #[test]
    fn skip_record_separators_noop_on_non_separator() {
        assert_eq!(skip_record_separators("abc", 0), 0);
    }

    #[test]
    fn skip_spaces_basic() {
        assert_eq!(skip_spaces("   abc", 0), 3);
    }

    #[test]
    fn looks_like_field_start_true_for_key_equals() {
        assert!(looks_like_field_start("key=value", 0));
    }

    #[test]
    fn looks_like_field_start_false_for_plain_text() {
        assert!(!looks_like_field_start("plain text", 0));
    }

    #[test]
    fn looks_like_field_start_false_at_pipe() {
        assert!(!looks_like_field_start("| key=value", 0));
    }

    #[test]
    fn parse_bracketed_list_single_item() {
        let items = parse_bracketed_list("test", "[hello]").unwrap();
        assert_eq!(items, vec!["hello"]);
    }

    #[test]
    fn parse_bracketed_list_multiple_items() {
        let items = parse_bracketed_list("test", "[a | b | c]").unwrap();
        assert_eq!(items, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_bracketed_list_empty_brackets() {
        let items = parse_bracketed_list("test", "[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_bracketed_list_nested_brackets() {
        let items = parse_bracketed_list("test", "[outer [inner] | second]").unwrap();
        assert_eq!(items, vec!["outer [inner]", "second"]);
    }

    #[test]
    fn parse_bracketed_list_missing_open_bracket() {
        assert!(parse_bracketed_list("test", "no brackets").is_err());
    }

    #[test]
    fn parse_bracketed_list_empty_item_errors() {
        assert!(parse_bracketed_list("test", "[a | | b]").is_err());
    }

    #[test]
    fn parse_bracketed_list_unterminated_bracket() {
        assert!(parse_bracketed_list("test", "[a [b | c]").is_err());
    }

    #[test]
    fn parse_bracketed_list_unexpected_close_bracket() {
        assert!(parse_bracketed_list("test", "[a ] b]").is_err());
    }

    #[test]
    fn parse_bracketed_list_whitespace_trimming() {
        let items = parse_bracketed_list("test", "  [ foo | bar ]  ").unwrap();
        assert_eq!(items, vec!["foo", "bar"]);
    }

    #[test]
    fn parse_non_negative_count_valid() {
        assert_eq!(parse_non_negative_count("n", "42").unwrap(), 42);
    }

    #[test]
    fn parse_non_negative_count_zero() {
        assert_eq!(parse_non_negative_count("n", "0").unwrap(), 0);
    }

    #[test]
    fn parse_non_negative_count_with_whitespace() {
        assert_eq!(parse_non_negative_count("n", "  7  ").unwrap(), 7);
    }

    #[test]
    fn parse_non_negative_count_negative_errors() {
        assert!(parse_non_negative_count("n", "-1").is_err());
    }

    #[test]
    fn parse_non_negative_count_non_numeric_errors() {
        assert!(parse_non_negative_count("n", "abc").is_err());
    }

    #[test]
    fn parse_persisted_record_pairs_simple() {
        let pairs = parse_persisted_record_pairs("key=value").unwrap();
        assert_eq!(pairs, vec![("key", "value")]);
    }

    #[test]
    fn parse_persisted_record_pairs_multiple() {
        let pairs = parse_persisted_record_pairs("a=1 | b=2 | c=3").unwrap();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("a", "1"));
        assert_eq!(pairs[1], ("b", "2"));
        assert_eq!(pairs[2], ("c", "3"));
    }

    #[test]
    fn parse_persisted_record_pairs_with_prefix() {
        let pairs = parse_persisted_record_pairs("improvement-curation-record | key=val").unwrap();
        assert_eq!(pairs, vec![("key", "val")]);
    }

    #[test]
    fn parse_persisted_record_pairs_bracketed_value() {
        let pairs = parse_persisted_record_pairs("items=[a | b] | count=2").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "items");
        assert_eq!(pairs[0].1, "[a | b]");
        assert_eq!(pairs[1], ("count", "2"));
    }

    #[test]
    fn parse_persisted_record_pairs_empty_errors() {
        assert!(parse_persisted_record_pairs("").is_err());
    }

    #[test]
    fn parse_persisted_record_pairs_no_equals_errors() {
        assert!(parse_persisted_record_pairs("just-a-key").is_err());
    }

    #[test]
    fn skip_record_separators_pipes_and_spaces() {
        assert_eq!(skip_record_separators("| | abc", 0), 4);
    }

    #[test]
    fn skip_record_separators_no_separators() {
        assert_eq!(skip_record_separators("abc", 0), 0);
    }

    #[test]
    fn skip_spaces_leading_whitespace() {
        assert_eq!(skip_spaces("   abc", 0), 3);
    }

    #[test]
    fn skip_spaces_no_spaces() {
        assert_eq!(skip_spaces("abc", 0), 0);
    }

    #[test]
    fn looks_like_field_start_with_equals() {
        assert!(looks_like_field_start("key=value", 0));
    }

    #[test]
    fn looks_like_field_start_no_equals() {
        assert!(!looks_like_field_start("just text", 0));
    }

    #[test]
    fn looks_like_field_start_starts_with_equals() {
        assert!(!looks_like_field_start("=value", 0));
    }

    #[test]
    fn looks_like_field_start_with_pipe() {
        assert!(!looks_like_field_start("| key=value", 0));
    }

    #[test]
    fn looks_like_field_start_hyphenated_key() {
        assert!(looks_like_field_start("my-key=value", 0));
    }
}
