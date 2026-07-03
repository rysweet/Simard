//! Dashboard **Status** page endpoint (issue #2528).
//!
//! `GET /api/status/snapshot` returns the single unified
//! [`crate::status::StatusSnapshot`] the `simard status` CLI and the TUI
//! **Status** tab also render — assembled from durable, process-agnostic sources
//! (the telemetry snapshot, cost ledger, `/proc`, `systemctl`), never by
//! grepping journald. The response carries both the serialized snapshot (`data`)
//! and the canonical terminal rendering (`rendered`) so the SPA can show the
//! exact operator-approved layout with one `<pre>` while scripts consume the
//! structured form.
//!
//! Assembly shells out (systemctl / pgrep) and reads files, so it runs on a
//! blocking thread rather than the async reactor.

use axum::Json;
use serde_json::{Value, json};

use crate::status::{self, provider::AssembleOptions};

/// `GET /api/status/snapshot` — the unified status snapshot plus its terminal
/// rendering. Degrades gracefully: a serialization or join failure returns an
/// `error` object rather than a 500, matching the other dashboard panels.
pub(crate) async fn status_snapshot() -> Json<Value> {
    let assembled =
        tokio::task::spawn_blocking(|| status::assemble(&AssembleOptions::default())).await;

    match assembled {
        Ok(snap) => {
            let rendered = status::render::to_terminal(&snap);
            match serde_json::to_value(&snap) {
                Ok(data) => Json(json!({
                    "data": data,
                    "rendered": rendered,
                    "generated_at": snap.generated_at,
                })),
                Err(e) => Json(json!({ "error": format!("serialize snapshot: {e}") })),
            }
        }
        Err(e) => Json(json!({ "error": format!("status assembly join error: {e}") })),
    }
}
