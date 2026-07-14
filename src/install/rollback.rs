use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::paths::InstallLayout;
use super::{InstallError, InstallResult, err};

const MANIFEST_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BackupKind {
    Path,
    Sqlite,
    LadybugSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BackupEntry {
    name: String,
    destination: PathBuf,
    backup: PathBuf,
    existed: bool,
    digest: Option<String>,
    kind: BackupKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BackupManifest {
    version: u32,
    transaction_id: String,
    simard_home: PathBuf,
    entries: Vec<BackupEntry>,
    services: Vec<super::systemd::ServiceBaseline>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedBackup {
    pub manifest_path: PathBuf,
}

pub fn create_verified_backup(
    layout: &InstallLayout,
    systemctl: &Path,
) -> InstallResult<VerifiedBackup> {
    let services = super::systemd::capture_baseline(layout, systemctl)?;
    super::systemd::quiesce_for_snapshot(systemctl, &services)?;
    let result = create_backup_manifest(layout, &services);
    if let Err(snapshot_error) = result {
        return match super::systemd::restore_baseline(systemctl, &services) {
            Ok(()) => Err(snapshot_error),
            Err(service_error) => err(format!(
                "snapshot failed: {snapshot_error}; service baseline restoration failed: {service_error}"
            )),
        };
    }
    result
}

fn create_backup_manifest(
    layout: &InstallLayout,
    services: &[super::systemd::ServiceBaseline],
) -> InstallResult<VerifiedBackup> {
    let root = create_backup_directory(layout)?;
    let manifest = BackupManifest {
        version: MANIFEST_VERSION,
        transaction_id: layout.transaction_id.clone(),
        simard_home: layout.simard_home.clone(),
        entries: snapshot_surfaces(layout, &root)?,
        services: services.to_vec(),
    };
    let manifest_path = root.join("manifest.json");
    write_manifest(&manifest_path, &manifest)?;
    verify_manifest(layout, &manifest_path)?;
    Ok(VerifiedBackup { manifest_path })
}

fn create_backup_directory(layout: &InstallLayout) -> InstallResult<PathBuf> {
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
    Ok(root)
}

fn snapshot_surfaces(layout: &InstallLayout, root: &Path) -> InstallResult<Vec<BackupEntry>> {
    backup_surfaces(layout)
        .into_iter()
        .map(|(name, destination, kind)| {
            let backup = root.join(&name);
            let existed = destination.exists();
            let digest = if existed {
                snapshot_surface(kind, &destination, &backup)?;
                sync_path(&backup)?;
                sync_directory(backup.parent().ok_or_else(|| {
                    InstallError::new(format!(
                        "backup surface has no parent: {}",
                        backup.display()
                    ))
                })?)?;
                Some(digest_path(&backup)?)
            } else {
                None
            };
            Ok(BackupEntry {
                name,
                destination,
                backup,
                existed,
                digest,
                kind,
            })
        })
        .collect()
}

pub fn restore_verified_backup(
    layout: &InstallLayout,
    manifest_path: &Path,
    systemctl: &Path,
) -> InstallResult<()> {
    let manifest = verify_manifest(layout, manifest_path)?;
    super::systemd::quiesce_current(layout, systemctl)?;
    let staged = stage_restore_entries(&manifest)?;
    publish_restore_entries(&manifest, &staged)?;
    super::systemd::restore_baseline(systemctl, &manifest.services)
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

fn backup_surfaces(layout: &InstallLayout) -> Vec<(String, PathBuf, BackupKind)> {
    let state_root = layout.simard_home.join("state");
    vec![
        (
            "binary".to_string(),
            layout.binary_path.clone(),
            BackupKind::Path,
        ),
        (
            "prompt_assets".to_string(),
            layout.prompt_assets_dir.clone(),
            BackupKind::Path,
        ),
        (
            "ooda_unit".to_string(),
            layout.ooda_unit_path.clone(),
            BackupKind::Path,
        ),
        (
            "signal_unit".to_string(),
            layout.signal_unit_path.clone(),
            BackupKind::Path,
        ),
        (
            "config".to_string(),
            layout.simard_home.join("config.toml"),
            BackupKind::Path,
        ),
        (
            "typed_ooda.sqlite3".to_string(),
            crate::typed_ooda::ledger_path(&state_root),
            BackupKind::Sqlite,
        ),
        (
            "cognitive_export".to_string(),
            crate::cognitive_memory::live_store_path(&state_root),
            BackupKind::LadybugSnapshot,
        ),
    ]
}

fn snapshot_surface(kind: BackupKind, source: &Path, backup: &Path) -> InstallResult<()> {
    match kind {
        BackupKind::Path => copy_path(source, backup),
        BackupKind::Sqlite => snapshot_sqlite(source, backup),
        BackupKind::LadybugSnapshot => snapshot_ladybug(source, backup),
    }
}

fn snapshot_sqlite(source: &Path, destination: &Path) -> InstallResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            InstallError::new(format!(
                "failed to create SQLite snapshot parent {}: {error}",
                parent.display()
            ))
        })?;
    }
    let source_connection = rusqlite::Connection::open(source).map_err(|error| {
        InstallError::new(format!(
            "failed to open typed-OODA SQLite database {}: {error}",
            source.display()
        ))
    })?;
    let mut destination_connection = rusqlite::Connection::open(destination).map_err(|error| {
        InstallError::new(format!(
            "failed to create typed-OODA SQLite snapshot {}: {error}",
            destination.display()
        ))
    })?;
    {
        let backup = rusqlite::backup::Backup::new(&source_connection, &mut destination_connection)
            .map_err(|error| {
                InstallError::new(format!(
                    "failed to initialize SQLite online backup: {error}"
                ))
            })?;
        backup
            .run_to_completion(64, Duration::from_millis(10), None)
            .map_err(|error| {
                InstallError::new(format!("typed-OODA SQLite online backup failed: {error}"))
            })?;
    }
    verify_sqlite(&destination_connection, destination)
}

