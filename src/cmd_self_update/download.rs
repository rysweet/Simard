//! Binary download, extraction, and replacement logic.
//!
//! `simard update` replaces the **full** Simard binary set (`simard` plus
//! `simard-tui`, `simard-gym`, and the rest of the auxiliary binaries shipped in
//! the release tarball), not just the main daemon binary (issue #2252). See
//! `docs/reference/multi-binary-self-update.md` for the full contract.

use super::platform::{RELEASE_CERT_IDENTITY_REGEXP, RELEASE_CERT_OIDC_ISSUER};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Outcome of installing the full binary set from an extracted tarball.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InstallReport {
    /// `true` once the MAIN binary (`simard`) is installed. `false` means the
    /// update aborted: the caller MUST NOT relaunch.
    pub main_installed: bool,
    /// Basenames of auxiliary binaries that were installed successfully.
    pub aux_installed: Vec<String>,
    /// Auxiliary binaries that failed to install, as `(basename, reason)`.
    /// Logged and surfaced to the operator; never aborts the update.
    pub aux_failed: Vec<(String, String)>,
}

/// Create a fresh, private (mode `0700`) temp directory for the self-update
/// download/extract, returning its path.
///
/// R7 (hardening): the directory name carries an unpredictable random suffix and
/// is created with `create_dir` semantics that FAIL if the path already exists,
/// replacing the previous predictable `simard-update-<pid>` name created with
/// `create_dir_all`. On a shared host this defeats a local attacker who
/// pre-creates (or symlinks) the directory to redirect our `curl -o` write or to
/// smuggle a rogue executable into the discovery walk. The exclusive create is
/// what guarantees safety; the random suffix only makes the name hard to guess.
pub(crate) fn create_update_tmp_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = std::env::temp_dir();
    let mut last_err: Option<std::io::Error> = None;
    for _ in 0..32 {
        let dir = base.join(format!("simard-update-{}", random_token()));
        match create_private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(format!("Failed to create update temp dir: {e}").into()),
        }
    }
    Err(format!(
        "Failed to create a unique update temp dir after 32 attempts: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )
    .into())
}

/// Create `dir` with `create_dir` semantics (fails if it already exists) and, on
/// Unix, restrictive `0700` permissions so only the current user can read, write
/// or traverse it.
#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().create(dir)
}

/// 16 bytes of hex from the OS CSPRNG (`/dev/urandom`), falling back to a mix of
/// pid + high-resolution time if the CSPRNG is unavailable. The EXCLUSIVE create
/// in [`create_update_tmp_dir`] is what guarantees safety; this only makes the
/// directory name hard to guess and collision-resistant.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if !fill_random(&mut bytes) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seed = (nanos as u64) ^ ((std::process::id() as u64) << 32);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((seed >> ((i % 8) * 8)) as u8) ^ (i as u8);
        }
    }
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) -> bool {
    use std::io::Read;
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .is_ok()
}

#[cfg(not(unix))]
fn fill_random(_buf: &mut [u8]) -> bool {
    false
}

/// Download and extract the release, replacing the full set of installed
/// Simard binaries. The main `simard` swap is fatal; auxiliary binaries are
/// installed best-effort. Returns the `InstallReport` for the caller to
/// surface to the operator — this function only prints progress, never the
/// final summary, so the outcome is reported exactly once.
pub(crate) fn download_and_replace(
    url: &str,
    version: &str,
) -> Result<InstallReport, Box<dyn std::error::Error>> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;
    let install_dir = current_exe
        .parent()
        .ok_or("Cannot determine install directory from current executable")?
        .to_path_buf();

    // Download → verify checksum → extract into the private temp dir, then
    // discover EVERY executable in the extracted tree (main + auxiliaries) with
    // a single filesystem walk.
    let tmp_dir = download_and_extract(url, version)?;
    let binaries = find_all_binaries_in_dir(&tmp_dir)?;

    println!("Replacing {} binary(ies)...", binaries.len());
    let report = install_binaries(&binaries, &install_dir)?;

    // Post-install confirmation: every binary discovery installed must now be
    // present on disk. The set is dynamic, so this checks exactly what we
    // installed rather than a static manifest.
    let main_dest = install_dir.join("simard");
    if !main_dest.exists() {
        return Err(format!("Main binary missing after install: {}", main_dest.display()).into());
    }
    for name in &report.aux_installed {
        let dest = install_dir.join(name);
        if !dest.exists() {
            return Err(format!("Auxiliary binary reported installed but missing: {name}").into());
        }
    }

    // Clean up the temp directory (archive + any leftover extracted files).
    let _ = fs::remove_dir_all(&tmp_dir);

    Ok(report)
}

