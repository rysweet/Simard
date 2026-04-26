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

    // Also check for unprocessed meeting handoff artifacts.
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match crate::meeting_facilitator::load_meeting_handoff(&handoff_dir) {
        Ok(Some(handoff)) if !handoff.processed => {
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
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "[simard] warning: failed to load meeting handoff from '{}': {e}",
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
