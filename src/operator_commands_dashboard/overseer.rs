//! Dashboard **Overseer** endpoint (#2419).
//!
//! `GET /api/overseer` returns the `overseer` section of the single unified
//! [`crate::status::StatusSnapshot`] — the acting Overseer meta-loop's recent
//! activity feed (last-N ticks + per-thread status) that `simard status`, the
//! Status tab, and the TUI Overseer pane also render. Reusing the one
//! `status::assemble` provider means the dedicated Overseer tab and the Status
//! tab can never diverge.
//!
//! It sits **behind** the existing `require_auth` layer in `routes.rs` — it is
//! not a new auth surface — and degrades to an `error` object at HTTP 200
//! (never a 500), matching the other dashboard panels, so the SPA can show a
//! soft banner while keeping the last good render.
//!
//! Assembly shells out (systemctl) and reads files, so it runs on a blocking
//! thread rather than the async reactor.

use axum::Json;
use serde_json::{Value, json};

use crate::status::{self, StatusSnapshot, provider::AssembleOptions};

/// `GET /api/overseer` — the Overseer activity section plus assembly metadata.
pub(crate) async fn overseer() -> Json<Value> {
    let assembled =
        tokio::task::spawn_blocking(|| status::assemble(&AssembleOptions::default())).await;

    match assembled {
        Ok(snap) => Json(overseer_response(&snap)),
        Err(e) => Json(json!({ "error": format!("overseer assembly join error: {e}") })),
    }
}

/// Shape the `/api/overseer` response body from an assembled snapshot. Pure and
/// hermetic so it is unit-testable without a runtime or a live daemon: the SPA
/// reads `section` (a serialized `SectionEnvelope<OverseerActivity>`), checking
/// `availability`/`freshness` before `data`.
pub(crate) fn overseer_response(snap: &StatusSnapshot) -> Value {
    match serde_json::to_value(&snap.overseer) {
        Ok(section) => json!({
            "schema_version": crate::overseer::activity::SCHEMA_VERSION,
            "generated_at": snap.generated_at,
            "section": section,
        }),
        Err(e) => json!({ "error": format!("serialize overseer section: {e}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::OverseerTickReport;
    use crate::overseer::activity::{
        OverseerActivity, OverseerActivityRecord, OverseerThreadStatus,
    };
    use crate::status::SectionEnvelope;

    fn record(problems: usize, issues_filed: usize, held: usize) -> OverseerActivityRecord {
        OverseerActivityRecord {
            timestamp: "2026-07-05T15:30:00Z".to_string(),
            enabled: true,
            report: OverseerTickReport {
                problems,
                issues_filed,
                held,
                duration_ms: 843,
                ..OverseerTickReport::default()
            },
            problem_entries: Vec::new(),
        }
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

    fn snapshot_with(section: SectionEnvelope<OverseerActivity>) -> StatusSnapshot {
        let mut snap = StatusSnapshot::empty();
        snap.generated_at = "2026-07-05T15:31:39Z".to_string();
        snap.overseer = section;
        snap
    }

    #[test]
    fn response_carries_threads_and_a_sample_intervention() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        // A tick that filed one issue — a concrete, surfaced intervention.
        feed.push_record(record(2, 1, 1));

        let body = overseer_response(&snapshot_with(SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        )));

        assert_eq!(body["schema_version"], 1);
        let section = &body["section"];
        assert_eq!(section["availability"], "ok");
        assert_eq!(section["freshness"], "live");
        let data = &section["data"];
        assert_eq!(data["enabled"], true);
        // The overseer thread row is present with its status.
        assert_eq!(data["threads"][0]["id"], "overseer");
        assert_eq!(data["threads"][0]["health"], "ok");
        // The sample intervention (an issue filed) is surfaced in totals + recent.
        assert_eq!(data["totals"]["issues_filed"], 1);
        assert_eq!(data["recent"][0]["report"]["issues_filed"], 1);
    }

    #[test]
    fn response_states_zero_interventions_honestly() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        // Enabled, ticked, but took no action.
        feed.push_record(record(2, 0, 0));
        assert_eq!(feed.status_summary(), "enabled, observing, 0 interventions");

        let mut env = SectionEnvelope::live(feed, Some("2026-07-05T15:30:00Z".to_string()));
        env.note = Some("Overseer: enabled, observing, 0 interventions".to_string());
        let body = overseer_response(&snapshot_with(env));

        assert_eq!(body["section"]["data"]["totals"]["issues_filed"], 0);
        assert_eq!(
            body["section"]["note"],
            "Overseer: enabled, observing, 0 interventions"
        );
    }

    #[test]
    fn response_reports_disabled_state() {
        let feed = OverseerActivity {
            enabled: false,
            cadence_secs: 900,
            ..OverseerActivity::default()
        };
        let mut env = SectionEnvelope::live(feed, None);
        env.note = Some("Overseer: disabled".to_string());
        let body = overseer_response(&snapshot_with(env));

        assert_eq!(body["section"]["data"]["enabled"], false);
        assert_eq!(body["section"]["note"], "Overseer: disabled");
    }

    #[test]
    fn response_handles_absent_section() {
        let body = overseer_response(&snapshot_with(SectionEnvelope::absent(
            "Overseer: no ticks recorded yet",
        )));
        assert_eq!(body["section"]["availability"], "unavailable");
        assert_eq!(body["section"]["freshness"], "absent");
        assert_eq!(body["section"]["note"], "Overseer: no ticks recorded yet");
        assert!(body["section"]["data"].is_null());
    }

    /// #21 — the `/api/overseer` payload must carry the structured, human-readable
    /// detail arrays so the SPA can render WHAT the Overseer observed and did,
    /// not just counts.
    #[test]
    fn response_carries_observed_and_action_details() {
        let mut feed = OverseerActivity {
            enabled: true,
            cadence_secs: 900,
            threads: vec![overseer_thread()],
            ..OverseerActivity::default()
        };
        feed.push_record(OverseerActivityRecord {
            timestamp: "2026-07-05T15:30:00Z".to_string(),
            enabled: true,
            report: OverseerTickReport {
                problems: 2,
                prs_merged: 1,
                observed_details: vec!["PR rysweet/Simard#42 is green and merge-ready".to_string()],
                action_details: vec!["did: merged PR rysweet/Simard#42".to_string()],
                ..OverseerTickReport::default()
            },
            problem_entries: Vec::new(),
        });

        let body = overseer_response(&snapshot_with(SectionEnvelope::live(
            feed,
            Some("2026-07-05T15:30:00Z".to_string()),
        )));

        let rep = &body["section"]["data"]["recent"][0]["report"];
        assert_eq!(
            rep["observed_details"][0], "PR rysweet/Simard#42 is green and merge-ready",
            "the response must surface the concrete observed detail:\n{body}"
        );
        assert_eq!(
            rep["action_details"][0], "did: merged PR rysweet/Simard#42",
            "the response must surface the concrete action detail:\n{body}"
        );
    }
}
