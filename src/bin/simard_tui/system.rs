//! System information readers for the Overview tab.
//!
//! Reads daemon status from `systemctl show`, CPU/memory from `/proc/<PID>/stat`
//! and `/proc/<PID>/status`. All parsing functions are pure (no I/O) for testability;
//! the I/O wrappers live in `app.rs`.

use std::fmt;
use std::time::Instant;

/// State of the simard daemon service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonState {
    /// systemctl reports ActiveState=active
    Running,
    /// systemctl reports ActiveState=inactive or ActiveState=failed
    Stopped,
    /// Service unit not found (systemctl exit code 4 or LoadState=not-found)
    NotFound,
    /// systemctl not available or not a systemd host
    Unavailable,
}

impl fmt::Display for DaemonState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => f.write_str("running"),
            Self::Stopped => f.write_str("stopped"),
            Self::NotFound => f.write_str("not found"),
            Self::Unavailable => f.write_str("unavailable"),
        }
    }
}

/// A single CPU usage sample from `/proc/<PID>/stat`.
#[derive(Clone, Debug)]
pub struct CpuSample {
    pub utime: u64,
    pub stime: u64,
    pub starttime: u64,
    pub timestamp: Instant,
}

/// Aggregated daemon info for the Overview tab.
#[derive(Clone, Debug)]
pub struct DaemonInfo {
    pub state: DaemonState,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub memory_rss_kb: Option<u64>,
    pub service_name: String,
}

impl DaemonInfo {
    /// Construct a DaemonInfo for when systemctl is unavailable.
    pub fn unavailable(service_name: String) -> Self {
        Self {
            state: DaemonState::Unavailable,
            pid: None,
            uptime_secs: None,
            cpu_percent: None,
            memory_rss_kb: None,
            service_name,
        }
    }
}

/// Parse the `ActiveState` field from `systemctl show` output to a `DaemonState`.
///
/// Expects output from `systemctl show -p ActiveState,MainPID,... <service>`.
/// Lines are `Key=Value` pairs. Returns `DaemonState::Unavailable` if the
/// `ActiveState` line is missing entirely.
pub fn parse_daemon_state(systemctl_output: &str) -> DaemonState {
    let mut active_state: Option<&str> = None;
    let mut load_state: Option<&str> = None;

    for line in systemctl_output.lines() {
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "ActiveState" => active_state = Some(val.trim()),
                "LoadState" => load_state = Some(val.trim()),
                _ => {}
            }
        }
    }

    if load_state == Some("not-found") {
        return DaemonState::NotFound;
    }

    match active_state {
        Some("active") => DaemonState::Running,
        Some("inactive") | Some("failed") | Some("deactivating") | Some("activating") => {
            DaemonState::Stopped
        }
        Some(_) => DaemonState::Stopped,
        None => DaemonState::Unavailable,
    }
}

/// Parse the `MainPID` field from `systemctl show` output.
///
/// Returns `None` if the field is missing or the value is 0 (service not running).
pub fn parse_main_pid(systemctl_output: &str) -> Option<u32> {
    for line in systemctl_output.lines() {
        if let Some((key, val)) = line.split_once('=') {
            if key.trim() != "MainPID" {
                continue;
            }
            let pid: u32 = val.trim().parse().ok()?;
            return if pid == 0 { None } else { Some(pid) };
        }
    }
    None
}

/// Parse the `ActiveEnterTimestamp` field from `systemctl show` output.
///
/// Returns the raw timestamp string (e.g. "Tue 2025-01-15 10:30:45 UTC"),
/// or `None` if the field is missing or empty.
pub fn parse_active_enter_timestamp(systemctl_output: &str) -> Option<String> {
    for line in systemctl_output.lines() {
        if let Some((key, val)) = line.split_once('=') {
            if key.trim() != "ActiveEnterTimestamp" {
                continue;
            }
            let val = val.trim();
            return if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            };
        }
    }
    None
}

