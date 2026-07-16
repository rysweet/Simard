//! Package orchestration, manifest, and verification.
//!
//! [`build_package`] is the end-to-end entry point: brief → scaled menu →
//! shopping list + cost + nutrition → prep schedule → menu card → optional prep
//! app → verified `manifest.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::analysis::{self, NutritionSummary, ShoppingList};
use super::app;
use super::brief::{MenuBrief, Nutrition};
use super::card;
use super::error::{GastronomeError, GastronomeResult};
use super::menu::{self, Menu};
use super::schedule::{self, PrepSchedule};

/// Options controlling which artifacts are produced.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Also emit the self-contained `prep_app.html` kitchen app.
    pub prep_app: bool,
}

/// One produced (or skipped) artifact in the package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub file: String,
    pub kind: String,
    pub present: bool,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A single verification check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Verification result for the whole package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub ok: bool,
    pub checks: Vec<Check>,
}

/// Nutrition figures embedded in the manifest.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NutritionManifest {
    pub kcal: f64,
    pub protein_g: f64,
    pub carbs_g: f64,
    pub fat_g: f64,
}

impl From<Nutrition> for NutritionManifest {
    fn from(n: Nutrition) -> Self {
        Self {
            kcal: round(n.kcal),
            protein_g: round(n.protein_g),
            carbs_g: round(n.carbs_g),
            fat_g: round(n.fat_g),
        }
    }
}

/// The package manifest, written as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub event: String,
    pub guests: u32,
    pub currency: String,
    pub dish_count: u32,
    pub course_count: u32,
    pub total_servings: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_total_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_per_guest: Option<f64>,
    pub over_budget: bool,
    pub per_guest_nutrition: NutritionManifest,
    pub total_prep_minutes: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_time: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub verification: Verification,
}

impl Manifest {
    /// Consume the manifest, returning an error if verification did not pass.
    /// Advisory checks (budget) never fail this; only the required minimum
    /// (menu, shopping list, nutrition, prep schedule) does.
    pub fn verified(self) -> GastronomeResult<Self> {
        if self.verification.ok {
            Ok(self)
        } else {
            let failed: Vec<String> = self
                .verification
                .checks
                .iter()
                .filter(|c| !c.ok)
                .map(|c| format!("{}: {}", c.name, c.detail))
                .collect();
            Err(GastronomeError::verification(failed.join("; ")))
        }
    }
}

const MENU_MD: &str = "menu.md";
const SHOPPING_CSV: &str = "shopping_list.csv";
const NUTRITION_CSV: &str = "nutrition.csv";
const SCHEDULE_CSV: &str = "prep_schedule.csv";
const PREP_APP_HTML: &str = "prep_app.html";
const MANIFEST_JSON: &str = "manifest.json";

fn write_file(path: &Path, contents: &str) -> GastronomeResult<()> {
    std::fs::write(path, contents)
        .map_err(|e| GastronomeError::io(format!("writing {}", path.display()), e))
}

fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Build the full menu plan for `brief_path` into `out_dir`.
pub fn build_package(
    brief_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> GastronomeResult<Manifest> {
    let brief = MenuBrief::from_path(brief_path)?;
    build_package_from_brief(&brief, out_dir, options)
}

/// Build a package from an already-parsed brief.
pub fn build_package_from_brief(
    brief: &MenuBrief,
    out_dir: &Path,
    options: BuildOptions,
) -> GastronomeResult<Manifest> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| GastronomeError::io(format!("creating {}", out_dir.display()), e))?;

    let menu = menu::scale(brief);
    let shopping = analysis::build_shopping_list(&menu, brief.budget);
    let nutrition = analysis::build_nutrition(&menu);
    let prep = schedule::build_schedule(brief);

    // 1. Shopping list (always).
    let shopping_path = out_dir.join(SHOPPING_CSV);
    write_file(&shopping_path, &shopping.to_csv())?;

    // 2. Nutrition breakdown (always).
    let nutrition_path = out_dir.join(NUTRITION_CSV);
    write_file(&nutrition_path, &nutrition.to_csv())?;

    // 3. Prep schedule (always).
    let schedule_path = out_dir.join(SCHEDULE_CSV);
    write_file(&schedule_path, &prep.to_csv())?;

    // 4. Menu card (always).
    let card_path = out_dir.join(MENU_MD);
    write_file(
        &card_path,
        &card::render_menu_card(&menu, &nutrition, &shopping),
    )?;

    let mut artifacts = vec![
        artifact(&card_path, MENU_MD, "menu-card", None),
        artifact(&shopping_path, SHOPPING_CSV, "shopping-list", None),
        artifact(&nutrition_path, NUTRITION_CSV, "nutrition", None),
        artifact(&schedule_path, SCHEDULE_CSV, "prep-schedule", None),
    ];

    // 5. Optional prep app.
    if options.prep_app {
        let app_path = out_dir.join(PREP_APP_HTML);
        let report = app::write_prep_app(&menu, &prep, &app_path)?;
        artifacts.push(Artifact {
            file: PREP_APP_HTML.to_string(),
            kind: "prep-app".to_string(),
            present: report.produced && bytes_of(&app_path) > 0,
            bytes: bytes_of(&app_path),
            detail: Some(report.detail),
        });
    }

    let verification = verify(&menu, &shopping, &nutrition, &prep);

    let manifest = Manifest {
        event: menu.event.clone(),
        guests: menu.guests,
        currency: menu.currency.clone(),
        dish_count: menu.dishes.len() as u32,
        course_count: menu.course_count() as u32,
        total_servings: menu.total_servings(),
        estimated_total_cost: shopping.total_cost.map(round),
        cost_per_guest: shopping.cost_per_guest.map(round),
        over_budget: shopping.over_budget,
        per_guest_nutrition: nutrition.per_guest.into(),
        total_prep_minutes: round(prep.total_minutes),
        service_time: prep.service_time.clone(),
        artifacts,
        verification,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| GastronomeError::parse("manifest json", e.to_string()))?;
    write_file(&out_dir.join(MANIFEST_JSON), &manifest_json)?;

    Ok(manifest)
}

