use axum::Json;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Hard bound on the blocking `azlin list` VM-discovery call. `azlin list`
/// queries Azure and routinely takes 10–20s; without a bound the `/api/hosts`
/// handler blocks the Hosts tab (and any API client) for the full duration —
/// or indefinitely if `azlin` hangs. Mirrors the `TMUX_LIST_TIMEOUT_SECS`
/// bound already applied on the tmux-sessions path.
///
/// Overridable via `SIMARD_AZLIN_LIST_TIMEOUT_SECS` (positive integer seconds)
/// so tests can drive the timeout path deterministically without waiting the
/// full production bound.
const AZLIN_LIST_TIMEOUT_SECS: u64 = 20;

/// Environment override for the discovery timeout.
const AZLIN_LIST_TIMEOUT_ENV: &str = "SIMARD_AZLIN_LIST_TIMEOUT_SECS";

/// Pure: resolve the discovery timeout from an optional raw override string,
/// falling back to `default_secs`. A missing, non-numeric, or non-positive
/// value uses the default (never a zero/instant timeout).
fn resolve_timeout_secs(raw: Option<&str>, default_secs: u64) -> Duration {
    let secs = raw
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}

/// The effective discovery timeout, honouring the env override.
fn azlin_list_timeout() -> Duration {
    resolve_timeout_secs(
        std::env::var(AZLIN_LIST_TIMEOUT_ENV).ok().as_deref(),
        AZLIN_LIST_TIMEOUT_SECS,
    )
}

/// How long a successful `azlin list` result is served from the in-process
/// cache before a fresh discovery is attempted. Keeps the Hosts tab responsive
/// on refresh (and on concurrent clients) instead of re-running the slow Azure
/// query on every request.
const DISCOVERY_CACHE_TTL_SECS: u64 = 60;

/// Cached VM-discovery result plus the instant it was fetched.
struct DiscoveryCache {
    fetched_at: Instant,
    discovered: Vec<Value>,
}

/// Process-global cache for the last successful `azlin list` discovery.
fn discovery_cache() -> &'static Mutex<Option<DiscoveryCache>> {
    static CACHE: OnceLock<Mutex<Option<DiscoveryCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Pure: `true` when a cache entry fetched at `fetched_at` is still within
/// `ttl` as of `now`.
fn cache_is_fresh(fetched_at: Instant, now: Instant, ttl: Duration) -> bool {
    now.duration_since(fetched_at) < ttl
}

/// Pure: parse `azlin list --output json` stdout into a Vec of VM entries.
/// `azlin` may print version/update warnings before the JSON array, so we skip
/// to the first `[`. Any parse failure degrades to an empty list.
fn parse_azlin_list(raw: &str) -> Vec<Value> {
    let json_start = raw.find('[').unwrap_or(0);
    serde_json::from_str::<Vec<Value>>(&raw[json_start..]).unwrap_or_default()
}

/// Blocking: run `azlin list --output json` and parse the discovered VMs.
/// Best-effort — returns an empty list when `azlin` is missing, fails, or
/// emits unparseable output.
fn run_azlin_list() -> Vec<Value> {
    let output = std::process::Command::new("azlin")
        .args(["list", "--output", "json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => parse_azlin_list(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Discovery outcome: the VM list plus whether it was served after a timeout
/// (`timed_out`) and whether the served list is a stale cached copy (`stale`).
struct Discovery {
    hosts: Vec<Value>,
    timed_out: bool,
    stale: bool,
}

/// Resolve the discovered-VM list: serve a fresh cache hit immediately,
/// otherwise run `azlin list` under a hard timeout, falling back to a stale
/// cached copy (flagged) rather than blocking the Hosts tab indefinitely.
async fn discover_vms() -> Discovery {
    // Fast path: a fresh cached result. Read + clone under the lock, then drop
    // the guard before any await (never hold a std Mutex across `.await`).
    if let Some(hosts) = discovery_cache().lock().ok().and_then(|guard| {
        guard.as_ref().and_then(|c| {
            cache_is_fresh(
                c.fetched_at,
                Instant::now(),
                Duration::from_secs(DISCOVERY_CACHE_TTL_SECS),
            )
            .then(|| c.discovered.clone())
        })
    }) {
        return Discovery {
            hosts,
            timed_out: false,
            stale: false,
        };
    }

    // Slow path: run `azlin list` under a hard timeout. If the timeout fires
    // first, the `spawn_blocking` thread keeps running to completion in the
    // background (blocking tasks aren't cancellable) — its result is simply
    // discarded. This is bounded because the Hosts tab fetches on demand
    // (no auto-poll) and successful results are cached below.
    let res = tokio::time::timeout(
        azlin_list_timeout(),
        tokio::task::spawn_blocking(run_azlin_list),
    )
    .await;

    match res {
        Ok(Ok(list)) => {
            // Refresh the cache only with a non-empty result. An empty list
            // means azlin is missing/failed or genuinely has no VMs; caching
            // it would mask real VMs for the TTL once azlin recovers, so we
            // let the next request re-attempt discovery instead.
            if !list.is_empty()
                && let Ok(mut guard) = discovery_cache().lock()
            {
                *guard = Some(DiscoveryCache {
                    fetched_at: Instant::now(),
                    discovered: list.clone(),
                });
            }
            Discovery {
                hosts: list,
                timed_out: false,
                stale: false,
            }
        }
        // Timeout or join error: fall back to a stale cached copy if we have
        // one, so the tab still shows the last-known VMs instead of blanking.
        _ => {
            let stale_hosts = discovery_cache()
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|c| c.discovered.clone()));
            match stale_hosts {
                Some(hosts) => Discovery {
                    hosts,
                    timed_out: true,
                    stale: true,
                },
                None => Discovery {
                    hosts: Vec::new(),
                    timed_out: true,
                    stale: false,
                },
            }
        }
    }
}

/// Hosts config file path.
pub(crate) fn hosts_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    std::path::PathBuf::from(home)
        .join(".simard")
        .join("hosts.json")
}