/// Parse utime, stime, and starttime from `/proc/<PID>/stat` content.
///
/// Returns `(utime, stime, starttime)` — fields 14, 15, and 22 of the stat file.
/// Handles comm fields that contain spaces or parentheses.
pub fn parse_proc_stat(content: &str) -> Option<(u64, u64, u64)> {
    // Find the last ')' to skip the comm field (which can contain spaces/parens).
    let close_paren = content.rfind(')')?;
    let after_comm = &content[close_paren + 1..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();

    // After comm (0-indexed): state(0) ppid(1) pgrp(2) session(3) tty(4)
    // tpgid(5) flags(6) minflt(7) cminflt(8) majflt(9) cmajflt(10)
    // utime(11) stime(12) cutime(13) cstime(14) priority(15) nice(16)
    // num_threads(17) itrealvalue(18) starttime(19)
    if fields.len() < 20 {
        return None;
    }

    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;
    let starttime: u64 = fields[19].parse().ok()?;

    Some((utime, stime, starttime))
}

/// Parse VmRSS (resident set size in kB) from `/proc/<PID>/status` content.
///
/// Looks for a line matching `VmRSS:\s+<number> kB`.
pub fn parse_vmrss_kb(content: &str) -> Option<u64> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let rest = rest.trim();
            let num_str = rest.strip_suffix("kB")?.trim();
            return num_str.parse().ok();
        }
    }
    None
}

