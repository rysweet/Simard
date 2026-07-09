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
    fs::copy(&layout.binary_path, &backup).map_err(|error| {
        InstallError::new(format!(
            "failed to preserve prior binary {} to {}: {error}",
            layout.binary_path.display(),
            backup.display()
        ))
    })?;

    let permissions = fs::metadata(&layout.binary_path)
        .map_err(|error| {
            InstallError::new(format!(
                "failed to read prior binary permissions {}: {error}",
                layout.binary_path.display()
            ))
        })?
        .permissions();
    fs::set_permissions(&backup, permissions).map_err(|error| {
        InstallError::new(format!(
            "failed to preserve prior binary permissions on {}: {error}",
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
