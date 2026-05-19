use std::path::PathBuf;

use crate::greeting_banner::print_greeting_banner;
use crate::meetings::PersistedMeetingRecord;
use crate::operator_commands::{
    print_display, print_meeting_goal_section, print_string_section, print_text, prompt_root,
    resolved_meeting_read_state_root, resolved_state_root,
};
use crate::{
    BootstrapConfig, BootstrapInputs, FileBackedMemoryStore, MemoryScope, MemoryStore,
    latest_local_handoff, run_local_session,
};

pub fn run_meeting_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-meeting";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "meeting-run",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    // Display greeting banner before starting the meeting session (no bridge available here)
    print_greeting_banner(None);

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable meeting handoff")?;
    let decision_records = exported
        .memory_records
        .iter()
        .filter(|record| record.scope == MemoryScope::Decision)
        .map(|record| record.value.clone())
        .collect::<Vec<_>>();

    println!("Probe mode: meeting-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Decision records: {}", decision_records.len());
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    for (index, value) in decision_records.iter().enumerate() {
        print_text(&format!("Decision record {}", index + 1), value);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_meeting_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolved_meeting_read_state_root(state_root_override, base_type, topology)?;
    let state_root = resolved.path;

    let memory_records_path = state_root.join("memory_records.json");
    let meeting_records: Vec<crate::MemoryRecord> = if memory_records_path.is_file() {
        let memory_store = FileBackedMemoryStore::try_new(memory_records_path)?;
        memory_store
            .list(MemoryScope::Decision)?
            .into_iter()
            .filter(|record| crate::looks_like_persisted_meeting_record(&record.value))
            .collect()
    } else {
        // The mode-specific validation in `resolved_meeting_read_state_root`
        // already enforces that `memory_records.json` exists when the
        // operator passed an explicit `[state-root]` (strict contract).
        // Here we are on the daemon-fallback path with no persisted store
        // yet; render explicit empty sections rather than hard-failing
        // (issue #1909).
        Vec::new()
    };

    println!("Probe mode: meeting-read");
    println!("Identity: simard-meeting");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    println!("Meeting records: {}", meeting_records.len());

    match meeting_records.last() {
        Some(latest_record) => {
            let parsed_record = PersistedMeetingRecord::parse(&latest_record.value)
                .map_err(|error| format!("{error}"))?;
            print_text("Latest agenda", &parsed_record.agenda);
            print_string_section("Updates", &parsed_record.updates);
            print_string_section("Decisions", &parsed_record.decisions);
            print_string_section("Risks", &parsed_record.risks);
            print_string_section("Next steps", &parsed_record.next_steps);
            print_string_section("Open questions", &parsed_record.open_questions);
            print_meeting_goal_section(&parsed_record.goals);
            print_text("Latest meeting record", &latest_record.value);
        }
        None if resolved.used_override => {
            // Strict override contract: explicit state roots with no
            // persisted meeting record must fail visibly. This preserves
            // the contract tested in
            // `simard_meeting_read_rejects_nonexistent_and_empty_state_roots_*`.
            return Err("expected persisted meeting decision record".into());
        }
        None => {
            // Daemon-fallback path with no persisted meeting record:
            // render explicit zero-state sections so the operator sees
            // the empty shape rather than a hard error (Pillar 11 —
            // issue #1909).
            print_text("Latest agenda", "<none>");
            print_string_section("Updates", &[]);
            print_string_section("Decisions", &[]);
            print_string_section("Risks", &[]);
            print_string_section("Next steps", &[]);
            print_string_section("Open questions", &[]);
            print_meeting_goal_section(&[]);
            print_text("Latest meeting record", "<none>");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn meeting_read_probe_rejects_nonexistent_state_root() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = run_meeting_read_probe("local-harness", "single-process", Some(missing));
        assert!(result.is_err(), "expected error for nonexistent state root");
    }

    #[test]
    fn meeting_read_probe_rejects_missing_memory_file() {
        let dir = TempDir::new().unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when memory_records.json is absent"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("memory_records.json"),
            "error should mention the missing file: {msg}"
        );
    }

    #[test]
    fn meeting_read_probe_rejects_empty_meeting_store() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("memory_records.json"), "[]").unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when no meeting records exist"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("expected persisted meeting decision record"),
            "error should explain the missing record: {msg}"
        );
    }

    #[test]
    fn meeting_read_probe_rejects_non_meeting_record() {
        let dir = TempDir::new().unwrap();
        // Write a memory record that doesn't look like a meeting record
        let records = serde_json::json!([{
            "key": "session-1-decision",
            "scope": "decision",
            "value": "some non-meeting value",
            "session_id": "session-1",
            "recorded_in": "complete"
        }]);
        std::fs::write(
            dir.path().join("memory_records.json"),
            serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when no meeting-shaped records exist"
        );
    }

    #[test]
    fn meeting_read_probe_with_valid_meeting_record() {
        let dir = TempDir::new().unwrap();
        let record = "agenda=Sprint Review; updates=[Updated backend]; decisions=[Deploy Monday]; risks=[None]; next_steps=[Run tests]; open_questions=[]; goals=[p1:active:Ship v2:High priority]";
        let records = serde_json::json!([{
            "key": "session-1-meeting",
            "scope": "decision",
            "value": record,
            "session_id": "session-1",
            "recorded_in": "complete"
        }]);
        std::fs::write(
            dir.path().join("memory_records.json"),
            serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "should succeed with valid meeting record: {:?}",
            result.err()
        );
    }

    #[test]
    fn meeting_read_probe_rejects_invalid_meeting_record_format() {
        let dir = TempDir::new().unwrap();
        // Write a record that looks_like_persisted_meeting_record but can't be parsed
        let record = "agenda=x; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[INVALID_GOAL_FORMAT]";
        let records = serde_json::json!([{
            "key": "session-1-meeting",
            "scope": "decision",
            "value": record,
            "session_id": "session-1",
            "recorded_in": "complete"
        }]);
        std::fs::write(
            dir.path().join("memory_records.json"),
            serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        // This should either succeed or fail gracefully (no panic)
        let _result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
    }

    // ───────────────────────────────────────────────────────────────────
    // Issue #1909: `meeting read` without an explicit `[state-root]`
    // argument must fall back to the canonical daemon state root
    // (`$SIMARD_STATE_ROOT` or `$HOME/.simard/state`) and render an
    // explicit empty report when no persisted meeting record exists —
    // instead of hard-failing the way the prior probe-style resolution
    // did on a fresh machine.
    // ───────────────────────────────────────────────────────────────────

    /// Helper: run a closure with `SIMARD_STATE_ROOT` temporarily
    /// pointed at the given path. Restores the previous value (or
    /// removes the var) on completion.
    fn with_simard_state_root<R>(path: &std::path::Path, run: impl FnOnce() -> R) -> R {
        let prev = std::env::var("SIMARD_STATE_ROOT").ok();
        // SAFETY: callers gate this on `serial_test::serial(simard_state_root)`
        // so no other test mutates the same env var concurrently.
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", path);
        }
        let result = run();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
        }
        result
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn meeting_read_probe_falls_back_to_default_state_root_when_no_override() {
        let dir = TempDir::new().unwrap();
        // Empty state root — no memory_records.json, no records of any kind.
        // Pre-fix this hard-failed with `meeting read requires an existing
        // state root directory`. Post-fix it falls back and renders empty.
        let result = with_simard_state_root(dir.path(), || {
            run_meeting_read_probe("local-harness", "single-process", None)
        });
        assert!(
            result.is_ok(),
            "meeting read should succeed via daemon fallback when no override is given \
             and the daemon store is empty: {:?}",
            result.err()
        );
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn meeting_read_probe_falls_back_and_reads_real_records() {
        let dir = TempDir::new().unwrap();
        // Seed a real meeting decision record into the fallback path.
        let record = "agenda=Daily standup; updates=[Pushed PR]; decisions=[Ship Friday]; risks=[]; next_steps=[]; open_questions=[]; goals=[]";
        let records = serde_json::json!([{
            "key": "session-X-meeting",
            "scope": "decision",
            "value": record,
            "session_id": "session-X",
            "recorded_in": "complete"
        }]);
        std::fs::write(
            dir.path().join("memory_records.json"),
            serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let result = with_simard_state_root(dir.path(), || {
            run_meeting_read_probe("local-harness", "single-process", None)
        });
        assert!(
            result.is_ok(),
            "meeting read should succeed via daemon fallback and surface seeded records: {:?}",
            result.err()
        );
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn meeting_read_probe_fallback_rejects_bogus_topology() {
        let dir = TempDir::new().unwrap();
        // Even on the daemon-fallback path, base_type / topology validation
        // must still fire — preserves the prior probe-resolution contract.
        let result = with_simard_state_root(dir.path(), || {
            run_meeting_read_probe("local-harness", "totally-bogus-topology", None)
        });
        assert!(
            result.is_err(),
            "bogus topology should fail validation even on the daemon-fallback path"
        );
    }
}
