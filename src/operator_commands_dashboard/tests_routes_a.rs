#[cfg(test)]
mod tests {
    use crate::operator_commands_dashboard::agent_log::sanitize_agent_name;
    use crate::operator_commands_dashboard::current_work::{
        format_recent_actions_for_cycle, read_recent_cycle_reports,
    };
    use crate::operator_commands_dashboard::distributed::remote_vms_from_hosts;
    use crate::operator_commands_dashboard::hosts::{
        host_entry_name, is_local_host, tag_local_membership,
    };
    use crate::operator_commands_dashboard::index_html::INDEX_HTML;
    use crate::operator_commands_dashboard::routes::*;
    use serde_json::json;

    #[test]
    fn remote_vms_panel_matches_configured_hosts() {
        use std::collections::BTreeSet;

        let hosts = vec![
            serde_json::json!({"name": "vm-alpha", "resource_group": "rg1"}),
            serde_json::json!({"name": "vm-beta",  "resource_group": "rg2"}),
        ];

        let remote_vms = remote_vms_from_hosts(&hosts);

        let host_names: BTreeSet<String> = hosts
            .iter()
            .filter_map(|h| h.get("name").and_then(|v| v.as_str()).map(String::from))
            .collect();
        let vm_names: BTreeSet<String> = remote_vms
            .iter()
            .filter_map(|v| v.get("vm_name").and_then(|x| x.as_str()).map(String::from))
            .collect();

        assert_eq!(
            host_names, vm_names,
            "Remote VMs panel must agree with configured hosts (Cluster Topology source)"
        );
        assert!(
            !vm_names.contains("Simard"),
            "Hardcoded 'Simard' default must not appear unless explicitly configured"
        );

        // Empty hosts -> empty remote_vms (frontend renders 'No remote VMs configured').
        let empty: Vec<serde_json::Value> = Vec::new();
        assert!(remote_vms_from_hosts(&empty).is_empty());

        // Each entry has expected fields with safe defaults.
        for vm in &remote_vms {
            assert!(vm.get("vm_name").and_then(|v| v.as_str()).is_some());
            assert!(vm.get("resource_group").is_some());
            assert!(vm.get("status").is_some());
        }
    }