pub(crate) fn load_hosts() -> Vec<Value> {
    let path = hosts_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_else(|_| "[]".to_string());
    serde_json::from_str(&content).unwrap_or_default()
}

pub(crate) fn save_hosts(hosts: &[Value]) -> std::io::Result<()> {
    let path = hosts_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(hosts).unwrap_or_default(),
    )
}

/// Compare two hostnames as short, case-insensitive names.
///
/// Strips the first dot onward (FQDN suffix) on both sides and lowercases
/// before comparing. Empty inputs never match (guards against false positives
/// when `/etc/hostname` is unreadable or an entry has no name).
///
/// **Security: This is a UI hint only — MUST NOT be used for authorization
/// decisions.** Hostnames are user-controlled and easily spoofed.
pub(crate) fn is_local_host(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let short = |s: &str| -> String { s.split('.').next().unwrap_or("").to_ascii_lowercase() };
    let sa = short(a);
    let sb = short(b);
    !sa.is_empty() && sa == sb
}

/// Extract the host "name" field from a host entry, accepting either lowercase
/// `name` (from `hosts.json`) or capitalized `Name` (from some `azlin list` outputs).
pub(crate) fn host_entry_name(entry: &Value) -> &str {
    entry
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| entry.get("Name").and_then(|v| v.as_str()))
        .unwrap_or("")
}

/// Tag each Azlin host entry in `hosts` with `is_local: true` when:
///   1. the local hostname matches the entry's name (short, case-insensitive), and
///   2. the entry also appears in `cluster_members` (i.e. it has actually joined
///      the cluster, not just been listed by azlin).
///
/// `cluster_members` is the list of host-name strings reported as currently
/// joined to the cluster (e.g. configured remote VMs from `hosts.json`). The
/// `local_hostname` is injected so this function is unit-testable without
/// depending on `/etc/hostname`.
///
/// **Security: This is a UI hint only — MUST NOT be used for authorization
/// decisions.** Hostnames are user-controlled and easily spoofed.
pub(crate) fn tag_local_membership(
    hosts: &mut [Value],
    cluster_members: &[String],
    local_hostname: &str,
) {
    let in_cluster =
        |name: &str| -> bool { cluster_members.iter().any(|m| is_local_host(m, name)) };
    for entry in hosts.iter_mut() {
        let name = host_entry_name(entry).to_string();
        let joined = is_local_host(local_hostname, &name) && in_cluster(&name);
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("is_local".to_string(), Value::Bool(joined));
        }
    }
}