fn verify_sqlite(connection: &rusqlite::Connection, path: &Path) -> InstallResult<()> {
    let result: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| {
            InstallError::new(format!(
                "typed-OODA SQLite integrity check failed for {}: {error}",
                path.display()
            ))
        })?;
    if result != "ok" {
        return err(format!(
            "typed-OODA SQLite snapshot {} failed integrity check: {result}",
            path.display()
        ));
    }
    Ok(())
}

fn snapshot_ladybug(source: &Path, destination: &Path) -> InstallResult<()> {
    {
        let database =
            lbug::Database::new(source, lbug::SystemConfig::default()).map_err(|error| {
                InstallError::new(format!(
                    "failed to open cognitive LadybugDB {}: {error}",
                    source.display()
                ))
            })?;
        let connection = lbug::Connection::new(&database).map_err(|error| {
            InstallError::new(format!("failed to connect to cognitive LadybugDB: {error}"))
        })?;
        connection.query("CHECKPOINT;").map_err(|error| {
            InstallError::new(format!(
                "cognitive LadybugDB checkpoint failed for {}: {error}",
                source.display()
            ))
        })?;
    }
    copy_path(source, destination)?;
    verify_ladybug_snapshot(destination)
}

fn verify_ladybug_snapshot(snapshot: &Path) -> InstallResult<()> {
    let database =
        lbug::Database::new(snapshot, lbug::SystemConfig::default()).map_err(|error| {
            InstallError::new(format!(
                "failed to open staged LadybugDB snapshot {}: {error}",
                snapshot.display()
            ))
        })?;
    let connection = lbug::Connection::new(&database).map_err(|error| {
        InstallError::new(format!("LadybugDB snapshot connection failed: {error}"))
    })?;
    connection
        .query("CALL show_tables() RETURN *")
        .map(|_| ())
        .map_err(|error| {
            InstallError::new(format!(
                "LadybugDB snapshot {} failed verification query: {error}",
                snapshot.display()
            ))
        })
}

