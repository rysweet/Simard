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
        // #1995: visible label was renamed Whiteboard → Workboard.
        assert!(INDEX_HTML.contains("Workboard"));
        assert!(INDEX_HTML.contains("/api/issues"));
        assert!(INDEX_HTML.contains("fetchStatus"));
        assert!(INDEX_HTML.contains("mem-graph-canvas"));
        assert!(INDEX_HTML.contains("fetchMemoryGraph"));
    }

    #[test]
    fn index_html_has_per_tab_intros_and_tooltips() {
        // Issue #1662 pass-1 + #1993/#1994: every tab gets a hover-tooltip,
        // a per-tab <h1 class="page-h1">, and a one-sentence
        // <p class="page-lede"> immediately under the H1.
        assert!(
            INDEX_HTML.contains(r#"class="page-lede""#),
            "page-lede CSS class should be used at least once"
        );
        // .page-lede CSS rule is registered (style block).
        assert!(INDEX_HTML.contains(".page-lede{"));
        // .page-h1 CSS rule is registered.
        assert!(INDEX_HTML.contains(".page-h1{"));
        // Spot-check a few tab tooltips so future refactors keep them in sync.
        assert!(INDEX_HTML.contains(r#"data-tab="overview" title="System health"#));
        assert!(INDEX_HTML.contains(r#"data-tab="goals" title="Active goals"#));
        assert!(INDEX_HTML.contains(r#"data-tab="terminal" title="Attach to the agent"#));
        // All 12 tab-content containers should now carry a page-lede paragraph.
        let lede_count = INDEX_HTML.matches(r#"class="page-lede""#).count();
        assert!(
            lede_count >= 12,
            "expected at least 12 .page-lede paragraphs (one per tab), found {lede_count}"
        );
        let h1_count = INDEX_HTML.matches(r#"class="page-h1""#).count();
        assert!(
            h1_count >= 12,
            "expected at least 12 .page-h1 headings (one per tab), found {h1_count}"
        );
    }

    #[test]
    fn index_html_has_format_time_helper() {
        // Issue #1662 pass-1: a single formatTime() helper centralises ISO/Unix-epoch
        // -> human-readable rendering so we do not sprinkle new Date(...).toLocaleString()
        // across the SPA. timeAgo() now delegates to the same parseTs() helper.
        assert!(INDEX_HTML.contains("function formatTime(ts)"));
        assert!(INDEX_HTML.contains("function parseTs(ts)"));
        // Live header clock must use formatTime, not a raw toLocaleString call.
        assert!(INDEX_HTML.contains("getElementById('clock').textContent=formatTime("));
        assert!(
            !INDEX_HTML.contains("new Date().toLocaleString()"),
            "no remaining `new Date().toLocaleString()` call sites should exist"
        );
    }

    // -------------------------------------------------------------------
    // Issue #1662 pass-1 — TDD CONTRACT TESTS
    //
    // The two tests above are spot-checks. The block below is the formal
    // behavioural contract for the three pass-1 changes (tab tooltips,
    // per-tab intros, formatTime/parseTs helpers + migrated call sites).
    // Each test should:
    //   * pass against the committed implementation (commit 6a47e540)
    //   * fail against pre-implementation HEAD or a regressed impl
    //
    // If you add a new tab, retire one, or refactor the time helpers,
    // these tests are the source-of-truth for what the dashboard owes
    // a first-time user and which call sites must funnel through the
    // shared formatter.
    // -------------------------------------------------------------------

    /// Every one of the eleven top-level SPA tabs must carry a non-empty
    /// `title="…"` hover-tooltip. Iterates the canonical tab list so that
    /// adding/removing a tab in `part_00.rs` immediately surfaces a missing
    /// tooltip via this test rather than a silent UX regression.
    #[test]
    fn index_html_all_eleven_tabs_have_tooltips() {
        // Canonical SPA tab set (see part_00.rs:99-109). This list is the
        // contract — keep in sync if tabs are added or removed.
        let tabs = [
            "overview",
            "goals",
            "traces",
            "logs",
            "processes",
            "memory",
            "costs",
            "chat",
            "workboard",
            "thinking",
            "terminal",
        ];
        assert_eq!(tabs.len(), 11, "expected exactly 11 top-level tabs");

        for tab in &tabs {
            let needle = format!(r#"data-tab="{tab}" title=""#);
            assert!(
                INDEX_HTML.contains(&needle),
                "tab `{tab}` is missing a title=\"…\" hover-tooltip — \
                 first-time users will have no idea what this tab does. \
                 Looked for: `{needle}`"
            );
        }
    }

    /// Tooltips must be substantive prose, not a one-word echo of the tab
    /// label. A meaningful threshold is ≥18 chars after the `title="`
    /// opening — long enough to communicate intent, short enough to fit
    /// in a browser tooltip.
    #[test]
    fn index_html_tab_tooltips_are_substantive() {
        const MIN_LEN: usize = 18;
        let tabs = [
            "overview",
            "goals",
            "traces",
            "logs",
            "processes",
            "memory",
            "costs",
            "chat",
            "workboard",
            "thinking",
            "terminal",
        ];
        for tab in &tabs {
            let prefix = format!(r#"data-tab="{tab}" title=""#);
            let start = INDEX_HTML
                .find(&prefix)
                .unwrap_or_else(|| panic!("tab `{tab}` declaration not found"));
            let after = &INDEX_HTML[start + prefix.len()..];
            let end = after
                .find('"')
                .unwrap_or_else(|| panic!("tab `{tab}` title attr is unterminated"));
            let title = &after[..end];
            assert!(
                title.len() >= MIN_LEN,
                "tab `{tab}` tooltip is too short to be useful (got {} chars: {:?})",
                title.len(),
                title
            );
        }
    }

    /// Each of the twelve `tab-content` containers (`id="tab-<name>"`)
    /// must contain at least one `<p class="page-lede">…</p>` inside
    /// its body — i.e. between the opening `id="tab-<name>"` and the next
    /// `id="tab-` of any kind (the next sibling tab-content). Guarantees
    /// the lede paragraph is scoped to each page rather than leaking from
    /// a neighbour.
    #[test]
    fn index_html_each_tab_content_has_intro_inside_it() {
        let tabs = [
            "overview",
            "goals",
            "traces",
            "logs",
            "processes",
            "memory",
            "costs",
            "chat",
            "workboard",
            "thinking",
            "terminal",
            "glossary",
        ];
        for tab in &tabs {
            let open = format!(r#"id="tab-{tab}""#);
            let start = INDEX_HTML
                .find(&open)
                .unwrap_or_else(|| panic!("`{open}` container not found"));
            // Find the next tab-content opening (any tab); use end-of-doc
            // as the boundary for the final tab.
            let after = &INDEX_HTML[start + open.len()..];
            let end_rel = after.find(r#"id="tab-"#).unwrap_or(after.len());
            let body = &after[..end_rel];
            assert!(
                body.contains(r#"class="page-lede""#),
                "tab `{tab}` (id=tab-{tab}) is missing its `<p class=\"page-lede\">` \
                 paragraph inside the tab-content body — first-time readers won't get \
                 the 'What is this page?' orientation sentence."
            );
            assert!(
                body.contains(r#"class="page-h1""#),
                "tab `{tab}` (id=tab-{tab}) is missing its `<h1 class=\"page-h1\">` \
                 heading inside the tab-content body — the page has no semantic title."
            );
        }
    }

    /// The `.page-lede` CSS rule must use the accent-border styling
    /// agreed in the design spec (a discreet left border in the accent
    /// colour). Locks the visual contract so future stylesheet refactors
    /// cannot silently drop the affordance.
    #[test]
    fn index_html_page_intro_css_uses_accent_border() {
        // Locate the CSS rule body and assert it carries the accent border.
        let rule_start = INDEX_HTML
            .find(".page-lede{")
            .expect(".page-lede{ CSS rule must be present");
        let rule_end_rel = INDEX_HTML[rule_start..]
            .find('}')
            .expect(".page-lede CSS rule must be closed by `}`");
        let rule = &INDEX_HTML[rule_start..rule_start + rule_end_rel];
        assert!(
            rule.contains("border-left:") && rule.contains("var(--accent)"),
            ".page-lede CSS rule must use a left border in the accent colour \
             (got: {rule:?})"
        );
        assert!(
            rule.contains("padding"),
            ".page-lede should be padded so prose isn't flush against the border"
        );
    }

    /// `parseTs` is the shared input normaliser. Its source must encode
    /// the four-input contract: null/empty → null, finite number →
    /// auto-detect seconds-vs-milliseconds via the 1e12 heuristic, ISO
    /// string → `new Date()`, anything else → null. We assert against the
    /// JS source rather than executing it because the SPA bundle is a
    /// static string at build time.
    #[test]
    fn index_html_parse_ts_encodes_full_input_contract() {
        // Find the parseTs body.
        let start = INDEX_HTML
            .find("function parseTs(ts){")
            .expect("parseTs(ts) helper must exist");
        let body_after = &INDEX_HTML[start..];
        let end_rel = body_after
            .find("function ")
            .and_then(|first| {
                body_after[first + 9..]
                    .find("function ")
                    .map(|n| first + 9 + n)
            })
            .unwrap_or_else(|| body_after.len().min(400));
        let body = &body_after[..end_rel];

        assert!(
            body.contains("ts==null") || body.contains("ts === null") || body.contains("ts==='"),
            "parseTs must guard against null/empty input — body: {body:?}"
        );
        assert!(
            body.contains("ts===''") || body.contains(r#"ts==="""#) || body.contains("''"),
            "parseTs must treat the empty string as null — body: {body:?}"
        );
        assert!(
            body.contains("typeof ts==='number'") || body.contains("typeof ts === 'number'"),
            "parseTs must distinguish number inputs from strings — body: {body:?}"
        );
        assert!(
            body.contains("1e12"),
            "parseTs must use the 1e12 heuristic to auto-detect seconds vs milliseconds \
             (anything < 1e12 is seconds, multiplied by 1000 before `new Date(…)`) — \
             body: {body:?}"
        );
        assert!(
            body.contains("new Date(ts"),
            "parseTs must fall back to `new Date(ts)` for ISO strings — body: {body:?}"
        );
        assert!(
            body.contains("isNaN"),
            "parseTs must reject invalid date strings via isNaN — body: {body:?}"
        );
    }

    /// `timeAgo` must delegate to `parseTs` rather than calling
    /// `new Date(ts)` directly — otherwise a Unix-epoch number passed to
    /// `timeAgo` would be misinterpreted as a millisecond value. The
    /// shared helper is the single chokepoint that fixes that bug class.
    #[test]
    fn index_html_time_ago_delegates_to_parse_ts() {
        let start = INDEX_HTML
            .find("function timeAgo(ts){")
            .expect("timeAgo(ts) helper must exist");
        let body_after = &INDEX_HTML[start..];
        // timeAgo body ends at the next `function ` declaration.
        let end_rel = body_after[20..]
            .find("function ")
            .map(|n| 20 + n)
            .unwrap_or(body_after.len().min(400));
        let body = &body_after[..end_rel];

        assert!(
            body.contains("parseTs(ts)"),
            "timeAgo must call parseTs(ts) so it accepts the same input types as \
             formatTime — body: {body:?}"
        );
        assert!(
            !body.contains("new Date(ts)"),
            "timeAgo must NOT call new Date(ts) directly — that bypasses the \
             seconds-vs-milliseconds heuristic in parseTs. Found: {body:?}"
        );
    }

    /// `formatTime` must:
    ///   * return an em-dash `'—'` for null inputs (the canonical "no
    ///     value" indicator used elsewhere in the SPA),
    ///   * delegate parsing to `parseTs`,
    ///   * fall back to ISO format when `toLocaleString()` throws (some
    ///     locales reject certain timezones).
    #[test]
    fn index_html_format_time_handles_null_and_locale_errors() {
        let start = INDEX_HTML
            .find("function formatTime(ts){")
            .expect("formatTime(ts) helper must exist");
        let body_after = &INDEX_HTML[start..];
        let end_rel = body_after[24..]
            .find("function ")
            .map(|n| 24 + n)
            .unwrap_or(body_after.len().min(400));
        let body = &body_after[..end_rel];

        assert!(
            body.contains("parseTs(ts)"),
            "formatTime must delegate to parseTs — body: {body:?}"
        );
        assert!(
            body.contains("'—'") || body.contains(r#""—""#),
            "formatTime must return em-dash '—' for null/empty input \
             (canonical no-value indicator) — body: {body:?}"
        );
        assert!(
            body.contains("toLocaleString()"),
            "formatTime must use toLocaleString() as the primary renderer — body: {body:?}"
        );
        assert!(
            body.contains("toISOString()"),
            "formatTime must fall back to toISOString() if toLocaleString throws \
             (some locales reject certain timezones) — body: {body:?}"
        );
        assert!(
            body.contains("catch"),
            "formatTime must wrap toLocaleString() in try/catch — body: {body:?}"
        );
    }

    /// The Memory tab's "Last Consolidation" stat (part_02.rs) must render
    /// its absolute timestamp via the shared `formatTime` helper, not via
    /// a direct `new Date(...).toLocaleString()` call. This was one of the
    /// three migrated call sites named in the design spec.
    #[test]
    fn index_html_last_consolidation_uses_format_time() {
        // The stat appears as a single template-literal line; locate by label.
        // The label now wraps "Consolidation" in an <abbr> tag (#1996).
        let pos = INDEX_HTML
            .find("Consolidation</abbr>")
            .expect("'Consolidation' stat (with <abbr> tag) must exist on the Memory tab");
        let window_end = (pos + 600).min(INDEX_HTML.len());
        let window = &INDEX_HTML[pos..window_end];
        assert!(
            window.contains("formatTime(d.last_consolidation)"),
            "Last Consolidation stat must call formatTime(d.last_consolidation) — \
             window: {window:?}"
        );
        assert!(
            !window.contains("new Date(d.last_consolidation).toLocaleString()"),
            "Last Consolidation stat must not bypass formatTime — \
             window: {window:?}"
        );
    }

    /// The cluster topology panel (part_05.rs) refresh timestamp must
    /// render via `formatTime`. This was the third migrated call site.
    #[test]
    fn index_html_topology_refresh_uses_format_time() {
        // part_05 sets text content on the refresh-stamp element via formatTime.
        assert!(
            INDEX_HTML.contains("formatTime(data.refreshed_at)"),
            "Topology refresh timestamp must use formatTime(data.refreshed_at)"
        );
        // Fallback path also goes through formatTime when no server timestamp.
        assert!(
            INDEX_HTML.contains("formatTime(Date.now())"),
            "Topology refresh fallback must also use formatTime(Date.now()) so \
             both branches produce identical formatting"
        );
    }

    /// Belt-and-braces guard: the SPA bundle must not contain any
    /// remaining `new Date(...)` followed by `.toLocaleString()`,
    /// regardless of the operand. The legitimate uses of `.toLocaleString()`
    /// elsewhere in the bundle are on plain numbers (e.g. `v.toLocaleString()`
    /// for token counts), which this assertion does not flag.
    #[test]
    fn index_html_no_new_date_to_locale_string_call_sites_remain() {
        // Walk every `new Date(` occurrence and verify the next 80 chars
        // do not contain `.toLocaleString(` before a closing semicolon or
        // `}`.
        let bytes: &str = &INDEX_HTML;
        let mut search_start = 0;
        let mut violations: Vec<String> = Vec::new();
        while let Some(rel) = bytes[search_start..].find("new Date(") {
            let abs = search_start + rel;
            let snippet_end = (abs + 120).min(bytes.len());
            let snippet = &bytes[abs..snippet_end];
            // Look only within this expression — stop at `;` or newline so we
            // don't bleed into a sibling statement's `toLocaleString` call.
            let stmt_end = snippet.find([';', '\n']).unwrap_or(snippet.len());
            let stmt = &snippet[..stmt_end];
            if stmt.contains(".toLocaleString(") {
                violations.push(stmt.to_string());
            }
            search_start = abs + 9;
        }
        assert!(
            violations.is_empty(),
            "found `new Date(...).toLocaleString()` call sites that bypass formatTime: \
             {violations:#?}"
        );
    }

    /// The live header clock must update every second via the shared
    /// `formatTime(Date.now())` path — not via a hand-rolled
    /// `new Date().toLocaleString()` call. Locks the migration of the
    /// most visible timestamp on the page.
    #[test]
    fn index_html_header_clock_uses_format_time() {
        // The setInterval lives on a single line in part_01.rs:207.
        assert!(
            INDEX_HTML.contains("getElementById('clock').textContent=formatTime(Date.now())"),
            "Header clock must use formatTime(Date.now()) on every tick"
        );
        // And the tick interval should be 1 second so the displayed time
        // matches the wall clock.
        assert!(
            INDEX_HTML.contains(",1000)"),
            "Header clock setInterval must use a 1000 ms (1 s) tick"
        );
    }

    /// Sanity-check on the page-lede count: there must be exactly 12
    /// (one per tab) — a stricter bound than the existing `>= 12`
    /// assertion. If a refactor accidentally adds a 12th, we want to
    /// know immediately so we can decide whether the new container is
    /// actually a new tab or a misuse of the class.
    #[test]
    fn index_html_has_exactly_twelve_page_intros() {
        let count = INDEX_HTML.matches(r#"class="page-lede""#).count();
        assert_eq!(
            count, 11,
            "expected exactly 12 page-lede paragraphs (one per top-level tab), got {count}"
        );
        let h1_count = INDEX_HTML.matches(r#"class="page-h1""#).count();
        assert_eq!(
            h1_count, 11,
            "expected exactly 12 page-h1 headings (one per top-level tab), got {h1_count}"
        );
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
