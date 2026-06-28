//! Behavioural contract for the multi-binary self-update path (issue #2252).
//!
//! These tests pin down the multi-binary self-update API documented in
//! `docs/reference/multi-binary-self-update.md`:
//!
//!   * `find_all_binaries_in_dir` — dynamic discovery of every executable in an
//!     extracted tarball (replaces the single-binary `find_binary_in_dir`).
//!   * `install_binary` — atomic single-binary install (rename → copy fallback →
//!     `chmod 0o755`, `.old` backup/restore).
//!   * `install_binaries` + `InstallReport` — full-set install with the
//!     main-fatal / aux-best-effort policy.
//!   * `sha256_file` / `verify_sha256` — the R1 checksum gate and R3 https-only
//!     transport guard.
//!   * shared-primitive regression guards for `download_to_temp` and the
//!     `safe-update` download-only path.
//!
//! Everything here is hermetic: no network, no live release, and no reliance on
//! a real GitHub asset.

use super::download::{
    InstallReport, create_update_tmp_dir, find_all_binaries_in_dir, install_binaries,
    install_binary, install_from_extracted, sha256_file, verify_sha256, verify_signature,
};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Write `contents` to `path` and mark it executable on Unix. Discovery treats
/// an "executable" as a regular file with an execute bit set, so binary
/// fixtures must carry the bit; data-file fixtures deliberately do not.
fn write_exec(path: &Path, contents: &[u8]) -> PathBuf {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_path_buf()
}

/// Collect the basenames of discovered/installed paths for order-independent
/// assertions.
fn basenames(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ===========================================================================
// find_all_binaries_in_dir — dynamic discovery
// ===========================================================================

#[test]
fn discovery_finds_main_and_all_aux_at_root() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");
    write_exec(&dir.path().join("simard-tui"), b"tui");
    write_exec(&dir.path().join("simard-gym"), b"gym");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let mut names = basenames(&found);
    names.sort();
    assert_eq!(names, vec!["simard", "simard-gym", "simard-tui"]);
}

#[test]
fn discovery_returns_main_binary_first() {
    let dir = tempfile::tempdir().unwrap();
    // An auxiliary whose name sorts BEFORE "simard" proves the main binary is
    // explicitly hoisted to the front, not merely alphabetically first.
    write_exec(&dir.path().join("aaa-first"), b"aux");
    write_exec(&dir.path().join("simard"), b"main");
    write_exec(&dir.path().join("simard-tui"), b"tui");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    assert_eq!(
        found[0].file_name().unwrap(),
        "simard",
        "main binary must sort first in the returned vec"
    );
}

#[test]
fn discovery_is_name_agnostic() {
    // The set is discovered by executability, not by a hard-coded name list:
    // a non-`simard*` executable is still installed.
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");
    write_exec(&dir.path().join("helper-tool"), b"helper");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    assert!(names.contains(&"helper-tool".to_string()));
}

#[test]
fn discovery_finds_nested_aux() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");
    write_exec(&dir.path().join("bin").join("simard-gym"), b"gym");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    assert!(names.contains(&"simard".to_string()));
    assert!(names.contains(&"simard-gym".to_string()));
}

#[test]
fn discovery_finds_at_depth_3_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("a").join("b").join("c");
    write_exec(&deep.join("simard"), b"main");

    let found = find_all_binaries_in_dir(dir.path());
    assert!(found.is_ok(), "binary at depth 3 must be discovered");
}

#[test]
fn discovery_excludes_aux_beyond_depth_3() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");
    // depth 4 — must be ignored, but the update still succeeds via main.
    let too_deep = dir.path().join("a").join("b").join("c").join("d");
    write_exec(&too_deep.join("simard-gym"), b"gym");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    assert!(names.contains(&"simard".to_string()));
    assert!(
        !names.contains(&"simard-gym".to_string()),
        "executables beyond depth 3 must not be discovered"
    );
}

#[test]
fn discovery_errors_when_main_beyond_depth_3() {
    let dir = tempfile::tempdir().unwrap();
    let too_deep = dir.path().join("a").join("b").join("c").join("d");
    write_exec(&too_deep.join("simard"), b"main");

    let result = find_all_binaries_in_dir(dir.path());
    assert!(
        result.is_err(),
        "a tarball with simard only beyond depth 3 cannot update the daemon"
    );
}

#[test]
fn discovery_errors_when_no_simard_present() {
    // A tree with auxiliary binaries but no `simard` is rejected outright: a
    // tarball that cannot update the daemon is invalid.
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard-tui"), b"tui");
    write_exec(&dir.path().join("simard-gym"), b"gym");

    let result = find_all_binaries_in_dir(dir.path());
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("simard"),
        "error must name the missing main binary"
    );
}

