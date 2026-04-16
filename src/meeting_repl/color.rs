//! ANSI terminal color helpers for the meeting REPL.
//!
//! All functions respect the `NO_COLOR` environment variable (see
//! <https://no-color.org/>): when `NO_COLOR` is set to any value, the plain
//! string is returned without escape codes.

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

/// Return `true` when ANSI colors should be suppressed.
#[inline]
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// Wrap `s` in green ANSI escape codes, or return `s` unchanged if NO_COLOR is set.
pub fn green(s: &str) -> String {
    if no_color() {
        s.to_string()
    } else {
        format!("{GREEN}{s}{RESET}")
    }
}

/// Wrap `s` in yellow ANSI escape codes, or return `s` unchanged if NO_COLOR is set.
pub fn yellow(s: &str) -> String {
    if no_color() {
        s.to_string()
    } else {
        format!("{YELLOW}{s}{RESET}")
    }
}

/// Wrap `s` in cyan ANSI escape codes, or return `s` unchanged if NO_COLOR is set.
pub fn cyan(s: &str) -> String {
    if no_color() {
        s.to_string()
    } else {
        format!("{CYAN}{s}{RESET}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn with_no_color<F: FnOnce()>(f: F) {
        // SAFETY: test-only, single-threaded guard via serial_test
        unsafe { std::env::set_var("NO_COLOR", "1") };
        f();
        unsafe { std::env::remove_var("NO_COLOR") };
    }

    fn without_no_color<F: FnOnce()>(f: F) {
        unsafe { std::env::remove_var("NO_COLOR") };
        f();
    }

    #[test]
    #[serial]
    fn green_with_no_color_returns_plain() {
        with_no_color(|| {
            assert_eq!(green("hello"), "hello");
        });
    }

    #[test]
    #[serial]
    fn yellow_with_no_color_returns_plain() {
        with_no_color(|| {
            assert_eq!(yellow("warn"), "warn");
        });
    }

    #[test]
    #[serial]
    fn cyan_with_no_color_returns_plain() {
        with_no_color(|| {
            assert_eq!(cyan("info"), "info");
        });
    }

    #[test]
    #[serial]
    fn green_without_no_color_contains_escape() {
        without_no_color(|| {
            let result = green("ok");
            assert!(
                result.contains("\x1b[32m"),
                "expected green escape: {result:?}"
            );
            assert!(result.contains("ok"));
            assert!(result.contains("\x1b[0m"));
        });
    }

    #[test]
    #[serial]
    fn yellow_without_no_color_contains_escape() {
        without_no_color(|| {
            let result = yellow("warn");
            assert!(
                result.contains("\x1b[33m"),
                "expected yellow escape: {result:?}"
            );
        });
    }

    #[test]
    #[serial]
    fn cyan_without_no_color_contains_escape() {
        without_no_color(|| {
            let result = cyan("section");
            assert!(
                result.contains("\x1b[36m"),
                "expected cyan escape: {result:?}"
            );
        });
    }
}
