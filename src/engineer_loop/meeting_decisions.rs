//! Meeting decision/handoff carry-over and architecture gap helpers.

use std::path::Path;

use crate::error::SimardResult;
use std::fs;

use crate::error::SimardError;
use crate::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};

use super::MAX_CARRIED_MEETING_DECISIONS;

pub(crate) fn load_carried_meeting_decisions(state_root: &Path) -> SimardResult<Vec<String>> {
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let mut carried = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter_map(|record| match is_meeting_decision_record(&record.value) {
            true => Some(record.value),
            false => None,
        })
        .collect::<Vec<_>>();

    // Use find_oldest_unprocessed_handoff (FIFO queue) instead of
    // load_meeting_handoff (newest-by-filename). Issue #1985 / #1649.
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match crate::meeting_facilitator::find_oldest_unprocessed_handoff(&handoff_dir) {
        Ok(Some(handoff_path)) => {
            let raw = fs::read_to_string(&handoff_path).map_err(|e| SimardError::ArtifactIo {
                path: handoff_path.clone(),
                reason: format!("reading handoff: {e}"),
            })?;
            let handoff: crate::meeting_facilitator::MeetingHandoff = serde_json::from_str(&raw)
                .map_err(|e| SimardError::ArtifactIo {
                    path: handoff_path.clone(),
                    reason: format!("parsing handoff JSON: {e}"),
                })?;

            // Extract decisions and action items from the handoff.
            for d in &handoff.decisions {
                carried.push(format!(
                    "meeting handoff — {}: {} (rationale: {})",
                    handoff.topic, d.description, d.rationale,
                ));
            }
            for a in &handoff.action_items {
                carried.push(format!(
                    "meeting handoff — {} action: {} (owner: {}, priority: {})",
                    handoff.topic, a.description, a.owner, a.priority,
                ));
            }
            // Issue #1954: surface `next_owner` and `artifacts[]`.
            if let Some(ref owner) = handoff.next_owner {
                carried.push(format!(
                    "meeting handoff — {} next_owner: {}",
                    handoff.topic, owner,
                ));
            }
            for art in &handoff.artifacts {
                let desc = art
                    .description
                    .as_deref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                carried.push(format!(
                    "meeting handoff — {} artifact [{}]: {}{}",
                    handoff.topic, art.kind, art.uri_or_path, desc,
                ));
            }

            // Issue #1985: derive meeting_id and try to load the
            // per-meeting bundle (transcript.json + meeting_handoff.md).
            let meeting_id = if handoff.meeting_id.is_empty() {
                crate::meeting_facilitator::derive_meeting_id(&handoff.started_at, &handoff.topic)
            } else {
                handoff.meeting_id.clone()
            };

            match crate::meeting_facilitator::load_meeting_bundle(&meeting_id) {
                Ok(Some(bundle)) => {
                    let bundle_dir = crate::meeting_facilitator::meeting_bundle_dir(&meeting_id);
                    carried.push(format!(
                        "meeting handoff — {} bundle: {}",
                        handoff.topic,
                        bundle_dir.display(),
                    ));

                    if !bundle.transcript.is_empty() {
                        carried.push(format!(
                            "meeting handoff — {} transcript: {} lines from {}",
                            handoff.topic,
                            bundle.transcript.len(),
                            crate::meeting_facilitator::bundle_transcript_path(&meeting_id)
                                .display(),
                        ));
                    }

                    if bundle.markdown_report.is_some() {
                        carried.push(format!(
                            "meeting handoff — {} markdown report: {}",
                            handoff.topic,
                            crate::meeting_facilitator::bundle_markdown_path(&meeting_id).display(),
                        ));
                    }
                }
                Ok(None) => {
                    tracing::info!(
                        meeting_id = %meeting_id,
                        "no per-meeting bundle found; legacy handoff without bundle"
                    );
                }
                Err(e) => {
                    tracing::info!(
                        meeting_id = %meeting_id,
                        error = %e,
                        "failed to load per-meeting bundle; continuing with handoff only"
                    );
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!(
                "[simard] warning: failed to scan for unprocessed handoffs in '{}': {e}",
                handoff_dir.display()
            );
        }
    }

    if carried.len() > MAX_CARRIED_MEETING_DECISIONS {
        carried = carried.split_off(carried.len() - MAX_CARRIED_MEETING_DECISIONS);
    }

    Ok(carried)
}

pub(crate) fn is_meeting_decision_record(value: &str) -> bool {
    [
        "agenda=",
        "updates=",
        "decisions=",
        "risks=",
        "next_steps=",
        "open_questions=",
        "goals=",
    ]
    .into_iter()
    .all(|fragment| value.contains(fragment))
}

pub(crate) fn architecture_gap_summary(repo_root: &Path) -> SimardResult<String> {
    let architecture_path = repo_root.join("Specs/ProductArchitecture.md");
    let probe_path = repo_root.join("src/bin/simard_operator_probe.rs");
    let runtime_contracts_path = repo_root.join("docs/reference/runtime-contracts.md");

    let architecture_exists = architecture_path.is_file();
    let probe_exists = probe_path.is_file();
    let operator_surface = if probe_exists {
        let source = fs::read_to_string(&probe_path).map_err(|error| SimardError::ArtifactIo {
            path: probe_path.clone(),
            reason: error.to_string(),
        })?;
        if source.contains("\"engineer-loop-run\"") {
            "operator probe already exposes engineer-loop-run".to_string()
        } else if source.contains("\"terminal-run\"") {
            "operator probe exposes terminal-run but not a repo-grounded engineer-loop-run"
                .to_string()
        } else {
            "operator probe exists but does not yet expose a terminal engineer loop".to_string()
        }
    } else {
        "operator probe file is missing".to_string()
    };

    let review_trace = if runtime_contracts_path.is_file() {
        "runtime contracts docs mention operator/runtime public surfaces and prior spec reconciliation"
    } else {
        "runtime contracts docs are absent, so gap trace comes only from code and architecture files"
    };

    Ok(match architecture_exists {
        true => format!(
            "Compared current operator/runtime surfaces against Specs/ProductArchitecture.md: the architecture requires inspect -> act -> verify -> persist in engineer mode, and {operator_surface}; {review_trace}."
        ),
        false => format!(
            "Repository is missing Specs/ProductArchitecture.md, so the gap trace falls back to current operator/runtime surfaces only; {operator_surface}; {review_trace}."
        ),
    })
}