#[test]
fn discovery_ignores_directory_named_simard() {
    let dir = tempfile::tempdir().unwrap();
    // A directory (not a regular file) named `simard` must not satisfy the
    // main-binary requirement.
    fs::create_dir(dir.path().join("simard")).unwrap();
    write_exec(&dir.path().join("simard-tui"), b"tui");

    let result = find_all_binaries_in_dir(dir.path());
    assert!(
        result.is_err(),
        "a directory named simard is not the main binary"
    );
}

#[test]
fn discovery_dedups_by_basename() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"root-main");
    // A second file also named `simard`, nested — must NOT yield two install
    // targets for the same destination name.
    write_exec(&dir.path().join("nested").join("simard"), b"nested-main");
    write_exec(&dir.path().join("simard-tui"), b"tui");

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    let simard_count = names.iter().filter(|n| *n == "simard").count();
    assert_eq!(simard_count, 1, "basenames must be de-duplicated");
    assert_eq!(found.len(), 2, "expected exactly simard + simard-tui");
}

#[cfg(unix)]
#[test]
fn discovery_skips_non_executable_files() {
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");
    // A regular, NON-executable data file (0o644) must not be treated as a
    // binary — it must never receive a 0o755 chmod (R4).
    let data = dir.path().join("config.toml");
    fs::write(&data, b"not a binary").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&data, fs::Permissions::from_mode(0o644)).unwrap();
    }

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    assert_eq!(
        names,
        vec!["simard"],
        "non-executable files are not binaries"
    );
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlinks() {
    // R2 zip-slip defense: a symlink (even one pointing at a real file) is not a
    // regular file and must not be installed. The real `simard` is still found.
    let dir = tempfile::tempdir().unwrap();
    write_exec(&dir.path().join("simard"), b"main");

    let outside = tempfile::tempdir().unwrap();
    let target = write_exec(&outside.path().join("payload"), b"evil");
    std::os::unix::fs::symlink(&target, dir.path().join("simard-evil")).unwrap();

    let found = find_all_binaries_in_dir(dir.path()).unwrap();
    let names = basenames(&found);
    assert!(names.contains(&"simard".to_string()));
    assert!(
        !names.contains(&"simard-evil".to_string()),
        "symlinked entries must be rejected (zip-slip defense)"
    );
}

// ===========================================================================
// install_binary — atomic single-binary install
// ===========================================================================

#[test]
fn install_binary_installs_new_file() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = write_exec(&src_dir.path().join("simard"), b"new-binary");
    let dest = dst_dir.path().join("simard");

    install_binary(&src, &dest).unwrap();

    assert!(dest.exists());
    assert_eq!(fs::read(&dest).unwrap(), b"new-binary");
}

#[test]
fn install_binary_overwrites_existing() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = write_exec(&src_dir.path().join("simard"), b"NEW");
    let dest = write_exec(&dst_dir.path().join("simard"), b"OLD");

    install_binary(&src, &dest).unwrap();

    assert_eq!(fs::read(&dest).unwrap(), b"NEW");
}

#[test]
fn install_binary_removes_old_backup_on_success() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src = write_exec(&src_dir.path().join("simard"), b"NEW");
    let dest = write_exec(&dst_dir.path().join("simard"), b"OLD");

    install_binary(&src, &dest).unwrap();

    let backup = dest.with_extension("old");
    assert!(
        !backup.exists(),
        "the .old backup must be cleaned up after a successful install"
    );
}

#[cfg(unix)]
#[test]
fn install_binary_sets_executable_permissions() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    // Source deliberately NOT executable; install must apply 0o755.
    let src = src_dir.path().join("simard");
    fs::write(&src, b"new").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&src, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let dest = dst_dir.path().join("simard");

    install_binary(&src, &dest).unwrap();

    assert_eq!(mode_of(&dest), 0o755, "installed binary must be 0o755");
}

#[test]
fn install_binary_restores_backup_on_failure() {
    // If the swap fails AFTER the existing dest was moved aside, the backup must
    // be restored so the install location is never left empty.
    let dst_dir = tempfile::tempdir().unwrap();
    let dest = write_exec(&dst_dir.path().join("simard"), b"OLD");
    // A non-existent source forces both rename and copy to fail.
    let missing_src = dst_dir.path().join("does-not-exist");

    let result = install_binary(&missing_src, &dest);

    assert!(result.is_err(), "installing a missing source must fail");
    assert!(dest.exists(), "dest must be restored, never left missing");
    assert_eq!(
        fs::read(&dest).unwrap(),
        b"OLD",
        "the previous binary must be restored on failure"
    );
    assert!(
        !dest.with_extension("old").exists(),
        "no stale .old backup may be left behind after restore"
    );
}

