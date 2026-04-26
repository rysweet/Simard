#[cfg(test)]
mod tests_b {
    use crate::operator_commands_dashboard::routes::*;
    use crate::operator_commands_dashboard::memory::{build_agent_graph, classify_agent_layer};
    use crate::operator_commands_dashboard::routes::TmuxSession;
    use serde_json::json;
    #[test]
    fn sanitize_agent_name_rejects_invalid_names() {
        assert_eq!(sanitize_agent_name(""), None);
        // 65 chars (boundary).
        let too_long: String = std::iter::repeat_n('x', 65).collect();
        assert_eq!(sanitize_agent_name(&too_long), None);
        // Path traversal attempts (INV-7): every disallowed byte must reject.
        assert_eq!(sanitize_agent_name(".."), None);
        assert_eq!(sanitize_agent_name("../etc/passwd"), None);
        assert_eq!(sanitize_agent_name("a/b"), None);
        assert_eq!(sanitize_agent_name("a\\b"), None);
        assert_eq!(sanitize_agent_name("a.b"), None);
        assert_eq!(sanitize_agent_name("a b"), None);
        assert_eq!(sanitize_agent_name("a\0b"), None);
        assert_eq!(sanitize_agent_name("a\nb"), None);
        assert_eq!(sanitize_agent_name("café"), None);
        assert_eq!(sanitize_agent_name("a:b"), None);
        assert_eq!(sanitize_agent_name("a;b"), None);
        assert_eq!(sanitize_agent_name("a*b"), None);
    }