/// Download → verify checksum → extract the release into the private temp dir,
/// returning the populated temp directory. Shared by `download_to_temp` and
/// `download_and_replace` so the download/verify/extract work runs once and the
/// binary-discovery walk that follows it also runs exactly once per update
/// (previously `download_and_replace` walked the extracted tree twice).
fn download_and_extract(url: &str, version: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let tmp_dir = create_update_tmp_dir()?;
    let archive_path = tmp_dir.join("simard.tar.gz");

    println!("Downloading simard v{version}...");
    let archive_str = archive_path.to_str().unwrap_or("simard.tar.gz");
    let mut last_err = String::from("Download failed");
    let mut downloaded = false;
    for attempt in 0..3u32 {
        if attempt > 0 {
            let delay = 1u64 << attempt; // 2s, 4s
            println!(
                "Retrying download (attempt {}/3, waiting {delay}s)...",
                attempt + 1
            );
            std::thread::sleep(std::time::Duration::from_secs(delay));
        }
        match std::process::Command::new("curl")
            .args([
                "-sS",
                "-L",
                // R3: https-only transport — refuse a plaintext URL and never
                // let a redirect downgrade the scheme to http://.
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--connect-timeout",
                "15",
                "--max-time",
                "120",
                "--retry",
                "2",
                "-o",
                archive_str,
                // `--` terminates option parsing so a URL beginning with `-`
                // can never be reinterpreted as a curl flag (arg injection).
                "--",
                url,
            ])
            .status()
        {
            Ok(status) if status.success() => {
                downloaded = true;
                break;
            }
            Ok(status) => last_err = format!("curl exited with status {status}"),
            Err(e) => last_err = format!("Failed to run curl: {e}"),
        }
    }
    if !downloaded {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!("Download failed after 3 attempts: {last_err}").into());
    }

    // R1: verify the downloaded tarball against its published `.sha256` sidecar
    // BEFORE extracting anything. A mismatch (or missing sidecar) aborts.
    println!("Verifying checksum...");
    if let Err(e) = verify_sha256(&archive_path, url) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!("Checksum verification failed: {e}").into());
    }

    // R8: authenticate the tarball's cosign keyless signature — defense-in-depth
    // beyond the same-origin sha256 (a host that swaps both the tarball and its
    // `.sha256` defeats the checksum alone). A present-but-invalid signature
    // ABORTS; an absent signature or missing cosign warns and continues.
    if let Err(e) = verify_signature(&archive_path, url, &tmp_dir) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!("Signature verification failed: {e}").into());
    }

    println!("Extracting...");
    let tar_status = std::process::Command::new("tar")
        .args([
            "xzf",
            archive_path.to_str().ok_or_else(|| {
                format!(
                    "archive path is not valid UTF-8: {}",
                    archive_path.display()
                )
            })?,
            // R4: never let the archive restore its own ownership or permission
            // bits onto extracted files. Discovery applies a scoped `chmod 0755`
            // only to the binaries it chooses to install (see `install_into`).
            "--no-same-owner",
            "--no-same-permissions",
            "-C",
            tmp_dir.to_str().ok_or_else(|| {
                format!("temp dir path is not valid UTF-8: {}", tmp_dir.display())
            })?,
        ])
        .status()
        .map_err(|e| format!("Failed to extract archive: {e}"))?;
    if !tar_status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err("Extraction failed".into());
    }

    Ok(tmp_dir)
}