pub(crate) async fn get_hosts() -> Json<Value> {
    let mut configured = load_hosts();

    // Discover available VMs via `azlin list` (best-effort, hard-timeout,
    // short-cached). See `discover_vms` — `azlin list` queries Azure and can
    // take 10–20s, so it must never block the Hosts tab unbounded.
    let discovery = discover_vms().await;
    let mut discovered = discovery.hosts;

    // Tag entries matching the local daemon's hostname so the dashboard can
    // render a "joined" badge. UI hint only — do not use for authorization.
    let local = crate::agent_registry::hostname();

    // Cluster members = configured hosts from hosts.json (the canonical
    // membership list). A host is shown as "joined" only when the local
    // hostname matches a member of this list — i.e. localhost has actually
    // joined the cluster, not merely been discovered by `azlin list`.
    let cluster_members: Vec<String> = configured
        .iter()
        .map(|e| host_entry_name(e).to_string())
        .filter(|s| !s.is_empty())
        .collect();

    tag_local_membership(&mut configured, &cluster_members, &local);
    tag_local_membership(&mut discovered, &cluster_members, &local);

    Json(json!({
        "hosts": configured,
        "discovered": discovered,
        "local_hostname": local,
        // Additive observability so the UI can distinguish "no VMs / azlin
        // absent" from "discovery was skipped because azlin was too slow".
        "discovery_timed_out": discovery.timed_out,
        "discovery_stale": discovery.stale,
    }))
}

