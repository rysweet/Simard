//! Prep scheduling.
//!
//! Turns the prep steps across every dish into an ordered, back-timed prep
//! schedule that finishes at the event's service time. The model is a single
//! sequential cook (one critical path): tasks are ordered deterministically and
//! assigned offsets working backwards from service, so each task carries the
//! minutes-before-service it must start and — when the brief supplies a service
//! time — a wall-clock start time.

use std::fmt::Write as _;

use super::brief::{MenuBrief, format_hhmm, parse_hhmm};

/// One scheduled prep task.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledTask {
    /// 1-based execution order (earliest first).
    pub order: u32,
    pub dish: String,
    pub task: String,
    pub station: String,
    pub minutes: f64,
    /// Minutes before service this task must start.
    pub start_offset_min: f64,
    /// Minutes before service this task finishes.
    pub end_offset_min: f64,
    /// Wall-clock start time `"HH:MM"`, when the brief has a service time.
    pub start_clock: Option<String>,
}

/// The full prep schedule.
#[derive(Debug, Clone)]
pub struct PrepSchedule {
    pub tasks: Vec<ScheduledTask>,
    /// Total prep minutes (the single-cook critical path).
    pub total_minutes: f64,
    /// Service clock time `"HH:MM"`, if the brief supplied one.
    pub service_time: Option<String>,
}

impl PrepSchedule {
    /// Earliest start offset (how many minutes before service prep must begin).
    pub fn earliest_start_offset(&self) -> f64 {
        self.tasks
            .first()
            .map(|t| t.start_offset_min)
            .unwrap_or(0.0)
    }

    /// True when task offsets are monotonically non-increasing toward service
    /// and every task fits within the total critical path — i.e. the schedule
    /// is internally consistent.
    pub fn is_ordered(&self) -> bool {
        let mut prev_end = self.total_minutes;
        for t in &self.tasks {
            // Each task starts exactly where the previous ended (sequential).
            if (t.start_offset_min - prev_end).abs() > 1e-6 {
                return false;
            }
            if t.end_offset_min < -1e-9 || t.start_offset_min < t.end_offset_min - 1e-9 {
                return false;
            }
            prev_end = t.end_offset_min;
        }
        true
    }
}

/// A prep task collected from a dish, before scheduling.
struct RawTask {
    dish: String,
    task: String,
    station: String,
    minutes: f64,
}

/// Build the prep schedule for a brief.
///
/// Tasks are grouped by station and ordered station-major so a cook works
/// through one station at a time; within a station, tasks keep dish/brief
/// order. Offsets are then assigned backwards from service so the last task
/// ends exactly at service time.
pub fn build_schedule(brief: &MenuBrief) -> PrepSchedule {
    let mut raw: Vec<RawTask> = Vec::new();
    for dish in &brief.dishes {
        for step in &dish.prep {
            raw.push(RawTask {
                dish: dish.name.clone(),
                task: step.task.clone(),
                station: step.station.clone().unwrap_or_else(|| "prep".to_string()),
                minutes: step.minutes,
            });
        }
    }

    // Deterministic station-major order: sort by station, preserving the
    // original within-station sequence via a stable sort on the station key.
    raw.sort_by(|a, b| a.station.cmp(&b.station));

    let total_minutes: f64 = raw.iter().map(|t| t.minutes).sum();
    let service_min = brief.service_time.as_deref().and_then(parse_hhmm);

    // Assign offsets forward through the ordered list; the first task starts at
    // `total_minutes` before service and each subsequent task follows on.
    let mut cursor = total_minutes; // minutes before service at which the next task starts
    let mut tasks = Vec::with_capacity(raw.len());
    for (i, t) in raw.into_iter().enumerate() {
        let start_offset = cursor;
        let end_offset = cursor - t.minutes;
        let start_clock = service_min.map(|svc| {
            let start_abs = svc as i64 - start_offset.round() as i64;
            format_hhmm(start_abs)
        });
        tasks.push(ScheduledTask {
            order: (i + 1) as u32,
            dish: t.dish,
            task: t.task,
            station: t.station,
            minutes: t.minutes,
            start_offset_min: start_offset,
            end_offset_min: end_offset,
            start_clock,
        });
        cursor = end_offset;
    }

    PrepSchedule {
        tasks,
        total_minutes,
        service_time: brief.service_time.clone(),
    }
}

