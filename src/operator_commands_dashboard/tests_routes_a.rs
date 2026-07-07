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
        // #1995 → #2627: the visible label lineage ended at "Work Board", now a
        // sub-section of the Goals tab (the API route path is unchanged).
        assert!(INDEX_HTML.contains("Work Board"));
        assert!(INDEX_HTML.contains("/api/issues"));
        assert!(INDEX_HTML.contains("fetchStatus"));
        assert!(INDEX_HTML.contains("mem-graph-canvas"));
        assert!(INDEX_HTML.contains("fetchMemoryGraph"));
    }

    #[test]
    fn index_html_has_per_tab_intros_and_tooltips() {
        // Issue #1662 pass-1 + #1993/#1994 + #2627: every tab gets a
        // hover-tooltip, a per-tab <h1 class="page-h1">, and a one-sentence
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
        assert!(INDEX_HTML.contains(r#"data-tab="workers" title="Processes"#));
        // Each of the nine consolidated tab-content containers carries a
        // page-lede paragraph and a page-h1 heading.
        let lede_count = INDEX_HTML.matches(r#"class="page-lede""#).count();
        assert!(
            lede_count >= 9,
            "expected at least 9 .page-lede paragraphs (one per tab), found {lede_count}"
        );
        let h1_count = INDEX_HTML.matches(r#"class="page-h1""#).count();
        assert!(
            h1_count >= 9,
            "expected at least 9 .page-h1 headings (one per tab), found {h1_count}"
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

    /// Every one of the eleven consolidated SPA tabs (#2627, incl. the restored
    /// Memory tab) must carry a non-empty `title="…"` hover-tooltip. Iterates
    /// the canonical tab list so that adding/removing a tab immediately surfaces
    /// a missing tooltip via this test rather than a silent UX regression.
    #[test]
    fn index_html_all_eleven_tabs_have_tooltips() {
        // Canonical consolidated SPA tab set (#2627). This list is the
        // contract — keep in sync if tabs are added or removed. The `memory`
        // entry (after Pull Requests) is the #2627 regression fix: its viz was
        // dropped by the 17->9 consolidation and is restored as a dedicated tab.
        let tabs = [
            "overview",
            "goals",
            "activity",
            "workers",
            "pull-requests",
            "memory",
            "resources",
            "chat",
            "overseer",
            "journal",
            "creative-ideas",
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
            "activity",
            "workers",
            "pull-requests",
            "resources",
            "chat",
            "overseer",
            "journal",
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

    /// Each of the nine `tab-content` containers (`id="tab-<name>"`)
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
            "activity",
            "workers",
            "pull-requests",
            "resources",
            "chat",
            "overseer",
            "journal",
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

    /// The Memory tab's "Last Memory Compaction" stat (part_02.rs) must render
    /// its absolute timestamp via the shared `formatTime` helper, not via
    /// a direct `new Date(...).toLocaleString()` call. This was one of the
    /// three migrated call sites named in the design spec.
    #[test]
    fn index_html_last_consolidation_uses_format_time() {
        // The stat appears as a single template-literal line; locate by label.
        let pos = INDEX_HTML
            .find("Last Memory Compaction")
            .expect("'Last Memory Compaction' stat must exist on the Memory tab");
        let window_end = (pos + 600).min(INDEX_HTML.len());
        let window = &INDEX_HTML[pos..window_end];
        assert!(
            window.contains("formatTime(d.last_consolidation)"),
            "Last Memory Compaction stat must call formatTime(d.last_consolidation) — \
             window: {window:?}"
        );
        assert!(
            !window.contains("new Date(d.last_consolidation).toLocaleString()"),
            "Last Memory Compaction stat must not bypass formatTime — \
             window: {window:?}"
        );
    }

    /// Regression for #1681. The Memory tab's "Memory Files" panel used to
    /// render four fixed tiles — including legacy JSON snapshot files
    /// (`memory_records`, `evidence_records`, `handoff`) — unconditionally.
    /// When those retired files were empty the panel showed
    /// "Memory Records 0 records 0 B / Evidence Records 0 records 0 B /
    /// Latest Handoff 0 B" right next to a populated native Memory Store,
    /// telling the operator memory was empty when it was rich. The fix only
    /// surfaces a legacy file when it actually has bytes, always shows the
    /// goals snapshot, and uses plain-language labels.
    #[test]
    fn index_html_memory_files_hides_empty_legacy_tiles() {
        // The old jargon label and the unconditional four-tile array are gone.
        assert!(
            !INDEX_HTML.contains("Goal Records (agent memory)"),
            "the legacy 'Goal Records (agent memory)' tile label must be \
             replaced with plain language"
        );

        // Legacy tiles are gated on real content so empty files never render.
        // The guard must reference size_bytes and the record count.
        assert!(
            INDEX_HTML.contains("legacyWithData"),
            "memory panel must filter legacy files to those with content"
        );
        assert!(
            INDEX_HTML.contains("(info.size_bytes||0)<=0"),
            "legacy file tiles must be gated on non-zero size_bytes so empty \
             '0 B' files never render (#1681)"
        );
        assert!(
            INDEX_HTML.contains("info.count<=0"),
            "legacy JSON files that report a count must have at least one \
             record to render, so an empty '[]' never shows '0 records' (#1681)"
        );

        // The single collapsed disclosure replaces the always-on tiles, using
        // plain language (no 'LadybugDB' jargon — it is the 'Memory Store').
        assert!(
            INDEX_HTML.contains("Legacy snapshots (superseded by the Memory Store)"),
            "legacy files must collapse into a single plain-language disclosure"
        );
        assert!(
            !INDEX_HTML.contains("superseded by LadybugDB"),
            "operator-facing labels must avoid the 'LadybugDB' jargon"
        );
    }

    /// #1681: the goals snapshot tile is always shown (it is sourced from
    /// cognitive memory, not a disk file) and links back to the Goals tab so
    /// an operator can reach the full board in one click.
    #[test]
    fn index_html_memory_files_goals_snapshot_links_to_goals_tab() {
        assert!(
            INDEX_HTML.contains("Goals (snapshot)"),
            "the goals snapshot tile must use the plain 'Goals (snapshot)' label"
        );
        let pos = INDEX_HTML
            .find("Goals (snapshot)")
            .expect("'Goals (snapshot)' tile must exist on the Memory tab");
        let window_end = (pos + 400).min(INDEX_HTML.len());
        let window = &INDEX_HTML[pos..window_end];
        assert!(
            window.contains("data-tab=goals"),
            "the goals snapshot tile must link to the Goals tab — window: {window:?}"
        );
    }

    /// #1681: "Last Memory Compaction" must not display the literal "Never"
    /// when no timestamp source exists — consolidation has demonstrably run,
    /// so "Never" is anti-information. The honest fallback is "Not tracked
    /// yet", and the absolute-timestamp branch still routes through formatTime.
    #[test]
    fn index_html_last_consolidation_not_never() {
        let pos = INDEX_HTML
            .find("Last Memory Compaction")
            .expect("'Last Memory Compaction' stat must exist on the Memory tab");
        let window_end = (pos + 400).min(INDEX_HTML.len());
        let window = &INDEX_HTML[pos..window_end];
        assert!(
            !window.contains("'Never'"),
            "Last Memory Compaction must not fall back to the literal 'Never' \
             — window: {window:?}"
        );
        assert!(
            window.contains("Not tracked yet"),
            "Last Memory Compaction must fall back to 'Not tracked yet' when \
             the timestamp source is missing — window: {window:?}"
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

    /// Sanity-check on the page-lede count: after the #2627 consolidation and
    /// the restored Memory tab there must be exactly 11 (one per top-level tab).
    /// If a refactor accidentally adds a 12th, we want to know immediately so we
    /// can decide whether the new container is actually a new tab or a misuse of
    /// the class (an absorbed panel should be an `<h2 class="subsection">`).
    #[test]
    fn index_html_has_exactly_eleven_page_intros() {
        let count = INDEX_HTML.matches(r#"class="page-lede""#).count();
        assert_eq!(
            count, 11,
            "expected exactly 11 page-lede paragraphs (one per top-level tab), got {count}"
        );
        let h1_count = INDEX_HTML.matches(r#"class="page-h1""#).count();
        assert_eq!(
            h1_count, 11,
            "expected exactly 11 page-h1 headings (one per top-level tab), got {h1_count}"
        );
    }

    /// #2419 / #2649 — the Overseer tab must be wired end-to-end in the SPA: a
    /// nav entry, a content panel, a fetch function that hits the auth-gated
    /// `/api/overseer` endpoint, and a background loader registered in the
    /// `TAB_LOADERS` registry so it is prefetched and refreshed automatically.
    #[test]
    fn index_html_wires_overseer_tab() {
        assert!(
            INDEX_HTML.contains(r#"data-tab="overseer""#),
            "Overseer nav entry missing"
        );
        assert!(
            INDEX_HTML.contains(r#"id="tab-overseer""#),
            "Overseer content panel missing"
        );
        assert!(
            INDEX_HTML.contains("/api/overseer"),
            "Overseer tab must fetch /api/overseer"
        );
        assert!(
            INDEX_HTML.contains("function fetchOverseer()"),
            "fetchOverseer() must be defined"
        );
        // #2649: the on-activate `runTabFetches` slug-branch chain was retired in
        // favour of the TAB_LOADERS background-prefetch registry, so the Overseer
        // tab is now wired by a registered loader entry rather than a
        // `slug==='overseer'` activation branch.
        assert!(
            INDEX_HTML.contains("'overseer':[{fn:fetchOverseer"),
            "Overseer tab must register fetchOverseer in the TAB_LOADERS \
             background-prefetch registry"
        );
    }

    /// #2419 — every value the Overseer tab interpolates into innerHTML must
    /// pass through `esc(...)` so a feed value that ever contained markup
    /// renders inert. This locks the XSS-safety contract for the tab.
    #[test]
    fn overseer_tab_escapes_all_interpolated_values() {
        let body = js_fn_body("function fetchOverseer()");
        // The thread id, health, timestamps, note, author, and per-tick text
        // are all attacker-influenceable in principle; each must be escaped.
        for needle in [
            "esc(summary)",
            "esc(t.id)",
            "esc(t.health||'—')",
            "esc(rec.timestamp||'')",
            "esc(overseerTickHuman(rec.report))",
            "esc(data.author_login||'—')",
        ] {
            assert!(
                body.contains(needle),
                "fetchOverseer must escape interpolated value via `{needle}`; \
                 an unescaped feed value is an XSS vector.\nbody:\n{body}"
            );
        }
        // And it must never inject a raw feed value straight into innerHTML.
        assert!(
            !body.contains("+data.author_login+"),
            "author_login must not be concatenated unescaped into innerHTML"
        );
    }

    /// #21 — the Overseer tab must render the informative per-tick DETAILS
    /// (what it observed + what it did), not only the summary one-liner. A
    /// dedicated `overseerTickDetails(r)` helper reads the two structured
    /// arrays the report now carries.
    #[test]
    fn overseer_tab_defines_and_wires_a_detail_renderer() {
        assert!(
            INDEX_HTML.contains("function overseerTickDetails("),
            "the SPA must define overseerTickDetails() to render observed/action details"
        );
        let details = js_fn_body("function overseerTickDetails(");
        assert!(
            details.contains("observed_details"),
            "overseerTickDetails must read the observed_details array:\n{details}"
        );
        assert!(
            details.contains("action_details"),
            "overseerTickDetails must read the action_details array:\n{details}"
        );
        // The recent-activity loop must actually call the detail renderer.
        let recent = js_fn_body("function fetchOverseer()");
        assert!(
            recent.contains("overseerTickDetails("),
            "fetchOverseer must invoke overseerTickDetails for each recent tick:\n{recent}"
        );
    }

    /// #21 — every detail string is attacker-influenceable feed content (repo
    /// slugs, issue URLs, blocked-goal reasons, anomaly text). Each must pass
    /// through `esc(...)`; a `</div><script>`-style payload must render inert.
    #[test]
    fn overseer_detail_renderer_escapes_every_string() {
        let details = js_fn_body("function overseerTickDetails(");
        assert!(
            details.contains("esc("),
            "overseerTickDetails must escape every interpolated detail string \
             via esc(...); an unescaped feed value is an XSS vector:\n{details}"
        );
        // No detail array element may be concatenated raw into innerHTML.
        for raw in ["+d+", "+line+", "+s+", "+detail+"] {
            assert!(
                !details.contains(raw),
                "a detail string must never be concatenated unescaped ('{raw}') \
                 into innerHTML:\n{details}"
            );
        }
    }

    // -------------------------------------------------------------------
    // Issue #1682 — Traces-tab cost rows must be human-readable.
    //
    // Before this fix each `[cost]` row rendered as three indent-padded
    // lines: the literal token `[cost]`, a raw ISO timestamp, and the
    // adapter brand (`copilot`) — no cost amount, model, tokens, or
    // attribution. These tests lock the readable replacement so a future
    // refactor can't silently regress the operator's cost-burn view.
    // -------------------------------------------------------------------

    /// Helper: extract a JS function body from `INDEX_HTML`, bounded by the
    /// next `function ` declaration (mirrors the formatTime/timeAgo tests).
    fn js_fn_body(decl: &str) -> &'static str {
        let start = INDEX_HTML
            .find(decl)
            .unwrap_or_else(|| panic!("expected JS helper `{decl}` in dashboard HTML"));
        let after = &INDEX_HTML[start..];
        let skip = decl.len();
        let end_rel = after[skip..]
            .find("function ")
            .map(|n| skip + n)
            .unwrap_or_else(|| after.len().min(1200));
        &after[..end_rel]
    }

    /// The Traces list must dispatch cost-ledger spans to a dedicated
    /// renderer (`renderCostTrace`) rather than the opaque generic row.
    #[test]
    fn index_html_traces_dispatch_cost_rows_to_dedicated_renderer() {
        assert!(
            INDEX_HTML.contains("s.source==='cost'?renderCostTrace(s.data):renderGenericTrace(s)"),
            "fetchTraces must route cost-source spans to renderCostTrace and \
             everything else to renderGenericTrace"
        );
        assert!(
            INDEX_HTML.contains("function renderCostTrace(data)"),
            "renderCostTrace(data) helper must exist"
        );
        assert!(
            INDEX_HTML.contains("function renderGenericTrace(s)"),
            "renderGenericTrace(s) helper must exist"
        );
    }

    /// "When": cost rows show a relative time via `timeAgo` plus the
    /// absolute timestamp via `formatTime` (surfaced as a hover title),
    /// never the old raw-ISO `substring(0,19)` slice.
    #[test]
    fn index_html_cost_trace_renders_relative_and_absolute_time() {
        let body = js_fn_body("function renderCostTrace(data){");
        assert!(
            body.contains("timeAgo(data.timestamp)"),
            "cost rows must render relative time via timeAgo — body: {body:?}"
        );
        assert!(
            body.contains("formatTime(data.timestamp)"),
            "cost rows must expose the absolute timestamp via formatTime — body: {body:?}"
        );
        assert!(
            !body.contains("substring(0,19)"),
            "cost rows must NOT slice a raw ISO string — that was the #1682 bug. body: {body:?}"
        );
    }

    /// "What": cost rows fold in the cost amount, token counts, and a
    /// plain-language model label (not just the bare adapter brand).
    #[test]
    fn index_html_cost_trace_renders_cost_model_and_tokens() {
        let body = js_fn_body("function renderCostTrace(data){");
        assert!(
            body.contains("fmtCostUsd(data.cost_usd_est)"),
            "cost rows must show the estimated USD cost — body: {body:?}"
        );
        assert!(
            body.contains("prompt_tokens_est") && body.contains("completion_tokens_est"),
            "cost rows must show prompt/completion token counts — body: {body:?}"
        );
        assert!(
            body.contains("costModelLabel(model)"),
            "cost rows must map the model token to a plain-language label — body: {body:?}"
        );
        // The label map must translate the common `copilot` brand into prose.
        assert!(
            INDEX_HTML.contains("'copilot':'Copilot SDK call'"),
            "costModelLabel must humanise the `copilot` adapter brand"
        );
    }

    /// "Who": cost rows surface per-call attribution — the call context
    /// and a shortened session id — so an operator can tell calls apart.
    #[test]
    fn index_html_cost_trace_renders_attribution() {
        let body = js_fn_body("function renderCostTrace(data){");
        assert!(
            body.contains("data.context"),
            "cost rows must surface the call context for attribution — body: {body:?}"
        );
        assert!(
            body.contains("shortSession(data.session_id)"),
            "cost rows must surface a shortened session id for attribution — body: {body:?}"
        );
    }

    /// Defense-in-depth (#2351): the cost row's hover `title` is a
    /// double-quoted HTML attribute fed by `abs`. `esc()` escapes
    /// `&<>` (element-content safe) but NOT `"`, so a quote-bearing
    /// `abs` would break out of the attribute. `formatTime` only ever
    /// returns its raw input on a parse failure, so `abs` MUST be
    /// guarded by a successful `parseTs` (mirroring `renderGenericTrace`)
    /// — never assigned unconditionally from `formatTime`. This makes
    /// the raw-passthrough branch unreachable in the attribute context.
    #[test]
    fn index_html_cost_trace_title_attr_is_parse_guarded() {
        let body = js_fn_body("function renderCostTrace(data){");
        // The absolute timestamp must be computed up front via parseTs…
        assert!(
            body.contains("parseTs(data.timestamp)"),
            "renderCostTrace must normalise the timestamp via parseTs so the \
             title-attribute value can be parse-guarded — body: {body:?}"
        );
        // …and `abs` must only call formatTime when the parse succeeded,
        // exactly like renderGenericTrace. The unguarded form
        // `abs=data.timestamp?formatTime(...)` would let a raw, unparseable
        // (possibly quote-bearing) timestamp reach the title attribute.
        assert!(
            body.contains("const abs=parsed?formatTime(data.timestamp):''"),
            "renderCostTrace must guard `abs` with the parse result \
             (`const abs=parsed?formatTime(data.timestamp):''`) so formatTime's \
             raw-input passthrough can never feed an unescaped quote into the \
             double-quoted title attribute (#2351) — body: {body:?}"
        );
        assert!(
            !body.contains("const abs=data.timestamp?formatTime"),
            "renderCostTrace must NOT assign `abs` directly from formatTime on a \
             truthy-but-unparsed timestamp — that is the #2351 attribute-injection \
             gap. body: {body:?}"
        );
        // The title attribute itself is still double-quoted and fed by `abs`,
        // so the guard above is what keeps it safe.
        assert!(
            body.contains(r#"'<span title="'+esc(abs)+'"#),
            "the cost-row hover title must remain a double-quoted attribute fed by \
             the parse-guarded `abs` — body: {body:?}"
        );
    }

    /// The Memory tab's recent-memories empty-state must distinguish "no NEW
    /// memories in the last hour" from "nothing stored, ever". Regression guard
    /// for the #2358 P1 finding: the panel told a human memory was empty
    /// ("No memories stored yet") while the store actually held tens of
    /// thousands of memories.
    #[test]
    fn index_html_recent_memories_empty_state_distinguishes_total() {
        let body = js_fn_body("async function fetchRecentMemories(){");
        // The empty branch is selected by the aggregate total, not shown blindly.
        assert!(
            body.contains("const total=d.total||0"),
            "fetchRecentMemories empty-state must read the aggregate total before \
             choosing copy (#2358) — body: {body:?}"
        );
        assert!(
            body.contains("total>0"),
            "fetchRecentMemories empty-state must branch on whether any memory is \
             stored (total>0) (#2358) — body: {body:?}"
        );
        // total>0 → say there are no NEW memories this hour, not that nothing exists.
        assert!(
            body.contains("No new memories in the last hour"),
            "when total>0 the empty-state must say there are no NEW memories in the \
             last hour, surfacing the stored total (#2358) — body: {body:?}"
        );
        // total==0 → keep the truthful original copy.
        assert!(
            body.contains("No memories stored yet"),
            "when total is zero the empty-state must still fall back to the \
             truthful 'No memories stored yet' copy (#2358) — body: {body:?}"
        );
    }

    /// The recent-memories aggregate total must render with thousands
    /// separators so large stored counts read as e.g. "32,342 total" rather
    /// than the raw, hard-to-scan "32342 total" (#2358).
    #[test]
    fn index_html_recent_memories_total_is_humanized() {
        let body = js_fn_body("async function fetchRecentMemories(){");
        assert!(
            body.contains("(d.total||0).toLocaleString()+' total'"),
            "the recent-memories stored total must be humanized via toLocaleString \
             (#2358) — body: {body:?}"
        );
        assert!(
            body.contains("'+total.toLocaleString()+' total stored.</span>'"),
            "the empty-state stored-total readout must also be humanized via \
             toLocaleString (#2358) — body: {body:?}"
        );
    }

    /// The fmtCostUsd helper keeps sub-cent estimates meaningful (4 dp)
    /// while showing larger amounts at 2 dp.
    #[test]
    fn index_html_has_fmt_cost_usd_helper() {
        let body = js_fn_body("function fmtCostUsd(v){");
        assert!(
            body.contains("toFixed(4)") && body.contains("toFixed(2)"),
            "fmtCostUsd must use 4dp for sub-cent and 2dp otherwise — body: {body:?}"
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

    // ---------------------------------------------------------------------
    // Issue #26 FIX 1 — the duplicative Overview "Open PRs" card is removed,
    // while the Merge Readiness card (its non-duplicative superset) is kept.
    // ---------------------------------------------------------------------

    /// The Overview "Open PRs" card duplicated the Merge Readiness card, so it
    /// was removed. Neither its heading nor its render target `open-prs-list`
    /// (used by both the markup `<div>` and the JS `getElementById`) may
    /// survive anywhere in the rendered dashboard.
    #[test]
    fn index_html_open_prs_card_removed() {
        assert!(
            !INDEX_HTML.contains("<h2>Open PRs</h2>"),
            "the duplicative Overview 'Open PRs' card heading must be removed (#26)"
        );
        assert!(
            !INDEX_HTML.contains("open-prs-list"),
            "the 'open-prs-list' element (card markup + its JS render block) must \
             be removed with the duplicative Open PRs card (#26)"
        );
    }

    /// Removing the Open PRs card must NOT touch the Merge Readiness card,
    /// which is the retained single source of open-PR state. Its container,
    /// refresh hook, and its OWN `d.open_prs` (from /api/merge-readiness — a
    /// different object than the removed /api/activity key) must all remain.
    #[test]
    fn index_html_merge_readiness_card_retained() {
        assert!(
            INDEX_HTML.contains(r#"data-testid="merge-readiness-card""#),
            "the Merge Readiness card must be retained after the Open PRs removal (#26)"
        );
        assert!(
            INDEX_HTML.contains("merge-readiness-panel"),
            "the Merge Readiness panel target must be retained (#26)"
        );
        assert!(
            INDEX_HTML.contains("fetchMergeReadiness"),
            "the Merge Readiness fetch/refresh hook must be retained (#26)"
        );
        assert!(
            INDEX_HTML.contains("Array.isArray(d.open_prs)"),
            "Merge Readiness reads its OWN open_prs from /api/merge-readiness — \
             that separate usage must survive the Overview-card removal (#26)"
        );
    }

    /// Issue #26 FIX 2 — the Memory tab must surface *live* consolidation
    /// activity, not just a single (previously stale) timestamp. The card
    /// renders the `recent_consolidation_activity` datum produced by
    /// `memory_metrics()` so it visibly changes as consolidation runs.
    #[test]
    fn index_html_memory_card_renders_live_consolidation_activity() {
        assert!(
            INDEX_HTML.contains("recent_consolidation_activity"),
            "the Memory tab must render the live 'recent_consolidation_activity' \
             datum so the operator sees consolidation actively running (#26)"
        );
    }

    // ===================================================================
    // Issue #2727 — deployment datetime in the dashboard header (PT/DST).
    //
    // TDD contract (Step 7): these tests are written BEFORE the
    // implementation and specify the behavior of the deployment-datetime
    // feature. Until `routes::{format_deployed_pt, deployed_timestamp_utc,
    // deployed_pt}` and the header JS append exist, they fail — first as a
    // compile error for the missing helpers, then (once stubs exist) as
    // assertion failures. They pass only when the feature is implemented
    // per the design spec.
    //
    // The pure formatter `format_deployed_pt` owns ALL timezone/DST logic
    // and is exercised with FIXED UTC instants so the assertions are
    // deterministic and independent of the build machine's clock/timezone.
    // Two of the cases (summer PDT, winter PST) directly prove daylight-
    // saving handling; two more pin the exact PST<->PDT transition edges.
    // ===================================================================

    /// Build a fixed `DateTime<Utc>` from an RFC3339 string for use as a
    /// deterministic test instant.
    fn utc(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .expect("valid RFC3339 test instant")
            .with_timezone(&chrono::Utc)
    }

    /// Summer instant → America/Los_Angeles is Pacific DAYLIGHT time
    /// (UTC-7). Proves the abbreviation is the date-correct daylight name,
    /// NOT a hardcoded "PST" or a fixed -08:00 offset.
    #[test]
    fn deployment_datetime_summer_renders_pdt() {
        // 2026-07-06 18:03:00 UTC == 11:03 local in Los Angeles (PDT, UTC-7).
        let dt = utc("2026-07-06T18:03:00Z");
        assert_eq!(
            format_deployed_pt(dt),
            "2026-07-06 11:03 PDT",
            "July in Los Angeles is Pacific DAYLIGHT time (UTC-7); the header \
             must show 11:03 PDT, proving DST is applied (not hardcoded PST)"
        );
    }

    /// Winter instant → America/Los_Angeles is Pacific STANDARD time
    /// (UTC-8).
    #[test]
    fn deployment_datetime_winter_renders_pst() {
        // 2026-01-06 19:03:00 UTC == 11:03 local in Los Angeles (PST, UTC-8).
        let dt = utc("2026-01-06T19:03:00Z");
        assert_eq!(
            format_deployed_pt(dt),
            "2026-01-06 11:03 PST",
            "January in Los Angeles is Pacific STANDARD time (UTC-8); the \
             header must show 11:03 PST"
        );
    }

    /// Spring-forward edge: at 2026-03-08 02:00 local the clock jumps to
    /// 03:00 PDT. An instant just after the gap must render in PDT (UTC-7).
    #[test]
    fn deployment_datetime_spring_forward_edge_is_pdt() {
        // 2026-03-08 10:00 UTC is the 02:00 PST -> 03:00 PDT jump.
        // 10:30 UTC therefore == 03:30 PDT (UTC-7), just past the gap.
        let dt = utc("2026-03-08T10:30:00Z");
        assert_eq!(
            format_deployed_pt(dt),
            "2026-03-08 03:30 PDT",
            "just after the spring-forward gap the zone is PDT (UTC-7)"
        );
    }

    /// Fall-back edge: at 2026-11-01 02:00 local the clock falls back to
    /// 01:00 PST. An instant just after the transition must render in PST
    /// (UTC-8).
    #[test]
    fn deployment_datetime_fall_back_edge_is_pst() {
        // 2026-11-01 09:00 UTC is the 02:00 PDT -> 01:00 PST fall-back.
        // 09:30 UTC therefore == 01:30 PST (UTC-8), just past the transition.
        let dt = utc("2026-11-01T09:30:00Z");
        assert_eq!(
            format_deployed_pt(dt),
            "2026-11-01 01:30 PST",
            "just after the fall-back transition the zone is PST (UTC-8)"
        );
    }

    /// The formatter's output shape is exactly `YYYY-MM-DD HH:MM ABBR`
    /// (24-hour, minute precision, single-space separated, PST/PDT
    /// abbreviation). This is the shape rendered into the header and the
    /// additive `/api/status` `deployed` field.
    #[test]
    fn deployment_datetime_format_shape_is_stable() {
        let s = format_deployed_pt(utc("2026-07-06T18:03:00Z"));
        let parts: Vec<&str> = s.split(' ').collect();
        assert_eq!(parts.len(), 3, "expected `date time abbr`, got {s:?}");
        assert_eq!(parts[0].len(), 10, "date part must be YYYY-MM-DD: {s:?}");
        assert_eq!(parts[1].len(), 5, "time part must be HH:MM: {s:?}");
        assert!(
            parts[2] == "PST" || parts[2] == "PDT",
            "abbreviation must be PST or PDT (real DST-aware zone), got {s:?}"
        );
    }

    /// build.rs bakes a compile-time deployment timestamp into the binary
    /// (issue #2727), mirroring SIMARD_BUILD_NUMBER / SIMARD_GIT_HASH. A
    /// normal build therefore always exposes it, so the `deployed` datetime
    /// is available to the header by default (real signal, not a placeholder).
    #[test]
    fn deployment_timestamp_is_baked_in_at_build_time() {
        assert!(
            deployed_timestamp_utc().is_some(),
            "build.rs must emit SIMARD_BUILD_TIMESTAMP so the running binary \
             knows when it was built/deployed (#2727)"
        );
        assert!(
            deployed_pt().is_some(),
            "with the build timestamp baked in, the header-ready `deployed` \
             string must be available (#2727)"
        );
    }

    /// End-to-end env path: `deployed_pt()` is exactly the string surfaced as
    /// the additive `/api/status` `deployed` field. When the compile-time
    /// timestamp is present it must equal `format_deployed_pt` applied to the
    /// parsed instant and carry the stable shape. When absent (unusual
    /// toolchains) the pipeline degrades to `None` — the back-compatible,
    /// silently-omitted contract.
    #[test]
    fn deployed_pt_matches_formatter_over_env_timestamp() {
        match deployed_timestamp_utc() {
            Some(dt) => {
                let composed =
                    deployed_pt().expect("deployed_pt() must be Some when the timestamp parses");
                assert_eq!(
                    composed,
                    format_deployed_pt(dt),
                    "deployed_pt() must equal format_deployed_pt(deployed_timestamp_utc())"
                );
                let parts: Vec<&str> = composed.split(' ').collect();
                assert_eq!(parts.len(), 3, "deployed string shape: {composed:?}");
                assert!(
                    parts[2] == "PST" || parts[2] == "PDT",
                    "deployed abbreviation must be PST or PDT: {composed:?}"
                );
            }
            None => {
                assert!(
                    deployed_pt().is_none(),
                    "deployed_pt() must be None when the build-timestamp env is absent"
                );
            }
        }
    }

    /// Header rendering contract (issue #2727): the dashboard header must show
    /// BOTH the build/version number (existing behavior) AND the deployment
    /// datetime, sourced from the additive `/api/status` `deployed` field. The
    /// header JS reads `d.deployed` and appends it to the `header-version`
    /// element alongside the existing `v<version> (<hash>)` build string.
    #[test]
    fn index_html_header_shows_build_number_and_deployment_datetime() {
        // Build-number location is retained (this change is additive).
        assert!(
            INDEX_HTML.contains("header-version"),
            "the header build-number element must be retained (#2727 is additive)"
        );
        assert!(
            INDEX_HTML.contains("d.version"),
            "the header must keep rendering the build/version number (#2727)"
        );
        // Deployment datetime is now rendered from the additive `deployed` field.
        assert!(
            INDEX_HTML.contains("d.deployed"),
            "the header JS must render the deployment datetime from the additive \
             /api/status `deployed` field, alongside the build number (#2727)"
        );
    }

    /// Integration wiring (issue #2727) — closes the two review-flagged gaps the
    /// pure-helper unit tests above cannot reach, in a single end-to-end test
    /// against the REAL `build_router()`:
    ///
    /// * **Security:** an *unauthenticated* `GET /api/status` must be denied
    ///   with `401`, so the additive `deployed` field never leaks past the auth
    ///   layer (defends the posture against future router/middleware refactors).
    /// * **Philosophy:** the private `status()` handler must actually *wire* the
    ///   `deployed` string into the response JSON when the compile-time build
    ///   timestamp is present — the single `json!()` insertion the unit tests
    ///   can't exercise. The surfaced value must equal the canonical
    ///   `deployed_pt()` and carry the DST-aware `YYYY-MM-DD HH:MM PST|PDT` shape.
    ///
    /// Runs the router over an ephemeral loopback server and speaks raw HTTP/1.1
    /// so no extra test dependency is needed. Authenticates via the deterministic
    /// `SIMARD_DASHBOARD_TOKEN` bearer path (independent of the process-global
    /// `LOGIN_CODE` value). Carries the `cognitive_memory` serial key because it
    /// mutates a process-global env var and the `status` handler reads the
    /// state-root env — the #2360/#2375 env-tearing surface.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn api_status_denies_unauth_and_wires_deployed_2727() {
        use crate::operator_commands_dashboard::auth;
        use std::net::SocketAddr;
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // One-shot HTTP/1.1 GET over a raw socket: returns (status_code, body).
        // `Connection: close` lets the server delimit the body by EOF so
        // `read_to_end` completes without an HTTP client dependency.
        async fn http_get(addr: SocketAddr, path: &str, bearer: Option<&str>) -> (u16, String) {
            let mut stream = tokio::net::TcpStream::connect(addr)
                .await
                .expect("connect to ephemeral dashboard server");
            let mut req =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
            if let Some(b) = bearer {
                req.push_str(&format!("Authorization: Bearer {b}\r\n"));
            }
            req.push_str("\r\n");
            stream
                .write_all(req.as_bytes())
                .await
                .expect("write request");
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await.expect("read response");
            let text = String::from_utf8_lossy(&raw).into_owned();
            let code = text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(0);
            let body = text
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            (code, body)
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind ephemeral loopback port");
            let addr: SocketAddr = listener.local_addr().expect("local addr");
            let server = tokio::spawn(async move {
                let _ = axum::serve(listener, build_router()).await;
            });

            // --- Security: unauthenticated request is denied (401). ---
            let (unauth_code, _) =
                tokio::time::timeout(Duration::from_secs(30), http_get(addr, "/api/status", None))
                    .await
                    .expect("unauthenticated /api/status request timed out");
            assert_eq!(
                unauth_code, 401,
                "unauthenticated GET /api/status must be denied (401) — the additive \
                 `deployed` field must never bypass the auth layer"
            );

            // --- Authenticated: the `deployed` field is wired into the JSON. ---
            // `require_auth` first requires a login code to be configured, then
            // accepts a bearer equal to SIMARD_DASHBOARD_TOKEN. Configure both;
            // the bearer path is deterministic regardless of the LOGIN_CODE value.
            auth::init_login_code();
            let token = "itest-deployed-2727";
            // SAFETY: env mutation is serialised by `#[serial_test::serial(
            // cognitive_memory)]`; the var is set before the request that reads it
            // and cleared only after the full response has been received.
            unsafe { std::env::set_var("SIMARD_DASHBOARD_TOKEN", token) };

            let (ok_code, body) = tokio::time::timeout(
                Duration::from_secs(30),
                http_get(addr, "/api/status", Some(token)),
            )
            .await
            .expect("authenticated /api/status request timed out");

            // SAFETY: see the paired set_var above; the server has finished
            // handling the request (its response was fully read) before we clear it.
            unsafe { std::env::remove_var("SIMARD_DASHBOARD_TOKEN") };

            assert_eq!(
                ok_code, 200,
                "authenticated GET /api/status must succeed (200); body={body:?}"
            );
            let json: serde_json::Value =
                serde_json::from_str(&body).expect("/api/status must return a JSON object");

            // The test binary bakes SIMARD_BUILD_TIMESTAMP via build.rs, so
            // deployed_pt() is Some here and status() must surface exactly it.
            let expected = deployed_pt()
                .expect("deployed_pt() is Some in a normal build (build.rs bakes the timestamp)");
            let deployed = json.get("deployed").and_then(|v| v.as_str()).expect(
                "status() must wire the `deployed` field into the JSON when the build \
                 timestamp is present (#2727)",
            );
            assert_eq!(
                deployed, expected,
                "the wired `deployed` field must equal the canonical deployed_pt()"
            );
            let parts: Vec<&str> = deployed.split(' ').collect();
            assert_eq!(
                parts.len(),
                3,
                "deployed must be `YYYY-MM-DD HH:MM PST|PDT`: {deployed:?}"
            );
            assert!(
                parts[2] == "PST" || parts[2] == "PDT",
                "the wired `deployed` must carry a DST-aware PST/PDT abbreviation: {deployed:?}"
            );

            server.abort();
        });
    }
}
