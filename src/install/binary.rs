use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::paths::{InstallLayout, backup_path};
use super::{InstallError, InstallResult, err};

pub fn current_binary() -> InstallResult<PathBuf> {
    let path = std::env::current_exe().map_err(|error| {
        InstallError::new(format!("cannot determine current executable: {error}"))
    })?;
    if !path.is_file() {
        return err(format!(
            "current executable is not a regular file: {}",
            path.display()
        ));
    }
    Ok(path)
}

pub fn stage_binary(source: &Path, staged: &Path) -> InstallResult<()> {
    fs::copy(source, staged).map_err(|error| {
        InstallError::new(format!(
            "failed to stage Simard binary from {} to {}: {error}",
            source.display(),
            staged.display()
        ))
    })?;
    set_executable(staged)
}

pub fn preserve_prior_binary(layout: &InstallLayout) -> InstallResult<Option<PathBuf>> {
    if !layout.binary_path.exists() {
        return Ok(None);
    }
    if !layout.binary_path.is_file() {
        return err(format!(
            "existing install path is not a regular binary: {}",
            layout.binary_path.display()
        ));
    }

    let backup = backup_path(layout, "simard");
    if backup.exists() {
        return err(format!(
            "prior binary backup path already exists: {}",
            backup.display()
        ));
    }
    let temp_backup = temp_backup_path(&backup)?;

    if let Err(error) = fs::copy(&layout.binary_path, &temp_backup) {
        remove_failed_temp_backup(&temp_backup, "copy backup")?;
        return Err(InstallError::new(format!(
            "failed to stage prior binary backup {} to {}: {error}",
            layout.binary_path.display(),
            temp_backup.display()
        )));
    }

    let permissions = fs::metadata(&layout.binary_path)
        .map_err(|error| {
            InstallError::new(format!(
                "failed to read prior binary permissions {}: {error}",
                layout.binary_path.display()
            ))
        })?
        .permissions();
    if let Err(error) = fs::set_permissions(&temp_backup, permissions) {
        remove_failed_temp_backup(&temp_backup, "set permissions")?;
        return Err(InstallError::new(format!(
            "failed to preserve prior binary permissions on {}: {error}",
            temp_backup.display()
        )));
    }

    if let Err(error) = fs::File::open(&temp_backup).and_then(|file| file.sync_all()) {
        remove_failed_temp_backup(&temp_backup, "sync backup")?;
        return Err(InstallError::new(format!(
            "failed to sync staged prior binary backup {}: {error}",
            temp_backup.display()
        )));
    }

    fs::rename(&temp_backup, &backup).map_err(|error| {
        InstallError::new(format!(
            "failed atomic prior binary backup publish from {} to {}: {error}",
            temp_backup.display(),
            backup.display()
        ))
    })?;

    Ok(Some(backup))
}

pub fn live_binary_matches_source(source: &Path, layout: &InstallLayout) -> InstallResult<bool> {
    if !layout.binary_path.exists() {
        return Ok(false);
    }
    if !layout.binary_path.is_file() {
        return err(format!(
            "existing install path is not a regular binary: {}",
            layout.binary_path.display()
        ));
    }

    if let (Ok(source_canonical), Ok(live_canonical)) =
        (source.canonicalize(), layout.binary_path.canonicalize())
        && source_canonical == live_canonical
    {
        return Ok(true);
    }

    files_have_same_bytes(source, &layout.binary_path)
}

pub fn replace_live_binary(staged: &Path, live: &Path) -> InstallResult<()> {
    fs::rename(staged, live).map_err(|error| {
        InstallError::new(format!(
            "failed atomic binary replacement from {} to {}: {error}",
            staged.display(),
            live.display()
        ))
    })
}

fn files_have_same_bytes(left: &Path, right: &Path) -> InstallResult<bool> {
    let left_metadata = fs::metadata(left).map_err(|error| {
        InstallError::new(format!(
            "failed to inspect source binary {}: {error}",
            left.display()
        ))
    })?;
    let right_metadata = fs::metadata(right).map_err(|error| {
        InstallError::new(format!(
            "failed to inspect installed binary {}: {error}",
            right.display()
        ))
    })?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    let mut left_file = fs::File::open(left).map_err(|error| {
        InstallError::new(format!(
            "failed to read source binary {}: {error}",
            left.display()
        ))
    })?;
    let mut right_file = fs::File::open(right).map_err(|error| {
        InstallError::new(format!(
            "failed to read installed binary {}: {error}",
            right.display()
        ))
    })?;
    let mut left_buffer = [0_u8; 8192];
    let mut right_buffer = [0_u8; 8192];

    loop {
        let left_read = left_file.read(&mut left_buffer).map_err(|error| {
            InstallError::new(format!(
                "failed while comparing source binary {}: {error}",
                left.display()
            ))
        })?;
        let right_read = right_file.read(&mut right_buffer).map_err(|error| {
            InstallError::new(format!(
                "failed while comparing installed binary {}: {error}",
                right.display()
            ))
        })?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

fn set_executable(path: &Path) -> InstallResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| {
            InstallError::new(format!(
                "failed to set executable permissions on {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

fn temp_backup_path(backup: &Path) -> InstallResult<PathBuf> {
    let file_name = backup
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            InstallError::new(format!(
                "prior binary backup path has no valid UTF-8 filename: {}",
                backup.display()
            ))
        })?;
    Ok(backup.with_file_name(format!(".{file_name}.tmp")))
}

fn remove_failed_temp_backup(path: &Path, failed_action: &str) -> InstallResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => err(format!(
            "failed to remove staged prior binary backup {} after {failed_action} failed: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> InstallLayout {
        InstallLayout {
            simard_home: root.to_path_buf(),
            bin_dir: root.join("bin"),
            binary_path: root.join("bin/simard"),
            prompt_assets_dir: root.join("prompt_assets"),
            staging_root: root.join(".install-staging"),
            backup_root: root.join(".install-backups"),
            systemd_user_dir: root.join("systemd"),
            ooda_unit_path: root.join("systemd/simard-ooda.service"),
            signal_unit_path: root.join("systemd/simard-signal.service"),
            transaction_id: "test-tx".to_string(),
        }
    }

    fn collect_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if !root.exists() {
            return files;
        }
        for entry in fs::read_dir(root).expect("read backup root") {
            files.push(entry.expect("backup entry").path());
        }
        files.sort();
        files
    }

    #[test]
    fn preserve_prior_binary_publishes_backup_without_leaving_temp_file() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let layout = layout(temp.path());
        let prior_binary = b"old-simard-binary";
        fs::create_dir_all(&layout.bin_dir).expect("bin dir");
        fs::create_dir_all(&layout.backup_root).expect("backup dir");
        fs::write(&layout.binary_path, prior_binary).expect("prior binary");

        let backup = preserve_prior_binary(&layout)
            .expect("preserve prior binary")
            .expect("backup path");

        assert_eq!(backup, backup_path(&layout, "simard"));
        assert_eq!(fs::read(&backup).expect("backup bytes"), prior_binary);
        let files = collect_files(&layout.backup_root);
        assert!(
            files.iter().all(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.ends_with(".tmp"))
            }),
            "backup temp files should not remain: {files:?}"
        );
    }
}