fn artifact(path: &Path, file: &str, kind: &str, detail: Option<String>) -> Artifact {
    let bytes = bytes_of(path);
    Artifact {
        file: file.to_string(),
        kind: kind.to_string(),
        present: bytes > 0,
        bytes,
        detail,
    }
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

/// Verify the produced plan against the menu and analysis outputs.
fn verify(
    menu: &Menu,
    shopping: &ShoppingList,
    nutrition: &NutritionSummary,
    prep: &PrepSchedule,
) -> Verification {
    let mut checks = Vec::new();

    // Menu sanity: every dish produces at least one whole serving and has
    // ingredients.
    let menu_ok = !menu.dishes.is_empty()
        && menu
            .dishes
            .iter()
            .all(|d| d.total_servings > 0 && !d.ingredients.is_empty());
    checks.push(check(
        "menu-valid",
        menu_ok,
        if menu_ok {
            format!(
                "{} dish(es), all with servings and ingredients",
                menu.dishes.len()
            )
        } else {
            "a dish has no servings or no ingredients".to_string()
        },
    ));

    // Shopping list non-empty.
    let shopping_ok = !shopping.rows.is_empty();
    checks.push(check(
        "shopping-list-present",
        shopping_ok,
        format!("{} shopping-list line(s)", shopping.rows.len()),
    ));

    // Nutrition breakdown present (a row per dish always exists once scaled).
    let nutrition_ok = !nutrition.dishes.is_empty();
    checks.push(check(
        "nutrition-present",
        nutrition_ok,
        if nutrition.has_data {
            format!("{} kcal per guest", round(nutrition.per_guest.kcal))
        } else {
            "nutrition breakdown emitted (no per-ingredient data in brief)".to_string()
        },
    ));

    // Prep schedule internally consistent (sequential, finishes at service).
    let schedule_ok = prep.is_ordered();
    checks.push(check(
        "prep-schedule-ordered",
        schedule_ok,
        if schedule_ok {
            format!(
                "{} task(s), {} prep-minute critical path",
                prep.tasks.len(),
                round(prep.total_minutes)
            )
        } else {
            "prep schedule offsets are inconsistent".to_string()
        },
    ));

    // Budget (advisory only — does not fail the package).
    checks.push(check(
        "within-budget",
        !shopping.over_budget,
        if shopping.over_budget {
            "estimated cost exceeds the brief budget".into()
        } else {
            "within budget (or no budget set)".to_string()
        },
    ));

    // Required minimum for `ok`. Budget is advisory.
    let ok = menu_ok && shopping_ok && nutrition_ok && schedule_ok;

    Verification { ok, checks }
}

/// Read and re-check an existing menu plan in `out_dir`.
///
/// `inspect` re-verifies the package against what is actually on disk: it
/// re-scans every artifact and, when a **required** artifact (menu card,
/// shopping list, nutrition, prep schedule) has gone missing or empty since
/// build time, it flips the corresponding verification check and the aggregate
/// `verification.ok` to `false`. The menu/analysis are not re-derived at inspect
/// time, so the menu-valid check is trusted from the persisted manifest; only
/// the artifact-presence checks are recomputed.
pub fn inspect(out_dir: &Path) -> GastronomeResult<Manifest> {
    let manifest_path = out_dir.join(MANIFEST_JSON);
    let bytes = std::fs::read(&manifest_path)
        .map_err(|e| GastronomeError::io(format!("reading {}", manifest_path.display()), e))?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| GastronomeError::parse("manifest json", e.to_string()))?;

    // Re-confirm artifacts still exist on disk.
    for artifact in &mut manifest.artifacts {
        let path: PathBuf = out_dir.join(&artifact.file);
        let bytes = bytes_of(&path);
        if artifact.present && bytes == 0 {
            artifact.present = false;
            artifact.detail = Some("artifact missing on disk at inspect time".into());
        }
    }

    let present = |file: &str| -> bool {
        manifest
            .artifacts
            .iter()
            .any(|a| a.file == file && a.present)
    };
    let shopping_ok = present(SHOPPING_CSV);
    let nutrition_ok = present(NUTRITION_CSV);
    let schedule_ok = present(SCHEDULE_CSV);
    let card_ok = present(MENU_MD);

    for check in &mut manifest.verification.checks {
        let (ok, detail): (bool, Option<String>) = match check.name.as_str() {
            "shopping-list-present" if !shopping_ok => (
                false,
                Some("shopping_list.csv missing or empty at inspect time".into()),
            ),
            "nutrition-present" if !nutrition_ok => (
                false,
                Some("nutrition.csv missing or empty at inspect time".into()),
            ),
            "prep-schedule-ordered" if !schedule_ok => (
                false,
                Some("prep_schedule.csv missing or empty at inspect time".into()),
            ),
            "menu-valid" if !card_ok => (
                false,
                Some("menu.md missing or empty at inspect time".into()),
            ),
            _ => (check.ok, None),
        };
        check.ok = ok;
        if let Some(detail) = detail {
            check.detail = detail;
        }
    }

    let required_ok = manifest
        .verification
        .checks
        .iter()
        .filter(|c| {
            matches!(
                c.name.as_str(),
                "menu-valid"
                    | "shopping-list-present"
                    | "nutrition-present"
                    | "prep-schedule-ordered"
            )
        })
        .all(|c| c.ok);
    manifest.verification.ok = required_ok;

    Ok(manifest)
}