impl PrepSchedule {
    /// Render the schedule as CSV.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("order,dish,task,station,minutes,start_offset_min,start_clock\n");
        for t in &self.tasks {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{}",
                t.order,
                csv_field(&t.dish),
                csv_field(&t.task),
                csv_field(&t.station),
                trim(t.minutes),
                trim(t.start_offset_min),
                t.start_clock.clone().unwrap_or_default(),
            );
        }
        out
    }
}

fn trim(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() && r.abs() < 9_007_199_254_740_992.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Escape a CSV field for safe spreadsheet consumption.
///
/// Two protections, since every field here is untrusted brief text:
/// - **Formula injection (CWE-1236):** a leading `=`, `+`, `-`, `@`, tab, or CR
///   makes a spreadsheet evaluate the cell as a formula, so such a field is
///   prefixed with a single quote to force it to be read as literal text.
/// - **RFC-4180 quoting:** a field containing a comma, quote, CR, or newline is
///   wrapped in quotes with any embedded quotes doubled.
fn csv_field(s: &str) -> String {
    let needs_formula_guard = s
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
    let guarded = if needs_formula_guard {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::brief::MenuBrief;

    const BRIEF: &str = r#"{
        "event":"Dinner","guests":10,"service_time":"19:00",
        "dishes":[
            {"name":"Soup","course":"starter",
             "ingredients":[{"name":"Squash","qty_per_serving":100,"unit":"g"}],
             "prep":[{"task":"Roast squash","minutes":40,"station":"oven"},
                     {"task":"Blend","minutes":10,"station":"prep"}]},
            {"name":"Roast","course":"main",
             "ingredients":[{"name":"Beef","qty_per_serving":180,"unit":"g"}],
             "prep":[{"task":"Sear beef","minutes":20,"station":"stove"}]}
        ]}"#;

    fn schedule() -> PrepSchedule {
        build_schedule(&MenuBrief::from_json_bytes(BRIEF.as_bytes()).unwrap())
    }

    #[test]
    fn total_minutes_is_sum_of_steps() {
        let s = schedule();
        assert!((s.total_minutes - 70.0).abs() < 1e-9);
    }

    #[test]
    fn first_task_starts_at_total_before_service_and_last_ends_at_service() {
        let s = schedule();
        assert!((s.earliest_start_offset() - 70.0).abs() < 1e-9);
        let last = s.tasks.last().unwrap();
        assert!((last.end_offset_min - 0.0).abs() < 1e-9);
    }

    #[test]
    fn schedule_is_ordered_and_sequential() {
        let s = schedule();
        assert!(s.is_ordered());
        // Orders are 1..=n contiguous.
        for (i, t) in s.tasks.iter().enumerate() {
            assert_eq!(t.order, (i + 1) as u32);
        }
    }

    #[test]
    fn clock_times_are_present_when_service_time_set() {
        let s = schedule();
        // First task (oven, 70 min before 19:00) starts at 17:50.
        assert_eq!(s.tasks[0].start_clock.as_deref(), Some("17:50"));
    }

    #[test]
    fn no_service_time_leaves_clock_empty() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}],
                "prep":[{"task":"chop","minutes":5}]}]}"#,
        )
        .unwrap();
        let s = build_schedule(&brief);
        assert!(s.tasks[0].start_clock.is_none());
        assert!(s.is_ordered());
    }

    #[test]
    fn empty_prep_yields_empty_ordered_schedule() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#,
        )
        .unwrap();
        let s = build_schedule(&brief);
        assert!(s.tasks.is_empty());
        assert_eq!(s.total_minutes, 0.0);
        assert!(s.is_ordered());
    }

    #[test]
    fn csv_header_is_stable() {
        let s = schedule();
        assert!(
            s.to_csv()
                .starts_with("order,dish,task,station,minutes,start_offset_min,start_clock")
        );
    }

    #[test]
    fn csv_neutralizes_formula_injection_in_task_fields() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":2,"dishes":[{"name":"=evil","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}],
                "prep":[{"task":"@calc","minutes":5,"station":"-cmd"}]}]}"#,
        )
        .unwrap();
        let csv = build_schedule(&brief).to_csv();
        assert!(csv.contains("'=evil"), "dish not defused: {csv}");
        assert!(csv.contains("'@calc"), "task not defused: {csv}");
        assert!(csv.contains("'-cmd"), "station not defused: {csv}");
    }
}
