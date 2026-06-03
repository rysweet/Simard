//! Application state and event handling for the TUI.
//!
//! Owns the active tab, cached daemon info and goal board, and handles
//! key events. Refresh logic uses dual rates: 2s for /proc + goals,
//! 10s for systemctl (to avoid hammering D-Bus).

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};

use crate::goals;
use crate::system::{self, CpuSample, DaemonInfo};
use crate::types::GoalBoard;

/// The six tabs in the TUI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Goals,
    Engineers,
    Activity,
    Meeting,
    Stats,
}

/// All tabs in display order.
pub const ALL_TABS: [Tab; 6] = [
    Tab::Overview,
    Tab::Goals,
    Tab::Engineers,
    Tab::Activity,
    Tab::Meeting,
    Tab::Stats,
];

impl Tab {
    /// Human-readable label for the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Goals => "Goals",
            Self::Engineers => "Engineers",
            Self::Activity => "Activity",
            Self::Meeting => "Meeting",
            Self::Stats => "Stats",
        }
    }

    /// Map a key character to a tab. '1' → Overview, '2' → Goals, etc.
    pub fn from_key(c: char) -> Option<Tab> {
        match c {
            '1' => Some(Tab::Overview),
            '2' => Some(Tab::Goals),
            '3' => Some(Tab::Engineers),
            '4' => Some(Tab::Activity),
            '5' => Some(Tab::Meeting),
            '6' => Some(Tab::Stats),
            _ => None,
        }
    }

    /// 1-based index for display in the tab bar.
    pub fn number(self) -> u8 {
        match self {
            Self::Overview => 1,
            Self::Goals => 2,
            Self::Engineers => 3,
            Self::Activity => 4,
            Self::Meeting => 5,
            Self::Stats => 6,
        }
    }
}

/// Meeting process lifecycle status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetingStatus {
    NotStarted,
    Running,
    Exited(i32),
    Error(String),
}

/// Info about a child process of the daemon.
#[derive(Clone, Debug)]
pub struct ChildProcessInfo {
    pub pid: u32,
    pub command: String,
    pub cpu_percent: Option<f64>,
    pub memory_kb: Option<u64>,
    pub runtime_secs: Option<u64>,
}

/// Cached stats for the Stats tab (refreshed on slow cycle).
#[derive(Clone, Debug, Default)]
pub struct StatsCache {
    pub state_files: Option<usize>,
    pub session_dirs: Option<usize>,
    pub open_issues: Option<usize>,
    pub open_prs: Option<usize>,
}

/// Main application state.
pub struct App {
    pub active_tab: Tab,
    pub daemon_info: DaemonInfo,
    pub goal_board: GoalBoard,
    pub should_quit: bool,
    pub state_root: std::path::PathBuf,
    prev_cpu_sample: Option<CpuSample>,
    // Engineers tab
    pub child_processes: Vec<ChildProcessInfo>,
    prev_child_cpu_samples: HashMap<u32, CpuSample>,
    // Activity tab
    pub log_lines: Vec<String>,
    // Meeting tab
    pub meeting_status: MeetingStatus,
    pub meeting_input: String,
    pub meeting_output: Vec<String>,
    meeting_child: Option<std::process::Child>,
    meeting_stdin: Option<std::process::ChildStdin>,
    meeting_stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    // Stats tab
    pub stats_cache: StatsCache,
    pub tick_count: u32,
}

impl App {
    /// Create a new App with the given service name.
    ///
    /// Starts on the Overview tab with unavailable daemon info and empty goals.
    /// Call `refresh()` after construction to populate.
    pub fn new(service_name: String) -> Self {
        let state_root = goals::resolve_state_root();
        Self {
            active_tab: Tab::Overview,
            daemon_info: DaemonInfo::unavailable(service_name),
            goal_board: GoalBoard::default(),
            should_quit: false,
            state_root,
            prev_cpu_sample: None,
            child_processes: Vec::new(),
            prev_child_cpu_samples: HashMap::new(),
            log_lines: Vec::new(),
            meeting_status: MeetingStatus::NotStarted,
            meeting_input: String::new(),
            meeting_output: Vec::new(),
            meeting_child: None,
            meeting_stdin: None,
            meeting_stdout: None,
            stats_cache: StatsCache::default(),
            tick_count: 0,
        }
    }

