//! Toolchain action acceptance test. Runs in cargo test and directly after
//! the action in verify.yml, without building Simard's native dependencies.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct SmokeDir(PathBuf);

impl Drop for SmokeDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("launch toolchain command");
    assert!(
        output.status.success(),
        "{command:?} failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn installed_toolchain_builds_and_tests_an_offline_edition_2024_crate() {
    let expected = std::env::var("RUST_TOOLCHAIN_SMOKE_EXPECTED").unwrap_or_default();
    if !expected.is_empty() {
        let active = run(Command::new("rustup").args(["show", "active-toolchain"]));
        assert!(
            String::from_utf8_lossy(&active.stdout).starts_with(&format!("{expected}-")),
            "action did not select {expected}: {}",
            String::from_utf8_lossy(&active.stdout)
        );
    }
    let root = SmokeDir(std::env::temp_dir().join(format!(
        "rust-toolchain-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    fs::create_dir(&root.0).unwrap();
    fs::create_dir(root.0.join("src")).unwrap();
    fs::write(
        root.0.join("Cargo.toml"),
        "[package]\nname = \"toolchain-smoke\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        root.0.join("src/lib.rs"),
        "pub fn sum(values: &[u32]) -> u32 { values.iter().sum() }\n\
         #[test] fn sums_values() { assert_eq!(sum(&[2, 3, 5]), 10); }\n",
    )
    .unwrap();
    run(Command::new("cargo")
        .current_dir(&root.0)
        .args(["generate-lockfile", "--offline"]));
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(&root.0)
        .env("CARGO_TARGET_DIR", root.0.join("target"))
        .args(["test", "--offline", "--locked"]);
    let coverage = expected.starts_with("nightly-");
    if coverage {
        cargo
            .env("RUSTFLAGS", "-C instrument-coverage")
            .env("LLVM_PROFILE_FILE", root.0.join("smoke-%p.profraw"));
    }
    let output = run(&mut cargo);
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
    println!("{}", String::from_utf8_lossy(&output.stdout));
    if coverage {
        assert!(fs::read_dir(&root.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "profraw")
        }));
        let libdir = run(Command::new("rustc").args(["--print", "target-libdir"]));
        let libdir = PathBuf::from(String::from_utf8(libdir.stdout).unwrap().trim());
        run(Command::new(libdir.parent().unwrap().join("bin/llvm-cov")).arg("--version"));
    }
}
