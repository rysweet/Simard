use std::fs;
use std::path::{Component, Path, PathBuf};

use super::paths::{InstallLayout, backup_path};
use super::{InstallError, InstallResult, err};

pub fn discover_prompt_asset_root() -> InstallResult<PathBuf> {
    let mut candidates = Vec::new();
    for key in [
        "SIMARD_INSTALL_PROMPT_ASSETS_ROOT",
        "SIMARD_PROMPT_ASSET_ROOT",
    ] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            candidates.push(PathBuf::from(value));
        }
    }

    if let Some(value) = std::env::var_os("SIMARD_PROMPT_ASSETS_DIR")
        && !value.is_empty()
    {
        let path = PathBuf::from(value);
        if path.file_name().is_some_and(|name| name == "simard") {
            if let Some(parent) = path.parent() {
                candidates.push(parent.to_path_buf());
            }
        } else {
            candidates.push(path);
        }
    }

    candidates.push(
        std::env::current_dir()
            .map(|path| path.join("prompt_assets"))
            .map_err(|error| {
                InstallError::new(format!("failed to inspect current directory: {error}"))
            })?,
    );
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets"));

    for candidate in candidates {
        if has_required_assets(&candidate) {
            return Ok(candidate);
        }
    }

    err(format!(
        "prompt_assets source not found; expected prompt_assets/simard/ooda_orient.md and prompt_assets/simard/recipes/ooda-orient.yaml under SIMARD_INSTALL_PROMPT_ASSETS_ROOT, SIMARD_PROMPT_ASSET_ROOT, SIMARD_PROMPT_ASSETS_DIR, current directory, or {}",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("prompt_assets")
            .display()
    ))
}

pub fn validate_prompt_source(source: &Path) -> InstallResult<()> {
    if !source.is_dir() {
        return err(format!(
            "prompt_assets source is not a directory: {}",
            source.display()
        ));
    }
    for required in ["simard/ooda_orient.md", "simard/recipes/ooda-orient.yaml"]
        .into_iter()
        .chain(super::REQUIRED_TYPED_OODA_ASSETS)
    {
        let path = source.join(required);
        if !path.is_file() {
            return err(format!(
                "prompt_assets source is missing required asset {}",
                path.display()
            ));
        }
    }
    Ok(())
}

pub fn validate_live_prompt_target(path: &Path) -> InstallResult<()> {
    if path.exists() && !path.is_dir() {
        return err(format!(
            "cannot install prompt_assets because {} exists and is not a directory",
            path.display()
        ));
    }
    Ok(())
}

pub fn stage_prompt_assets(source: &Path, staged: &Path) -> InstallResult<()> {
    copy_dir_recursive(source, staged, source)
}

pub fn replace_live_prompt_assets(staged: &Path, layout: &InstallLayout) -> InstallResult<()> {
    validate_live_prompt_target(&layout.prompt_assets_dir)?;

    let backup = if layout.prompt_assets_dir.exists() {
        let backup = backup_path(layout, "prompt_assets");
        fs::rename(&layout.prompt_assets_dir, &backup).map_err(|error| {
            InstallError::new(format!(
                "failed to preserve previous prompt_assets tree {} to {}: {error}",
                layout.prompt_assets_dir.display(),
                backup.display()
            ))
        })?;
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(staged, &layout.prompt_assets_dir) {
        if let Some(backup) = backup {
            let restore = fs::rename(&backup, &layout.prompt_assets_dir);
            return match restore {
                Ok(()) => Err(InstallError::new(format!(
                    "failed atomic prompt_assets replacement from {} to {}; restored previous prompt_assets from {}: {error}",
                    staged.display(),
                    layout.prompt_assets_dir.display(),
                    backup.display()
                ))),
                Err(restore_error) => Err(InstallError::new(format!(
                    "failed atomic prompt_assets replacement from {} to {}: {error}; additionally failed to restore previous prompt_assets from {}: {restore_error}",
                    staged.display(),
                    layout.prompt_assets_dir.display(),
                    backup.display()
                ))),
            };
        }
        return Err(InstallError::new(format!(
            "failed atomic prompt_assets replacement from {} to {}: {error}",
            staged.display(),
            layout.prompt_assets_dir.display()
        )));
    }

    Ok(())
}

fn has_required_assets(root: &Path) -> bool {
    root.join("simard/ooda_orient.md").is_file()
        && root.join("simard/recipes/ooda-orient.yaml").is_file()
        && root
            .join("simard/recipes/goal-session-actor.yaml")
            .is_file()
        && root
            .join("simard/policies/goal-session-capabilities.toml")
            .is_file()
}

fn copy_dir_recursive(source: &Path, destination: &Path, root: &Path) -> InstallResult<()> {
    let metadata = fs::metadata(source).map_err(|error| {
        InstallError::new(format!(
            "failed to read prompt_assets path {}: {error}",
            source.display()
        ))
    })?;

    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|error| {
            InstallError::new(format!(
                "failed to create staged prompt_assets directory {}: {error}",
                destination.display()
            ))
        })?;
        for entry in fs::read_dir(source).map_err(|error| {
            InstallError::new(format!(
                "failed to list prompt_assets directory {}: {error}",
                source.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                InstallError::new(format!(
                    "failed to read prompt_assets directory entry under {}: {error}",
                    source.display()
                ))
            })?;
            let child_source = entry.path();
            let child_name = entry.file_name();
            let child_destination = destination.join(child_name);
            copy_dir_recursive(&child_source, &child_destination, root)?;
        }
        return Ok(());
    }

    if metadata.is_file() {
        validate_asset_path(source, root)?;
        fs::copy(source, destination).map_err(|error| {
            InstallError::new(format!(
                "failed to stage prompt asset {} to {}: {error}",
                source.display(),
                destination.display()
            ))
        })?;
        return Ok(());
    }

    err(format!(
        "unsupported prompt_assets entry {}; only regular files and directories are installable",
        source.display()
    ))
}

fn validate_asset_path(path: &Path, root: &Path) -> InstallResult<()> {
    let relative = path.strip_prefix(root).map_err(|error| {
        InstallError::new(format!(
            "prompt asset {} is not under source root {}: {error}",
            path.display(),
            root.display()
        ))
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return err(format!(
            "unsafe prompt_assets path escapes asset root: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::paths::InstallLayout;

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

    #[test]
    fn prompt_assets_restore_previous_tree_when_staged_rename_fails() {
        let root =
            std::env::temp_dir().join(format!("simard-prompt-rollback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let layout = layout(&root);
        fs::create_dir_all(layout.prompt_assets_dir.join("simard/recipes")).unwrap();
        fs::create_dir_all(&layout.backup_root).unwrap();
        fs::write(
            layout.prompt_assets_dir.join("simard/ooda_orient.md"),
            "previous prompt",
        )
        .unwrap();

        let missing_staged = root.join(".install-staging/missing/prompt_assets");
        let error =
            replace_live_prompt_assets(&missing_staged, &layout).expect_err("missing staged tree");

        assert!(
            error
                .to_string()
                .contains("restored previous prompt_assets"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(layout.prompt_assets_dir.join("simard/ooda_orient.md")).unwrap(),
            "previous prompt"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