    /// Handle a key press event.
    ///
    /// - `1`–`6`: switch tabs (always, even in meeting mode)
    /// - On Meeting tab with Running: chars → input, Enter → send,
    ///   Backspace → delete, Esc → stop meeting
    /// - `q`/`Q`: quit (unless meeting is Running on Meeting tab)
    pub fn handle_key(&mut self, key: KeyEvent) {
        let code = key.code;

        // Tab switch keys always work regardless of mode
        if let KeyCode::Char(c) = code
            && let Some(tab) = Tab::from_key(c)
        {
            self.active_tab = tab;
            return;
        }

        // Meeting-specific input routing
        if self.active_tab == Tab::Meeting && self.meeting_status == MeetingStatus::Running {
            match code {
                KeyCode::Enter => self.send_meeting_input(),
                KeyCode::Backspace => {
                    self.meeting_input.pop();
                }
                KeyCode::Esc => self.stop_meeting(),
                KeyCode::Char(c) if self.meeting_input.len() < 4096 => {
                    self.meeting_input.push(c);
                }
                _ => {}
            }
            return;
        }

        // Default key handling
        if let KeyCode::Char('q' | 'Q') = code {
            self.should_quit = true;
        }
    }

    /// Send the current meeting input to the process and echo to output.
    fn send_meeting_input(&mut self) {
        if self.meeting_input.is_empty() {
            return;
        }
        let line = format!("> {}", self.meeting_input);
        self.meeting_output.push(line);
        // Cap output at 1000 lines
        if self.meeting_output.len() > 1000 {
            let excess = self.meeting_output.len() - 1000;
            self.meeting_output.drain(..excess);
        }
        // Write to child process stdin if available
        if let Some(ref mut stdin) = self.meeting_stdin {
            use std::io::Write;
            if stdin.write_all(self.meeting_input.as_bytes()).is_err()
                || stdin.write_all(b"\n").is_err()
                || stdin.flush().is_err()
            {
                self.meeting_status = MeetingStatus::Error("broken pipe".to_string());
                self.meeting_stdin = None;
            }
        }
        self.meeting_input.clear();
    }

    /// Stop the meeting process.
    fn stop_meeting(&mut self) {
        if let Some(ref mut child) = self.meeting_child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.meeting_child = None;
        self.meeting_stdin = None;
        self.meeting_stdout = None;
        self.meeting_status = MeetingStatus::Exited(0);
    }

    /// Spawn the meeting child process.
    fn spawn_meeting(&mut self) {
        let simard_bin = self.state_root.join("bin/simard");
        if !simard_bin.exists() {
            self.meeting_status = MeetingStatus::Error("simard binary not found".to_string());
            return;
        }
        match std::process::Command::new(&simard_bin)
            .args(["meeting", "start"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                self.meeting_stdin = child.stdin.take();
                // Set stdout to non-blocking on Unix
                #[cfg(unix)]
                {
                    if let Some(ref stdout) = child.stdout {
                        use std::os::unix::io::AsRawFd;
                        let fd = stdout.as_raw_fd();
                        unsafe {
                            let flags = libc::fcntl(fd, libc::F_GETFL);
                            if flags != -1 {
                                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                            }
                        }
                    }
                }
                self.meeting_stdout = child.stdout.take().map(std::io::BufReader::new);
                self.meeting_child = Some(child);
                self.meeting_status = MeetingStatus::Running;
                self.meeting_output
                    .push("Meeting started. Type your input below. Press Esc to stop.".to_string());
            }
            Err(e) => {
                self.meeting_status = MeetingStatus::Error(format!("Failed to start meeting: {e}"));
            }
        }
    }

