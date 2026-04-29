//! Pure context gathering — best-effort, never panics, never propagates IO.

use super::EngineerLifecycleCtx;
use crate::ooda_loop::OodaState;
use std::path::Path;
use std::time::SystemTime;

/// Maximum bytes of engineer log to feed the brain. Caps the prompt size so
/// long-running engineers don't blow the LLM context window.
const MAX_LOG_TAIL_BYTES: u64 = 8 * 1024;

/// Maximum lines of log to keep after byte truncation. A second cap so a log
/// of giant single lines still produces a reviewable tail.
const MAX_LOG_TAIL_LINES: usize = 50;

/// How far back to walk cycle reports when counting consecutive skips. We
/// stop walking on the first non-skip outcome for the goal regardless, so
/// this is just a safety bound.
const MAX_CYCLE_REPORTS_TO_SCAN: usize = 200;

/// Assemble `EngineerLifecycleCtx` from the live state, the on-disk worktree,
/// and recent cycle reports under `<state_root>/cycle_reports/`. Errors
/// degrade to defaults — never panic, never propagate.
pub fn gather_engineer_lifecycle_ctx(
    state: &OodaState,
    state_root: &Path,
    goal_id: &str,
    worktree_path: &Path,
) -> EngineerLifecycleCtx {
    let goal_description = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .map(|g| g.description.clone())
        .unwrap_or_default();

    let failure_count = state.goal_failure_counts.get(goal_id).copied().unwrap_or(0);

    let worktree_mtime_secs_ago = std::fs::metadata(worktree_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let consecutive_skip_count =
        count_consecutive_skips(&state_root.join("cycle_reports"), goal_id);

    let sentinel_pid = read_sentinel_pid(worktree_path);

    let last_engineer_log_tail = read_engineer_log_tail(state_root, goal_id);

    EngineerLifecycleCtx {
        goal_id: goal_id.to_string(),
        goal_description,
        cycle_number: state.cycle_count,
        consecutive_skip_count,
        failure_count,
        worktree_path: worktree_path.to_path_buf(),
        worktree_mtime_secs_ago,
        sentinel_pid,
        last_engineer_log_tail: redact_secrets(&last_engineer_log_tail),
    }
}

/// Walk `cycle_reports` newest-to-oldest and count how many in a row contain
/// a "spawn_engineer skipped" outcome for `goal_id`. Stops on the first
/// non-skip outcome for this goal (or on missing/unreadable files).
fn count_consecutive_skips(cycle_reports_dir: &Path, goal_id: &str) -> u32 {
    let entries = match std::fs::read_dir(cycle_reports_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    // Collect (cycle_number, path) pairs. File names look like
    // `cycle_<N>.json` with optional zero-padding.
    let mut reports: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let stem = match name
            .strip_prefix("cycle_")
            .and_then(|s| s.strip_suffix(".json"))
        {
            Some(s) => s,
            None => continue,
        };
        let n: u32 = match stem.trim_start_matches('0').parse() {
            Ok(n) => n,
            Err(_) => {
                // All zeros (cycle_0000.json) → trim leaves empty string.
                if stem.chars().all(|c| c == '0') {
                    0
                } else {
                    continue;
                }
            }
        };
        reports.push((n, path));
    }
    reports.sort_by_key(|r| std::cmp::Reverse(r.0)); // newest first
    let mut count = 0u32;
    for (_, path) in reports.into_iter().take(MAX_CYCLE_REPORTS_TO_SCAN) {
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => break,
        };
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => break,
        };
        let outcomes = match value.get("outcomes").and_then(|v| v.as_array()) {
            Some(o) => o,
            None => break,
        };
        // Find the outcome (if any) for this goal_id.
        let goal_outcome = outcomes.iter().find(|o| {
            o.get("action")
                .and_then(|a| a.get("goal_id"))
                .and_then(|g| g.as_str())
                == Some(goal_id)
        });
        match goal_outcome {
            None => {
                // No outcome for this goal in this cycle → does not break the
                // chain, but does not extend it either. Continue walking.
                continue;
            }
            Some(o) => {
                let detail = o.get("detail").and_then(|d| d.as_str()).unwrap_or("");
                // Match any of the skip-path detail prefixes that
                // `dispatch_spawn_engineer` actually emits:
                //   - "engineer alive — skipping" (pre-#1266 / brain-error fallback)
                //   - "brain: continue_skipping" (brain ContinueSkipping decision)
                //   - "spawn_engineer skipped"   (board-assignment skip)
                let is_skip = detail.contains("spawn_engineer skipped")
                    || detail.contains("engineer alive")
                    || detail.contains("brain: continue_skipping");
                if is_skip {
                    count = count.saturating_add(1);
                } else {
                    break;
                }
            }
        }
    }
    count
}

