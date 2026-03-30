pub fn objective_metadata(objective: &str) -> String {
    let chars = objective.chars().count();
    let words = objective.split_whitespace().count();
    let lines = if objective.is_empty() {
        0
    } else {
        objective.lines().count()
    };

    format!("objective-metadata(chars={chars}, words={words}, lines={lines})")
}

pub fn sanitize_terminal_text(raw: &str) -> String {
    redact_secret_values(&strip_terminal_control_sequences(raw))
}

fn strip_terminal_control_sequences(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                    continue;
                }
                Some(']') => {
                    chars.next();
                    strip_string_terminator_sequence(&mut chars);
                    continue;
                }
                Some('P' | 'X' | '^' | '_') => {
                    chars.next();
                    strip_escape_terminated_sequence(&mut chars);
                    continue;
                }
                Some(_) => {
                    chars.next();
                    continue;
                }
                None => continue,
            }
        }

        if matches!(ch, '\n' | '\t') {
            sanitized.push(ch);
            continue;
        }

        if !ch.is_control() {
            sanitized.push(ch);
        }
    }

    sanitized
}

fn strip_string_terminator_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(next) = chars.next() {
        if next == '\u{7}' {
            break;
        }
        if next == '\u{1b}' && matches!(chars.peek().copied(), Some('\\')) {
            chars.next();
            break;
        }
    }
}

fn strip_escape_terminated_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(next) = chars.next() {
        if next == '\u{1b}' && matches!(chars.peek().copied(), Some('\\')) {
            chars.next();
            break;
        }
    }
}

fn redact_secret_values(raw: &str) -> String {
    raw.lines()
        .map(redact_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_secret_line(line: &str) -> String {
    if let Some((prefix, _)) = line.split_once('=')
        && is_sensitive_key(prefix)
    {
        return format!("{prefix}=[REDACTED]");
    }

    if let Some((prefix, _)) = line.split_once(':')
        && is_sensitive_key(prefix)
    {
        return format!("{prefix}: [REDACTED]");
    }

    if line
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("bearer ")
    {
        return "[REDACTED]".to_string();
    }

    line.to_string()
}

fn is_sensitive_key(prefix: &str) -> bool {
    let normalized = prefix
        .trim()
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if matches!(ch, '_' | '-' | ' ') {
                Some('_')
            } else {
                None
            }
        })
        .collect::<String>();

    matches!(
        normalized.as_str(),
        "token" | "secret" | "password" | "passwd" | "api_key" | "apikey" | "authorization"
    ) || normalized.ends_with("_token")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_password")
        || normalized.ends_with("_passwd")
        || normalized.ends_with("_api_key")
        || normalized.ends_with("_apikey")
        || normalized.ends_with("_authorization")
}

#[cfg(test)]
mod tests {
    use super::sanitize_terminal_text;

    #[test]
    fn terminal_sanitization_strips_ansi_sequences() {
        let raw = "\u{1b}[32mgreen\u{1b}[0m\r\nplain";
        assert_eq!(sanitize_terminal_text(raw), "green\nplain");
    }

    #[test]
    fn terminal_sanitization_strips_osc_and_other_controls() {
        let raw = "\u{1b}]8;;https://example.invalid\u{7}linked\u{1b}]8;;\u{7}\u{1} text";
        assert_eq!(sanitize_terminal_text(raw), "linked text");
    }

    #[test]
    fn terminal_sanitization_redacts_secret_like_lines() {
        let raw = "token=abc123\nAuthorization: Bearer secret-value\nplain";
        assert_eq!(
            sanitize_terminal_text(raw),
            "token=[REDACTED]\nAuthorization: [REDACTED]\nplain"
        );
    }

    #[test]
    fn terminal_sanitization_keeps_normal_operator_text_with_security_words() {
        let raw = "\
State root: /tmp/simard-secret-path.12345
Active goal 1: p1 [active] Secret scanning follow-up
Rationale: operators need inspectable paths and titles";
        assert_eq!(sanitize_terminal_text(raw), raw);
    }
}