    /// Drain any available output from the meeting process stdout.
    fn drain_meeting_output(&mut self) {
        // Check if meeting process has exited
        if let Some(ref mut child) = self.meeting_child
            && let Ok(Some(status)) = child.try_wait()
        {
            let code = status.code().unwrap_or(-1);
            self.meeting_status = MeetingStatus::Exited(code);
            self.meeting_output
                .push(format!("Meeting process exited with code {code}"));
            self.meeting_child = None;
            self.meeting_stdin = None;
            self.meeting_stdout = None;
            return;
        }
        // Read available lines without blocking
        if let Some(ref mut reader) = self.meeting_stdout {
            use std::io::BufRead;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        self.meeting_output.push(line.trim_end().to_string());
                        if self.meeting_output.len() > 1000 {
                            let excess = self.meeting_output.len() - 1000;
                            self.meeting_output.drain(..excess);
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    /// Refresh the child process list for the Engineers tab.
    fn refresh_engineers(&mut self) {
        let pid = match self.daemon_info.pid {
            Some(p) => p,
            None => {
                self.child_processes.clear();
                self.prev_child_cpu_samples.clear();
                return;
            }
        };

        // Try pgrep first, fall back to /proc scan
        let child_pids = std::process::Command::new("pgrep")
            .arg("--parent")
            .arg(pid.to_string())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.lines()
                    .filter_map(|l| l.trim().parse::<u32>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| self.scan_proc_for_children(pid));

        let boot_uptime = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok());
        let clk_tck: u64 = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }
            .try_into()
            .unwrap_or(100);

        let mut new_processes = Vec::new();
        let mut new_samples = HashMap::new();

        for child_pid in child_pids {
            let command = std::fs::read(format!("/proc/{child_pid}/cmdline"))
                .ok()
                .map(|bytes| system::read_proc_cmdline(&bytes))
                .unwrap_or_default();

            let memory_kb = std::fs::read_to_string(format!("/proc/{child_pid}/status"))
                .ok()
                .and_then(|c| system::parse_vmrss_kb(&c));

            let (cpu_percent, runtime_secs) =
                std::fs::read_to_string(format!("/proc/{child_pid}/stat"))
                    .ok()
                    .and_then(|c| system::parse_proc_stat(&c))
                    .map(|(utime, stime, starttime)| {
                        let sample = CpuSample {
                            utime,
                            stime,
                            starttime,
                            timestamp: std::time::Instant::now(),
                        };
                        let pct = self
                            .prev_child_cpu_samples
                            .get(&child_pid)
                            .and_then(|prev| {
                                if prev.starttime != sample.starttime {
                                    return None;
                                }
                                let elapsed = sample
                                    .timestamp
                                    .duration_since(prev.timestamp)
                                    .as_secs_f64();
                                let du = sample.utime.saturating_sub(prev.utime);
                                let ds = sample.stime.saturating_sub(prev.stime);
                                Some(system::compute_cpu_percent(du, ds, elapsed, clk_tck))
                            });
                        let runtime =
                            boot_uptime.map(|up| (up as u64).saturating_sub(starttime / clk_tck));
                        new_samples.insert(child_pid, sample);
                        (pct, runtime)
                    })
                    .unwrap_or((None, None));

            let truncated_cmd = if command.chars().count() > 80 {
                let s: String = command.chars().take(79).collect();
                format!("{s}…")
            } else {
                command
            };

            new_processes.push(ChildProcessInfo {
                pid: child_pid,
                command: truncated_cmd,
                cpu_percent,
                memory_kb,
                runtime_secs,
            });
        }

        self.prev_child_cpu_samples = new_samples;
        self.child_processes = new_processes;
    }

    /// Fallback: scan /proc for child processes by PPID.
    fn scan_proc_for_children(&self, parent_pid: u32) -> Vec<u32> {
        let mut children = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Ok(pid) = name_str.parse::<u32>()
                    && let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    && system::parse_proc_ppid(&stat) == Some(parent_pid)
                {
                    children.push(pid);
                }
            }
        }
        children
    }

    /// Refresh log lines for the Activity tab.
    fn refresh_logs(&mut self) {
        let output = std::process::Command::new("journalctl")
            .args([
                "--user",
                "-u",
                &self.daemon_info.service_name,
                "--no-pager",
                "-n",
                "50",
                "--output=short-iso",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());

        if let Some(output) = output {
            self.log_lines = output.lines().map(String::from).collect();
        }
    }

    /// Refresh stats cache (runs on slow cycle).
    fn refresh_stats(&mut self) {
        // Count state files recursively
        let state_dir = self.state_root.join("state");
        if state_dir.exists() {
            self.stats_cache.state_files = Some(count_files_recursive(&state_dir));
        }

        // Count session directories
        let sessions_dir = self.state_root.join("sessions");
        if sessions_dir.exists() {
            self.stats_cache.session_dirs = std::fs::read_dir(&sessions_dir).ok().map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .count()
            });
        }

        // Open issues via gh CLI
        self.stats_cache.open_issues = std::process::Command::new("gh")
            .args([
                "issue", "list", "--state", "open", "--limit", "1000", "--json", "number",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
            .map(|v| v.len());

        // Open PRs via gh CLI
        self.stats_cache.open_prs = std::process::Command::new("gh")
            .args([
                "pr", "list", "--state", "open", "--limit", "1000", "--json", "number",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
            .map(|v| v.len());
    }

    /// Clean up resources (meeting process) before exit.
    pub fn cleanup(&mut self) {
        self.stop_meeting();
    }

    /// Refresh daemon info, goal board, and tab-specific data.
    pub fn refresh(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);

        let service = &self.daemon_info.service_name;

        let systemctl_output = std::process::Command::new("systemctl")
            .arg("--user")
            .arg("show")
            .arg("-p")
            .arg("ActiveState,MainPID,ActiveEnterTimestamp,LoadState")
            .arg(service)
            .env("TZ", "UTC")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());

        let (state, pid, uptime_secs) = if let Some(ref output) = systemctl_output {
            let state = system::parse_daemon_state(output);
            let pid = system::parse_main_pid(output);
            let ts = system::parse_active_enter_timestamp(output);
            let uptime = ts.and_then(|t| compute_uptime_from_timestamp(&t));
            (state, pid, uptime)
        } else {
            (system::DaemonState::Unavailable, None, None)
        };

        let (cpu_percent, memory_rss_kb) = if let Some(pid_val) = pid {
            let rss = std::fs::read_to_string(format!("/proc/{pid_val}/status"))
                .ok()
                .and_then(|c| system::parse_vmrss_kb(&c));

            let cpu = std::fs::read_to_string(format!("/proc/{pid_val}/stat"))
                .ok()
                .and_then(|c| system::parse_proc_stat(&c))
                .and_then(|(utime, stime, starttime)| {
                    let sample = CpuSample {
                        utime,
                        stime,
                        starttime,
                        timestamp: std::time::Instant::now(),
                    };
                    let pct = self.prev_cpu_sample.as_ref().and_then(|prev| {
                        if prev.starttime != sample.starttime {
                            return None;
                        }
                        let elapsed = sample
                            .timestamp
                            .duration_since(prev.timestamp)
                            .as_secs_f64();
                        let du = sample.utime.saturating_sub(prev.utime);
                        let ds = sample.stime.saturating_sub(prev.stime);
                        Some(system::compute_cpu_percent(du, ds, elapsed, 100))
                    });
                    self.prev_cpu_sample = Some(sample);
                    pct
                });

            (cpu, rss)
        } else {
            self.prev_cpu_sample = None;
            (None, None)
        };

        self.daemon_info = DaemonInfo {
            state,
            pid,
            uptime_secs,
            cpu_percent,
            memory_rss_kb,
            service_name: self.daemon_info.service_name.clone(),
        };

        self.goal_board = goals::read_goal_board(&self.state_root);

        // Engineers tab: refresh child processes
        self.refresh_engineers();

        // Activity tab: refresh log lines
        self.refresh_logs();

        // Stats tab: slow cycle (every 5 ticks ≈ 10s)
        if self.tick_count.is_multiple_of(5) {
            self.refresh_stats();
        }

        // Meeting tab: auto-spawn + drain output
        if self.active_tab == Tab::Meeting
            && self.meeting_status == MeetingStatus::NotStarted
            && self.meeting_child.is_none()
        {
            self.spawn_meeting();
        }
        self.drain_meeting_output();
    }
}

