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
    redact_secret_values(&strip_ansi_sequences(raw))
}

fn strip_ansi_sequences(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}'
            && let Some('[') = chars.peek().copied()
        {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }

        if ch != '\r' {
            sanitized.push(ch);
        }
    }

    sanitized
}

fn redact_secret_values(raw: &str) -> String {
    let markers = [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "authorization",
        "bearer ",
    ];

    raw.lines()
        .map(|line| {
            let lowered = line.to_ascii_lowercase();
            if markers.iter().any(|marker| lowered.contains(marker)) {
                if let Some((prefix, _)) = line.split_once('=') {
                    return format!("{prefix}=[REDACTED]");
                }
                if let Some((prefix, _)) = line.split_once(':') {
                    return format!("{prefix}: [REDACTED]");
                }
                return "[REDACTED]".to_string();
            }

            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    fn terminal_sanitization_redacts_secret_like_lines() {
        let raw = "token=abc123\nAuthorization: Bearer secret-value\nplain";
        assert_eq!(
            sanitize_terminal_text(raw),
            "token=[REDACTED]\nAuthorization: [REDACTED]\nplain"
        );
    }
}
