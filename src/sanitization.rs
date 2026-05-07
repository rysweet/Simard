#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObjectiveMetadataSummary {
    chars: usize,
    words: usize,
    lines: usize,
}

impl ObjectiveMetadataSummary {
    fn from_objective(objective: &str) -> Self {
        Self {
            chars: objective.chars().count(),
            words: objective.split_whitespace().count(),
            lines: if objective.is_empty() {
                0
            } else {
                objective.lines().count()
            },
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        let inner = raw.strip_prefix("objective-metadata(")?.strip_suffix(')')?;
        let mut chars = None;
        let mut words = None;
        let mut lines = None;
        for segment in inner.split(", ") {
            let (key, value) = segment.split_once('=')?;
            let value = value.parse::<usize>().ok()?;
            match key {
                "chars" if chars.is_none() => chars = Some(value),
                "words" if words.is_none() => words = Some(value),
                "lines" if lines.is_none() => lines = Some(value),
                _ => return None,
            }
        }

        Some(Self {
            chars: chars?,
            words: words?,
            lines: lines?,
        })
    }

    fn render(self) -> String {
        format!(
            "objective-metadata(chars={}, words={}, lines={})",
            self.chars, self.words, self.lines
        )
    }
}

pub fn objective_metadata(objective: &str) -> String {
    ObjectiveMetadataSummary::from_objective(objective).render()
}

pub fn normalize_objective_metadata(value: &str) -> Option<String> {
    ObjectiveMetadataSummary::parse(value).map(ObjectiveMetadataSummary::render)
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
    use super::{normalize_objective_metadata, objective_metadata};

    #[test]
    fn objective_metadata_round_trips_only_the_strict_shape() {
        let summary = objective_metadata("hello world");
        assert_eq!(
            normalize_objective_metadata(&summary).as_deref(),
            Some(summary.as_str())
        );
    }

    #[test]
    fn objective_metadata_rejects_untrusted_extra_fields() {
        assert_eq!(
            normalize_objective_metadata(
                "objective-metadata(chars=11, words=2, lines=1, token=LEAKME)"
            ),
            None
        );
    }

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
