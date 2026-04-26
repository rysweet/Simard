use axum::Json;
use serde_json::{Value, json};

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

    // Discover available VMs via `azlin list --json` (best-effort, with timeout).
    let mut discovered: Vec<Value> = tokio::task::spawn_blocking(|| {
        let output = std::process::Command::new("azlin")
            .args(["list", "--output", "json"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let raw = String::from_utf8_lossy(&o.stdout);
                // azlin may print version warnings before JSON — find the first '['
                let json_start = raw.find('[').unwrap_or(0);
                serde_json::from_str::<Vec<Value>>(&raw[json_start..]).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    })
    .await
    .unwrap_or_default();

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
