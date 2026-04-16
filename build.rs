fn main() {
    // Git commit hash
    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());
    println!("cargo:rustc-env=SIMARD_GIT_HASH={git_hash}");

    // Build number: count of git commits on HEAD, or env var override
    let build_number = std::env::var("SIMARD_BUILD_NUMBER").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map_or_else(|| "0".to_string(), |s| s.trim().to_string())
    });
    println!("cargo:rustc-env=SIMARD_BUILD_NUMBER={build_number}");

    // Rebuild when HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-env-changed=SIMARD_BUILD_NUMBER");
}
