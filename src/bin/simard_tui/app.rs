//! Application state and event handling for the TUI.
//!
//! Owns the active tab, cached daemon info and goal board, and handles
//! key events. Refresh logic uses dual rates: 2s for /proc + goals,
//! 10s for systemctl (to avoid hammering D-Bus).

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

/// Main application state.
pub struct App {
    pub active_tab: Tab,
    pub daemon_info: DaemonInfo,
    pub goal_board: GoalBoard,
    pub should_quit: bool,
    pub state_root: std::path::PathBuf,
    prev_cpu_sample: Option<CpuSample>,
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
        }
    }

    /// Handle a key press event.
    ///
    /// - `q`: set `should_quit = true`
    /// - `1`–`6`: switch to the corresponding tab
    /// - Other keys: ignored
    pub fn handle_key(&mut self, c: char) {
        match c {
            'q' | 'Q' => self.should_quit = true,
            c => {
                if let Some(tab) = Tab::from_key(c) {
                    self.active_tab = tab;
                }
            }
        }
    }

    /// Refresh daemon info and goal board from system sources.
    pub fn refresh(&mut self) {
        let service = &self.daemon_info.service_name;

        let systemctl_output = std::process::Command::new("systemctl")
            .arg("show")
            .arg("-p")
            .arg("ActiveState,MainPID,ActiveEnterTimestamp,LoadState")
            .arg(service)
            .env("TZ", "UTC") // Force UTC so timestamp parsing is unambiguous
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
                            return None; // PID was reused
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

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── Key handling ────────────────────────────────────────────────

    #[test]
    fn handle_key_quit() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert!(!app.should_quit);
        app.handle_key('q');
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_tab_switch() {
        let mut app = App::new("simard-ooda.service".to_string());
        assert_eq!(app.active_tab, Tab::Overview);

        app.handle_key('2');
        assert_eq!(app.active_tab, Tab::Goals);

        app.handle_key('5');
        assert_eq!(app.active_tab, Tab::Meeting);

        app.handle_key('1');
        assert_eq!(app.active_tab, Tab::Overview);
    }

    #[test]
    fn handle_key_unknown_is_noop() {
        let mut app = App::new("simard-ooda.service".to_string());
        let tab_before = app.active_tab;
        let quit_before = app.should_quit;
        app.handle_key('z');
        assert_eq!(app.active_tab, tab_before);
        assert_eq!(app.should_quit, quit_before);
    }
}