/// Download and extract the simard release into a temp directory, returning
/// the path to the extracted **main** `simard` binary. Used by the
/// `safe-update` flow (which copies the main candidate to an install path and
/// runs a pre-test before swapping). Its signature is a hard contract: it must
/// keep returning a single `PathBuf` to the main candidate.
pub(crate) fn download_to_temp(
    url: &str,
    version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let tmp_dir = download_and_extract(url, version)?;
    // Return the main `simard` candidate (discovery guarantees it sorts first).
    let binaries = find_all_binaries_in_dir(&tmp_dir)?;
    binaries
        .into_iter()
        .next()
        .ok_or_else(|| "Binary 'simard' not found in downloaded archive".into())
}

/// On Unix an "executable" is a regular file with any execute bit set. The
/// `DirEntry::metadata` call does not traverse symlinks (and callers skip
/// symlinks before reaching here anyway).
#[cfg(unix)]
fn is_executable_file(entry: &fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    entry
        .metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// On non-Unix targets there is no execute bit, so treat regular files as
/// candidates but exclude the archive and its checksum sidecar that share the
/// temp directory.
#[cfg(not(unix))]
fn is_executable_file(entry: &fs::DirEntry) -> bool {
    !matches!(
        entry.path().extension().and_then(|e| e.to_str()),
        Some("gz") | Some("tar") | Some("sha256")
    )
}

/// Discover every executable file in an extracted tarball tree (max depth 3).
///
/// On Unix, "executable" means a regular file with any execute bit set. The
/// returned paths are de-duplicated by basename (shallowest directory-walk
/// match wins) so a tarball can never ask to install two different files to the
/// same destination name. The main binary `simard` is hoisted to the front of
/// the returned vec when present.
///
/// Returns an error only if the tree contains no `simard` binary at all.
pub(crate) fn find_all_binaries_in_dir(
    dir: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    fn collect(
        dir: &Path,
        depth: u32,
        seen: &mut HashSet<std::ffi::OsString>,
        out: &mut Vec<PathBuf>,
    ) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        // Process files in this directory before recursing so that a shallower
        // basename match always wins the de-duplication.
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // R2: a symlink (file or dir) is never followed — neither installed
            // nor descended into. This blocks zip-slip via symlinked entries.
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                subdirs.push(entry.path());
                continue;
            }
            if file_type.is_file() && is_executable_file(&entry) && seen.insert(entry.file_name()) {
                out.push(entry.path());
            }
        }
        for sub in subdirs {
            collect(&sub, depth + 1, seen, out);
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<std::ffi::OsString> = HashSet::new();
    collect(dir, 0, &mut seen, &mut out);

    let main = OsStr::new("simard");
    let Some(pos) = out.iter().position(|p| p.file_name() == Some(main)) else {
        return Err("Binary 'simard' not found in downloaded archive".into());
    };
    // Hoist the main binary to the front so callers can rely on `[0]`.
    let main_path = out.remove(pos);
    out.insert(0, main_path);
    Ok(out)
}

/// Install a single binary `src` to `dest`. Sequence:
///   1. If `dest` exists, move it aside to `dest.old` (best-effort cleanup of a
///      stale `.old` first).
///   2. `rename(src, dest)` — O(1) on the same filesystem.
///   3. On cross-device failure, fall back to `copy(src, dest)`.
///   4. On Unix, `chmod 0o755` the installed file.
///   5. On success, remove the `dest.old` backup.
///
/// On failure after step 1, the `.old` backup is restored over `dest` so the
/// install location is never left empty.
pub(crate) fn install_binary(src: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let backup = dest.with_extension("old");
    let mut backed_up = false;
    if dest.exists() {
        if backup.exists() {
            let _ = fs::remove_file(&backup);
        }
        fs::rename(dest, &backup).map_err(|e| {
            format!(
                "Failed to back up existing binary {} (try running with sudo): {e}",
                dest.display()
            )
        })?;
        backed_up = true;
    }

    let install_result = install_into(src, dest);

    match install_result {
        Ok(()) => {
            if backed_up {
                let _ = fs::remove_file(&backup);
            }
            Ok(())
        }
        Err(e) => {
            if backed_up {
                // Restore the previous binary; the rename also removes `.old`.
                let _ = fs::rename(&backup, dest);
            }
            Err(e)
        }
    }
}

