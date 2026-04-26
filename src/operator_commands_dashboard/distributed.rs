use axum::Json;
use serde_json::{Value, json};

use super::hosts::{host_entry_name, load_hosts, save_hosts};

/// Map the configured hosts list (the canonical source used by the Cluster
/// Topology panel via `load_hosts()`) into the entries rendered by the
/// Remote VMs panel.
///
/// Pure function — no I/O, no SSH, no filesystem access. Each input host
/// produces exactly one output entry with `vm_name` taken from `host.name`,
/// `resource_group` from `host.resource_group` (empty string if absent), and
/// `status` initialized to `"unknown"`. The caller is responsible for
/// enriching individual entries with probe data (e.g., `check_vm.sh`).
///
/// Empty input yields empty output; the frontend renders this as
/// "No remote VMs configured".
pub(crate) fn remote_vms_from_hosts(hosts: &[Value]) -> Vec<Value> {
    hosts
        .iter()
        .filter_map(|h| {
            // Use the same name-extraction helper as the Cluster Topology
            // panel (`host_entry_name`) so the two panels never disagree on
            // which entries are present — including entries that use the
            // legacy "Name" capitalization.
            let name = host_entry_name(h);
            if name.is_empty() {
                return None;
            }
            let resource_group = h
                .get("resource_group")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(json!({
                "vm_name": name,
                "resource_group": resource_group,
                "status": "unknown",
            }))
        })
        .collect()
}