    /// Config-validation: the Remote VMs panel and the Cluster Topology panel
    /// MUST derive their VM identifier set from the same canonical source
    /// (`load_hosts()` → ~/.simard/hosts.json). Regression guard for the bug
    /// where Remote VMs displayed a stale hard-coded list while Topology read
    /// the live config. Mirrors how `distributed()` (Remote VMs) and
    /// `get_hosts()` (Topology) extract names from the same hosts vector.
    #[test]
    fn remote_vms_and_topology_agree_on_vm_set() {
        use std::collections::BTreeSet;

        // Includes the "Name" alias variant accepted by host_entry_name to
        // ensure both extractors handle every shape load_hosts() may yield.
        let hosts = vec![
            serde_json::json!({"name": "vm-alpha", "resource_group": "rg1"}),
            serde_json::json!({"name": "vm-beta",  "resource_group": "rg2"}),
            serde_json::json!({"Name": "vm-gamma", "resource_group": "rg3"}),
        ];

        // Topology side: get_hosts() builds cluster_members via host_entry_name.
        let topology_set: BTreeSet<String> = hosts
            .iter()
            .map(|e| host_entry_name(e).to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Remote VMs side: distributed() builds entries via remote_vms_from_hosts.
        let remote_vms_set: BTreeSet<String> = remote_vms_from_hosts(&hosts)
            .iter()
            .filter_map(|v| v.get("vm_name").and_then(|x| x.as_str()).map(String::from))
            .collect();

        assert_eq!(
            topology_set, remote_vms_set,
            "Remote VMs panel and Cluster Topology panel must report the same VM set \
             when fed the same load_hosts() output"
        );
    }

    #[test]
    fn is_local_host_exact_match() {
        assert!(is_local_host("myhost", "myhost"));
    }

    #[test]
    fn is_local_host_case_insensitive() {
        assert!(is_local_host("MyHost", "myhost"));
        assert!(is_local_host("myhost", "MYHOST"));
        assert!(is_local_host("MyHost.Example.COM", "myhost"));
    }

    #[test]
    fn is_local_host_fqdn_vs_short() {
        // FQDN on either side reduces to short name
        assert!(is_local_host("myhost", "myhost.example.com"));
        assert!(is_local_host("myhost.example.com", "myhost"));
        assert!(is_local_host("myhost.a.b", "myhost.c.d"));
    }

    #[test]
    fn is_local_host_non_match() {
        assert!(!is_local_host("myhost", "otherhost"));
        assert!(!is_local_host(
            "myhost.example.com",
            "otherhost.example.com"
        ));
        assert!(!is_local_host("host1", "host2"));
    }

    #[test]
    fn is_local_host_empty_inputs() {
        assert!(!is_local_host("", "myhost"));
        assert!(!is_local_host("myhost", ""));
        assert!(!is_local_host("", ""));
    }

    #[test]
    fn tag_local_membership_marks_only_local_when_in_cluster() {
        // Three Azlin hosts; cluster membership lists vm-a and vm-b.
        // Local hostname is vm-a (with FQDN suffix to exercise short-name match).
        let mut hosts = vec![
            serde_json::json!({"name": "vm-a", "resource_group": "rg1"}),
            serde_json::json!({"name": "vm-b", "resource_group": "rg1"}),
            serde_json::json!({"name": "vm-c", "resource_group": "rg2"}),
        ];
        let cluster_members: Vec<String> = vec!["vm-a".into(), "vm-b".into()];
        let local_hostname = "VM-A.internal.example.com";

        tag_local_membership(&mut hosts, &cluster_members, local_hostname);

        assert_eq!(
            hosts[0]["is_local"],
            serde_json::Value::Bool(true),
            "vm-a matches local hostname AND is in cluster -> joined"
        );
        assert_eq!(
            hosts[1]["is_local"],
            serde_json::Value::Bool(false),
            "vm-b is in cluster but is not local -> not joined"
        );
        assert_eq!(
            hosts[2]["is_local"],
            serde_json::Value::Bool(false),
            "vm-c is neither local nor in cluster"
        );

        // Local hostname matches an entry, but that entry is NOT in cluster_members.
        let mut hosts2 = vec![serde_json::json!({"name": "vm-x"})];
        tag_local_membership(&mut hosts2, &cluster_members, "vm-x");
        assert_eq!(
            hosts2[0]["is_local"],
            serde_json::Value::Bool(false),
            "vm-x matches local but is not a cluster member -> not joined"
        );

        // Capitalized "Name" key (azlin discovered VMs) is also recognized.
        let mut discovered = vec![serde_json::json!({"Name": "VM-A"})];
        tag_local_membership(&mut discovered, &cluster_members, "vm-a");
        assert_eq!(
            discovered[0]["is_local"],
            serde_json::Value::Bool(true),
            "Capitalized Name field should also be matched"
        );

        // Empty local hostname must never produce a match (guards bad /etc/hostname reads).
        let mut hosts3 = vec![serde_json::json!({"name": "vm-a"})];
        tag_local_membership(&mut hosts3, &cluster_members, "");
        assert_eq!(
            hosts3[0]["is_local"],
            serde_json::Value::Bool(false),
            "Empty local hostname must not produce a match"
        );
    }

    #[test]
    fn build_router_creates_valid_router() {
        let router = build_router();
        // Verify the router can be constructed without panicking.
        // Axum routers are opaque, but construction succeeding validates
        // that all route paths, handlers, and middleware are well-formed.
        let _ = router;
    }

    #[test]
    fn login_html_contains_form() {
        assert!(crate::operator_commands_dashboard::auth::LOGIN_HTML.contains("<form"));
        assert!(crate::operator_commands_dashboard::auth::LOGIN_HTML.contains("login-form"));
        assert!(crate::operator_commands_dashboard::auth::LOGIN_HTML.contains("/api/login"));
    }

    #[test]
    fn index_html_contains_dashboard_structure() {
        assert!(INDEX_HTML.contains("Simard Dashboard"));
        assert!(INDEX_HTML.contains("/api/status"));
        assert!(INDEX_HTML.contains("/api/workboard"));
        assert!(INDEX_HTML.contains("Whiteboard"));
        assert!(INDEX_HTML.contains("/api/issues"));
        assert!(INDEX_HTML.contains("fetchStatus"));
        assert!(INDEX_HTML.contains("mem-graph-canvas"));
        assert!(INDEX_HTML.contains("fetchMemoryGraph"));
    }

    #[test]
    fn login_html_has_code_input() {
        assert!(crate::operator_commands_dashboard::auth::LOGIN_HTML.contains(r#"type="text""#));
        assert!(crate::operator_commands_dashboard::auth::LOGIN_HTML.contains("maxlength"));
    }

    #[test]
    fn read_recent_cycle_reports_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert!(reports.is_empty());
    }

    #[test]
    fn read_recent_cycle_reports_returns_sorted_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        for i in 1..=15 {
            std::fs::write(
                cycle_dir.join(format!("cycle_{i}.json")),
                format!("Cycle {i}: 1 action, 1 succeeded"),
            )
            .unwrap();
        }

        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert_eq!(reports.len(), 5);
        // Should be sorted descending by cycle number
        assert_eq!(reports[0]["cycle_number"], 15);
        assert_eq!(reports[4]["cycle_number"], 11);
    }

    #[test]
    fn read_recent_cycle_reports_parses_json_content() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            r#"{"actions": 3, "succeeded": 2}"#,
        )
        .unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["cycle_number"], 1);
        // JSON content should be nested under "report"
        assert!(reports[0].get("report").is_some());
        assert_eq!(reports[0]["report"]["actions"], 3);
    }

    #[test]
    fn read_recent_cycle_reports_deduplicates_across_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Create both candidate directories with overlapping cycle numbers
        let dir_a = dir.path().join("cycle_reports");
        let dir_b = dir.path().join("state").join("cycle_reports");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        std::fs::write(dir_a.join("cycle_5.json"), "from dir_a").unwrap();
        std::fs::write(dir_b.join("cycle_5.json"), "from dir_b").unwrap();
        std::fs::write(dir_b.join("cycle_6.json"), "unique to dir_b").unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 10);
        // Should have 2 unique cycle numbers (5 and 6), not 3
        assert_eq!(reports.len(), 2);
    }

    #[tokio::test]
    async fn run_gh_json_returns_empty_array_on_failure() {
        // gh is unlikely to succeed without auth in test; verify graceful handling
        let result = run_gh_json(&["pr", "list", "--json", "number"]).await;
        assert!(result.is_array());
    }

    #[test]
    fn format_recent_actions_prefers_outcome_detail_truncated() {
        let long: String = "x".repeat(250);
        let report = json!({
            "cycle_number": 103,
            "report": {
                "outcomes": [
                    {"action_kind": "advance-goal", "action_description": "not yet started", "detail": long},
                    {"action_kind": "advance-goal", "action_description": "not yet started", "detail": "short detail"}
                ],
                "planned_actions": [
                    {"kind": "advance-goal", "description": "not yet started"}
                ],
                "summary": "should-not-show"
            }
        });
        let entries = format_recent_actions_for_cycle(103, &report);
        assert_eq!(entries.len(), 2);
        let first = entries[0]["result"].as_str().unwrap();
        // 200 chars + the trailing ellipsis
        assert_eq!(first.chars().count(), 201);
        assert!(first.ends_with('…'));
        assert!(first.starts_with("xxxx"));
        assert_eq!(entries[0]["action"], "advance-goal");
        assert_eq!(entries[0]["cycle"], 103);
        assert_eq!(entries[1]["result"], "short detail");
    }

    #[test]
    fn format_recent_actions_outcome_short_detail_passthrough() {
        let report = json!({
            "report": {
                "outcomes": [
                    {"action_kind": "run-improvement", "detail": "improvement cycle ok"}
                ]
            }
        });
        let entries = format_recent_actions_for_cycle(7, &report);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["result"], "improvement cycle ok");
        assert!(!entries[0]["result"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn format_recent_actions_falls_back_to_planned_actions_when_outcomes_empty() {
        let report = json!({
            "report": {
                "outcomes": [],
                "planned_actions": [
                    {"kind": "advance-goal", "description": "kick off the work"}
                ]
            }
        });
        let entries = format_recent_actions_for_cycle(42, &report);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["action"], "advance-goal");
        assert_eq!(entries[0]["result"], "kick off the work");
    }

    #[test]
    fn format_recent_actions_sensible_default_when_both_missing() {
        // Neither outcomes nor planned_actions present, but a summary exists.
        let report = json!({
            "report": {"summary": "OODA cycle #5: 0 actions"}
        });
        let entries = format_recent_actions_for_cycle(5, &report);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["action"], "cycle-summary");
        assert_eq!(entries[0]["result"], "OODA cycle #5: 0 actions");

        // Completely empty report yields no entries (no panic).
        let empty = json!({"report": {}});
        assert!(format_recent_actions_for_cycle(0, &empty).is_empty());

        // Outcome with neither detail nor action_description still produces
        // a sensible placeholder rather than dropping the row.
        let bare = json!({"report": {"outcomes": [{"action_kind": "noop"}]}});
        let entries = format_recent_actions_for_cycle(1, &bare);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["result"], "(no detail)");
    }

    // ---------------------------------------------------------------------
    // Issue #947 — Agent terminal widget tests (TDD: written before impl).
    // These tests define the contract for `sanitize_agent_name`,
    // `agent_log_path`, the WS route registration, and the inline HTML
    // additions for the Terminal tab.
    // ---------------------------------------------------------------------

    #[test]
    fn sanitize_agent_name_accepts_valid_names() {
        // Allow-list: ^[A-Za-z0-9_-]{1,64}$
        assert_eq!(sanitize_agent_name("planner"), Some("planner".to_string()));
        assert_eq!(sanitize_agent_name("agent_1"), Some("agent_1".to_string()));
        assert_eq!(
            sanitize_agent_name("Agent-42"),
            Some("Agent-42".to_string())
        );
        assert_eq!(sanitize_agent_name("a"), Some("a".to_string()));
        // Exactly 64 chars (boundary).
        let max_len: String = std::iter::repeat_n('x', 64).collect();
        assert_eq!(sanitize_agent_name(&max_len), Some(max_len.clone()));
    }
}