fn read_sentinel_pid(worktree_path: &Path) -> Option<i32> {
    let claim = worktree_path.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
    let raw = std::fs::read_to_string(&claim).ok()?;
    raw.lines().next()?.trim().parse().ok()
}

/// Read the tail of the most recent engineer log for `goal_id`. Looks under
/// `<state_root>/agent_logs/engineer-<goal_id>-*.log` (matches the convention
/// in `agent_supervisor::lifecycle::open_agent_log`). Returns empty string
/// on any failure.
fn read_engineer_log_tail(state_root: &Path, goal_id: &str) -> String {
    let logs_dir = state_root.join("agent_logs");
    let entries = match std::fs::read_dir(&logs_dir) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };
    let prefix = format!("engineer-{goal_id}-");
    let mut newest: Option<(SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(&prefix) || !name.ends_with(".log") {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        match &newest {
            Some((t, _)) if *t >= mtime => {}
            _ => newest = Some((mtime, path)),
        }
    }
    let path = match newest {
        Some((_, p)) => p,
        None => return String::new(),
    };
    tail_file(&path).unwrap_or_default()
}

/// Read up to MAX_LOG_TAIL_BYTES from the end of `path`, drop the first
/// partial line if seek was non-zero, and cap to MAX_LOG_TAIL_LINES.
fn tail_file(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let seek_to = len.saturating_sub(MAX_LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(seek_to)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    if seek_to > 0 {
        // Drop the first (likely partial) line.
        if let Some(idx) = buf.find('\n') {
            buf = buf[idx + 1..].to_string();
        }
    }
    let lines: Vec<&str> = buf.lines().collect();
    let tail: Vec<&str> = if lines.len() > MAX_LOG_TAIL_LINES {
        lines[lines.len() - MAX_LOG_TAIL_LINES..].to_vec()
    } else {
        lines
    };
    Some(tail.join("\n"))
}

/// Strip secret-looking values from a log tail before sending to the LLM.
/// Conservative: any line with a key that looks like a token / secret has
/// the value portion replaced with `***`.
pub fn redact_secrets(raw: &str) -> String {
    raw.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    // Trigger keywords — case-insensitive substring match keeps this tiny
    // and dependency-free (no regex crate required per #1266 spec).
    let triggers = ["token", "key", "secret", "password", "bearer", "api_key"];
    let mut hit_idx: Option<usize> = None;
    for trigger in triggers {
        if let Some(i) = lower.find(trigger) {
            hit_idx = Some(match hit_idx {
                Some(prev) => prev.min(i),
                None => i,
            });
        }
    }
    let Some(start) = hit_idx else {
        return line.to_string();
    };
    // Find the value boundary: first `:` or `=` after the trigger, then take
    // up to end-of-line. If neither is present, also handle `bearer <value>`
    // by treating the next whitespace as the separator.
    let after = &line[start..];
    let sep_offset = after.find(|c: char| c == ':' || c == '=' || c.is_whitespace());
    let Some(off) = sep_offset else {
        return line.to_string();
    };
    let prefix_end = start + off + 1;
    if prefix_end > line.len() {
        return line.to_string();
    }
    // Preserve everything up through the separator, then redact.
    format!("{}***", &line[..prefix_end])
}

#[cfg(test)]
mod inner_tests {
    use super::*;

    #[test]
    fn tail_file_caps_at_50_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.log");
        let content: String = (0..200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, content).unwrap();
        let tail = tail_file(&path).unwrap();
        let count = tail.lines().count();
        assert!(count <= MAX_LOG_TAIL_LINES, "got {count} lines");
        assert!(tail.contains("line 199"), "newest line missing: {tail}");
    }

    #[test]
    fn redact_line_handles_equals_separator() {
        let r = redact_line("token=abc123");
        assert!(!r.contains("abc123"), "got: {r}");
        assert!(r.contains("***"));
    }

    #[test]
    fn redact_line_handles_colon_separator() {
        let r = redact_line("GITHUB_TOKEN: ghp_xxx");
        assert!(!r.contains("ghp_xxx"));
    }

    #[test]
    fn redact_line_handles_bearer_whitespace() {
        let r = redact_line("Authorization: bearer eyJabc");
        assert!(!r.contains("eyJabc"), "got: {r}");
    }

    #[test]
    fn redact_line_passes_normal_lines() {
        assert_eq!(redact_line("normal log line"), "normal log line");
    }
}