// ===========================================================================
// install_binaries + InstallReport — full-set install policy
// ===========================================================================

/// Build an extracted-tarball-style source dir containing `simard` plus the
/// named auxiliary binaries, and return (tempdir, ordered binary paths) with
/// `simard` first — the shape `install_binaries` expects from discovery.
fn make_binaries(aux: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = vec![write_exec(&dir.path().join("simard"), b"main")];
    for name in aux {
        paths.push(write_exec(&dir.path().join(name), name.as_bytes()));
    }
    (dir, paths)
}

#[test]
fn install_binaries_happy_path_installs_full_set() {
    let (_src, bins) = make_binaries(&["simard-tui", "simard-gym"]);
    let install = tempfile::tempdir().unwrap();

    let report = install_binaries(&bins, install.path()).unwrap();

    assert!(report.main_installed, "main must be installed");
    let mut aux = report.aux_installed.clone();
    aux.sort();
    assert_eq!(
        aux,
        vec!["simard-gym".to_string(), "simard-tui".to_string()]
    );
    assert!(report.aux_failed.is_empty());

    assert!(install.path().join("simard").exists());
    assert!(install.path().join("simard-tui").exists());
    assert!(install.path().join("simard-gym").exists());
}

#[test]
fn install_binaries_reports_aux_as_basenames() {
    let (_src, bins) = make_binaries(&["simard-tui"]);
    let install = tempfile::tempdir().unwrap();

    let report = install_binaries(&bins, install.path()).unwrap();

    assert_eq!(report.aux_installed, vec!["simard-tui".to_string()]);
}

#[test]
fn install_binaries_old_single_binary_tarball_is_not_an_error() {
    // Backward compatibility: an old release tarball containing only `simard`
    // installs cleanly with no auxiliary work and no error.
    let (_src, bins) = make_binaries(&[]);
    let install = tempfile::tempdir().unwrap();

    let report = install_binaries(&bins, install.path()).unwrap();

    assert!(report.main_installed);
    assert!(report.aux_installed.is_empty());
    assert!(
        report.aux_failed.is_empty(),
        "aux-missing must be non-fatal"
    );
}

#[test]
fn install_binaries_missing_main_is_fatal() {
    // A binary set with no `simard` basename must abort: the daemon cannot be
    // updated.
    let src = tempfile::tempdir().unwrap();
    let bins = vec![write_exec(&src.path().join("simard-tui"), b"tui")];
    let install = tempfile::tempdir().unwrap();

    let result = install_binaries(&bins, install.path());

    assert!(result.is_err(), "absence of the main binary must be fatal");
}

#[test]
fn install_binaries_main_write_failure_is_fatal() {
    // If the main swap itself fails, the whole update aborts (no relaunch).
    let (_src, bins) = make_binaries(&["simard-tui"]);
    // Use a regular FILE as the "install dir" so every join+rename underneath it
    // fails with ENOTDIR regardless of the running user (root included).
    let not_a_dir = tempfile::tempdir().unwrap();
    let install_path = not_a_dir.path().join("install-is-a-file");
    fs::write(&install_path, b"x").unwrap();

    let result = install_binaries(&bins, &install_path);

    assert!(result.is_err(), "a failed main swap must be fatal");
}

#[test]
fn install_binaries_aux_failure_is_non_fatal() {
    // The main binary installs, one auxiliary cannot. The update still succeeds;
    // the failure is recorded in `aux_failed`, never aborting.
    let src = tempfile::tempdir().unwrap();
    let main = write_exec(&src.path().join("simard"), b"main");
    // A discovered path whose source no longer exists forces this aux to fail
    // while the main install succeeds.
    let broken_aux = src.path().join("simard-tui");
    let bins = vec![main, broken_aux];
    let install = tempfile::tempdir().unwrap();

    let report = install_binaries(&bins, install.path()).unwrap();

    assert!(report.main_installed, "main must still install");
    assert!(
        report.aux_installed.is_empty(),
        "the broken aux must not be reported as installed"
    );
    assert_eq!(
        report.aux_failed.len(),
        1,
        "the aux failure must be recorded"
    );
    assert_eq!(report.aux_failed[0].0, "simard-tui");
    assert!(
        install.path().join("simard").exists(),
        "main must be present despite the aux failure"
    );
}