/// Round to two decimal places for stable manifest output.
fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const DINNER: &str = r#"{
        "event":"Autumn dinner","guests":12,"currency":"USD","service_time":"19:00","budget":200.0,
        "dishes":[
            {"name":"Squash soup","course":"starter","tags":["vegetarian"],
             "ingredients":[
                {"name":"Squash","qty_per_serving":180,"unit":"g","cost_per_unit":0.004,
                 "nutrition":{"kcal":0.45,"protein_g":0.01,"carbs_g":0.12,"fat_g":0.001}},
                {"name":"Cream","qty_per_serving":30,"unit":"ml","cost_per_unit":0.006}],
             "prep":[{"task":"Roast squash","minutes":40,"station":"oven"},
                     {"task":"Blend soup","minutes":10,"station":"prep"}]},
            {"name":"Beef roast","course":"main",
             "ingredients":[
                {"name":"Beef","qty_per_serving":220,"unit":"g","cost_per_unit":0.02,
                 "nutrition":{"kcal":2.5,"protein_g":0.26,"carbs_g":0.0,"fat_g":0.15}}],
             "prep":[{"task":"Sear beef","minutes":20,"station":"stove"},
                     {"task":"Roast beef","minutes":90,"station":"oven"}]}
        ]}"#;

    fn build(json: &str, options: BuildOptions) -> (Manifest, tempfile::TempDir) {
        let brief = MenuBrief::from_json_bytes(json.as_bytes()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_package_from_brief(&brief, dir.path(), options).unwrap();
        (manifest, dir)
    }

    #[test]
    fn build_produces_all_core_artifacts_and_passes() {
        let (m, dir) = build(DINNER, BuildOptions::default());
        for f in [
            MENU_MD,
            SHOPPING_CSV,
            NUTRITION_CSV,
            SCHEDULE_CSV,
            MANIFEST_JSON,
        ] {
            assert!(
                bytes_of(&dir.path().join(f)) > 0,
                "{f} should be produced non-empty"
            );
        }
        assert!(
            m.verification.ok,
            "verification should pass: {:?}",
            m.verification
        );
        assert_eq!(m.dish_count, 2);
        assert_eq!(m.guests, 12);
        assert!(m.estimated_total_cost.is_some());
        assert!(m.total_prep_minutes > 0.0);
    }

    #[test]
    fn build_without_prep_app_omits_it() {
        let (m, dir) = build(DINNER, BuildOptions::default());
        assert!(!dir.path().join(PREP_APP_HTML).exists());
        assert!(!m.artifacts.iter().any(|a| a.file == PREP_APP_HTML));
    }

    #[test]
    fn build_with_prep_app_emits_it() {
        let (m, dir) = build(DINNER, BuildOptions { prep_app: true });
        assert!(bytes_of(&dir.path().join(PREP_APP_HTML)) > 0);
        assert!(
            m.artifacts
                .iter()
                .any(|a| a.file == PREP_APP_HTML && a.present)
        );
    }

    #[test]
    fn over_budget_is_advisory_not_fatal() {
        let json = DINNER.replace("\"budget\":200.0", "\"budget\":1.0");
        let (m, _dir) = build(&json, BuildOptions::default());
        assert!(m.over_budget);
        // Required checks still pass -> verification.ok stays true.
        assert!(m.verification.ok);
        assert!(m.clone().verified().is_ok());
    }

    #[test]
    fn inspect_matches_a_fresh_build() {
        let (built, dir) = build(DINNER, BuildOptions { prep_app: true });
        let inspected = inspect(dir.path()).unwrap();
        assert_eq!(built.verification.ok, inspected.verification.ok);
        assert_eq!(built.dish_count, inspected.dish_count);
        assert!(inspected.verification.ok);
    }

    #[test]
    fn inspect_flips_to_fail_when_required_artifact_removed() {
        let (_m, dir) = build(DINNER, BuildOptions::default());
        std::fs::remove_file(dir.path().join(SHOPPING_CSV)).unwrap();
        let inspected = inspect(dir.path()).unwrap();
        assert!(!inspected.verification.ok);
        assert!(inspected.clone().verified().is_err());
    }

    #[test]
    fn verified_reports_failure_reasons() {
        let mut m = build(DINNER, BuildOptions::default()).0;
        m.verification.ok = false;
        m.verification.checks[1].ok = false;
        m.verification.checks[1].detail = "boom".into();
        let err = m.verified().unwrap_err();
        assert!(err.to_string().contains("boom"));
    }
}
