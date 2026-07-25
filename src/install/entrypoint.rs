//! Owned PATH entrypoint and orphan reconciliation for the installer.
//!
//! The install transaction OWNS the `simard` your shell resolves. After any
//! install, `~/.local/bin/simard` is a symlink to the versioned live binary
//! `~/.simard/bin/simard`, and no verified-ours stale `simard` (at
//! `~/.local/bin` or `~/.cargo/bin`) is left shadowing it earlier on PATH.
//!
//! Reconciliation is conservative and fail-closed: only a `simard` the
//! installer can prove is its own (`OursSymlink` or `OursMarker`) is ever
//! replaced or removed. A `Foreign` file is never modified — it is surfaced
//! loudly with a `[simard]` diagnostic instead.
//!
//! The owned entrypoint is a Unix symlink, so the whole step is gated behind
//! `#[cfg(unix)]`; on non-unix targets it compiles to a no-op.
//!
//! See `docs/reference/simard-installer.md#path-entrypoint-ownership-guarantee`.

use std::path::PathBuf;

use super::InstallResult;
use super::paths::InstallLayout;

/// The result of an entrypoint reconciliation pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EntrypointReport {
    /// The owned entrypoint path that was created or verified. Empty when the
    /// entrypoint was occupied by a foreign file (or on non-unix targets).
    pub entrypoint_path: PathBuf,
    /// Paths where a foreign `simard` occupies the entrypoint and was left
    /// untouched. A non-empty list means the owned entrypoint could not take
    /// over PATH and requires operator attention.
    pub foreign_shadows: Vec<PathBuf>,
}

impl EntrypointReport {
    /// Emit loud `[simard]` diagnostics for any foreign shadow left in place.
    pub fn report(&self) {
        for shadow in &self.foreign_shadows {
            eprintln!(
                "[simard] WARNING: a foreign 'simard' occupies the PATH entrypoint and was left untouched:"
            );
            eprintln!("[simard]   {}", shadow.display());
            eprintln!(
                "[simard]   (not an installer-owned symlink and its --version did not start with \"simard \")"
            );
            eprintln!(
                "[simard]   the owned entrypoint could not be installed; resolve this file manually"
            );
        }
    }
}

/// Reconcile the owned PATH entrypoint and prune verified-ours orphans.
///
/// Runs unconditionally on every install (even when the binary swap was
/// skipped), so a stale `simard` reintroduced onto PATH between deploys is
/// cleaned up on the next install.
#[cfg(unix)]
pub fn reconcile_entrypoint(layout: &InstallLayout) -> InstallResult<EntrypointReport> {
    unix::reconcile(layout)
}

