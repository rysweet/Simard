use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::paths::InstallLayout;
use super::{InstallError, InstallResult, err};

const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BackupEntry {
    name: String,
    destination: PathBuf,
    backup: PathBuf,
    existed: bool,
    digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BackupManifest {
    version: u32,
    transaction_id: String,
    simard_home: PathBuf,
    entries: Vec<BackupEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedBackup {
    pub manifest_path: PathBuf,
}

pub fn create_verified_backup(layout: &InstallLayout) -> InstallResult<VerifiedBackup> {
    fs::create_dir_all(&layout.backup_root).map_err(|error| {
        InstallError::new(format!(
            "failed to create backup root {}: {error}",
            layout.backup_root.display()
        ))
    })?;
    let root = layout
        .backup_root
        .join(format!("install-{}", layout.transaction_id));
    if root.exists() {
        return err(format!(
            "verified backup transaction already exists: {}",
            root.display()
        ));
    }
    fs::create_dir(&root).map_err(|error| {
        InstallError::new(format!(
            "failed to create verified backup directory {}: {error}",
            root.display()
        ))
    })?;

    let surfaces = backup_surfaces(layout);
    let mut entries = Vec::with_capacity(surfaces.len());
    for (name, destination) in surfaces {
        let backup = root.join(&name);
        let existed = destination.exists();
        let digest = if existed {
            copy_path(&destination, &backup)?;
            Some(digest_path(&backup)?)
        } else {
            None
        };
        entries.push(BackupEntry {
            name,
            destination,
            backup,
            existed,
            digest,
        });
    }
    let manifest = BackupManifest {
        version: MANIFEST_VERSION,
        transaction_id: layout.transaction_id.clone(),
        simard_home: layout.simard_home.clone(),
        entries,
    };
    let manifest_path = root.join("manifest.json");
    write_manifest(&manifest_path, &manifest)?;
    verify_manifest(layout, &manifest_path)?;
    Ok(VerifiedBackup { manifest_path })
}

pub fn restore_verified_backup(layout: &InstallLayout, manifest_path: &Path) -> InstallResult<()> {
    let manifest = verify_manifest(layout, manifest_path)?;
    for entry in &manifest.entries {
        remove_path_if_exists(&entry.destination)?;
        if entry.existed {
            copy_path(&entry.backup, &entry.destination)?;
            let restored = digest_path(&entry.destination)?;
            if Some(restored) != entry.digest {
                return err(format!(
                    "rollback verification failed for restored {}",
                    entry.destination.display()
                ));
            }
        }
    }
    Ok(())
}

pub fn print_guidance(
    layout: &InstallLayout,
    prior_binary_backup: Option<&Path>,
    manifest_path: &Path,
) {
    println!("Rollback planning:");
    match prior_binary_backup {
        Some(path) => println!("  Prior binary preserved at {}", path.display()),
        None => println!(
            "  No distinct prior binary copy was needed under {}",
            layout.backup_root.display()
        ),
    }
    println!(
        "  Verified deployment backup manifest: {}",
        manifest_path.display()
    );
    println!("  The manifest includes the compatible state tree as the verified memory backup.");
    println!(
        "  Roll back binary, prompts, recipes, policies, units, config, and compatible state with:"
    );
    println!(
        "  {} install --simard-home {} --systemd-user-dir {} --rollback {}",
        layout.binary_path.display(),
        layout.simard_home.display(),
        layout.systemd_user_dir.display(),
        manifest_path.display()
    );
}

fn backup_surfaces(layout: &InstallLayout) -> Vec<(String, PathBuf)> {
    vec![
        ("binary".to_string(), layout.binary_path.clone()),
        (
            "prompt_assets".to_string(),
            layout.prompt_assets_dir.clone(),
        ),
        ("ooda_unit".to_string(), layout.ooda_unit_path.clone()),
        ("signal_unit".to_string(), layout.signal_unit_path.clone()),
        ("config".to_string(), layout.simard_home.join("config.toml")),
        ("state".to_string(), layout.simard_home.join("state")),
    ]
}

fn verify_manifest(layout: &InstallLayout, manifest_path: &Path) -> InstallResult<BackupManifest> {
    let canonical_root = layout.backup_root.canonicalize().map_err(|error| {
        InstallError::new(format!(
            "failed to resolve backup root {}: {error}",
            layout.backup_root.display()
        ))
    })?;
    let canonical_manifest = manifest_path.canonicalize().map_err(|error| {
        InstallError::new(format!(
            "failed to resolve rollback manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    if !canonical_manifest.starts_with(&canonical_root) {
        return err(format!(
            "rollback manifest must be inside {}",
            layout.backup_root.display()
        ));
    }
    let bytes = fs::read(&canonical_manifest).map_err(|error| {
        InstallError::new(format!(
            "failed to read rollback manifest {}: {error}",
            canonical_manifest.display()
        ))
    })?;
    let manifest: BackupManifest = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::new(format!(
            "failed to decode rollback manifest {}: {error}",
            canonical_manifest.display()
        ))
    })?;
    if manifest.version != MANIFEST_VERSION || manifest.simard_home != layout.simard_home {
        return err("rollback manifest is incompatible with this install layout");
    }
    let expected: BTreeSet<_> = backup_surfaces(layout).into_iter().collect();
    let actual: BTreeSet<_> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.destination.clone()))
        .collect();
    if actual != expected || manifest.entries.len() != expected.len() {
        return err("rollback manifest surface inventory does not match this install");
    }
    for entry in &manifest.entries {
        validate_backup_path(&canonical_root, &entry.backup)?;
        match (&entry.digest, entry.existed) {
            (Some(expected), true) => {
                let observed = digest_path(&entry.backup)?;
                if &observed != expected {
                    return err(format!(
                        "verified backup digest mismatch for {}",
                        entry.backup.display()
                    ));
                }
            }
            (None, false) => {}
            _ => return err("rollback manifest has inconsistent existence metadata"),
        }
    }
    Ok(manifest)
}

fn validate_backup_path(root: &Path, path: &Path) -> InstallResult<()> {
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) && !path.is_absolute()
    {
        return err(format!("unsafe backup path {}", path.display()));
    }
    if !path.exists() {
        if path.starts_with(root) {
            return Ok(());
        }
        return err(format!(
            "missing backup surface escapes verified backup root: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        InstallError::new(format!(
            "failed to resolve backup surface {}: {error}",
            path.display()
        ))
    })?;
    if !canonical.starts_with(root) {
        return err(format!(
            "backup surface escapes verified backup root: {}",
            path.display()
        ));
    }
    Ok(())
}

