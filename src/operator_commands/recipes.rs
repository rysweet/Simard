use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;

use crate::copilot_status_probe::{
    CopilotStatusProbeResult, is_copilot_guarded_recipe, probe_local_copilot_status,
};
use crate::prompt_assets::{FilePromptAssetStore, PromptAsset, PromptAssetRef, PromptAssetStore};
use crate::sanitization::sanitize_terminal_text;

use super::format::{print_display, print_text};
use super::state_root::prompt_root;

pub(crate) fn ensure_terminal_recipe_is_runnable(recipe_name: &str) -> crate::SimardResult<()> {
    if !is_copilot_guarded_recipe(recipe_name) {
        return Ok(());
    }

    match probe_local_copilot_status() {
        CopilotStatusProbeResult::Available { .. } => Ok(()),
        CopilotStatusProbeResult::Unavailable {
            reason_code,
            detail,
        }
        | CopilotStatusProbeResult::Unsupported {
            reason_code,
            detail,
        } => Err(crate::SimardError::ActionExecutionFailed {
            action: recipe_name.to_string(),
            reason: format!("{reason_code}: {detail}"),
        }),
    }
}

const TERMINAL_RECIPE_DIRECTORY: &str = "simard/terminal_recipes";
const TERMINAL_RECIPE_EXTENSION: &str = "simard-terminal";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalRecipeDescriptor {
    pub(crate) name: String,
    pub(crate) reference: PromptAssetRef,
}

pub(crate) fn list_terminal_recipe_descriptors()
-> crate::SimardResult<Vec<TerminalRecipeDescriptor>> {
    let recipe_root = prompt_root().join(TERMINAL_RECIPE_DIRECTORY);
    let entries =
        fs::read_dir(&recipe_root).map_err(|error| crate::SimardError::PromptAssetRead {
            path: recipe_root.clone(),
            reason: error.to_string(),
        })?;
    let mut recipes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| crate::SimardError::PromptAssetRead {
            path: recipe_root.clone(),
            reason: error.to_string(),
        })?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| crate::SimardError::PromptAssetRead {
                path: entry_path.clone(),
                reason: error.to_string(),
            })?;
        if !file_type.is_file()
            || entry_path.extension() != Some(OsStr::new(TERMINAL_RECIPE_EXTENSION))
        {
            continue;
        }
        let Some(stem) = entry_path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        recipes.push(TerminalRecipeDescriptor {
            name: stem.to_string(),
            reference: terminal_recipe_reference(stem)?,
        });
    }
    recipes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(recipes)
}

pub(crate) fn load_terminal_recipe(recipe_name: &str) -> crate::SimardResult<PromptAsset> {
    let reference = terminal_recipe_reference(recipe_name)?;
    FilePromptAssetStore::new(prompt_root()).load(&reference)
}

fn terminal_recipe_reference(recipe_name: &str) -> crate::SimardResult<PromptAssetRef> {
    if recipe_name.is_empty()
        || !recipe_name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(crate::SimardError::InvalidPromptAssetPath {
            asset_id: format!("terminal-recipe:{recipe_name}"),
            path: PathBuf::from(recipe_name),
            reason: "recipe names may only use lowercase ASCII letters, digits, and hyphens"
                .to_string(),
        });
    }
    Ok(PromptAssetRef::new(
        format!("terminal-recipe:{recipe_name}"),
        PathBuf::from(TERMINAL_RECIPE_DIRECTORY)
            .join(format!("{recipe_name}.{TERMINAL_RECIPE_EXTENSION}")),
    ))
}

pub(crate) fn print_terminal_recipe(recipe_name: &str, recipe: &PromptAsset) {
    print_text("Terminal recipe", recipe_name);
    print_display("Recipe asset", recipe.relative_path.display());
    println!("Recipe contents:");
    for line in sanitize_terminal_text(&recipe.contents).lines() {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_recipe_reference_valid_name() {
        let reference = terminal_recipe_reference("my-recipe-1").unwrap();
        assert_eq!(
            reference.id,
            crate::prompt_assets::PromptAssetId::new("terminal-recipe:my-recipe-1")
        );
        assert_eq!(
            reference.relative_path,
            PathBuf::from("simard/terminal_recipes/my-recipe-1.simard-terminal")
        );
    }

    #[test]
    fn terminal_recipe_reference_rejects_empty() {
        assert!(terminal_recipe_reference("").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_uppercase() {
        assert!(terminal_recipe_reference("MyRecipe").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_spaces() {
        assert!(terminal_recipe_reference("my recipe").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_underscores() {
        assert!(terminal_recipe_reference("my_recipe").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_dots() {
        assert!(terminal_recipe_reference("my.recipe").is_err());
    }

    #[test]
    fn terminal_recipe_reference_accepts_digits() {
        let reference = terminal_recipe_reference("recipe-123").unwrap();
        assert!(reference.id.as_str().contains("recipe-123"));
    }

    #[test]
    fn terminal_recipe_reference_rejects_slashes() {
        assert!(terminal_recipe_reference("path/recipe").is_err());
    }

    #[test]
    fn list_terminal_recipe_descriptors_returns_sorted() {
        let recipes = list_terminal_recipe_descriptors().unwrap();
        for i in 0..recipes.len().saturating_sub(1) {
            assert!(
                recipes[i].name <= recipes[i + 1].name,
                "recipes should be sorted by name"
            );
        }
    }

    #[test]
    fn terminal_recipe_reference_rejects_special_chars() {
        assert!(terminal_recipe_reference("recipe!").is_err());
        assert!(terminal_recipe_reference("recipe@name").is_err());
        assert!(terminal_recipe_reference("recipe#1").is_err());
    }

    #[test]
    fn terminal_recipe_reference_accepts_all_lowercase_digits_hyphens() {
        assert!(terminal_recipe_reference("a").is_ok());
        assert!(terminal_recipe_reference("abc-def-123").is_ok());
        assert!(terminal_recipe_reference("0").is_ok());
    }
}
