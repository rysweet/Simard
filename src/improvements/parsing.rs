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

fn skip_record_separators(raw: &str, mut cursor: usize) -> usize {
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

fn skip_spaces(raw: &str, mut cursor: usize) -> usize {
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

fn looks_like_field_start(raw: &str, cursor: usize) -> bool {
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