pub(crate) async fn distributed() -> Json<Value> {
    // Query the Simard VM status via azlin connect with a timeout so the
    // dashboard doesn't hang if the bastion is slow.
    //
    // We use `systemd-run --user --pipe` to run the check script in a fresh
    // transient scope.  When azlin runs as a direct child of the daemon's
    // service cgroup, the bastion SSH produces empty stdout (the daemon's
    // inherited pipe/socket FDs or cgroup restrictions interfere with
    // azlin's PTY routing).  Running in a separate scope avoids this.
    let vm_status = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(|| {
            let state_root = std::env::var("SIMARD_STATE_ROOT").unwrap_or_else(|_| {
                format!(
                    "{}/.simard",
                    std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into())
                )
            });
            let script = format!("{}/bin/check_vm.sh", state_root);
            std::process::Command::new("systemd-run")
                .args(["--user", "--pipe", "--quiet", &script])
                .output()
        }),
    )
    .await;

    let mut vm_info = json!({
        "vm_name": "Simard",
        "resource_group": "",
        "status": "unknown",
    });

    match vm_status {
        Ok(Ok(Ok(output))) => {
            let raw_stdout = String::from_utf8_lossy(&output.stdout);
            let raw_stderr = String::from_utf8_lossy(&output.stderr);
            // azlin connect --no-tmux routes remote stdout to local stderr
            // when spawned without a TTY (rysweet/azlin#980). Strip ANSI
            // escape codes then search both streams for our KEY=value markers.
            let stdout = strip_ansi_codes(&raw_stdout);
            let stderr = strip_ansi_codes(&raw_stderr);
            let haystack = if stdout.contains("HOSTNAME=") {
                stdout
            } else if stderr.contains("HOSTNAME=") {
                stderr
            } else {
                // Last resort: combine both in case markers are split across streams
                let combined = format!("{}\n{}", stdout, stderr);
                if combined.contains("HOSTNAME=") {
                    combined
                } else {
                    String::new()
                }
            };
            if !haystack.is_empty() {
                vm_info["status"] = json!("reachable");
                for line in haystack.lines() {
                    if let Some((key, val)) = line.split_once('=') {
                        let key = key.trim().to_lowercase();
                        let val = val.trim();
                        match key.as_str() {
                            "hostname" => vm_info["hostname"] = json!(val),
                            "uptime" => vm_info["uptime"] = json!(val),
                            "disk_root" => {
                                vm_info["disk_root_pct"] = json!(val.parse::<u32>().ok());
                            }
                            "disk_data" => {
                                vm_info["disk_data_pct"] = json!(val.parse::<u32>().ok());
                            }
                            "disk_tmp" => vm_info["disk_tmp_pct"] = json!(val.parse::<u32>().ok()),
                            "simard_procs" => {
                                vm_info["simard_processes"] = json!(val.parse::<u32>().ok());
                            }
                            "cargo_procs" => {
                                vm_info["cargo_processes"] = json!(val.parse::<u32>().ok());
                            }
                            "load" => vm_info["load_avg"] = json!(val),
                            "mem_used" => vm_info["memory_mb"] = json!(val),
                            _ => {}
                        }
                    }
                }
            } else {
                vm_info["status"] = json!("unreachable");
                vm_info["debug_hint"] =
                    json!("HOSTNAME= not found in stdout or stderr after ANSI stripping");
            }
        }
        Ok(Ok(Err(e))) => {
            vm_info["status"] = json!("error");
            vm_info["error"] = json!(format!("azlin connect failed: {e}"));
        }
        Ok(Err(e)) => {
            vm_info["status"] = json!("error");
            vm_info["error"] = json!(format!("task join failed: {e}"));
        }
        Err(_) => {
            vm_info["status"] = json!("timeout");
            vm_info["error"] = json!("azlin connect timed out after 30s");
        }
    }

    // Local host info for comparison
    let local_host = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Build the Remote VMs panel from the same canonical source as the
    // Cluster Topology panel (`load_hosts()` → ~/.simard/hosts.json), so the
    // two panels never disagree. Then enrich the entry whose vm_name matches
    // the probe target ("Simard", historically) with the probe results.
    // Hosts not covered by the probe keep status: "unknown". Probe data for
    // a vm_name not present in the configured hosts is discarded.
    let hosts = tokio::task::spawn_blocking(load_hosts)
        .await
        .unwrap_or_default();
    let mut remote_vms = remote_vms_from_hosts(&hosts);
    let probe_vm_name = vm_info
        .get("vm_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(entry) = remote_vms.iter_mut().find(|v| {
        v.get("vm_name")
            .and_then(|s| s.as_str())
            .map(|s| s == probe_vm_name)
            .unwrap_or(false)
    }) && let Value::Object(probe_map) = &vm_info
        && let Value::Object(entry_map) = entry
    {
        for (k, val) in probe_map {
            // Don't overwrite the canonical fields sourced from hosts.json
            if k == "vm_name" || k == "resource_group" {
                continue;
            }
            entry_map.insert(k.clone(), val.clone());
        }
    }

    Json(json!({
        "local": {
            "hostname": local_host,
            "type": "dev-machine",
        },
        "remote_vms": remote_vms,
        "topology": "distributed",
        "hive_mind": {
            "protocol": "DHT+bloom gossip (peer-to-peer)",
            "status": "standalone",
            "peers": 0,
            "facts_shared": 0,
            "note": "No external message bus required — hive-mind uses direct peer gossip for memory replication",
        },
        // Additive: per-issue WS-4. Surfaces in-process event bus stats from
        // `HiveEventBus::global()`. Older clients ignore the unknown key.
        "event_bus": crate::hive_event_bus::HiveEventBus::global().stats_snapshot(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) async fn vacate_vm(Json(body): Json<Value>) -> Json<Value> {
    let vm_name = body.get("vm_name").and_then(|v| v.as_str()).unwrap_or("");
    if vm_name.is_empty() {
        return Json(json!({"error": "vm_name is required"}));
    }

    // Run vacate script via azlin connect
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::task::spawn_blocking({
            let vm = vm_name.to_string();
            move || {
                // Step 1: Stop simard-ooda service and kill processes
                let stop_script = r#"
                    systemctl --user stop simard-ooda 2>/dev/null || true
                    pkill -f 'simard ooda' 2>/dev/null || true
                    sleep 2
                    REMAINING=$(pgrep -c -f simard 2>/dev/null || echo 0)
                    echo "REMAINING_PROCS=$REMAINING"
                    echo "VACATE_STATUS=ok"
                "#;

                let output = std::process::Command::new("systemd-run")
                    .args([
                        "--user",
                        "--pipe",
                        "--quiet",
                        "azlin",
                        "connect",
                        &vm,
                        "--no-tmux",
                        "--",
                        "bash",
                        "-c",
                        stop_script,
                    ])
                    .output();

                match output {
                    Ok(o) => {
                        let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                        let combined = format!("{}\n{}", stdout, stderr);
                        if combined.contains("VACATE_STATUS=ok") {
                            let remaining = combined
                                .lines()
                                .find_map(|l| l.strip_prefix("REMAINING_PROCS="))
                                .and_then(|v| v.trim().parse::<u32>().ok())
                                .unwrap_or(0);
                            Ok((remaining, combined))
                        } else {
                            Err(format!(
                                "vacate script did not report success. stdout: {}",
                                &stdout[..stdout.len().min(500)]
                            ))
                        }
                    }
                    Err(e) => Err(format!("Failed to run azlin connect: {e}")),
                }
            }
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok((remaining, _output)))) => {
            // Remove from configured hosts
            let mut hosts = load_hosts();
            hosts.retain(|h| h.get("name").and_then(|v| v.as_str()) != Some(vm_name));
            if let Err(e) = save_hosts(&hosts) {
                return Json(json!({
                    "status": "partial",
                    "message": format!(
                        "Vacate succeeded on {vm_name} ({remaining} process(es) remaining) \
                         but failed to update hosts file: {e}. Manual cleanup of \
                         ~/.simard/hosts.json may be needed."
                    ),
                    "remaining_processes": remaining,
                }));
            }

            let msg = if remaining == 0 {
                format!("All processes stopped on {vm_name}.")
            } else {
                format!(
                    "{remaining} process(es) still running on {vm_name} — may need manual cleanup."
                )
            };
            Json(json!({"status": "ok", "message": msg, "remaining_processes": remaining}))
        }
        Ok(Ok(Err(e))) => Json(json!({"error": e})),
        Ok(Err(e)) => Json(json!({"error": format!("task join error: {e}")})),
        Err(_) => Json(json!({"error": "Vacate timed out after 60s — the VM may be unreachable"})),
    }
}

pub(crate) fn strip_ansi_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    // CSI sequence: consume until a letter or '@'-'~'
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch.is_ascii_alphabetic() || ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next(); // consume ']'
                    // OSC sequence: consume until BEL or ST (\x1b\\)
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    // Single-char escape (e.g. \x1b=, \x1b>)
                    chars.next();
                }
            }
        } else if c == '\r' {
            // Strip carriage returns (common in SSH/PTY output)
            continue;
        } else {
            out.push(c);
        }
    }
    out
}