#[test]
fn install_binaries_installs_by_basename_into_install_dir_only() {
    // R2: every binary is installed as `install_dir/<basename>` — a nested
    // discovery path must NOT recreate its directory structure under the install
    // dir, and nothing may escape the install root.
    let src = tempfile::tempdir().unwrap();
    let main = write_exec(&src.path().join("simard"), b"main");
    let nested_aux = write_exec(
        &src.path().join("deep").join("nest").join("simard-gym"),
        b"gym",
    );
    let bins = vec![main, nested_aux];
    let install = tempfile::tempdir().unwrap();

    let report = install_binaries(&bins, install.path()).unwrap();

    assert!(report.main_installed);
    assert!(
        install.path().join("simard-gym").exists(),
        "aux must be installed flat by basename"
    );
    assert!(
        !install.path().join("deep").exists(),
        "discovery path structure must not be recreated under the install dir"
    );
}

// ===========================================================================
// install_from_extracted — full-set install + unconditional temp-dir cleanup
// ===========================================================================

#[test]
fn install_from_extracted_installs_full_set_and_removes_tmp_dir() {
    // The extracted tarball dir lives inside an outer tempdir so we can assert
    // it is gone after the install without the outer tempdir's own Drop racing
    // the assertion.
    let outer = tempfile::tempdir().unwrap();
    let extracted = outer.path().join("extracted");
    write_exec(&extracted.join("simard"), b"main");
    write_exec(&extracted.join("simard-tui"), b"tui");
    let install = tempfile::tempdir().unwrap();

    let report = install_from_extracted(&extracted, install.path()).unwrap();

    assert!(report.main_installed);
    assert_eq!(report.aux_installed, vec!["simard-tui".to_string()]);
    assert!(install.path().join("simard").exists());
    assert!(install.path().join("simard-tui").exists());
    assert!(
        !extracted.exists(),
        "the extracted temp dir must be removed after a successful install"
    );
}

#[test]
fn install_from_extracted_missing_main_errors_and_removes_tmp_dir() {
    // A tarball with no `simard` is a discovery error. The temp dir must STILL
    // be cleaned up — the early `?` exit must not leak the extracted tree.
    let outer = tempfile::tempdir().unwrap();
    let extracted = outer.path().join("extracted");
    write_exec(&extracted.join("simard-tui"), b"tui");
    let install = tempfile::tempdir().unwrap();

    let result = install_from_extracted(&extracted, install.path());

    assert!(result.is_err(), "absence of the main binary must be fatal");
    assert!(
        !extracted.exists(),
        "the extracted temp dir must be removed even when discovery fails"
    );
}

#[test]
fn install_from_extracted_main_swap_failure_removes_tmp_dir() {
    // The realistic leak trigger: discovery succeeds but the main swap fails
    // (here, an install path that is a regular file, so every join+rename
    // underneath it fails with ENOTDIR regardless of user). The update aborts,
    // and the temp dir must not be leaked into /tmp.
    let outer = tempfile::tempdir().unwrap();
    let extracted = outer.path().join("extracted");
    write_exec(&extracted.join("simard"), b"main");
    write_exec(&extracted.join("simard-tui"), b"tui");
    let not_a_dir = tempfile::tempdir().unwrap();
    let install_path = not_a_dir.path().join("install-is-a-file");
    fs::write(&install_path, b"x").unwrap();

    let result = install_from_extracted(&extracted, &install_path);

    assert!(result.is_err(), "a failed main swap must be fatal");
    assert!(
        !extracted.exists(),
        "the extracted temp dir must be removed even when the main swap fails"
    );
}

// ===========================================================================
// InstallReport — struct contract
// ===========================================================================

#[test]
fn install_report_default_is_not_installed() {
    let report = InstallReport::default();
    assert!(
        !report.main_installed,
        "a default report must mean 'no main installed' (caller must not relaunch)"
    );
    assert!(report.aux_installed.is_empty());
    assert!(report.aux_failed.is_empty());
}

#[test]
fn install_report_equality_contract() {
    // Derives Clone + PartialEq + Eq so call sites can assert on outcomes.
    let a = InstallReport {
        main_installed: true,
        aux_installed: vec!["simard-tui".to_string()],
        aux_failed: vec![("simard-gym".to_string(), "boom".to_string())],
    };
    let b = a.clone();
    assert_eq!(a, b);

    let c = InstallReport {
        main_installed: false,
        ..a.clone()
    };
    assert_ne!(a, c);
}

// ===========================================================================
// sha256_file / verify_sha256 — R1 checksum gate, R3 transport guard
// ===========================================================================