/// Move (or copy, cross-device) `src` onto `dest` and apply executable perms.
fn install_into(src: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // rename is O(1) on same filesystem; copy handles cross-device installs.
    if fs::rename(src, dest).is_err() {
        fs::copy(src, dest)
            .map_err(|e| format!("Failed to install binary {}: {e}", dest.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // R4: scoped chmod — only files chosen by discovery get the exec bit.
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions on {}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Install every discovered binary into `install_dir`.
///
/// - The binary whose basename is `simard` is installed FIRST and is FATAL:
///   any error returns `Err` and the caller must abort (no relaunch).
/// - Every other binary is installed BEST-EFFORT: a failure is recorded in
///   `aux_failed` and the loop continues.
///
/// Returns `Ok(InstallReport)` once the main binary is installed, regardless of
/// auxiliary outcomes. R2: every binary is installed as `install_dir/<basename>`
/// — discovery path structure is never recreated and nothing escapes the root.
pub(crate) fn install_binaries(
    binaries: &[PathBuf],
    install_dir: &Path,
) -> Result<InstallReport, Box<dyn std::error::Error>> {
    let main = OsStr::new("simard");
    let main_src = binaries
        .iter()
        .find(|p| p.file_name() == Some(main))
        .ok_or("Main binary 'simard' missing from extracted release")?;

    let main_dest = install_dir.join("simard");
    install_binary(main_src, &main_dest)
        .map_err(|e| format!("Failed to install main binary 'simard': {e}"))?;

    let mut report = InstallReport {
        main_installed: true,
        ..Default::default()
    };

    for bin in binaries {
        let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "simard" {
            continue;
        }
        let dest = install_dir.join(name);
        match install_binary(bin, &dest) {
            Ok(()) => report.aux_installed.push(name.to_string()),
            Err(e) => report.aux_failed.push((name.to_string(), e.to_string())),
        }
    }

    Ok(report)
}

/// Compute the lowercase hex SHA-256 of a file, matching `sha256sum` output.
pub(crate) fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed to open {} for hashing: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read {} while hashing: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Verify a downloaded tarball against its published `<asset>.sha256`.
///
/// Fetches the sidecar checksum for `asset_url`, computes the SHA-256 of
/// `archive_path`, and compares. On mismatch (or a missing/unreadable sidecar)
/// returns an error WITHOUT extracting, and the caller cleans up the temp dir.
/// R3: the sidecar is only ever fetched over https-only transport — a non-https
/// asset URL is refused before any network I/O.
pub(crate) fn verify_sha256(
    archive_path: &Path,
    asset_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !asset_url.starts_with("https://") {
        return Err(format!("Refusing non-https release asset URL: {asset_url}").into());
    }

    let sidecar_url = format!("{asset_url}.sha256");
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "15",
            "--max-time",
            "60",
            // `--` terminates option parsing so the sidecar URL can never be
            // reinterpreted as a curl flag (arg injection).
            "--",
            &sidecar_url,
        ])
        .output()
        .map_err(|e| format!("Failed to fetch checksum sidecar: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to download checksum sidecar {sidecar_url}: curl exited {}",
            output.status
        )
        .into());
    }

    let sidecar = String::from_utf8_lossy(&output.stdout);
    let expected = sidecar
        .split_whitespace()
        .next()
        .ok_or("Checksum sidecar was empty")?
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Checksum sidecar did not contain a valid SHA-256 digest".into());
    }

    let actual = sha256_file(archive_path)?;
    if actual != expected {
        return Err(format!("Checksum mismatch: expected {expected}, got {actual}").into());
    }
    Ok(())
}