/// Compute CPU usage percentage from tick deltas and wall clock elapsed time.
///
/// Returns 0.0 if elapsed_secs is zero or negative (prevents division by zero).
pub fn compute_cpu_percent(
    delta_utime: u64,
    delta_stime: u64,
    elapsed_secs: f64,
    clock_ticks_per_sec: u64,
) -> f64 {
    if elapsed_secs <= 0.0 || clock_ticks_per_sec == 0 {
        return 0.0;
    }
    let total_ticks = (delta_utime + delta_stime) as f64;
    let wall_ticks = elapsed_secs * clock_ticks_per_sec as f64;
    (total_ticks / wall_ticks) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── DaemonState Display ─────────────────────────────────────────

    #[test]
    fn daemon_state_display_running() {
        assert_eq!(DaemonState::Running.to_string(), "running");
    }

    #[test]
    fn daemon_state_display_stopped() {
        assert_eq!(DaemonState::Stopped.to_string(), "stopped");
    }

    #[test]
    fn daemon_state_display_not_found() {
        assert_eq!(DaemonState::NotFound.to_string(), "not found");
    }

    #[test]
    fn daemon_state_display_unavailable() {
        assert_eq!(DaemonState::Unavailable.to_string(), "unavailable");
    }

    // ── DaemonInfo::unavailable ─────────────────────────────────────

    #[test]
    fn daemon_info_unavailable_constructor() {
        let info = DaemonInfo::unavailable("simard-ooda.service".to_string());
        assert_eq!(info.state, DaemonState::Unavailable);
        assert_eq!(info.pid, None);
        assert_eq!(info.uptime_secs, None);
        assert_eq!(info.cpu_percent, None);
        assert_eq!(info.memory_rss_kb, None);
        assert_eq!(info.service_name, "simard-ooda.service");
    }

    // ── parse_daemon_state ──────────────────────────────────────────

    const SYSTEMCTL_RUNNING: &str = "\
Type=simple
ActiveState=active
MainPID=12345
ActiveEnterTimestamp=Tue 2025-06-01 10:30:45 UTC
LoadState=loaded
";

    const SYSTEMCTL_STOPPED: &str = "\
Type=simple
ActiveState=inactive
MainPID=0
ActiveEnterTimestamp=
LoadState=loaded
";

    const SYSTEMCTL_FAILED: &str = "\
Type=simple
ActiveState=failed
MainPID=0
LoadState=loaded
";

    const SYSTEMCTL_NOT_FOUND: &str = "\
LoadState=not-found
ActiveState=inactive
MainPID=0
";

    #[test]
    fn parse_daemon_state_active() {
        assert_eq!(parse_daemon_state(SYSTEMCTL_RUNNING), DaemonState::Running);
    }

    #[test]
    fn parse_daemon_state_inactive() {
        assert_eq!(parse_daemon_state(SYSTEMCTL_STOPPED), DaemonState::Stopped);
    }

    #[test]
    fn parse_daemon_state_failed() {
        assert_eq!(parse_daemon_state(SYSTEMCTL_FAILED), DaemonState::Stopped);
    }

    #[test]
    fn parse_daemon_state_not_found() {
        // When LoadState=not-found, should return NotFound even if ActiveState=inactive.
        assert_eq!(
            parse_daemon_state(SYSTEMCTL_NOT_FOUND),
            DaemonState::NotFound
        );
    }

    #[test]
    fn parse_daemon_state_empty_input() {
        assert_eq!(parse_daemon_state(""), DaemonState::Unavailable);
    }

    #[test]
    fn parse_daemon_state_garbage_input() {
        assert_eq!(
            parse_daemon_state("this is not systemctl output"),
            DaemonState::Unavailable
        );
    }

    // ── parse_main_pid ──────────────────────────────────────────────

    #[test]
    fn parse_main_pid_valid() {
        assert_eq!(parse_main_pid(SYSTEMCTL_RUNNING), Some(12345));
    }

    #[test]
    fn parse_main_pid_zero_means_not_running() {
        assert_eq!(parse_main_pid(SYSTEMCTL_STOPPED), None);
    }

    #[test]
    fn parse_main_pid_missing_field() {
        assert_eq!(parse_main_pid("ActiveState=active\n"), None);
    }

    #[test]
    fn parse_main_pid_non_numeric() {
        assert_eq!(parse_main_pid("MainPID=abc\n"), None);
    }

    // ── parse_active_enter_timestamp ────────────────────────────────

    #[test]
    fn parse_active_enter_timestamp_valid() {
        let ts = parse_active_enter_timestamp(SYSTEMCTL_RUNNING);
        assert_eq!(ts.as_deref(), Some("Tue 2025-06-01 10:30:45 UTC"));
    }

    #[test]
    fn parse_active_enter_timestamp_empty_value() {
        let ts = parse_active_enter_timestamp(SYSTEMCTL_STOPPED);
        assert_eq!(ts, None);
    }

    #[test]
    fn parse_active_enter_timestamp_missing_field() {
        assert_eq!(parse_active_enter_timestamp("ActiveState=active\n"), None);
    }

    // ── parse_proc_stat ─────────────────────────────────────────────

    #[test]
    fn parse_proc_stat_valid() {
        // Standard /proc/PID/stat line.
        // Fields: pid (comm) state ppid pgrp session tty tpgid flags
        //         minflt cminflt majflt cmajflt utime stime cutime cstime
        //         priority nice num_threads itrealvalue starttime ...
        let content = "1234 (simard) S 1 1234 1234 0 -1 4194304 \
                        5000 0 100 0 1500 500 0 0 20 0 8 0 987654 \
                        536870912 6400 18446744073709551615";
        let result = parse_proc_stat(content);
        assert_eq!(result, Some((1500, 500, 987654)));
    }

    #[test]
    fn parse_proc_stat_comm_with_spaces() {
        // comm field can contain spaces: "(simard daemon)"
        let content = "1234 (simard daemon) S 1 1234 1234 0 -1 4194304 \
                        5000 0 100 0 2000 800 0 0 20 0 8 0 100000 \
                        536870912 6400 18446744073709551615";
        let result = parse_proc_stat(content);
        assert_eq!(result, Some((2000, 800, 100000)));
    }

    #[test]
    fn parse_proc_stat_comm_with_parens() {
        // comm field can contain parentheses: "(simard (v2))"
        let content = "1234 (simard (v2)) S 1 1234 1234 0 -1 4194304 \
                        5000 0 100 0 3000 1000 0 0 20 0 8 0 200000 \
                        536870912 6400 18446744073709551615";
        let result = parse_proc_stat(content);
        assert_eq!(result, Some((3000, 1000, 200000)));
    }

    #[test]
    fn parse_proc_stat_empty() {
        assert_eq!(parse_proc_stat(""), None);
    }

    #[test]
    fn parse_proc_stat_truncated() {
        // Fewer fields than expected.
        let content = "1234 (simard) S 1 1234";
        assert_eq!(parse_proc_stat(content), None);
    }

    // ── parse_vmrss_kb ──────────────────────────────────────────────

    #[test]
    fn parse_vmrss_kb_valid() {
        let content = "\
Name:\tsimard
State:\tS (sleeping)
Tgid:\t1234
VmPeak:\t 123456 kB
VmSize:\t 120000 kB
VmRSS:\t  25600 kB
VmData:\t  10000 kB
";
        assert_eq!(parse_vmrss_kb(content), Some(25600));
    }

    #[test]
    fn parse_vmrss_kb_zero() {
        let content = "VmRSS:\t       0 kB\n";
        assert_eq!(parse_vmrss_kb(content), Some(0));
    }

    #[test]
    fn parse_vmrss_kb_missing() {
        // Kernel threads don't have VmRSS.
        let content = "Name:\tkworker\nState:\tI (idle)\n";
        assert_eq!(parse_vmrss_kb(content), None);
    }

    #[test]
    fn parse_vmrss_kb_empty() {
        assert_eq!(parse_vmrss_kb(""), None);
    }

    // ── compute_cpu_percent ─────────────────────────────────────────

    #[test]
    fn compute_cpu_percent_normal() {
        // 75 CPU ticks over 2 seconds at 100 Hz = 200 wall ticks.
        // cpu% = 75/200 * 100 = 37.5%
        let result = compute_cpu_percent(50, 25, 2.0, 100);
        assert!(
            (result - 37.5).abs() < 0.01,
            "expected ~37.5%, got {result}"
        );
    }

    #[test]
    fn compute_cpu_percent_full_core() {
        // 200 CPU ticks over 2 seconds at 100 Hz = 100% of one core.
        let result = compute_cpu_percent(150, 50, 2.0, 100);
        assert!(
            (result - 100.0).abs() < 0.01,
            "expected ~100%, got {result}"
        );
    }

    #[test]
    fn compute_cpu_percent_zero_elapsed() {
        // Zero elapsed time should return 0.0, not panic or Inf.
        let result = compute_cpu_percent(100, 50, 0.0, 100);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn compute_cpu_percent_negative_elapsed() {
        // Negative elapsed (clock skew) should return 0.0.
        let result = compute_cpu_percent(100, 50, -1.0, 100);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn compute_cpu_percent_zero_ticks() {
        // No CPU usage.
        let result = compute_cpu_percent(0, 0, 2.0, 100);
        assert_eq!(result, 0.0);
    }

    // ── systemctl output: full parse scenario ───────────────────────

    #[test]
    fn full_systemctl_parse_running_service() {
        // Parse all fields from a running service's systemctl show output.
        let state = parse_daemon_state(SYSTEMCTL_RUNNING);
        let pid = parse_main_pid(SYSTEMCTL_RUNNING);
        let ts = parse_active_enter_timestamp(SYSTEMCTL_RUNNING);

        assert_eq!(state, DaemonState::Running);
        assert_eq!(pid, Some(12345));
        assert!(ts.is_some());
    }

    #[test]
    fn full_systemctl_parse_stopped_service() {
        let state = parse_daemon_state(SYSTEMCTL_STOPPED);
        let pid = parse_main_pid(SYSTEMCTL_STOPPED);
        let ts = parse_active_enter_timestamp(SYSTEMCTL_STOPPED);

        assert_eq!(state, DaemonState::Stopped);
        assert_eq!(pid, None);
        assert_eq!(ts, None);
    }
}