#[test]
fn sha256_file_matches_known_empty_vector() {
    // Pure digest core that the checksum gate compares against the published
    // .sha256 sidecar. Lowercase hex, matching `sha256sum` output.
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("empty");
    fs::write(&f, b"").unwrap();

    let digest = sha256_file(&f).unwrap();
    assert_eq!(
        digest,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn sha256_file_matches_known_abc_vector() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("abc");
    fs::write(&f, b"abc").unwrap();

    let digest = sha256_file(&f).unwrap();
    assert_eq!(
        digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn sha256_file_distinguishes_tampered_content() {
    // A one-byte change must yield a different digest — the property a checksum
    // mismatch relies on to abort before extraction.
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good");
    let bad = dir.path().join("bad");
    fs::write(&good, b"release tarball bytes").unwrap();
    fs::write(&bad, b"release tarball byteX").unwrap();

    assert_ne!(sha256_file(&good).unwrap(), sha256_file(&bad).unwrap());
}

#[test]
fn verify_sha256_rejects_non_https_url() {
    // R3: the sidecar must be fetched over https-only transport. A non-https
    // asset URL is refused without performing any network I/O. A real archive is
    // supplied so the URL scheme is the only possible cause of failure.
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("simard-linux-x86_64.tar.gz");
    fs::write(&archive, b"tarball").unwrap();

    for url in [
        "http://example.com/simard-linux-x86_64.tar.gz",
        "ftp://example.com/simard-linux-x86_64.tar.gz",
        "file:///tmp/simard-linux-x86_64.tar.gz",
    ] {
        let result = verify_sha256(&archive, url);
        assert!(
            result.is_err(),
            "verify_sha256 must refuse non-https asset URL: {url}"
        );
    }
}

// ===========================================================================
// verify_signature — R8 cosign keyless authenticity gate (transport guard)
// ===========================================================================

#[test]
fn verify_signature_rejects_non_https_url() {
    // R3/R8: signature material (.sig/.pem) is fetched over https-only transport.
    // A non-https asset URL is refused before any cosign or network work, so the
    // result is deterministic regardless of whether cosign is installed.
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("simard-linux-x86_64.tar.gz");
    fs::write(&archive, b"tarball").unwrap();

    for url in [
        "http://example.com/simard-linux-x86_64.tar.gz",
        "ftp://example.com/simard-linux-x86_64.tar.gz",
        "file:///tmp/simard-linux-x86_64.tar.gz",
    ] {
        let result = verify_signature(&archive, url, dir.path());
        assert!(
            result.is_err(),
            "verify_signature must refuse non-https asset URL: {url}"
        );
    }
}

// ===========================================================================
// create_update_tmp_dir — R7 private, unpredictable, exclusive temp dir
// ===========================================================================

#[test]
fn create_update_tmp_dir_makes_fresh_private_dir() {
    // R7: the download/extract dir must be freshly created (never a pre-existing
    // path an attacker could have seeded or symlinked) and, on Unix, private.
    let dir = create_update_tmp_dir().expect("should create a temp dir");
    assert!(
        dir.exists() && dir.is_dir(),
        "update temp dir must exist as a directory"
    );
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "freshly created update temp dir must be empty"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "update temp dir must be private (0700), got {mode:o}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn create_update_tmp_dir_returns_unique_paths() {
    // The randomized suffix must make two updates pick different directories,
    // replacing the predictable `simard-update-<pid>` name that a local attacker
    // could anticipate.
    let a = create_update_tmp_dir().unwrap();
    let b = create_update_tmp_dir().unwrap();
    assert_ne!(a, b, "two update temp dirs must not collide");
    let _ = fs::remove_dir_all(&a);
    let _ = fs::remove_dir_all(&b);
}

// ===========================================================================
// Shared-primitive regressions — preserve the safe-update contract
// ===========================================================================

#[test]
fn download_to_temp_keeps_single_pathbuf_return() {
    // Compile-time contract guard: `download_to_temp` must keep returning a
    // single PathBuf (the main `simard` candidate). `safe-update`'s
    // SafeUpdateOrchestrator::new(cfg, candidate, install) consumes exactly one
    // candidate path, so the multi-binary work must not change this signature.
    #[allow(clippy::type_complexity)]
    let _f: fn(&str, &str) -> Result<PathBuf, Box<dyn std::error::Error>> =
        super::download::download_to_temp;
}

#[test]
fn safe_update_download_only_keeps_option_pathbuf_return() {
    // The `simard safe-update` download-only entry point returns one optional
    // candidate path; multi-binary discovery lives only in `download_and_replace`.
    #[allow(clippy::type_complexity)]
    let _f: fn() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> =
        super::handle_self_update_download_only;
}