fn compute_uptime_from_timestamp(ts: &str) -> Option<u64> {
    // systemctl timestamp format: "Day YYYY-MM-DD HH:MM:SS TIMEZONE"
    let parts: Vec<&str> = ts.splitn(2, ' ').collect();
    let rest = parts.get(1)?;
    // Strip timezone suffix
    let dt_part = rest.rsplit_once(' ').map(|(dt, _tz)| dt).unwrap_or(rest);
    let naive = chrono::NaiveDateTime::parse_from_str(dt_part, "%Y-%m-%d %H:%M:%S").ok()?;
    let then = naive.and_utc();
    let now = chrono::Utc::now();
    let secs = now.signed_duration_since(then).num_seconds();
    if secs >= 0 { Some(secs as u64) } else { None }
}

fn count_files_recursive(path: &std::path::Path) -> usize {
    count_files_with_depth(path, 10)
}

fn count_files_with_depth(path: &std::path::Path, max_depth: u32) -> usize {
    if max_depth == 0 {
        return 0;
    }
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                count += count_files_with_depth(&entry.path(), max_depth - 1);
            } else if ft.is_file() {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Helper: construct a KeyEvent from a character.
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Helper: construct a KeyEvent from a KeyCode.
    fn key_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // ── Tab enum ────────────────────────────────────────────────────

    #[test]
    fn all_tabs_count() {
        assert_eq!(ALL_TABS.len(), 6);
    }

    #[test]
    fn tab_labels_are_nonempty() {
        for tab in &ALL_TABS {
            assert!(!tab.label().is_empty(), "tab {tab:?} has empty label");
        }
    }

    #[test]
    fn tab_labels_correct() {
        assert_eq!(Tab::Overview.label(), "Overview");
        assert_eq!(Tab::Goals.label(), "Goals");
        assert_eq!(Tab::Engineers.label(), "Engineers");
        assert_eq!(Tab::Activity.label(), "Activity");
        assert_eq!(Tab::Meeting.label(), "Meeting");
        assert_eq!(Tab::Stats.label(), "Stats");
    }

    #[test]
    fn tab_from_key_valid() {
        assert_eq!(Tab::from_key('1'), Some(Tab::Overview));
        assert_eq!(Tab::from_key('2'), Some(Tab::Goals));
        assert_eq!(Tab::from_key('3'), Some(Tab::Engineers));
        assert_eq!(Tab::from_key('4'), Some(Tab::Activity));
        assert_eq!(Tab::from_key('5'), Some(Tab::Meeting));
        assert_eq!(Tab::from_key('6'), Some(Tab::Stats));
    }

    #[test]
    fn tab_from_key_invalid() {
        assert_eq!(Tab::from_key('0'), None);
        assert_eq!(Tab::from_key('7'), None);
        assert_eq!(Tab::from_key('q'), None);
        assert_eq!(Tab::from_key('a'), None);
    }

    #[test]
    fn tab_numbers_sequential() {
        for (i, tab) in ALL_TABS.iter().enumerate() {
            assert_eq!(tab.number(), (i + 1) as u8);
        }
    }

    #[test]
    fn tab_from_key_roundtrips_with_number() {
        for tab in &ALL_TABS {
            let c = char::from(b'0' + tab.number());
            assert_eq!(Tab::from_key(c), Some(*tab));
        }
    }

    // ── App construction ────────────────────────────────────────────

    #[test]
    fn app_new_starts_on_overview() {
        let app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);
        assert!(!app.should_quit);
    }

    #[test]
    fn app_new_starts_with_empty_goals() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(app.goal_board.active.is_empty());
        assert!(app.goal_board.backlog.is_empty());
    }

    #[test]
    fn app_new_has_empty_log_lines() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(app.log_lines.is_empty());
    }

    #[test]
    fn app_new_has_empty_child_processes() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(app.child_processes.is_empty());
    }

    #[test]
    fn app_new_meeting_not_started() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(matches!(app.meeting_status, MeetingStatus::NotStarted));
    }

    #[test]
    fn app_new_has_empty_meeting_buffers() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(app.meeting_input.is_empty());
        assert!(app.meeting_output.is_empty());
    }

    #[test]
    fn app_new_tick_count_zero() {
        let app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.tick_count, 0);
    }

    #[test]
    fn app_new_stats_cache_all_none() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(app.stats_cache.state_files.is_none());
        assert!(app.stats_cache.session_dirs.is_none());
        assert!(app.stats_cache.open_issues.is_none());
        assert!(app.stats_cache.open_prs.is_none());
    }

    // ── Key handling (KeyEvent signature) ───────────────────────────

    #[test]
    fn handle_key_quit() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert!(!app.should_quit);
        app.handle_key(key('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_quit_uppercase() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.handle_key(key('Q'));
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_tab_switch() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        app.handle_key(key('2'));
        assert_eq!(app.active_tab, Tab::Goals);

        app.handle_key(key('5'));
        assert_eq!(app.active_tab, Tab::Meeting);

        app.handle_key(key('1'));
        assert_eq!(app.active_tab, Tab::Overview);
    }

    #[test]
    fn handle_key_all_tabs_reachable() {
        let mut app = App::new("simard-ooda.service".to_string());
        for (i, expected) in ALL_TABS.iter().enumerate() {
            let c = char::from(b'1' + i as u8);
            app.handle_key(key(c));
            assert_eq!(
                app.active_tab, *expected,
                "key '{c}' should reach {expected:?}"
            );
        }
    }

    #[test]
    fn handle_key_unknown_char_is_noop() {
        let mut app = App::new("simard-ooda.service".to_string());
        let tab_before = app.active_tab;
        let quit_before = app.should_quit;
        app.handle_key(key('z'));
        assert_eq!(app.active_tab, tab_before);
        assert_eq!(app.should_quit, quit_before);
    }

    #[test]
    fn handle_key_enter_is_noop_outside_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.handle_key(key_code(KeyCode::Enter));
        assert!(!app.should_quit);
        assert_eq!(app.active_tab, Tab::Overview);
    }

    #[test]
    fn handle_key_escape_is_noop_outside_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.handle_key(key_code(KeyCode::Esc));
        assert!(!app.should_quit);
    }

    #[test]
    fn handle_key_backspace_is_noop_outside_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.handle_key(key_code(KeyCode::Backspace));
        assert!(!app.should_quit);
    }

    // ── Meeting tab: input routing ─────────────────────────────────

    #[test]
    fn meeting_char_appends_to_input() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(key('h'));
        app.handle_key(key('i'));
        assert_eq!(app.meeting_input, "hi");
    }

    #[test]
    fn meeting_backspace_removes_last_char() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello".to_string();
        app.handle_key(key_code(KeyCode::Backspace));
        assert_eq!(app.meeting_input, "hell");
    }

    #[test]
    fn meeting_backspace_on_empty_is_noop() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(key_code(KeyCode::Backspace));
        assert!(app.meeting_input.is_empty());
    }

    #[test]
    fn meeting_enter_clears_input() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello world".to_string();
        app.handle_key(key_code(KeyCode::Enter));
        assert!(app.meeting_input.is_empty());
    }

    #[test]
    fn meeting_enter_echoes_to_output() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "test input".to_string();
        let before = app.meeting_output.len();
        app.handle_key(key_code(KeyCode::Enter));
        assert!(app.meeting_output.len() > before);
        assert!(app.meeting_output.iter().any(|l| l.contains("test input")));
    }

    #[test]
    fn meeting_escape_stops_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(key_code(KeyCode::Esc));
        assert!(!matches!(app.meeting_status, MeetingStatus::Running));
    }

    #[test]
    fn meeting_tab_switch_always_works() {
        // Digit keys (1-6) switch tabs even when meeting is running
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(key('1'));
        assert_eq!(app.active_tab, Tab::Overview);
    }

    #[test]
    fn meeting_q_goes_to_input_when_running() {
        // 'q' should NOT quit when meeting is active — it's meeting input
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(key('q'));
        assert!(!app.should_quit);
        assert_eq!(app.meeting_input, "q");
    }

    #[test]
    fn meeting_q_quits_when_not_running() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        // meeting_status defaults to NotStarted
        app.handle_key(key('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn meeting_input_capped_at_4096_bytes() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "x".repeat(4096);
        app.handle_key(key('a'));
        assert!(app.meeting_input.len() <= 4096);
    }

    #[test]
    fn meeting_output_capped_at_1000_lines() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.meeting_output = (0..1000).map(|i| format!("line {i}")).collect();
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "overflow".to_string();
        app.handle_key(key_code(KeyCode::Enter));
        assert!(app.meeting_output.len() <= 1000);
    }

    // ── ChildProcessInfo ───────────────────────────────────────────

    #[test]
    fn child_process_info_fields() {
        let info = ChildProcessInfo {
            pid: 1234,
            command: "simard".to_string(),
            cpu_percent: Some(5.5),
            memory_kb: Some(10240),
            runtime_secs: Some(3600),
        };
        assert_eq!(info.pid, 1234);
        assert_eq!(info.command, "simard");
        assert_eq!(info.cpu_percent, Some(5.5));
        assert_eq!(info.memory_kb, Some(10240));
        assert_eq!(info.runtime_secs, Some(3600));
    }

    #[test]
    fn child_process_info_optional_fields_none() {
        let info = ChildProcessInfo {
            pid: 99,
            command: "worker".to_string(),
            cpu_percent: None,
            memory_kb: None,
            runtime_secs: None,
        };
        assert_eq!(info.pid, 99);
        assert!(info.cpu_percent.is_none());
        assert!(info.memory_kb.is_none());
        assert!(info.runtime_secs.is_none());
    }

    // ── StatsCache ─────────────────────────────────────────────────

    #[test]
    fn stats_cache_default_all_none() {
        let cache = StatsCache::default();
        assert!(cache.state_files.is_none());
        assert!(cache.session_dirs.is_none());
        assert!(cache.open_issues.is_none());
        assert!(cache.open_prs.is_none());
    }

    // ── MeetingStatus ──────────────────────────────────────────────

    #[test]
    fn meeting_status_variants() {
        assert!(matches!(
            MeetingStatus::NotStarted,
            MeetingStatus::NotStarted
        ));
        assert!(matches!(MeetingStatus::Running, MeetingStatus::Running));
        assert!(matches!(MeetingStatus::Exited(0), MeetingStatus::Exited(0)));
        assert!(
            matches!(
                MeetingStatus::Error("test".to_string()),
                MeetingStatus::Error(_)
            ),
            "MeetingStatus::Error should exist"
        );
    }
}