pub(crate) async fn add_host(Json(body): Json<Value>) -> Json<Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let rg = body
        .get("resource_group")
        .and_then(|v| v.as_str())
        .unwrap_or("rysweet-linux-vm-pool");
    if name.is_empty() {
        return Json(json!({"error": "name is required"}));
    }
    let mut hosts = load_hosts();
    if hosts
        .iter()
        .any(|h| h.get("name").and_then(|v| v.as_str()) == Some(name))
    {
        return Json(json!({"error": format!("host '{name}' already exists")}));
    }
    hosts.push(json!({
        "name": name,
        "resource_group": rg,
        "added_at": chrono::Utc::now().to_rfc3339(),
    }));
    match save_hosts(&hosts) {
        Ok(_) => Json(json!({"status": "ok", "hosts": hosts})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn remove_host(Json(body): Json<Value>) -> Json<Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut hosts = load_hosts();
    let before = hosts.len();
    hosts.retain(|h| h.get("name").and_then(|v| v.as_str()) != Some(name));
    if hosts.len() == before {
        return Json(json!({"error": format!("host '{name}' not found")}));
    }
    match save_hosts(&hosts) {
        Ok(_) => Json(json!({"status": "ok", "hosts": hosts})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_local_host ----------------------------------------------------

    #[test]
    fn same_hostname_matches() {
        assert!(is_local_host("myhost", "myhost"));
    }

    #[test]
    fn case_insensitive_match() {
        assert!(is_local_host("MyHost", "myhost"));
        assert!(is_local_host("myhost", "MYHOST"));
    }

    #[test]
    fn fqdn_stripped_before_compare() {
        assert!(is_local_host("myhost.example.com", "myhost"));
        assert!(is_local_host("myhost", "myhost.corp.net"));
        assert!(is_local_host("MYHOST.DOMAIN.COM", "myhost.other.org"));
    }

    #[test]
    fn empty_input_never_matches() {
        assert!(!is_local_host("", "myhost"));
        assert!(!is_local_host("myhost", ""));
        assert!(!is_local_host("", ""));
    }

    #[test]
    fn different_hostnames_do_not_match() {
        assert!(!is_local_host("host-a", "host-b"));
        assert!(!is_local_host("alpha.example.com", "beta.example.com"));
    }

    #[test]
    fn dot_only_input_returns_false() {
        assert!(!is_local_host(".", "myhost"));
        assert!(!is_local_host("myhost", "."));
    }

    // ---- host_entry_name --------------------------------------------------

    #[test]
    fn extracts_lowercase_name() {
        let entry = json!({"name": "worker-1", "resource_group": "rg"});
        assert_eq!(host_entry_name(&entry), "worker-1");
    }

    #[test]
    fn extracts_capitalized_name() {
        let entry = json!({"Name": "Worker-2"});
        assert_eq!(host_entry_name(&entry), "Worker-2");
    }

    #[test]
    fn prefers_lowercase_over_capitalized() {
        let entry = json!({"name": "lower", "Name": "Upper"});
        assert_eq!(host_entry_name(&entry), "lower");
    }

    #[test]
    fn returns_empty_when_no_name_field() {
        let entry = json!({"host": "something"});
        assert_eq!(host_entry_name(&entry), "");
    }

    #[test]
    fn returns_empty_for_null_name() {
        let entry = json!({"name": null});
        assert_eq!(host_entry_name(&entry), "");
    }

    // ---- tag_local_membership ---------------------------------------------

    #[test]
    fn tags_local_host_as_joined_when_in_cluster() {
        let mut hosts = vec![json!({"name": "myhost"})];
        let cluster = vec!["myhost".to_string()];
        tag_local_membership(&mut hosts, &cluster, "myhost");
        assert_eq!(hosts[0]["is_local"], json!(true));
    }

    #[test]
    fn tags_non_local_host_as_not_joined() {
        let mut hosts = vec![json!({"name": "remote-vm"})];
        let cluster = vec!["remote-vm".to_string()];
        tag_local_membership(&mut hosts, &cluster, "myhost");
        assert_eq!(hosts[0]["is_local"], json!(false));
    }

    #[test]
    fn tags_local_host_not_in_cluster_as_not_joined() {
        let mut hosts = vec![json!({"name": "myhost"})];
        let cluster: Vec<String> = vec![];
        tag_local_membership(&mut hosts, &cluster, "myhost");
        assert_eq!(hosts[0]["is_local"], json!(false));
    }

    #[test]
    fn tags_multiple_hosts_correctly() {
        let mut hosts = vec![json!({"name": "local-vm"}), json!({"name": "remote-vm"})];
        let cluster = vec!["local-vm".to_string(), "remote-vm".to_string()];
        tag_local_membership(&mut hosts, &cluster, "local-vm");
        assert_eq!(hosts[0]["is_local"], json!(true));
        assert_eq!(hosts[1]["is_local"], json!(false));
    }

    #[test]
    fn tag_works_with_fqdn_hostname() {
        let mut hosts = vec![json!({"name": "myhost"})];
        let cluster = vec!["myhost".to_string()];
        tag_local_membership(&mut hosts, &cluster, "myhost.example.com");
        assert_eq!(hosts[0]["is_local"], json!(true));
    }

    #[test]
    fn empty_hosts_is_noop() {
        let mut hosts: Vec<Value> = vec![];
        tag_local_membership(&mut hosts, &["a".to_string()], "a");
        assert!(hosts.is_empty());
    }

    // ---- cache_is_fresh ---------------------------------------------------

    #[test]
    fn cache_is_fresh_within_ttl() {
        let now = Instant::now();
        let fetched = now - Duration::from_secs(10);
        assert!(cache_is_fresh(fetched, now, Duration::from_secs(60)));
    }

    #[test]
    fn cache_is_stale_past_ttl() {
        let now = Instant::now();
        let fetched = now - Duration::from_secs(120);
        assert!(!cache_is_fresh(fetched, now, Duration::from_secs(60)));
    }

    #[test]
    fn cache_at_exact_ttl_is_stale() {
        // Strictly-less-than semantics: a cache exactly `ttl` old is stale.
        let now = Instant::now();
        let fetched = now - Duration::from_secs(60);
        assert!(!cache_is_fresh(fetched, now, Duration::from_secs(60)));
    }

    // ---- parse_azlin_list -------------------------------------------------

    #[test]
    fn parses_clean_json_array() {
        let raw = r#"[{"name":"vm-a"},{"name":"vm-b"}]"#;
        let out = parse_azlin_list(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], json!("vm-a"));
    }

    #[test]
    fn parses_json_after_version_warning_preamble() {
        // azlin prints update warnings on stderr, but may also leak a banner
        // to stdout before the JSON array — we skip to the first '['.
        let raw =
            "A newer version of azlin is available. Run 'azlin update'.\n[{\"name\":\"vm-a\"}]";
        let out = parse_azlin_list(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], json!("vm-a"));
    }

    #[test]
    fn unparseable_output_degrades_to_empty() {
        assert!(parse_azlin_list("not json at all").is_empty());
        assert!(parse_azlin_list("").is_empty());
        assert!(parse_azlin_list("[not valid json").is_empty());
    }

    // ---- resolve_timeout_secs ---------------------------------------------

    #[test]
    fn timeout_uses_default_when_unset() {
        assert_eq!(resolve_timeout_secs(None, 20), Duration::from_secs(20));
    }

    #[test]
    fn timeout_honours_valid_override() {
        assert_eq!(resolve_timeout_secs(Some("2"), 20), Duration::from_secs(2));
        assert_eq!(
            resolve_timeout_secs(Some("  5 "), 20),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn timeout_rejects_zero_and_garbage() {
        // Zero would mean an instant timeout — always fall back to the default.
        assert_eq!(resolve_timeout_secs(Some("0"), 20), Duration::from_secs(20));
        assert_eq!(
            resolve_timeout_secs(Some("abc"), 20),
            Duration::from_secs(20)
        );
        assert_eq!(resolve_timeout_secs(Some(""), 20), Duration::from_secs(20));
        assert_eq!(
            resolve_timeout_secs(Some("-3"), 20),
            Duration::from_secs(20)
        );
    }
}
