//! Application state and event handling for the TUI.
//!
//! Owns the active tab, cached daemon info and goal board, and handles
//! key events. Refresh logic uses dual rates: 2s for /proc + goals,
//! 10s for systemctl (to avoid hammering D-Bus).

use std::collections::HashMap;
use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    /// Map an Alt+digit or Ctrl+digit key event to a tab.
    /// Alt+1 / Ctrl+1 → Overview, Alt+2 / Ctrl+2 → Goals, etc.
    /// Bare digits (without ALT or CONTROL) are ignored to avoid stealing input on the Meeting tab.
    pub fn from_key(key: &KeyEvent) -> Option<Tab> {
        let has_modifier = key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::CONTROL);
        if !has_modifier {
            return None;
        }
        match key.code {
            KeyCode::Char('1') => Some(Tab::Overview),
            KeyCode::Char('2') => Some(Tab::Goals),
            KeyCode::Char('3') => Some(Tab::Engineers),
            KeyCode::Char('4') => Some(Tab::Activity),
            KeyCode::Char('5') => Some(Tab::Meeting),
            KeyCode::Char('6') => Some(Tab::Stats),
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
    pub category: Option<String>,
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
    pub cursor_position: usize,
    meeting_child: Option<std::process::Child>,
    meeting_stdin: Option<std::process::ChildStdin>,
    meeting_stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    // Stats tab
    pub stats_cache: StatsCache,
    pub gh_receiver: Option<mpsc::Receiver<(Option<usize>, Option<usize>)>>,
    pub gh_in_flight: bool,
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
            cursor_position: 0,
            meeting_child: None,
            meeting_stdin: None,
            meeting_stdout: None,
            stats_cache: StatsCache::default(),
            gh_receiver: None,
            gh_in_flight: false,
            tick_count: 0,
        }
    }

    /// Handle a key press event.
    ///
    /// - `Alt+1`–`Alt+6` / `Ctrl+1`–`Ctrl+6`: switch tabs (always, even in meeting mode)
    /// - `Tab` / `Shift+Tab`: cycle tabs forward/backward (always)
    /// - On Meeting tab with Running: chars → input at cursor, Enter → send,
    ///   Backspace → delete before cursor, Left/Right → move cursor,
    ///   Home/End → jump cursor, Esc → stop meeting
    /// - `←`/`→`: cycle tabs with wrapping (when not in running meeting)
    /// - `q`/`Q`: quit (unless meeting is Running on Meeting tab)
    pub fn handle_key(&mut self, key: KeyEvent) {
        let code = key.code;

        // Alt+digit or Ctrl+digit tab switch always works regardless of mode
        if let Some(tab) = Tab::from_key(&key) {
            self.active_tab = tab;
            return;
        }

        // Tab/Shift+Tab for tab cycling always works
        let tab_count = ALL_TABS.len();
        let current_idx = ALL_TABS
            .iter()
            .position(|t| *t == self.active_tab)
            .unwrap_or(0);
        match code {
            KeyCode::Tab => {
                self.active_tab = ALL_TABS[(current_idx + 1) % tab_count];
                return;
            }
            KeyCode::BackTab => {
                self.active_tab = ALL_TABS[(current_idx + tab_count - 1) % tab_count];
                return;
            }
            _ => {}
        }

        // Meeting-specific input routing (BEFORE arrow tab cycling)
        if self.active_tab == Tab::Meeting && self.meeting_status == MeetingStatus::Running {
            match code {
                KeyCode::Enter => self.send_meeting_input(),
                KeyCode::Backspace if self.cursor_position > 0 => {
                    // Find the byte index of the char at cursor_position - 1
                    let byte_idx = self
                        .meeting_input
                        .char_indices()
                        .nth(self.cursor_position - 1)
                        .map(|(i, _)| i);
                    if let Some(idx) = byte_idx {
                        self.meeting_input.remove(idx);
                        self.cursor_position -= 1;
                    }
                }
                KeyCode::Left if self.cursor_position > 0 => {
                    self.cursor_position -= 1;
                }
                KeyCode::Right => {
                    let char_len = self.meeting_input.chars().count();
                    if self.cursor_position < char_len {
                        self.cursor_position += 1;
                    }
                }
                KeyCode::Home => {
                    self.cursor_position = 0;
                }
                KeyCode::End => {
                    self.cursor_position = self.meeting_input.chars().count();
                }
                KeyCode::Esc => self.stop_meeting(),
                KeyCode::Char(c) if self.meeting_input.len() < 4096 => {
                    // Insert at cursor position (char-indexed)
                    let byte_idx = self
                        .meeting_input
                        .char_indices()
                        .nth(self.cursor_position)
                        .map(|(i, _)| i)
                        .unwrap_or(self.meeting_input.len());
                    self.meeting_input.insert(byte_idx, c);
                    self.cursor_position += 1;
                }
                _ => {}
            }
            return;
        }

        // Left/Right arrow tab cycling (only when not in running meeting)
        match code {
            KeyCode::Right => {
                self.active_tab = ALL_TABS[(current_idx + 1) % tab_count];
                return;
            }
            KeyCode::Left => {
                self.active_tab = ALL_TABS[(current_idx + tab_count - 1) % tab_count];
                return;
            }
            _ => {}
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
        self.cursor_position = 0;
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
        self.cursor_position = 0;
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
                        let trimmed = line.trim_end().to_string();
                        if !trimmed.contains(" INFO ") {
                            self.meeting_output.push(trimmed);
                        }
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

        let pid_categories = load_pid_categories(&self.state_root);

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
                category: pid_categories.get(&child_pid).cloned(),
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
    ///
    /// Local filesystem counts are synchronous (fast, <1ms).
    /// GitHub CLI calls are spawned in a background thread to avoid
    /// blocking the TUI render loop (2-6s per call). Results arrive
    /// via `gh_receiver` and are drained by `drain_gh_results()`.
    fn refresh_stats(&mut self) {
        // ── Sync: local filesystem counts ──────────────────────────
        let state_dir = self.state_root.join("state");
        if state_dir.exists() {
            self.stats_cache.state_files = Some(count_files_recursive(&state_dir));
        }

        let sessions_dir = self.state_root.join("sessions");
        if sessions_dir.exists() {
            self.stats_cache.session_dirs = std::fs::read_dir(&sessions_dir).ok().map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .count()
            });
        }

        // ── Async: gh CLI in background thread ─────────────────────
        if self.gh_in_flight {
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.gh_receiver = Some(rx);
        self.gh_in_flight = true;

        std::thread::spawn(move || {
            let issues = std::process::Command::new("gh")
                .args([
                    "issue", "list", "--state", "open", "--limit", "1000", "--json", "number",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
                .map(|v| v.len());

            let prs = std::process::Command::new("gh")
                .args([
                    "pr", "list", "--state", "open", "--limit", "1000", "--json", "number",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
                .map(|v| v.len());

            // Send result; ignore error if receiver was dropped (app quit)
            let _ = tx.send((issues, prs));
        });
    }

    /// Drain any completed gh CLI results from the background thread.
    ///
    /// Called every tick from `refresh()`. Uses `try_recv()` so it never
    /// blocks. Clears `gh_in_flight` and `gh_receiver` once the sender
    /// disconnects (thread finished).
    pub fn drain_gh_results(&mut self) {
        let receiver = match self.gh_receiver.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match receiver.try_recv() {
                Ok((issues, prs)) => {
                    self.stats_cache.open_issues = issues;
                    self.stats_cache.open_prs = prs;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.gh_in_flight = false;
                    self.gh_receiver = None;
                    break;
                }
            }
        }
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

        // Stats tab: drain background gh CLI results every tick
        self.drain_gh_results();

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

/// Lightweight DTOs for deserializing subagent_sessions.json.
/// Mirrors the subset of fields from `subagent_sessions::Registry` that
/// we need, without coupling to that module's types.
mod pid_category_dto {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Registry {
        #[serde(default)]
        pub sessions: Vec<Session>,
    }

    #[derive(Deserialize)]
    pub struct Session {
        pub agent_id: String,
        pub pid: u32,
        #[serde(default)]
        pub ended_at: Option<i64>,
    }
}

/// Read `<state_root>/state/subagent_sessions.json` and return a map of
/// `pid → agent_id` for active (non-ended, non-zero-pid) sessions.
/// Returns an empty map on missing file, corrupt JSON, or I/O errors.
pub fn load_pid_categories(state_root: &std::path::Path) -> HashMap<u32, String> {
    let path = state_root.join("state").join("subagent_sessions.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return HashMap::new(),
    };
    let registry: pid_category_dto::Registry = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    registry
        .sessions
        .into_iter()
        .filter(|s| s.pid != 0 && s.ended_at.is_none())
        .map(|s| (s.pid, s.agent_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Helper: construct a KeyEvent from a character (no modifiers).
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Helper: construct an Alt+char KeyEvent.
    fn alt_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    /// Helper: construct a KeyEvent from a KeyCode.
    fn key_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Helper: construct a Ctrl+char KeyEvent.
    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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
        // BUG1: from_key now takes &KeyEvent and requires ALT modifier
        assert_eq!(Tab::from_key(&alt_key('1')), Some(Tab::Overview));
        assert_eq!(Tab::from_key(&alt_key('2')), Some(Tab::Goals));
        assert_eq!(Tab::from_key(&alt_key('3')), Some(Tab::Engineers));
        assert_eq!(Tab::from_key(&alt_key('4')), Some(Tab::Activity));
        assert_eq!(Tab::from_key(&alt_key('5')), Some(Tab::Meeting));
        assert_eq!(Tab::from_key(&alt_key('6')), Some(Tab::Stats));
    }

    #[test]
    fn tab_from_key_invalid() {
        // BUG1: bare digits without ALT should NOT match
        assert_eq!(Tab::from_key(&key('0')), None);
        assert_eq!(Tab::from_key(&key('7')), None);
        assert_eq!(Tab::from_key(&key('q')), None);
        assert_eq!(Tab::from_key(&key('a')), None);
        // Bare digits (no ALT) are also invalid now
        assert_eq!(Tab::from_key(&key('1')), None);
        assert_eq!(Tab::from_key(&key('2')), None);
        assert_eq!(Tab::from_key(&key('6')), None);
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
            assert_eq!(Tab::from_key(&alt_key(c)), Some(*tab));
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
        // BUG1: Tab switching now requires Alt+digit
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        app.handle_key(alt_key('2'));
        assert_eq!(app.active_tab, Tab::Goals);

        app.handle_key(alt_key('5'));
        assert_eq!(app.active_tab, Tab::Meeting);

        app.handle_key(alt_key('1'));
        assert_eq!(app.active_tab, Tab::Overview);
    }

    #[test]
    fn handle_key_all_tabs_reachable() {
        // BUG1: All tabs reachable via Alt+digit
        let mut app = App::new("simard-ooda.service".to_string());
        for (i, expected) in ALL_TABS.iter().enumerate() {
            let c = char::from(b'1' + i as u8);
            app.handle_key(alt_key(c));
            assert_eq!(
                app.active_tab, *expected,
                "Alt+'{c}' should reach {expected:?}"
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
        app.cursor_position = 5; // cursor at end for backspace to remove last char
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
        // BUG1: Alt+digit switches tabs even when meeting is running
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.handle_key(alt_key('1'));
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
            category: Some("code-writer".to_string()),
        };
        assert_eq!(info.pid, 1234);
        assert_eq!(info.command, "simard");
        assert_eq!(info.cpu_percent, Some(5.5));
        assert_eq!(info.memory_kb, Some(10240));
        assert_eq!(info.runtime_secs, Some(3600));
        assert_eq!(info.category.as_deref(), Some("code-writer"));
    }

    #[test]
    fn child_process_info_optional_fields_none() {
        let info = ChildProcessInfo {
            pid: 99,
            command: "worker".to_string(),
            cpu_percent: None,
            memory_kb: None,
            runtime_secs: None,
            category: None,
        };
        assert_eq!(info.pid, 99);
        assert!(info.cpu_percent.is_none());
        assert!(info.memory_kb.is_none());
        assert!(info.runtime_secs.is_none());
        assert!(info.category.is_none());
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

    // ── Background gh CLI: field initialization ────────────────────

    #[test]
    fn app_new_gh_not_in_flight() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(
            !app.gh_in_flight,
            "gh_in_flight should be false on construction"
        );
    }

    #[test]
    fn app_new_gh_receiver_is_none() {
        let app = App::new("simard-ooda.service".to_string());
        assert!(
            app.gh_receiver.is_none(),
            "gh_receiver should be None on construction"
        );
    }

    // ── drain_gh_results: channel → stats_cache ────────────────────

    #[test]
    fn drain_gh_results_updates_stats_from_channel() {
        let mut app = App::new("simard-ooda.service".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        app.gh_receiver = Some(rx);
        app.gh_in_flight = true;

        tx.send((Some(42), Some(7))).unwrap();
        app.drain_gh_results();

        assert_eq!(app.stats_cache.open_issues, Some(42));
        assert_eq!(app.stats_cache.open_prs, Some(7));
    }

    #[test]
    fn drain_gh_results_handles_none_values() {
        // gh CLI failure sends (None, None) — stats stay as dashes
        let mut app = App::new("simard-ooda.service".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        app.gh_receiver = Some(rx);
        app.gh_in_flight = true;

        tx.send((None, None)).unwrap();
        app.drain_gh_results();

        assert!(app.stats_cache.open_issues.is_none());
        assert!(app.stats_cache.open_prs.is_none());
    }

    #[test]
    fn drain_gh_results_clears_in_flight_on_disconnect() {
        let mut app = App::new("simard-ooda.service".to_string());
        let (tx, rx) = std::sync::mpsc::channel::<(Option<usize>, Option<usize>)>();
        app.gh_receiver = Some(rx);
        app.gh_in_flight = true;

        // Drop sender to simulate thread completion
        drop(tx);
        app.drain_gh_results();

        assert!(!app.gh_in_flight, "gh_in_flight should clear on disconnect");
        assert!(
            app.gh_receiver.is_none(),
            "gh_receiver should be set to None on disconnect"
        );
    }

    #[test]
    fn drain_gh_results_noop_when_no_receiver() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert!(app.gh_receiver.is_none());

        // Should not panic or change anything
        app.drain_gh_results();

        assert!(app.stats_cache.open_issues.is_none());
        assert!(app.stats_cache.open_prs.is_none());
        assert!(!app.gh_in_flight);
    }

    #[test]
    fn drain_gh_results_keeps_in_flight_while_channel_open() {
        let mut app = App::new("simard-ooda.service".to_string());
        let (_tx, rx) = std::sync::mpsc::channel::<(Option<usize>, Option<usize>)>();
        app.gh_receiver = Some(rx);
        app.gh_in_flight = true;

        // Channel open but empty — try_recv returns Empty, not Disconnected
        app.drain_gh_results();

        assert!(
            app.gh_in_flight,
            "gh_in_flight should remain true while channel is open"
        );
        assert!(
            app.gh_receiver.is_some(),
            "gh_receiver should remain while channel is open"
        );
    }

    #[test]
    fn drain_gh_results_takes_last_value_when_multiple_sent() {
        // If the thread sends multiple times (unlikely but possible),
        // drain should process all and keep the last value.
        let mut app = App::new("simard-ooda.service".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        app.gh_receiver = Some(rx);
        app.gh_in_flight = true;

        tx.send((Some(10), Some(2))).unwrap();
        tx.send((Some(15), Some(5))).unwrap();
        drop(tx);

        app.drain_gh_results();

        assert_eq!(
            app.stats_cache.open_issues,
            Some(15),
            "should have last value"
        );
        assert_eq!(app.stats_cache.open_prs, Some(5), "should have last value");
        assert!(!app.gh_in_flight, "should clear after disconnect");
    }

    // ── refresh_stats: local fs counts with tempdir ────────────────

    #[test]
    fn refresh_stats_counts_state_files() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        // Create some files
        std::fs::write(state_dir.join("goal_board.json"), "{}").unwrap();
        std::fs::write(state_dir.join("cycle_report.json"), "{}").unwrap();
        std::fs::write(state_dir.join("memory.json"), "{}").unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert_eq!(
            app.stats_cache.state_files,
            Some(3),
            "should count 3 state files"
        );
    }

    #[test]
    fn refresh_stats_counts_session_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        // Create some session subdirectories
        std::fs::create_dir(sessions_dir.join("session-001")).unwrap();
        std::fs::create_dir(sessions_dir.join("session-002")).unwrap();
        // A regular file should NOT be counted
        std::fs::write(sessions_dir.join("index.json"), "[]").unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert_eq!(
            app.stats_cache.session_dirs,
            Some(2),
            "should count 2 session dirs (not the file)"
        );
    }

    #[test]
    fn refresh_stats_state_files_none_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // Don't create state/ dir — it doesn't exist

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert!(
            app.stats_cache.state_files.is_none(),
            "should be None when state dir doesn't exist"
        );
    }

    #[test]
    fn refresh_stats_session_dirs_none_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert!(
            app.stats_cache.session_dirs.is_none(),
            "should be None when sessions dir doesn't exist"
        );
    }

    #[test]
    fn refresh_stats_empty_dirs_return_zero() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("state")).unwrap();
        std::fs::create_dir_all(tmp.path().join("sessions")).unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert_eq!(app.stats_cache.state_files, Some(0));
        assert_eq!(app.stats_cache.session_dirs, Some(0));
    }

    #[test]
    fn refresh_stats_counts_nested_state_files() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        let sub_dir = state_dir.join("cycles");
        std::fs::create_dir_all(&sub_dir).unwrap();

        std::fs::write(state_dir.join("top.json"), "{}").unwrap();
        std::fs::write(sub_dir.join("cycle1.json"), "{}").unwrap();
        std::fs::write(sub_dir.join("cycle2.json"), "{}").unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.refresh_stats();

        assert_eq!(
            app.stats_cache.state_files,
            Some(3),
            "should count files recursively"
        );
    }

    // ── refresh_stats: gh_in_flight guard ──────────────────────────

    #[test]
    fn refresh_stats_does_not_spawn_when_gh_in_flight() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("state")).unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        app.gh_in_flight = true;

        // Set up an existing receiver to prove it's not replaced
        let (_tx, rx) = std::sync::mpsc::channel::<(Option<usize>, Option<usize>)>();
        app.gh_receiver = Some(rx);

        app.refresh_stats();

        // Local fs stats should still be updated
        assert_eq!(app.stats_cache.state_files, Some(0));
        // gh_in_flight should remain true (no new spawn)
        assert!(app.gh_in_flight);
        // The receiver should be the same one we set (not replaced)
        assert!(app.gh_receiver.is_some());
    }

    #[test]
    fn refresh_stats_sets_gh_in_flight_when_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("state")).unwrap();

        let mut app = App::new("simard-ooda.service".to_string());
        app.state_root = tmp.path().to_path_buf();
        assert!(!app.gh_in_flight);

        app.refresh_stats();

        // After refresh_stats, gh_in_flight should be true (thread spawned)
        assert!(
            app.gh_in_flight,
            "should set gh_in_flight after spawning thread"
        );
        assert!(
            app.gh_receiver.is_some(),
            "should have a receiver after spawning thread"
        );
    }

    // ── BUG1: Left/Right arrow tab cycling ─────────────────────────

    #[test]
    fn handle_key_right_arrow_cycles_forward() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview); // index 0

        app.handle_key(key_code(KeyCode::Right));
        assert_eq!(app.active_tab, Tab::Goals); // index 1

        app.handle_key(key_code(KeyCode::Right));
        assert_eq!(app.active_tab, Tab::Engineers); // index 2
    }

    #[test]
    fn handle_key_left_arrow_cycles_backward() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Engineers; // index 2

        app.handle_key(key_code(KeyCode::Left));
        assert_eq!(app.active_tab, Tab::Goals); // index 1

        app.handle_key(key_code(KeyCode::Left));
        assert_eq!(app.active_tab, Tab::Overview); // index 0
    }

    #[test]
    fn handle_key_right_arrow_wraps_at_end() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Stats; // index 5 (last)

        app.handle_key(key_code(KeyCode::Right));
        assert_eq!(app.active_tab, Tab::Overview); // wraps to index 0
    }

    #[test]
    fn handle_key_left_arrow_wraps_at_start() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview); // index 0

        app.handle_key(key_code(KeyCode::Left));
        assert_eq!(app.active_tab, Tab::Stats); // wraps to index 5
    }

    #[test]
    fn handle_key_left_right_cycling_full_loop() {
        // Right arrow 6 times should come back to Overview
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        for _ in 0..ALL_TABS.len() {
            app.handle_key(key_code(KeyCode::Right));
        }
        assert_eq!(app.active_tab, Tab::Overview, "full loop returns to start");
    }

    #[test]
    fn handle_key_arrows_work_during_meeting_running() {
        // BUG C fix: Arrow keys should move cursor, NOT cycle tabs, when meeting is running
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting; // index 4
        app.meeting_status = MeetingStatus::Running;

        app.handle_key(key_code(KeyCode::Right));
        assert_eq!(
            app.active_tab,
            Tab::Meeting,
            "Right arrow should NOT change tab when meeting is running"
        );
    }

    // ── BUG1: Bare digit regression ────────────────────────────────

    #[test]
    fn meeting_bare_digit_goes_to_input_when_running() {
        // BUG1 regression: bare '1' in meeting mode should go to input, NOT switch tabs
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;

        app.handle_key(key('1'));
        assert_eq!(
            app.active_tab,
            Tab::Meeting,
            "bare digit should NOT switch tabs"
        );
        assert_eq!(app.meeting_input, "1", "bare digit should go to input");
    }

    #[test]
    fn bare_digit_is_noop_outside_meeting() {
        // Bare digits should be ignored outside meeting mode (no tab switch)
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        app.handle_key(key('2'));
        assert_eq!(
            app.active_tab,
            Tab::Overview,
            "bare digit should NOT switch tabs outside meeting"
        );
    }

    // ── BUG2: load_pid_categories ──────────────────────────────────

    #[test]
    fn load_pid_categories_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        let json = r#"{
            "sessions": [
                {"agent_id": "code-writer", "session_name": "s1", "host": "h",
                 "pid": 1234, "created_at": 100, "goal_id": "g1"},
                {"agent_id": "reviewer", "session_name": "s2", "host": "h",
                 "pid": 5678, "created_at": 200, "goal_id": "g2"}
            ]
        }"#;
        std::fs::write(state_dir.join("subagent_sessions.json"), json).unwrap();

        let map = load_pid_categories(tmp.path());
        assert_eq!(map.get(&1234), Some(&"code-writer".to_string()));
        assert_eq!(map.get(&5678), Some(&"reviewer".to_string()));
    }

    #[test]
    fn load_pid_categories_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        // No state/ dir or JSON file

        let map = load_pid_categories(tmp.path());
        assert!(map.is_empty(), "missing file should return empty map");
    }

    #[test]
    fn load_pid_categories_corrupt_json() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("subagent_sessions.json"), "NOT JSON!!!").unwrap();

        let map = load_pid_categories(tmp.path());
        assert!(map.is_empty(), "corrupt JSON should return empty map");
    }

    #[test]
    fn load_pid_categories_filters_ended_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        let json = r#"{
            "sessions": [
                {"agent_id": "live-agent", "session_name": "s1", "host": "h",
                 "pid": 1000, "created_at": 100, "goal_id": "g1"},
                {"agent_id": "dead-agent", "session_name": "s2", "host": "h",
                 "pid": 2000, "created_at": 200, "ended_at": 300, "goal_id": "g2"}
            ]
        }"#;
        std::fs::write(state_dir.join("subagent_sessions.json"), json).unwrap();

        let map = load_pid_categories(tmp.path());
        assert_eq!(map.get(&1000), Some(&"live-agent".to_string()));
        assert!(
            !map.contains_key(&2000),
            "ended sessions should be filtered out"
        );
    }

    #[test]
    fn load_pid_categories_filters_pid_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        let json = r#"{
            "sessions": [
                {"agent_id": "no-pid", "session_name": "s1", "host": "h",
                 "pid": 0, "created_at": 100, "goal_id": "g1"},
                {"agent_id": "has-pid", "session_name": "s2", "host": "h",
                 "pid": 999, "created_at": 200, "goal_id": "g2"}
            ]
        }"#;
        std::fs::write(state_dir.join("subagent_sessions.json"), json).unwrap();

        let map = load_pid_categories(tmp.path());
        assert!(!map.contains_key(&0), "pid 0 should be filtered out");
        assert_eq!(map.get(&999), Some(&"has-pid".to_string()));
    }

    #[test]
    fn load_pid_categories_empty_sessions_array() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("subagent_sessions.json"),
            r#"{"sessions": []}"#,
        )
        .unwrap();

        let map = load_pid_categories(tmp.path());
        assert!(map.is_empty());
    }

    // ── BUG A: INFO log line filtering ─────────────────────────────

    #[test]
    fn drain_meeting_output_filters_info_lines() {
        use std::process::{Command, Stdio};

        let mut app = App::new("simard-ooda.service".to_string());

        let mut child = Command::new("sh")
            .args([
                "-c",
                concat!(
                    "printf 'normal line\\n",
                    "2024-01-01T10:00:00 INFO server started\\n",
                    "another line\\n'; sleep 5"
                ),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        {
            use std::os::unix::io::AsRawFd;
            let fd = stdout.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }

        app.meeting_stdout = Some(std::io::BufReader::new(stdout));
        app.meeting_child = Some(child);
        app.meeting_status = MeetingStatus::Running;

        // Poll until we get output (max 2 seconds)
        for _ in 0..200 {
            app.drain_meeting_output();
            if app.meeting_output.len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Cleanup
        if let Some(ref mut c) = app.meeting_child {
            let _ = c.kill();
            let _ = c.wait();
        }

        assert!(
            app.meeting_output.iter().any(|l| l.contains("normal line")),
            "non-INFO lines should pass through, got: {:?}",
            app.meeting_output
        );
        assert!(
            app.meeting_output
                .iter()
                .any(|l| l.contains("another line")),
            "non-INFO lines should pass through"
        );
        assert!(
            !app.meeting_output.iter().any(|l| l.contains(" INFO ")),
            "lines containing ' INFO ' should be filtered out, got: {:?}",
            app.meeting_output
        );
    }

    #[test]
    fn drain_meeting_output_preserves_non_info_words() {
        use std::process::{Command, Stdio};

        let mut app = App::new("simard-ooda.service".to_string());

        let mut child = Command::new("sh")
            .args(["-c", "printf 'INFORMATION about meeting\\n'; sleep 5"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        {
            use std::os::unix::io::AsRawFd;
            let fd = stdout.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }

        app.meeting_stdout = Some(std::io::BufReader::new(stdout));
        app.meeting_child = Some(child);
        app.meeting_status = MeetingStatus::Running;

        for _ in 0..200 {
            app.drain_meeting_output();
            if !app.meeting_output.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if let Some(ref mut c) = app.meeting_child {
            let _ = c.kill();
            let _ = c.wait();
        }

        assert!(
            app.meeting_output.iter().any(|l| l.contains("INFORMATION")),
            "'INFORMATION' (without space padding) should NOT be filtered"
        );
    }

    // ── BUG C: Cursor position basics ──────────────────────────────

    #[test]
    fn cursor_position_starts_at_zero() {
        let app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.cursor_position, 0);
    }

    // ── BUG C: Cursor movement in meeting mode ─────────────────────

    #[test]
    fn arrow_right_moves_cursor_in_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello".to_string();
        app.cursor_position = 2;

        app.handle_key(key_code(KeyCode::Right));

        assert_eq!(app.active_tab, Tab::Meeting, "tab should not change");
        assert_eq!(app.cursor_position, 3, "cursor should move right");
    }

    #[test]
    fn arrow_left_moves_cursor_in_meeting() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello".to_string();
        app.cursor_position = 3;

        app.handle_key(key_code(KeyCode::Left));

        assert_eq!(app.active_tab, Tab::Meeting, "tab should not change");
        assert_eq!(app.cursor_position, 2, "cursor should move left");
    }

    #[test]
    fn cursor_left_at_zero_stays() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hi".to_string();
        app.cursor_position = 0;

        app.handle_key(key_code(KeyCode::Left));

        assert_eq!(app.cursor_position, 0, "cursor should not underflow");
        assert_eq!(app.active_tab, Tab::Meeting);
    }

    #[test]
    fn cursor_right_at_end_stays() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hi".to_string();
        app.cursor_position = 2;

        app.handle_key(key_code(KeyCode::Right));

        assert_eq!(
            app.cursor_position, 2,
            "cursor should not exceed input length"
        );
    }

    // ── BUG C: Cursor-aware insert and delete ──────────────────────

    #[test]
    fn cursor_insert_at_position() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hllo".to_string();
        app.cursor_position = 1;

        app.handle_key(key('e'));

        assert_eq!(
            app.meeting_input, "hello",
            "char should insert at cursor position"
        );
        assert_eq!(app.cursor_position, 2, "cursor should advance after insert");
    }

    #[test]
    fn cursor_backspace_at_position() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello".to_string();
        app.cursor_position = 3;

        app.handle_key(key_code(KeyCode::Backspace));

        assert_eq!(
            app.meeting_input, "helo",
            "backspace should remove char before cursor"
        );
        assert_eq!(
            app.cursor_position, 2,
            "cursor should decrement after backspace"
        );
    }

    #[test]
    fn cursor_insert_unicode_char() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hllo".to_string();
        app.cursor_position = 1;

        app.handle_key(key('é'));

        assert_eq!(
            app.meeting_input, "héllo",
            "unicode insert should work correctly"
        );
        assert_eq!(
            app.cursor_position, 2,
            "cursor should advance by 1 char position"
        );
    }

    // ── BUG C: Cursor reset on send/stop ───────────────────────────

    #[test]
    fn cursor_resets_on_send() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "test".to_string();
        app.cursor_position = 3;

        app.handle_key(key_code(KeyCode::Enter));

        assert_eq!(app.cursor_position, 0, "cursor should reset after send");
        assert!(app.meeting_input.is_empty());
    }

    #[test]
    fn cursor_resets_on_stop() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.cursor_position = 5;

        app.handle_key(key_code(KeyCode::Esc));

        assert_eq!(
            app.cursor_position, 0,
            "cursor should reset on meeting stop"
        );
    }

    // ── BUG C: Home/End cursor movement ────────────────────────────

    #[test]
    fn home_end_move_cursor() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;
        app.meeting_input = "hello".to_string();
        app.cursor_position = 3;

        app.handle_key(key_code(KeyCode::Home));
        assert_eq!(app.cursor_position, 0, "Home should move cursor to start");

        app.cursor_position = 0;
        app.handle_key(key_code(KeyCode::End));
        assert_eq!(
            app.cursor_position, 5,
            "End should move cursor to end of input"
        );
    }

    // ── BUG C: Tab/Shift+Tab navigation ────────────────────────────

    #[test]
    fn tab_key_cycles_forward() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        app.handle_key(key_code(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Goals, "Tab should cycle forward");
    }

    #[test]
    fn shift_tab_cycles_backward() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Goals;

        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(
            app.active_tab,
            Tab::Overview,
            "Shift+Tab should cycle backward"
        );
    }

    #[test]
    fn tab_key_cycles_during_meeting_running() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.active_tab = Tab::Meeting;
        app.meeting_status = MeetingStatus::Running;

        app.handle_key(key_code(KeyCode::Tab));
        assert_eq!(
            app.active_tab,
            Tab::Stats,
            "Tab should cycle forward even when meeting is running"
        );
    }

    // ── BUG C: Ctrl+digit tab switching ────────────────────────────

    #[test]
    fn tab_from_key_ctrl_digit() {
        assert_eq!(
            Tab::from_key(&ctrl_key('1')),
            Some(Tab::Overview),
            "Ctrl+1 should switch to Overview"
        );
        assert_eq!(
            Tab::from_key(&ctrl_key('5')),
            Some(Tab::Meeting),
            "Ctrl+5 should switch to Meeting"
        );
    }
}