fn write_manifest(path: &Path, manifest: &BackupManifest) -> InstallResult<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
        InstallError::new(format!("failed to encode rollback manifest: {error}"))
    })?;
    let temporary = path.with_extension("json.tmp");
    let mut file = fs::File::create(&temporary).map_err(|error| {
        InstallError::new(format!(
            "failed to create rollback manifest {}: {error}",
            temporary.display()
        ))
    })?;
    file.write_all(&bytes).map_err(|error| {
        InstallError::new(format!(
            "failed to write rollback manifest {}: {error}",
            temporary.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        InstallError::new(format!(
            "failed to sync rollback manifest {}: {error}",
            temporary.display()
        ))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        InstallError::new(format!(
            "failed to publish rollback manifest {}: {error}",
            path.display()
        ))
    })
}

fn copy_path(source: &Path, destination: &Path) -> InstallResult<()> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        InstallError::new(format!(
            "failed to inspect backup source {}: {error}",
            source.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return err(format!(
            "verified backups do not follow symbolic links: {}",
            source.display()
        ));
    }
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                InstallError::new(format!(
                    "failed to create backup parent {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::copy(source, destination).map_err(|error| {
            InstallError::new(format!(
                "failed to copy backup file {} to {}: {error}",
                source.display(),
                destination.display()
            ))
        })?;
        fs::set_permissions(destination, metadata.permissions()).map_err(|error| {
            InstallError::new(format!(
                "failed to preserve permissions on {}: {error}",
                destination.display()
            ))
        })?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|error| {
            InstallError::new(format!(
                "failed to create backup directory {}: {error}",
                destination.display()
            ))
        })?;
        for entry in fs::read_dir(source).map_err(|error| {
            InstallError::new(format!(
                "failed to enumerate backup source {}: {error}",
                source.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                InstallError::new(format!(
                    "failed to read backup entry under {}: {error}",
                    source.display()
                ))
            })?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    err(format!("unsupported backup source {}", source.display()))
}

fn remove_path_if_exists(path: &Path) -> InstallResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(InstallError::new(format!(
                "failed to inspect rollback destination {}: {error}",
                path.display()
            )));
        }
    }
    .map_err(|error| {
        InstallError::new(format!(
            "failed to remove rollback destination {}: {error}",
            path.display()
        ))
    })
}

