//! Overseer tab: the acting Overseer meta-loop's recent activity (#2419).
//!
//! Renders the same honest data the dashboard **Overseer** tab and `simard
//! status` show — the steward status line, the operator-thread rows, and a
//! newest-first activity timeline — read from the one durable
//! [`crate::status::StatusSnapshot`] (its `overseer` section). Disabled,
//! observing-with-zero-interventions, and absent all render as plain one-liners.

use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use simard::overseer::activity::{
    OverseerActivity, human_cadence, humanize_tick, humanize_tick_details,
};
use simard::status::{Availability, Freshness, SectionEnvelope};

/// Render the Overseer tab content within the given area.
pub fn draw(f: &mut ratatui::Frame, app: &crate::app::App, area: ratatui::layout::Rect) {
    let _ = app;

    let snapshot = simard::status::assemble(&simard::status::provider::AssembleOptions::default());
    let lines: Vec<Line> = render_lines(&snapshot.overseer)
        .into_iter()
        .map(Line::from)
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Overseer"))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

/// Build the plain-text lines for the Overseer pane from the section envelope.
///
/// Pure and hermetic so the pane is unit-testable without a live daemon: every
/// honest state (present/disabled/observing/absent) maps to a truthful line.
fn render_lines(env: &SectionEnvelope<OverseerActivity>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // Absent / error states: a single honest line, never a blank pane.
    if !matches!(env.availability, Availability::Ok) || env.data.is_none() {
        let note = env
            .note
            .clone()
            .unwrap_or_else(|| "Overseer: no ticks recorded yet".to_string());
        out.push(note);
        out.push(String::new());
        out.push(
            "The steward starts recording activity after the daemon is redeployed.".to_string(),
        );
        return out;
    }

    let a = env.data.as_ref().expect("data present");
    let stale = if env.freshness == Freshness::Stale {
        "  (stale)"
    } else {
        ""
    };

    // Status line + cadence + identity + last tick.
    out.push(format!("Overseer: {}{}", a.status_summary(), stale));
    out.push(format!(
        "Runs every {}  ·  acting as {}",
        human_cadence(a.cadence_secs),
        a.author_login
    ));
    out.push(format!(
        "Last check: {}",
        a.last_tick_at.as_deref().unwrap_or("never")
    ));
    out.push(String::new());

    // Operator threads.
    out.push("Operator threads:".to_string());
    if a.threads.is_empty() {
        out.push("  (only the steward loop is running)".to_string());
    } else {
        for t in &a.threads {
            out.push(format!(
                "  {:<16} {}  ·  last {}  ·  next {}  ·  {}",
                t.id,
                if t.enabled { "on" } else { "off" },
                t.last_run.as_deref().unwrap_or("—"),
                t.next_due.as_deref().unwrap_or("—"),
                t.health,
            ));
        }
    }
    out.push(String::new());

    // Recent activity, newest-first.
    out.push("Recent activity:".to_string());
    if a.recent.is_empty() {
        out.push("  enabled and observing — 0 interventions so far".to_string());
    } else {
        for r in a.recent.iter().take(RECENT_ROWS) {
            out.push(format!("  {}  {}", r.timestamp, humanize_tick(&r.report)));
            // WHAT it observed + WHAT it did, beneath the summary (issue #21).
            let details = humanize_tick_details(&r.report);
            let shown = details.len().min(DETAIL_ROWS);
            for d in details.iter().take(DETAIL_ROWS) {
                out.push(format!("      {d}"));
            }
            if details.len() > shown {
                out.push(format!("      … {} more", details.len() - shown));
            }
        }
        if a.recent.len() > RECENT_ROWS {
            out.push(format!(
                "  … {} older tick(s) retained",
                a.recent.len() - RECENT_ROWS
            ));
        }
    }

    out
}

/// How many recent-activity rows the pane shows before summarizing the rest.
const RECENT_ROWS: usize = 30;

/// How many per-tick detail lines the pane shows before summarizing the rest.
const DETAIL_ROWS: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use simard::overseer::OverseerTickReport;
    use simard::overseer::activity::{OverseerActivityRecord, OverseerThreadStatus};

    fn joined(env: &SectionEnvelope<OverseerActivity>) -> String {
        render_lines(env).join("\n")
    }

    fn overseer_thread() -> OverseerThreadStatus {
        OverseerThreadStatus {
            id: "overseer".to_string(),
            enabled: true,
            last_run: Some("2026-07-05T15:30:00Z".to_string()),
            next_due: Some("2026-07-05T15:45:00Z".to_string()),
            last_success: Some(true),
            consecutive_errors: 0,
            backoff_until: None,
            health: "ok".to_string(),
        }
    }

    fn record(problems: usize, issues_filed: usize, held: usize) -> OverseerActivityRecord {
        OverseerActivityRecord {
            timestamp: "2026-07-05T15:30:00Z".to_string(),
            enabled: true,
            report: OverseerTickReport {
                problems,
                issues_filed,
                held,
                ..OverseerTickReport::default()
            },
            problem_entries: Vec::new(),
        }
    }

    #[test]
    fn renders_threads_and_a_sample_intervention() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        feed.push_record(record(2, 1, 1));
        let text = joined(&SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        ));

        assert!(
            text.contains("Operator threads:"),
            "missing threads header:\n{text}"
        );
        // The overseer thread row with its health word.
        assert!(
            text.contains("overseer"),
            "missing overseer thread row:\n{text}"
        );
        assert!(text.contains("ok"), "missing thread health:\n{text}");
        // The sample intervention appears in the timeline.
        assert!(
            text.contains("Recent activity:"),
            "missing recent header:\n{text}"
        );
        assert!(
            text.contains("filed 1 issue"),
            "missing intervention:\n{text}"
        );
    }

    #[test]
    fn renders_zero_interventions_honestly() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        feed.push_record(record(2, 0, 0));
        let text = joined(&SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        ));
        assert!(
            text.contains("0 interventions"),
            "an enabled-but-idle overseer must render '0 interventions':\n{text}"
        );
    }

    #[test]
    fn renders_disabled_state() {
        let feed = OverseerActivity {
            enabled: false,
            cadence_secs: 900,
            ..OverseerActivity::default()
        };
        let mut env = SectionEnvelope::live(feed, None);
        env.note = Some("Overseer: disabled".to_string());
        let text = joined(&env);
        assert!(
            text.to_lowercase().contains("disabled"),
            "a disabled overseer must say 'disabled':\n{text}"
        );
    }

    #[test]
    fn renders_absent_state_without_blank_pane() {
        let env: SectionEnvelope<OverseerActivity> =
            SectionEnvelope::absent("Overseer: no ticks recorded yet");
        let text = joined(&env);
        assert!(
            text.contains("no ticks recorded yet"),
            "absent must render an honest one-liner, not a blank pane:\n{text}"
        );
    }

    // ── issue #21: informative detail lines under each tick summary ────────

    fn record_with_details(
        observed_details: Vec<String>,
        action_details: Vec<String>,
    ) -> OverseerActivityRecord {
        OverseerActivityRecord {
            timestamp: "2026-07-05T15:30:00Z".to_string(),
            enabled: true,
            report: OverseerTickReport {
                problems: 2,
                observed_details,
                action_details,
                ..OverseerTickReport::default()
            },
            problem_entries: Vec::new(),
        }
    }

    #[test]
    fn renders_observed_and_action_detail_lines_beneath_the_summary() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        feed.push_record(record_with_details(
            vec!["distillation parse-failure rate 34%".to_string()],
            vec!["did: merged PR rysweet/Simard#42".to_string()],
        ));
        let text = joined(&SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        ));

        // The existing summary one-liner is preserved …
        assert!(
            text.contains("saw 2 problems"),
            "the summary count line must remain:\n{text}"
        );
        // … and the SPECIFIC observed value now appears beneath it …
        assert!(
            text.contains("34%"),
            "the Overseer pane must show WHAT was observed (concrete value):\n{text}"
        );
        // … along with the concrete action taken.
        assert!(
            text.contains("rysweet/Simard#42"),
            "the Overseer pane must show WHAT it did (concrete PR):\n{text}"
        );
    }

    #[test]
    fn caps_detail_lines_per_tick_and_summarises_the_overflow() {
        let many: Vec<String> = (0..30).map(|i| format!("did: action-marker-{i}")).collect();
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            ..OverseerActivity::default()
        };
        feed.push_record(record_with_details(vec![], many));
        let text = joined(&SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        ));

        assert!(
            text.contains("action-marker-0"),
            "the first detail line must be shown:\n{text}"
        );
        assert!(
            text.to_lowercase().contains("more"),
            "when a tick has more detail lines than the pane shows, the overflow \
             must be summarised (e.g. '… N more'):\n{text}"
        );
    }
}