/// Non-unix targets have no symlink entrypoint; reconciliation is a no-op.
#[cfg(not(unix))]
pub fn reconcile_entrypoint(_layout: &InstallLayout) -> InstallResult<EntrypointReport> {
    Ok(EntrypointReport::default())
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::super::paths::InstallLayout;
    use super::super::{InstallError, InstallResult, err};
    use super::EntrypointReport;

    /// How a `simard` at a known path is classified before the installer acts.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Classification {
        /// Nothing at the path.
        Absent,
        /// A symlink whose canonical target is inside `$SIMARD_HOME/bin/`.
        OursSymlink,
        /// A regular file whose `--version` prints a line starting `simard `.
        OursMarker,
        /// Anything else — never modified or deleted.
        Foreign,
    }

    pub(super) fn reconcile(layout: &InstallLayout) -> InstallResult<EntrypointReport> {
        // Canonicalize the live bin dir once so symlink targets can be tested
        // for containment. The binary was already installed by this point.
        let bin_dir_canonical = layout
            .bin_dir
            .canonicalize()
            .unwrap_or_else(|_| layout.bin_dir.clone());

        let mut report = EntrypointReport::default();

        // 1. Own the entrypoint.
        match classify(&layout.entrypoint_path, &bin_dir_canonical) {
            Classification::Absent | Classification::OursMarker => {
                ensure_owned_symlink(&layout.entrypoint_path, &layout.binary_path)?;
                report.entrypoint_path = layout.entrypoint_path.clone();
            }
            Classification::OursSymlink => {
                if symlink_points_at(&layout.entrypoint_path, &layout.binary_path) {
                    // Already the exact owned symlink — leave it in place.
                } else {
                    ensure_owned_symlink(&layout.entrypoint_path, &layout.binary_path)?;
                }
                report.entrypoint_path = layout.entrypoint_path.clone();
            }
            Classification::Foreign => {
                // Never clobber a foreign file: surface it loudly instead.
                report.foreign_shadows.push(layout.entrypoint_path.clone());
            }
        }

        // 2. Prune verified-ours orphans so none can shadow the entrypoint.
        for orphan in &layout.orphan_paths {
            match classify(orphan, &bin_dir_canonical) {
                Classification::OursSymlink | Classification::OursMarker => {
                    remove_orphan(orphan)?;
                }
                Classification::Absent | Classification::Foreign => {
                    // Absent: nothing to prune. Foreign: never delete.
                }
            }
        }

        Ok(report)
    }

    /// Total, fail-closed classification: anything that cannot be positively
    /// proven ours is `Foreign`.
    fn classify(path: &Path, bin_dir_canonical: &Path) -> Classification {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(_) => return Classification::Absent,
        };

        if meta.file_type().is_symlink() {
            // Ours only when the link resolves to a file inside the live bin dir.
            match fs::canonicalize(path) {
                Ok(target) if target.starts_with(bin_dir_canonical) => Classification::OursSymlink,
                _ => Classification::Foreign,
            }
        } else if meta.file_type().is_file() {
            if version_banner_is_ours(path) {
                Classification::OursMarker
            } else {
                Classification::Foreign
            }
        } else {
            Classification::Foreign
        }
    }

    /// `true` when `<path> --version` prints a line starting with `simard `.
    /// argv-only (no shell), bounded, and any failure classifies as not-ours.
    fn version_banner_is_ours(path: &Path) -> bool {
        let output = match crate::util::spawn_retry::retry_spawn_sync(|| {
            Command::new(path).arg("--version").output()
        }) {
            Ok(output) => output,
            Err(_) => return false,
        };
        if !output.status.success() {
            return false;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next().is_some_and(|line| {
            let line = line.trim_start();
            line.starts_with("simard ") || line == "simard"
        })
    }

    /// `true` when `path` is a symlink whose canonical target equals the
    /// canonical versioned binary — i.e. already the exact owned symlink.
    fn symlink_points_at(path: &Path, binary_path: &Path) -> bool {
        match (fs::canonicalize(path), fs::canonicalize(binary_path)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    /// Create/repair the owned entrypoint symlink atomically: create a uniquely
    /// named temp symlink in the same directory, then `rename` it over the
    /// entrypoint path. Same-directory temp + rename closes the swap race.
    fn ensure_owned_symlink(entrypoint: &Path, target: &Path) -> InstallResult<()> {
        let dir = entrypoint.parent().ok_or_else(|| {
            InstallError::new(format!(
                "entrypoint path has no parent directory: {}",
                entrypoint.display()
            ))
        })?;
        fs::create_dir_all(dir).map_err(|error| {
            InstallError::new(format!(
                "failed to create entrypoint directory {}: {error}",
                dir.display()
            ))
        })?;

        let temp = temp_symlink_path(dir)?;
        // Clear any stale temp from a crashed prior run.
        let _ = fs::remove_file(&temp);
        std::os::unix::fs::symlink(target, &temp).map_err(|error| {
            InstallError::new(format!(
                "failed to stage entrypoint symlink {} -> {}: {error}",
                temp.display(),
                target.display()
            ))
        })?;

        fs::rename(&temp, entrypoint).map_err(|error| {
            let _ = fs::remove_file(&temp);
            InstallError::new(format!(
                "failed atomic entrypoint symlink publish from {} to {}: {error}",
                temp.display(),
                entrypoint.display()
            ))
        })?;

        eprintln!(
            "[simard] owned PATH entrypoint {} -> {}",
            entrypoint.display(),
            target.display()
        );
        Ok(())
    }

    fn remove_orphan(orphan: &Path) -> InstallResult<()> {
        match fs::remove_file(orphan) {
            Ok(()) => {
                eprintln!(
                    "[simard] removed stale verified-ours entrypoint orphan {}",
                    orphan.display()
                );
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => err(format!(
                "failed to remove stale entrypoint orphan {}: {error}",
                orphan.display()
            )),
        }
    }

    fn temp_symlink_path(dir: &Path) -> InstallResult<PathBuf> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| InstallError::new(format!("system clock is before epoch: {error}")))?
            .as_nanos();
        Ok(dir.join(format!(".simard.{}-{nanos}.tmp", std::process::id())))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{PermissionsExt, symlink};

        fn write_exec(path: &Path, body: &str) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, body).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn layout_for(root: &Path) -> InstallLayout {
            InstallLayout {
                simard_home: root.join(".simard"),
                bin_dir: root.join(".simard/bin"),
                binary_path: root.join(".simard/bin/simard"),
                prompt_assets_dir: root.join(".simard/prompt_assets"),
                staging_root: root.join(".simard/.install-staging"),
                backup_root: root.join(".simard/.install-backups"),
                systemd_user_dir: root.join("systemd"),
                ooda_unit_path: root.join("systemd/simard-ooda.service"),
                signal_unit_path: root.join("systemd/simard-signal.service"),
                entrypoint_path: root.join(".local/bin/simard"),
                orphan_paths: vec![root.join(".cargo/bin/simard")],
                transaction_id: "test-tx".to_string(),
            }
        }

        /// Install a fake versioned binary at `$SIMARD_HOME/bin/simard` so the
        /// reconciler has a real symlink target and marker executable.
        fn install_fake_binary(layout: &InstallLayout) {
            write_exec(&layout.binary_path, "#!/bin/sh\necho 'simard 9.9.9-test'\n");
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_creates_owned_symlink_on_fresh_home() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            let report = reconcile(&layout).unwrap();

            let meta = fs::symlink_metadata(&layout.entrypoint_path).unwrap();
            assert!(meta.file_type().is_symlink());
            assert_eq!(
                fs::canonicalize(&layout.entrypoint_path).unwrap(),
                fs::canonicalize(&layout.binary_path).unwrap()
            );
            assert_eq!(report.entrypoint_path, layout.entrypoint_path);
            assert!(report.foreign_shadows.is_empty());
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_removes_ours_symlink_orphan() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            let orphan = &layout.orphan_paths[0];
            fs::create_dir_all(orphan.parent().unwrap()).unwrap();
            symlink(&layout.binary_path, orphan).unwrap();

            reconcile(&layout).unwrap();
            assert!(
                fs::symlink_metadata(orphan).is_err(),
                "ours symlink orphan removed"
            );
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_removes_ours_marker_orphan() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            let orphan = &layout.orphan_paths[0];
            write_exec(orphan, "#!/bin/sh\necho 'simard 0.0.1-orphan'\n");

            reconcile(&layout).unwrap();
            assert!(
                fs::symlink_metadata(orphan).is_err(),
                "ours marker orphan removed"
            );
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_preserves_foreign_orphan() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            let orphan = &layout.orphan_paths[0];
            let body = "#!/bin/sh\necho 'othertool 1.0.0'\n";
            write_exec(orphan, body);

            reconcile(&layout).unwrap();
            assert_eq!(
                fs::read_to_string(orphan).unwrap(),
                body,
                "foreign orphan untouched"
            );
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_replaces_ours_marker_at_entrypoint() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            write_exec(
                &layout.entrypoint_path,
                "#!/bin/sh\necho 'simard 0.0.1-stale'\n",
            );

            let report = reconcile(&layout).unwrap();
            let meta = fs::symlink_metadata(&layout.entrypoint_path).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "ours-marker entrypoint replaced by symlink"
            );
            assert!(report.foreign_shadows.is_empty());
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_surfaces_foreign_shadow_at_entrypoint_untouched() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            let body = "#!/bin/sh\necho 'othertool 1.2.3'\n";
            write_exec(&layout.entrypoint_path, body);

            let report = reconcile(&layout).unwrap();
            let meta = fs::symlink_metadata(&layout.entrypoint_path).unwrap();
            assert!(
                !meta.file_type().is_symlink(),
                "foreign entrypoint not replaced"
            );
            assert_eq!(fs::read_to_string(&layout.entrypoint_path).unwrap(), body);
            assert_eq!(report.foreign_shadows, vec![layout.entrypoint_path.clone()]);
            assert!(report.entrypoint_path.as_os_str().is_empty());
        }

        #[test]
        #[serial_test::serial(install)]
        fn reconcile_is_idempotent() {
            let temp = tempfile::TempDir::new().unwrap();
            let layout = layout_for(temp.path());
            install_fake_binary(&layout);

            reconcile(&layout).unwrap();
            reconcile(&layout).unwrap();

            let entries: Vec<_> = fs::read_dir(layout.entrypoint_path.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            let count = entries.iter().filter(|n| *n == "simard").count();
            assert_eq!(
                count, 1,
                "exactly one entrypoint after repeat reconcile: {entries:?}"
            );
            let meta = fs::symlink_metadata(&layout.entrypoint_path).unwrap();
            assert!(meta.file_type().is_symlink());
        }

        #[test]
        #[serial_test::serial(install)]
        fn classify_broken_symlink_is_foreign() {
            let temp = tempfile::TempDir::new().unwrap();
            let bin_dir = temp.path().join(".simard/bin");
            fs::create_dir_all(&bin_dir).unwrap();
            let link = temp.path().join("dangling");
            symlink(temp.path().join("does-not-exist"), &link).unwrap();
            assert_eq!(classify(&link, &bin_dir), Classification::Foreign);
        }
    }
}