fn digest_path(path: &Path) -> InstallResult<String> {
    let mut hasher = Sha256::new();
    digest_into(path, path, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn digest_into(root: &Path, path: &Path, hasher: &mut Sha256) -> InstallResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        InstallError::new(format!("failed to hash {}: {error}", path.display()))
    })?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    hasher.update(relative.as_os_str().as_encoded_bytes());
    if metadata.is_file() {
        hasher.update([0]);
        hasher.update(fs::read(path).map_err(|error| {
            InstallError::new(format!("failed to hash file {}: {error}", path.display()))
        })?);
        return Ok(());
    }
    if metadata.is_dir() {
        hasher.update([1]);
        let mut children = fs::read_dir(path)
            .map_err(|error| {
                InstallError::new(format!(
                    "failed to hash directory {}: {error}",
                    path.display()
                ))
            })?
            .map(|entry| entry.map(|value| value.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                InstallError::new(format!("failed to hash directory entry: {error}"))
            })?;
        children.sort();
        for child in children {
            digest_into(root, &child, hasher)?;
        }
        return Ok(());
    }
    err(format!("unsupported backup entry {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> InstallLayout {
        InstallLayout {
            simard_home: root.join("home"),
            bin_dir: root.join("home/bin"),
            binary_path: root.join("home/bin/simard"),
            prompt_assets_dir: root.join("home/prompt_assets"),
            staging_root: root.join("home/.install-staging"),
            backup_root: root.join("home/.install-backups"),
            systemd_user_dir: root.join("systemd"),
            ooda_unit_path: root.join("systemd/simard-ooda.service"),
            signal_unit_path: root.join("systemd/simard-signal.service"),
            transaction_id: "rollback-test".to_string(),
        }
    }

    #[test]
    fn verified_backup_restores_every_deployment_surface() {
        let temp = tempfile::tempdir().expect("tempdir");
        let layout = layout(temp.path());
        fs::create_dir_all(&layout.bin_dir).expect("bin");
        fs::create_dir_all(layout.prompt_assets_dir.join("simard/recipes")).expect("assets");
        fs::create_dir_all(layout.simard_home.join("state")).expect("state");
        fs::create_dir_all(&layout.systemd_user_dir).expect("units");
        fs::write(&layout.binary_path, b"old-binary").expect("binary");
        fs::write(
            layout
                .prompt_assets_dir
                .join("simard/recipes/goal-session-actor.yaml"),
            b"old-recipe",
        )
        .expect("recipe");
        fs::write(&layout.ooda_unit_path, b"old-ooda-unit").expect("unit");
        fs::write(&layout.signal_unit_path, b"old-signal-unit").expect("unit");
        fs::write(layout.simard_home.join("config.toml"), b"old-config").expect("config");
        fs::write(
            layout.simard_home.join("state/outcomes.sqlite3"),
            b"old-state",
        )
        .expect("state");

        let backup = create_verified_backup(&layout).expect("verified backup");
        fs::write(&layout.binary_path, b"new-binary").expect("mutate");
        fs::remove_dir_all(&layout.prompt_assets_dir).expect("mutate");
        fs::write(&layout.ooda_unit_path, b"new-unit").expect("mutate");
        fs::write(layout.simard_home.join("config.toml"), b"new-config").expect("mutate");
        fs::remove_dir_all(layout.simard_home.join("state")).expect("mutate");

        restore_verified_backup(&layout, &backup.manifest_path).expect("rollback");

        assert_eq!(fs::read(&layout.binary_path).unwrap(), b"old-binary");
        assert_eq!(
            fs::read(
                layout
                    .prompt_assets_dir
                    .join("simard/recipes/goal-session-actor.yaml")
            )
            .unwrap(),
            b"old-recipe"
        );
        assert_eq!(fs::read(&layout.ooda_unit_path).unwrap(), b"old-ooda-unit");
        assert_eq!(
            fs::read(&layout.signal_unit_path).unwrap(),
            b"old-signal-unit"
        );
        assert_eq!(
            fs::read(layout.simard_home.join("config.toml")).unwrap(),
            b"old-config"
        );
        assert_eq!(
            fs::read(layout.simard_home.join("state/outcomes.sqlite3")).unwrap(),
            b"old-state"
        );
    }
}