    #[test]
    fn agent_log_path_layout_under_state_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = agent_log_path(root, "planner");
        assert_eq!(p, root.join("agent_logs").join("planner.log"));
    }

    #[test]
    fn agent_log_path_does_not_escape_state_root_for_valid_names() {
        // INV-7: any name that passed the sanitizer must produce a path
        // strictly inside <state_root>/agent_logs/.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let log_dir = root.join("agent_logs");
        for name in ["planner", "agent_1", "Agent-42", "a", "abc-_123"] {
            let p = agent_log_path(root, name);
            assert!(
                p.starts_with(&log_dir),
                "agent_log_path({name:?}) = {p:?} escaped {log_dir:?}"
            );
            let expected = format!("{name}.log");
            assert_eq!(
                p.file_name().and_then(|n| n.to_str()),
                Some(expected.as_str())
            );
        }
    }

    #[test]
    fn index_html_contains_terminal_tab_and_xterm() {
        // Tab button + pane.
        assert!(
            INDEX_HTML.contains("Terminal"),
            "Index HTML should include a Terminal tab label"
        );
        assert!(
            INDEX_HTML.contains("tab-terminal"),
            "Index HTML should include a tab-terminal pane id"
        );
        assert!(
            INDEX_HTML.contains("xterm-host"),
            "Index HTML should include the xterm-host container"
        );
        // xterm.js pinned to 5.3.0 from jsdelivr CDN (per design).
        assert!(
            INDEX_HTML.contains("xterm@5.3.0"),
            "Index HTML should pin xterm.js to version 5.3.0"
        );
        // WS endpoint path is referenced by the client JS.
        assert!(
            INDEX_HTML.contains("/ws/agent_log/"),
            "Index HTML should reference the /ws/agent_log/ endpoint"
        );
    }

    #[test]
    fn build_router_registers_ws_agent_log_route() {
        // Smoke-check: build_router constructs without panic and references
        // the new route. Axum's Router does not expose its route table for
        // direct inspection in stable, so we assert via build success and
        // a marker constant exposed by the module.
        let _router = build_router();
        assert!(
            WS_AGENT_LOG_ROUTE.starts_with("/ws/agent_log/"),
            "WS_AGENT_LOG_ROUTE should be the agent log WS path; got {WS_AGENT_LOG_ROUTE:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Issue #951 — Agent graph endpoint tests.
    // ---------------------------------------------------------------------

    fn make_entry(id: &str, role: &str) -> crate::agent_registry::AgentEntry {
        crate::agent_registry::AgentEntry {
            id: id.to_string(),
            pid: 1,
            host: "localhost".to_string(),
            start_time: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
            state: crate::agent_registry::AgentState::Running,
            role: role.to_string(),
            resources: crate::agent_registry::ResourceUsage {
                rss_bytes: None,
                cpu_percent: None,
            },
        }
    }

    #[test]
    fn classify_agent_layer_buckets_roles() {
        assert_eq!(classify_agent_layer("ooda-loop"), "ooda");
        assert_eq!(classify_agent_layer("operator"), "ooda");
        assert_eq!(classify_agent_layer("agent_supervisor"), "ooda");
        assert_eq!(classify_agent_layer("engineer"), "engineer");
        assert_eq!(classify_agent_layer("planner"), "engineer");
        assert_eq!(classify_agent_layer("builder"), "engineer");
        assert_eq!(classify_agent_layer("session-42"), "session");
        assert_eq!(classify_agent_layer("anything-else"), "session");
    }

    #[test]
    fn build_agent_graph_emits_layered_topology() {
        let entries = vec![
            make_entry("o1", "ooda"),
            make_entry("e1", "engineer"),
            make_entry("e2", "engineer"),
            make_entry("s1", "session"),
            make_entry("s2", "session"),
        ];
        let graph = build_agent_graph(&entries);

        let nodes = graph["nodes"].as_array().expect("nodes array");
        assert_eq!(nodes.len(), 5);
        assert!(
            nodes
                .iter()
                .all(|n| n.get("id").is_some() && n.get("type").is_some())
        );

        // OODA -> 2 engineers (2 edges) + each engineer -> 2 sessions (4 edges) = 6
        let edges = graph["edges"].as_array().expect("edges array");
        assert_eq!(edges.len(), 6);
        assert!(
            edges
                .iter()
                .all(|e| e.get("src").is_some() && e.get("dst").is_some())
        );

        assert_eq!(graph["layers"]["ooda"], 1);
        assert_eq!(graph["layers"]["engineer"], 2);
        assert_eq!(graph["layers"]["session"], 2);
    }

    #[test]
    fn build_agent_graph_handles_empty_input() {
        let graph = build_agent_graph(&[]);
        assert_eq!(graph["nodes"].as_array().unwrap().len(), 0);
        assert_eq!(graph["edges"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 200), "hello");
    }

    #[test]
    fn truncate_exactly_200_unchanged() {
        let s = "x".repeat(200);
        let out = truncate_with_ellipsis(&s, 200);
        assert_eq!(out, s);
        assert_eq!(out.chars().count(), 200);
    }

    #[test]
    fn truncate_201_truncated_with_ellipsis() {
        let s = "x".repeat(201);
        let out = truncate_with_ellipsis(&s, 200);
        assert_eq!(out.chars().count(), 201); // 200 + ellipsis
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().filter(|&c| c == 'x').count(), 200);
    }

    #[test]
    fn truncate_utf8_boundary_safe() {
        // 250 emoji (4-byte UTF-8 each) — must not panic and must truncate by codepoint.
        let s: String = "🎉".repeat(250);
        let out = truncate_with_ellipsis(&s, 200);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().filter(|&c| c == '🎉').count(), 200);
    }

    #[test]
    fn truncate_accented_chars() {
        let s: String = "é".repeat(250);
        let out = truncate_with_ellipsis(&s, 200);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_combining_chars_no_panic() {
        // base 'e' + combining acute U+0301 repeated; must not panic.
        let unit = "e\u{0301}";
        let s: String = unit.repeat(150); // 300 codepoints
        let out = truncate_with_ellipsis(&s, 200);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_empty_returns_empty() {
        assert_eq!(truncate_with_ellipsis("", 200), "");
    }

    #[test]
    fn truncate_with_zero_max() {
        // Defensive: zero-max on non-empty returns just the ellipsis;
        // empty input returns empty.
        assert_eq!(truncate_with_ellipsis("hi", 0), "…");
        assert_eq!(truncate_with_ellipsis("", 0), "");
    }

    // ---------------------------------------------------------------------
    // WS-1 AZLIN-TMUX-SESSIONS-LIST — TDD tests (Step 7).
    //
    // These tests specify the contract for `parse_tmux_sessions` (the pure
    // parser used by `/api/azlin/tmux-sessions`) and for host enumeration
    // (which MUST go through `load_hosts()` so the panel agrees with the
    // Cluster Topology / Remote VMs source).
    //
    // They are expected to FAIL TO COMPILE until the implementation lands
    // (Step 8): symbols `parse_tmux_sessions` and `TmuxSession` do not yet
    // exist. That compile failure IS the failing test.
    // ---------------------------------------------------------------------

    /// Basic happy-path: 3 sessions, mixed `attached`. Verifies field types
    /// and tab-separated parse of `#S\t#{session_created}\t#{session_attached}\t#{session_windows}`.
    #[test]
    fn parse_tmux_sessions_basic() {
        let input = "main\t1700000000\t1\t3\nwork\t1700000500\t0\t1\nidle\t1700000999\t0\t2\n";
        let out = parse_tmux_sessions(input);
        assert_eq!(out.len(), 3, "should parse 3 well-formed rows");

        assert_eq!(out[0].name, "main");
        assert_eq!(out[0].created, 1_700_000_000_i64);
        assert!(out[0].attached);
        assert_eq!(out[0].windows, 3_u32);

        assert_eq!(out[1].name, "work");
        assert!(!out[1].attached);
        assert_eq!(out[1].windows, 1);

        assert_eq!(out[2].name, "idle");
        assert!(!out[2].attached);
        assert_eq!(out[2].windows, 2);
    }

    /// Empty input → empty vec (no panic, no synthetic row).
    #[test]
    fn parse_tmux_sessions_empty() {
        assert!(parse_tmux_sessions("").is_empty());
        assert!(parse_tmux_sessions("\n").is_empty());
        assert!(parse_tmux_sessions("\n\n  \n").is_empty());
    }

    /// `tmux: no server running` exits 1 with empty stdout; the route maps
    /// that to `reachable:true, sessions:[]`. The parser itself just needs
    /// to handle the typical stderr-style content gracefully (no panic, no
    /// rows). The route layer is responsible for the reachable flag.
    #[test]
    fn parse_tmux_sessions_no_server() {
        // Real-world: tmux writes "no server running on /tmp/tmux-1000/default"
        // to stderr and stdout is empty. But if a wrapper conflates streams,
        // the parser must still return [] (no tabs ⇒ malformed ⇒ skipped).
        assert!(parse_tmux_sessions("no server running on /tmp/tmux-1000/default\n").is_empty());
        assert!(parse_tmux_sessions("").is_empty());
    }

    /// Malformed rows (wrong field count, non-numeric created/windows,
    /// non-0/1 attached) are skipped; valid rows survive.
    #[test]
    fn parse_tmux_sessions_malformed() {
        let input = concat!(
            "good\t1700000000\t1\t2\n",
            "too\tfew\tfields\n",                // 3 fields — skip
            "bad-created\tNaN\t0\t1\n",          // created not int — skip
            "bad-windows\t1700000000\t0\tabc\n", // windows not uint — skip
            "another-good\t1700001000\t0\t5\n",
            "trailing-tabs\t1700002000\t1\t1\t\t\n", // extra empties — also skip
            "\n",                                    // blank
        );
        let out = parse_tmux_sessions(input);
        assert_eq!(out.len(), 2, "only the two well-formed rows should survive");
        assert_eq!(out[0].name, "good");
        assert_eq!(out[1].name, "another-good");
        assert_eq!(out[1].windows, 5);
    }

    /// Host enumeration MUST go through `load_hosts()` (the canonical
    /// `~/.simard/hosts.json` reader). Setting `HOME` to a tempdir with a
    /// synthetic `hosts.json` and calling `load_hosts()` must yield exactly
    /// the synthetic entries — proving the tmux route would see the same
    /// host set as the Topology / Remote-VMs panels.
    #[test]
    fn host_enumeration_reads_load_hosts() {
        use std::io::Write;

        // Use a unique tempdir to avoid races with other tests touching HOME.
        let tmp = std::env::temp_dir().join(format!(
            "simard-tmux-tdd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(tmp.join(".simard")).expect("mkdir");
        let mut f = std::fs::File::create(tmp.join(".simard").join("hosts.json"))
            .expect("create hosts.json");
        writeln!(
            f,
            r#"[{{"name":"vm-tmux-1","resource_group":"rg-x"}},{{"name":"vm-tmux-2","resource_group":"rg-y"}}]"#
        )
        .expect("write");

        // SAFETY: tests in this module share a process; we save & restore HOME.
        let prev_home = std::env::var("HOME").ok();
        // Rust 2024: env mutation is unsafe.
        // The compile-time gate makes the unsafe block harmless when not on edition 2024.
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

        let hosts = load_hosts();

        // Restore HOME before assertions so a panic doesn't leak state.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        let names: Vec<String> = hosts
            .iter()
            .map(|h| host_entry_name(h).to_string())
            .collect();
        assert_eq!(
            names,
            vec!["vm-tmux-1".to_string(), "vm-tmux-2".to_string()],
            "tmux-sessions route MUST enumerate via load_hosts() (canonical source)"
        );

        // Cleanup tempdir (best-effort).
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
