//! Bundled skill catalog — loads pre-built skill definitions from `skills/`.
//!
//! Each skill lives in `skills/<name>/SKILL.md` and contains YAML front-matter
//! (name, description, version, activation conditions) followed by markdown
//! prompt content.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// Parsed metadata from a skill's YAML front-matter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    pub auto_activates: Vec<String>,
}

/// A bundled skill with metadata and raw markdown body.
#[derive(Clone, Debug)]
pub struct BundledSkill {
    pub meta: SkillMeta,
    /// Full markdown content (including front-matter).
    pub content: String,
    /// Path to the SKILL.md on disk.
    pub path: PathBuf,
}

/// An in-memory catalog of all bundled skills.
#[derive(Clone, Debug, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, BundledSkill>,
}

impl SkillCatalog {
    /// Load every `<dir>/<name>/SKILL.md` under `skills_dir`.
    pub fn load(skills_dir: &Path) -> SimardResult<Self> {
        let mut catalog = Self::default();

        if !skills_dir.is_dir() {
            return Ok(catalog);
        }

        let entries = fs::read_dir(skills_dir).map_err(|e| SimardError::ArtifactIo {
            path: skills_dir.to_path_buf(),
            reason: format!("read skills directory: {e}"),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| SimardError::ArtifactIo {
                path: skills_dir.to_path_buf(),
                reason: format!("read directory entry: {e}"),
            })?;
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }

            let skill_md = dir_path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }

            match load_single_skill(&skill_md) {
                Ok(skill) => {
                    catalog.skills.insert(skill.meta.name.clone(), skill);
                }
                Err(e) => {
                    eprintln!(
                        "warning: failed to load skill at {}: {e}",
                        skill_md.display()
                    );
                }
            }
        }

        Ok(catalog)
    }

    /// Load from the default `skills/` directory adjacent to the binary or
    /// in the repository root.
    pub fn load_default() -> SimardResult<Self> {
        if let Some(dir) = find_skills_dir() {
            Self::load(&dir)
        } else {
            Ok(Self::default())
        }
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&BundledSkill> {
        self.skills.get(name)
    }

    /// Iterate over all skills sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &BundledSkill)> {
        self.skills.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Return skills whose `auto_activates` patterns match the given text.
    pub fn match_auto_activate(&self, text: &str) -> Vec<&BundledSkill> {
        let lower = text.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.meta
                    .auto_activates
                    .iter()
                    .any(|pat| lower.contains(&pat.to_lowercase()))
            })
            .collect()
    }

    /// List all skill names.
    pub fn names(&self) -> Vec<&str> {
        self.skills.keys().map(String::as_str).collect()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_skills_dir() -> Option<PathBuf> {
    // Walk up from current directory.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("skills");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Parse a single SKILL.md file into a `BundledSkill`.
fn load_single_skill(path: &Path) -> SimardResult<BundledSkill> {
    let content = fs::read_to_string(path).map_err(|e| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: format!("read skill file: {e}"),
    })?;

    let meta = parse_front_matter(&content, path)?;

    Ok(BundledSkill {
        meta,
        content,
        path: path.to_path_buf(),
    })
}

/// Extract YAML front-matter delimited by `---` lines.
fn parse_front_matter(content: &str, path: &Path) -> SimardResult<SkillMeta> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SimardError::ArtifactIo {
            path: path.to_path_buf(),
            reason: "skill file missing YAML front-matter (no leading ---)".to_string(),
        });
    }

    // Find closing ---
    let after_first = &trimmed[3..];
    let end = after_first
        .find("\n---")
        .ok_or_else(|| SimardError::ArtifactIo {
            path: path.to_path_buf(),
            reason: "skill file missing closing --- in front-matter".to_string(),
        })?;

    let yaml_block = &after_first[..end];

    // Lightweight YAML parsing — we only need a few fields.
    let name =
        extract_yaml_scalar(yaml_block, "name").unwrap_or_else(|| skill_name_from_path(path));
    let version = extract_yaml_scalar(yaml_block, "version").unwrap_or_default();
    let description = extract_yaml_field(yaml_block, "description").unwrap_or_default();
    let auto_activates = extract_yaml_list(yaml_block, "auto_activates");

    Ok(SkillMeta {
        name,
        version,
        description,
        auto_activates,
    })
}