/// Verify the release tarball's cosign keyless signature (R8, issue #2261).
///
/// Defense-in-depth on top of [`verify_sha256`]: the sha256 sidecar is fetched
/// from the same origin as the tarball, so a compromised host that swaps both
/// the tarball and its `.sha256` defeats it. The cosign signature is bound to a
/// short-lived Fulcio certificate whose Subject Alternative Name is pinned to
/// THIS repo's release-workflow identity ([`RELEASE_CERT_IDENTITY_REGEXP`]) and
/// issuer ([`RELEASE_CERT_OIDC_ISSUER`]) — an identity an attacker cannot forge.
///
/// Policy (fail-closed where it counts, non-breaking otherwise):
///   * cosign present AND both sidecars (`.sig`, `.pem`) fetch as non-empty →
///     run `cosign verify-blob`; a verification FAILURE ABORTS the update.
///   * cosign not installed, OR a signature sidecar is absent (e.g. an older,
///     pre-signing release) → emit a prominent warning and continue, so updates
///     are not broken on hosts without cosign or for legacy releases. The R1
///     sha256 gate still applies in every case.
///
/// R3: signature material is only ever fetched over https-only transport.
pub(crate) fn verify_signature(
    archive_path: &Path,
    asset_url: &str,
    tmp_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !asset_url.starts_with("https://") {
        return Err(format!("Refusing non-https release asset URL: {asset_url}").into());
    }

    if !cosign_available() {
        eprintln!(
            "WARNING: cosign not found — skipping release signature verification. \
             Install cosign to authenticate releases (the sha256 checksum was still verified)."
        );
        return Ok(());
    }

    let sig_path = tmp_dir.join("simard.tar.gz.sig");
    let cert_path = tmp_dir.join("simard.tar.gz.pem");
    let sig_ok = fetch_sidecar(&format!("{asset_url}.sig"), &sig_path);
    let cert_ok = fetch_sidecar(&format!("{asset_url}.pem"), &cert_path);
    if !sig_ok || !cert_ok {
        let _ = fs::remove_file(&sig_path);
        let _ = fs::remove_file(&cert_path);
        eprintln!(
            "WARNING: signature material (.sig/.pem) is not available for this release — \
             skipping cosign verification (the sha256 checksum was still verified)."
        );
        return Ok(());
    }

    println!("Verifying cosign signature...");
    let status = std::process::Command::new("cosign")
        .args([
            "verify-blob",
            "--certificate",
            cert_path
                .to_str()
                .ok_or("certificate path is not valid UTF-8")?,
            "--signature",
            sig_path
                .to_str()
                .ok_or("signature path is not valid UTF-8")?,
            "--certificate-identity-regexp",
            RELEASE_CERT_IDENTITY_REGEXP,
            "--certificate-oidc-issuer",
            RELEASE_CERT_OIDC_ISSUER,
            // `--` terminates option parsing so the archive path can never be
            // reinterpreted as a cosign flag (arg injection).
            "--",
            archive_path
                .to_str()
                .ok_or("archive path is not valid UTF-8")?,
        ])
        .status()
        .map_err(|e| format!("Failed to run cosign: {e}"))?;

    let _ = fs::remove_file(&sig_path);
    let _ = fs::remove_file(&cert_path);

    if !status.success() {
        return Err(format!(
            "cosign signature verification FAILED ({status}) — refusing to install this release"
        )
        .into());
    }
    Ok(())
}

/// Whether a usable `cosign` binary is on `PATH`.
fn cosign_available() -> bool {
    std::process::Command::new("cosign")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Fetch a signature sidecar over https-only transport into `dest`. Returns
/// `true` only when curl succeeded AND the written file is non-empty.
fn fetch_sidecar(url: &str, dest: &Path) -> bool {
    let Some(dest_str) = dest.to_str() else {
        return false;
    };
    let ok = std::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            // R3: https-only transport — refuse plaintext and never downgrade.
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "15",
            "--max-time",
            "60",
            "-o",
            dest_str,
            // `--` terminates option parsing (arg-injection guard).
            "--",
            url,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && dest.metadata().map(|m| m.len() > 0).unwrap_or(false)
}
