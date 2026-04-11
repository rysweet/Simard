//! Integration tests for Phase 7: Remote VM Orchestration via azlin.
//!
//! All tests use a mock azlin executor — no real VMs are created.
//! Tests verify the full session lifecycle: create, deploy, PTY, transfer,
//! memory snapshot round-trip, and destroy.
#![allow(deprecated)]

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;

use simard::remote_azlin::{
    AzlinConfig, AzlinExecutor, AzlinVm, MockAzlinExecutor, azlin_create, azlin_destroy, azlin_ssh,
};
use simard::remote_session::{
    RemoteConfig, RemoteStatus, begin_transfer, create_remote_session, deploy_agent,
    destroy_session, end_transfer, establish_pty,
};
use simard::remote_transfer::{
    MemorySnapshot, export_memory_snapshot, import_memory_snapshot, load_snapshot_from_file,
};
use simard::{
    BridgeErrorPayload, CognitiveFact, CognitiveMemoryBridge, CognitiveProcedure,
    InMemoryBridgeTransport, SimardError,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn mock_executor() -> MockAzlinExecutor {
    MockAzlinExecutor::new(|args| match args.first().copied() {
        Some("create") => {
            let name = args.get(1).unwrap_or(&"unknown");
            Ok(format!("name={name}\nip=10.0.0.42\nstatus=running\n"))
        }
        Some("ssh") => Ok("ssh azureuser@10.0.0.42 -t".to_string()),
        Some("destroy") => Ok(String::new()),
        Some("scp") => Ok(String::new()),
        _ => Ok(String::new()),
    })
}

fn failing_executor(fail_on: &'static str) -> MockAzlinExecutor {
    MockAzlinExecutor::new(move |args| {
        if args.first().copied() == Some(fail_on) {
            Err(SimardError::BridgeTransportError {
                bridge: "azlin".to_string(),
                reason: format!("simulated failure on {fail_on}"),
            })
        } else {
            mock_executor().run(args)
        }
    })
}

struct MockStore {
    facts: Vec<CognitiveFact>,
    procedures: Vec<CognitiveProcedure>,
}

fn mock_bridge() -> CognitiveMemoryBridge {
    let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
        facts: vec![],
        procedures: vec![],
    })));

    let transport =
        InMemoryBridgeTransport::new("test-memory", move |method, params| match method {
            "memory.search_facts" => {
                let s = store.lock().unwrap();
                let facts: Vec<serde_json::Value> = s
                    .facts
                    .iter()
                    .map(|f| {
                        json!({
                            "node_id": f.node_id, "concept": f.concept,
                            "content": f.content, "confidence": f.confidence,
                            "source_id": f.source_id, "tags": f.tags,
                        })
                    })
                    .collect();
                Ok(json!({"facts": facts}))
            }
            "memory.recall_procedure" => {
                let s = store.lock().unwrap();
                let procs: Vec<serde_json::Value> = s
                    .procedures
                    .iter()
                    .map(|p| {
                        json!({
                            "node_id": p.node_id, "name": p.name,
                            "steps": p.steps, "prerequisites": p.prerequisites,
                            "usage_count": p.usage_count,
                        })
                    })
                    .collect();
                Ok(json!({"procedures": procs}))
            }
            "memory.store_fact" => {
                let mut s = store.lock().unwrap();
                let id = format!("fact-{}", s.facts.len() + 1);
                s.facts.push(CognitiveFact {
                    node_id: id.clone(),
                    concept: params["concept"].as_str().unwrap_or("").to_string(),
                    content: params["content"].as_str().unwrap_or("").to_string(),
                    confidence: params["confidence"].as_f64().unwrap_or(0.0),
                    source_id: params["source_id"].as_str().unwrap_or("").to_string(),
                    tags: params["tags"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                });
                Ok(json!({"id": id}))
            }
            "memory.store_procedure" => {
                let mut s = store.lock().unwrap();
                let id = format!("proc-{}", s.procedures.len() + 1);
                s.procedures.push(CognitiveProcedure {
                    node_id: id.clone(),
                    name: params["name"].as_str().unwrap_or("").to_string(),
                    steps: params["steps"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    prerequisites: params["prerequisites"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    usage_count: 0,
                });
                Ok(json!({"id": id}))
            }
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("method not found: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

// ---------------------------------------------------------------------------
// azlin CLI wrapper tests
// ---------------------------------------------------------------------------

#[test]
fn azlin_create_and_destroy_lifecycle() {
    let executor = mock_executor();
    let config = AzlinConfig::default();
    let vm = azlin_create("int-test-vm", &config, &executor).unwrap();
    assert_eq!(vm.name, "int-test-vm");
    assert_eq!(vm.ip, "10.0.0.42");
    assert_eq!(vm.status, "running");

    azlin_destroy(&vm, &executor).unwrap();
}

#[test]
fn azlin_ssh_requires_running_status() {
    let executor = mock_executor();
    let vm = AzlinVm {
        name: "test".to_string(),
        ip: "10.0.0.1".to_string(),
        status: "stopped".to_string(),
    };
    let err = azlin_ssh(&vm, &executor).unwrap_err();
    assert!(matches!(err, SimardError::BridgeTransportError { .. }));
}

#[test]
fn azlin_create_failure_propagates() {
    let executor = failing_executor("create");
    let err = azlin_create("fail-vm", &AzlinConfig::default(), &executor).unwrap_err();
    assert!(matches!(err, SimardError::BridgeTransportError { .. }));
}

// ---------------------------------------------------------------------------
// Remote session lifecycle tests
// ---------------------------------------------------------------------------

#[test]
fn full_session_lifecycle() {
    let executor = mock_executor();
    let config = RemoteConfig {
        vm_name: "lifecycle-vm".to_string(),
        agent_name: "remote-agent-1".to_string(),
        azlin_config: AzlinConfig::default(),
    };

    // Create session.
    let mut session = create_remote_session(&config, &executor).unwrap();
    assert_eq!(session.status, RemoteStatus::Running);
    assert_eq!(session.ip_address, "10.0.0.42");
    assert!(session.is_active());
    assert!(!session.is_terminal());

    // Establish PTY.
    let pty_cmd = establish_pty(&session, &executor).unwrap();
    assert!(pty_cmd.contains("azureuser@10.0.0.42"));

    // Deploy agent binary.
    let binary = PathBuf::from("/tmp/simard");
    deploy_agent(&session, &binary, &executor).unwrap();

    // Transfer lifecycle.
    begin_transfer(&mut session).unwrap();
    assert_eq!(session.status, RemoteStatus::Transferring);
    end_transfer(&mut session).unwrap();
    assert_eq!(session.status, RemoteStatus::Running);

    // Destroy session.
    destroy_session(&mut session, &executor).unwrap();
    assert_eq!(session.status, RemoteStatus::Completed);
    assert!(session.is_terminal());
}

#[test]
fn session_deploy_rejects_wrong_binary() {
    let executor = mock_executor();
    let config = RemoteConfig {
        vm_name: "deploy-vm".to_string(),
        agent_name: "agent-d".to_string(),
        azlin_config: AzlinConfig::default(),
    };
    let session = create_remote_session(&config, &executor).unwrap();
    let bad_binary = PathBuf::from("/tmp/not-simard-binary");
    let err = deploy_agent(&session, &bad_binary, &executor).unwrap_err();
    assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
}

#[test]
fn session_establish_pty_rejects_non_running() {
    let executor = mock_executor();
    let config = RemoteConfig {
        vm_name: "pty-vm".to_string(),
        agent_name: "agent-p".to_string(),
        azlin_config: AzlinConfig::default(),
    };
    let mut session = create_remote_session(&config, &executor).unwrap();
    destroy_session(&mut session, &executor).unwrap();
    let err = establish_pty(&session, &executor).unwrap_err();
    assert!(matches!(err, SimardError::BridgeTransportError { .. }));
}

#[test]
fn session_destroy_failure_marks_failed() {
    let executor = failing_executor("destroy");
    let config = RemoteConfig {
        vm_name: "fail-destroy-vm".to_string(),
        agent_name: "agent-fd".to_string(),
        azlin_config: AzlinConfig::default(),
    };
    let mut session = create_remote_session(&config, &executor).unwrap();
    let err = destroy_session(&mut session, &executor);
    assert!(err.is_err());
    assert!(matches!(session.status, RemoteStatus::Failed(_)));
}

// ---------------------------------------------------------------------------
// Memory transfer tests
// ---------------------------------------------------------------------------

#[test]
fn memory_snapshot_export_and_import() {
    let source = mock_bridge();
    source
        .store_fact("architecture", "uses bridge pattern", 0.95, &[], "ep-1")
        .unwrap();
    source
        .store_fact("testing", "mock executors for CLI tools", 0.8, &[], "ep-2")
        .unwrap();
    source
        .store_procedure(
            "deploy-remote",
            &["scp binary".to_string(), "chmod +x".to_string()],
            &[],
        )
        .unwrap();

    let snapshot = export_memory_snapshot(&source, "source-agent", None).unwrap();
    assert_eq!(snapshot.facts.len(), 2);
    assert_eq!(snapshot.procedures.len(), 1);
    assert_eq!(snapshot.source_agent, "source-agent");

    let target = mock_bridge();
    let count = import_memory_snapshot(&target, &snapshot).unwrap();
    assert_eq!(count, 3);

    // Verify target has the imported data.
    let target_snapshot = export_memory_snapshot(&target, "target-agent", None).unwrap();
    assert_eq!(target_snapshot.facts.len(), 2);
    assert_eq!(target_snapshot.procedures.len(), 1);
}

#[test]
fn memory_snapshot_file_round_trip() {
    let bridge = mock_bridge();
    bridge
        .store_fact("serialization", "JSON round-trip works", 0.99, &[], "")
        .unwrap();

    let dir = std::env::temp_dir().join("simard-int-test-snapshot");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test-snapshot.json");

    let original = export_memory_snapshot(&bridge, "file-agent", Some(&path)).unwrap();
    let loaded = load_snapshot_from_file(&path).unwrap();

    assert_eq!(original.facts.len(), loaded.facts.len());
    assert_eq!(original.source_agent, loaded.source_agent);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_snapshot_import_returns_zero() {
    let target = mock_bridge();
    let empty = MemorySnapshot {
        facts: vec![],
        procedures: vec![],
        exported_at: 0,
        source_agent: "empty".to_string(),
    };
    let count = import_memory_snapshot(&target, &empty).unwrap();
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// End-to-end: session + memory migration
// ---------------------------------------------------------------------------

#[test]
fn session_with_memory_migration() {
    let executor = mock_executor();

    // Create a remote session.
    let config = RemoteConfig {
        vm_name: "migration-vm".to_string(),
        agent_name: "migrator".to_string(),
        azlin_config: AzlinConfig::default(),
    };
    let mut session = create_remote_session(&config, &executor).unwrap();

    // Export local memory.
    let local_bridge = mock_bridge();
    local_bridge
        .store_fact("project", "Simard is a self-building agent", 0.99, &[], "")
        .unwrap();

    begin_transfer(&mut session).unwrap();
    let snapshot = export_memory_snapshot(&local_bridge, "migrator", None).unwrap();
    assert_eq!(snapshot.facts.len(), 1);

    // Import into a "remote" bridge.
    let remote_bridge = mock_bridge();
    let count = import_memory_snapshot(&remote_bridge, &snapshot).unwrap();
    assert_eq!(count, 1);
    end_transfer(&mut session).unwrap();

    // Deploy and establish PTY.
    let binary = PathBuf::from("/tmp/simard");
    deploy_agent(&session, &binary, &executor).unwrap();
    let pty = establish_pty(&session, &executor).unwrap();
    assert!(!pty.is_empty());

    // Clean up.
    destroy_session(&mut session, &executor).unwrap();
    assert!(session.is_terminal());
}