fn stage_restore_entries(manifest: &BackupManifest) -> InstallResult<Vec<Option<PathBuf>>> {
    let mut staged = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let parent = entry.destination.parent().ok_or_else(|| {
            InstallError::new(format!(
                "rollback destination has no parent: {}",
                entry.destination.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            InstallError::new(format!(
                "failed to create rollback destination parent {}: {error}",
                parent.display()
            ))
        })?;
        if !entry.existed {
            staged.push(None);
            continue;
        }
        let stage = parent.join(format!(
            ".simard-rollback-{}-{}",
            manifest.transaction_id, entry.name
        ));
        remove_path_if_exists(&stage)?;
        match entry.kind {
            BackupKind::Path => copy_path(&entry.backup, &stage)?,
            BackupKind::Sqlite => snapshot_sqlite(&entry.backup, &stage)?,
            BackupKind::LadybugSnapshot => restore_ladybug_snapshot(&entry.backup, &stage)?,
        }
        sync_path(&stage)?;
        sync_directory(parent)?;
        staged.push(Some(stage));
    }
    Ok(staged)
}

fn restore_ladybug_snapshot(snapshot: &Path, destination: &Path) -> InstallResult<()> {
    copy_path(snapshot, destination)?;
    verify_ladybug_snapshot(destination)
}

fn publish_restore_entries(
    manifest: &BackupManifest,
    staged: &[Option<PathBuf>],
) -> InstallResult<()> {
    publish_restore_entries_inner(manifest, staged, None)
}

fn publish_restore_entries_inner(
    manifest: &BackupManifest,
    staged: &[Option<PathBuf>],
    fail_at: Option<usize>,
) -> InstallResult<()> {
    let mut swapped: Vec<(&BackupEntry, PathBuf)> = Vec::new();
    for (index, (entry, stage)) in manifest.entries.iter().zip(staged).enumerate() {
        let compensation =
            match publish_restore_entry(manifest, index, entry, stage.as_deref(), fail_at) {
                Ok(compensation) => compensation,
                Err(error) => return fail_with_compensation(&swapped, error.to_string()),
            };
        swapped.push((entry, compensation));
        let parent = entry
            .destination
            .parent()
            .expect("validated during staging");
        if let Err(error) = sync_directory(parent) {
            return fail_with_compensation(&swapped, error.to_string());
        }
    }
    for (entry, compensation) in swapped {
        remove_path_if_exists(&compensation)?;
        sync_directory(
            entry
                .destination
                .parent()
                .expect("validated during staging"),
        )?;
    }
    Ok(())
}

fn publish_restore_entry(
    manifest: &BackupManifest,
    index: usize,
    entry: &BackupEntry,
    stage: Option<&Path>,
    fail_at: Option<usize>,
) -> InstallResult<PathBuf> {
    let parent = entry
        .destination
        .parent()
        .expect("validated during staging");
    let compensation = parent.join(format!(
        ".simard-compensation-{}-{}",
        manifest.transaction_id, entry.name
    ));
    if compensation.exists() {
        return err(format!(
            "unfinished rollback compensation requires recovery before retry: {}",
            compensation.display()
        ));
    }
    if entry.destination.exists() {
        fs::rename(&entry.destination, &compensation).map_err(|error| {
            InstallError::new(format!(
                "failed to stage rollback compensation for {}: {error}",
                entry.destination.display()
            ))
        })?;
        if let Err(error) = sync_directory(parent) {
            let current = fs::rename(&compensation, &entry.destination)
                .map_err(|restore| {
                    InstallError::new(format!(
                        "failed to restore {} after durability failure: {restore}",
                        entry.destination.display()
                    ))
                })
                .and_then(|()| sync_directory(parent));
            return Err(current_compensation_error(error.to_string(), current));
        }
    }
    if fail_at == Some(index) {
        return Err(current_compensation_error(
            "injected atomic rollback publication failure".to_string(),
            restore_current_surface(entry, &compensation),
        ));
    }
    if let Some(stage) = stage
        && let Err(error) = fs::rename(stage, &entry.destination)
    {
        return Err(current_compensation_error(
            format!(
                "failed atomic rollback replacement for {}: {error}",
                entry.destination.display()
            ),
            restore_current_surface(entry, &compensation),
        ));
    }
    Ok(compensation)
}

fn compensate_restore(swapped: &[(&BackupEntry, PathBuf)]) -> InstallResult<()> {
    let mut failures = Vec::new();
    for (entry, compensation) in swapped.iter().rev() {
        if let Err(error) = remove_path_if_exists(&entry.destination) {
            failures.push(error.to_string());
            continue;
        }
        if compensation.exists()
            && let Err(error) = fs::rename(compensation, &entry.destination)
        {
            failures.push(format!(
                "rollback compensation failed for {}: {error}",
                entry.destination.display()
            ));
            continue;
        }
        if let Some(parent) = entry.destination.parent()
            && let Err(error) = sync_directory(parent)
        {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        err(failures.join("; "))
    }
}

fn restore_current_surface(entry: &BackupEntry, compensation: &Path) -> InstallResult<()> {
    if !compensation.exists() {
        return Ok(());
    }
    fs::rename(compensation, &entry.destination).map_err(|error| {
        InstallError::new(format!(
            "failed to restore current surface {}: {error}",
            entry.destination.display()
        ))
    })?;
    sync_directory(
        entry
            .destination
            .parent()
            .expect("validated during staging"),
    )
}

fn fail_with_compensation(
    swapped: &[(&BackupEntry, PathBuf)],
    failure: String,
) -> InstallResult<()> {
    match compensate_restore(swapped) {
        Ok(()) => err(failure),
        Err(compensation) => err(format!(
            "{failure}; rollback compensation also failed: {compensation}"
        )),
    }
}

fn current_compensation_error(failure: String, current: InstallResult<()>) -> InstallError {
    match current {
        Ok(()) => InstallError::new(failure),
        Err(error) => InstallError::new(format!(
            "{failure}; current-surface compensation failed: {error}"
        )),
    }
}

fn sync_path(path: &Path) -> InstallResult<()> {
    let metadata = fs::metadata(path).map_err(|error| {
        InstallError::new(format!("failed to inspect staged rollback path: {error}"))
    })?;
    if metadata.is_file() {
        fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                InstallError::new(format!("failed to sync {}: {error}", path.display()))
            })?;
    } else {
        for entry in fs::read_dir(path).map_err(|error| {
            InstallError::new(format!("failed to enumerate {}: {error}", path.display()))
        })? {
            sync_path(
                &entry
                    .map_err(|error| InstallError::new(error.to_string()))?
                    .path(),
            )?;
        }
        sync_directory(path)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> InstallResult<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            InstallError::new(format!(
                "failed to sync directory {}: {error}",
                path.display()
            ))
        })
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
    let expected: BTreeSet<_> = backup_surfaces(layout)
        .into_iter()
        .map(|(name, destination, kind)| (name, destination, kind as u8))
        .collect();
    let actual: BTreeSet<_> = manifest
        .entries
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                entry.destination.clone(),
                entry.kind as u8,
            )
        })
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
    })?;
    let parent = path
        .parent()
        .ok_or_else(|| InstallError::new("rollback manifest has no parent"))?;
    sync_directory(parent)
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
        let mut file = fs::File::open(path).map_err(|error| {
            InstallError::new(format!("failed to hash file {}: {error}", path.display()))
        })?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                InstallError::new(format!("failed to hash file {}: {error}", path.display()))
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
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
        fs::create_dir_all(layout.simard_home.join("state/typed-ooda")).expect("state");
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
        let typed_ooda = rusqlite::Connection::open(
            layout.simard_home.join("state/typed-ooda/outcomes.sqlite3"),
        )
        .expect("state");
        typed_ooda
            .execute_batch(
                "CREATE TABLE rollback_probe(value TEXT NOT NULL);
                 INSERT INTO rollback_probe VALUES ('old-state');",
            )
            .expect("state row");
        drop(typed_ooda);

        let backup =
            create_verified_backup(&layout, Path::new("/bin/true")).expect("verified backup");
        fs::write(&layout.binary_path, b"new-binary").expect("mutate");
        fs::remove_dir_all(&layout.prompt_assets_dir).expect("mutate");
        fs::write(&layout.ooda_unit_path, b"new-unit").expect("mutate");
        fs::write(layout.simard_home.join("config.toml"), b"new-config").expect("mutate");
        fs::remove_dir_all(layout.simard_home.join("state")).expect("mutate");

        restore_verified_backup(&layout, &backup.manifest_path, Path::new("/bin/true"))
            .expect("rollback");

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
        let typed_ooda = rusqlite::Connection::open(
            layout.simard_home.join("state/typed-ooda/outcomes.sqlite3"),
        )
        .expect("restored state");
        assert_eq!(
            typed_ooda
                .query_row("SELECT value FROM rollback_probe", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("restored row"),
            "old-state"
        );
    }

    #[test]
    fn multi_surface_publication_failure_compensates_prior_swaps() {
        let temp = tempfile::tempdir().expect("tempdir");
        let layout = layout(temp.path());
        fs::create_dir_all(&layout.bin_dir).expect("bin");
        fs::create_dir_all(&layout.prompt_assets_dir).expect("assets");
        fs::create_dir_all(&layout.systemd_user_dir).expect("units");
        fs::write(&layout.binary_path, b"prior-binary").expect("binary");
        fs::write(layout.prompt_assets_dir.join("prior"), b"prior-assets").expect("assets");
        let backup =
            create_verified_backup(&layout, Path::new("/bin/true")).expect("verified backup");
        fs::write(&layout.binary_path, b"current-binary").expect("mutate binary");
        fs::write(layout.prompt_assets_dir.join("prior"), b"current-assets")
            .expect("mutate assets");

        let manifest = verify_manifest(&layout, &backup.manifest_path).expect("manifest");
        let staged = stage_restore_entries(&manifest).expect("stage restore");
        publish_restore_entries_inner(&manifest, &staged, Some(1))
            .expect_err("injected publication failure");

        assert_eq!(fs::read(&layout.binary_path).unwrap(), b"current-binary");
        assert_eq!(
            fs::read(layout.prompt_assets_dir.join("prior")).unwrap(),
            b"current-assets"
        );
    }
}