/// Extract a simple `key: value` scalar from YAML text.
fn extract_yaml_scalar(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in yaml.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() && val != "|" && val != ">" {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Extract a field that may be a scalar or multi-line block scalar.
fn extract_yaml_field(yaml: &str, key: &str) -> Option<String> {
    // Try scalar first.
    if let Some(s) = extract_yaml_scalar(yaml, key) {
        return Some(s);
    }

    // Try block scalar (key: |\n  indented lines).
    let prefix = format!("{key}:");
    let lines: Vec<&str> = yaml.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let rest = trimmed[prefix.len()..].trim();
            if rest == "|" || rest == ">" || rest.is_empty() {
                // Collect indented continuation lines.
                let mut parts = Vec::new();
                for cont in &lines[i + 1..] {
                    if cont.starts_with("  ") || cont.starts_with('\t') {
                        parts.push(cont.trim());
                    } else {
                        break;
                    }
                }
                if !parts.is_empty() {
                    return Some(parts.join(" "));
                }
            }
        }
    }
    None
}

/// Extract a YAML list (key:\n  - item1\n  - item2).
fn extract_yaml_list(yaml: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    let lines: Vec<&str> = yaml.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().starts_with(&prefix) {
            let mut items = Vec::new();
            for cont in &lines[i + 1..] {
                let t = cont.trim();
                if let Some(item) = t.strip_prefix("- ") {
                    items.push(item.trim_matches('"').trim_matches('\'').to_string());
                } else if let Some(item_text) = t.strip_prefix('-') {
                    items.push(
                        item_text
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string(),
                    );
                } else if t.is_empty() {
                    continue;
                } else {
                    break;
                }
            }
            return items;
        }
    }
    Vec::new()
}

/// Derive a skill name from its directory path.
fn skill_name_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_front_matter_extracts_fields() {
        let content = r#"---
name: test-skill
version: 1.0.0
description: A test skill for validation
auto_activates:
  - "run tests"
  - "validate code"
---

# Test Skill

Do the thing.
"#;
        let meta = parse_front_matter(content, Path::new("test/SKILL.md")).unwrap();
        assert_eq!(meta.name, "test-skill");
        assert_eq!(meta.version, "1.0.0");
        assert_eq!(meta.description, "A test skill for validation");
        assert_eq!(meta.auto_activates, vec!["run tests", "validate code"]);
    }

    #[test]
    fn parse_front_matter_block_description() {
        let content = r#"---
name: multi-line
version: 2.0.0
description: |
  This is a multi-line
  description field.
auto_activates:
  - "trigger"
---
body
"#;
        let meta = parse_front_matter(content, Path::new("test/SKILL.md")).unwrap();
        assert_eq!(meta.name, "multi-line");
        assert!(meta.description.contains("multi-line"));
    }

    #[test]
    fn parse_front_matter_missing_opening_errors() {
        let content = "no front matter here";
        let err = parse_front_matter(content, Path::new("bad/SKILL.md"));
        assert!(err.is_err());
    }

    #[test]
    fn catalog_load_directory() {
        let base = std::env::temp_dir().join("simard-test-skill-catalog");
        let _ = fs::remove_dir_all(&base);

        // Create two skill dirs.
        let skill_a = base.join("skill-a");
        fs::create_dir_all(&skill_a).unwrap();
        fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: skill-a\nversion: 1.0.0\ndescription: Alpha\nauto_activates:\n  - \"alpha\"\n---\n# A\n",
        )
        .unwrap();

        let skill_b = base.join("skill-b");
        fs::create_dir_all(&skill_b).unwrap();
        fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: skill-b\nversion: 1.0.0\ndescription: Beta\nauto_activates:\n  - \"beta\"\n---\n# B\n",
        )
        .unwrap();

        let catalog = SkillCatalog::load(&base).unwrap();
        assert_eq!(catalog.len(), 2);
        assert!(catalog.get("skill-a").is_some());
        assert!(catalog.get("skill-b").is_some());

        let names = catalog.names();
        assert!(names.contains(&"skill-a"));
        assert!(names.contains(&"skill-b"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn catalog_auto_activate_matching() {
        let base = std::env::temp_dir().join("simard-test-skill-activate");
        let _ = fs::remove_dir_all(&base);

        let skill_dir = base.join("code-review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: code-review\nversion: 1.0.0\ndescription: Review code\nauto_activates:\n  - \"review pull request\"\n  - \"check code quality\"\n---\n# Review\n",
        )
        .unwrap();

        let catalog = SkillCatalog::load(&base).unwrap();
        let matches = catalog.match_auto_activate("please review pull request #42");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].meta.name, "code-review");

        let no_match = catalog.match_auto_activate("deploy to production");
        assert!(no_match.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn catalog_empty_dir() {
        let base = std::env::temp_dir().join("simard-test-skill-empty-catalog");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let catalog = SkillCatalog::load(&base).unwrap();
        assert!(catalog.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn catalog_nonexistent_dir() {
        let catalog = SkillCatalog::load(Path::new("/nonexistent/path")).unwrap();
        assert!(catalog.is_empty());
    }
}
