//! Pure tmux command-line builder for wrapping engineer subprocesses (WS-2).

use std::path::Path;

/// POSIX shell single-quote escape: wrap the value in single quotes,
/// replacing any embedded `'` with the sequence `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Build the argv vector for launching `inner_argv` inside a detached tmux
/// session named `session_name`, redirecting stdout+stderr through `tee -a`
/// so `<log_path>` continues to receive the engineer log stream that the
/// dashboard `/ws/agent_log/{agent}` viewer tails.
///
/// Returned shape:
/// ```text
/// ["tmux", "new-session", "-d", "-s", <session_name>,
///  "sh", "-c", "<shell-quoted inner argv> 2>&1 | tee -a <quoted log_path>"]
/// ```
pub fn build_tmux_wrapped_command(
    session_name: &str,
    inner_argv: &[String],
    log_path: &Path,
) -> Vec<String> {
    let inner_quoted: Vec<String> = inner_argv.iter().map(|s| shell_single_quote(s)).collect();
    let log_quoted = shell_single_quote(&log_path.to_string_lossy());
    let shell_cmd = format!("{} 2>&1 | tee -a {}", inner_quoted.join(" "), log_quoted);

    vec![
        "tmux".to_string(),
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session_name.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        shell_cmd,
    ]
}
