//! ANSI color helpers for meeting REPL output.
//!
//! Respects the `NO_COLOR` environment variable per <https://no-color.org/>.

/// Returns `true` when color output is disabled (NO_COLOR is set).
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// Wrap text in an ANSI color code. Returns plain text when NO_COLOR is set.
fn colorize(text: &str, code: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

/// Green text — used for success confirmations.
pub fn green(text: &str) -> String {
    colorize(text, "32")
}

/// Yellow text — used for warnings and questions.
pub fn yellow(text: &str) -> String {
    colorize(text, "33")
}

/// Cyan text — used for decisions and informational headers.
pub fn cyan(text: &str) -> String {
    colorize(text, "36")
}

/// Bold text.
pub fn bold(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        format!("\x1b[1m{text}\x1b[0m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn green_with_color() {
        std::env::remove_var("NO_COLOR");
        let result = green("ok");
        assert!(result.contains("\x1b[32m"));
        assert!(result.contains("ok"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    #[serial]
    fn no_color_env_disables_output() {
        std::env::set_var("NO_COLOR", "1");
        assert_eq!(green("ok"), "ok");
        assert_eq!(yellow("warn"), "warn");
        assert_eq!(cyan("info"), "info");
        assert_eq!(bold("bold"), "bold");
        std::env::remove_var("NO_COLOR");
    }

    #[test]
    #[serial]
    fn yellow_with_color() {
        std::env::remove_var("NO_COLOR");
        let result = yellow("warn");
        assert!(result.contains("\x1b[33m"));
    }

    #[test]
    #[serial]
    fn cyan_with_color() {
        std::env::remove_var("NO_COLOR");
        let result = cyan("info");
        assert!(result.contains("\x1b[36m"));
    }
}
