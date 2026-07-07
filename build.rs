fn main() {
    // Git commit hash
    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SIMARD_GIT_HASH={git_hash}");

    // Build number: count of git commits on HEAD, or env var override
    let build_number = std::env::var("SIMARD_BUILD_NUMBER").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "0".to_string())
    });
    println!("cargo:rustc-env=SIMARD_BUILD_NUMBER={build_number}");

    // Deployment/build timestamp (issue #2727): baked in at compile time so the
    // running binary knows when THIS build was produced. In this project a build
    // is a deploy (the daemon runs the freshly compiled binary), so the
    // compile-time instant is the most durable, deterministic "when was this
    // deployed?" signal — chosen over binary mtime (mutated by copies/install)
    // or a deploy-marker file (none exists). It sits symmetrically alongside the
    // SIMARD_BUILD_NUMBER / SIMARD_GIT_HASH signals emitted above. Emitted as an
    // RFC3339 UTC string; the dashboard converts it to America/Los_Angeles
    // (PST/PDT) for display. Honors SOURCE_DATE_EPOCH for reproducible builds.
    let build_timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    println!("cargo:rustc-env=SIMARD_BUILD_TIMESTAMP={build_timestamp}");

    // Rebuild when HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-env-changed=SIMARD_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}
