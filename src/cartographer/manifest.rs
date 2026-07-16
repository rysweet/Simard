//! Package orchestration, manifest, and verification.
//!
//! [`build_package`] is the end-to-end entry point: brief → profiled dataset →
//! designed charts → interactive dashboard + written narrative + analysis JSON
//! (+ optional Streamlit app) → verified `manifest.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::brief::DashboardBrief;
use super::dashboard;
use super::dataset::{Dataset, DatasetProfile};
use super::drivers::{self, ToolReport};
use super::error::{CartographerError, CartographerResult};
use super::narrative;
use super::viz::{self, ChartSpec};

/// Options controlling which deliverables are produced.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// Also emit a Streamlit `app.py` alternate-delivery file.
    pub streamlit: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { streamlit: true }
    }
}

/// The persisted analysis document (`analysis.json`): the machine-readable
/// counterpart of the narrative, and the data source for `app.py`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDocument {
    pub name: String,
    pub question: String,
    pub profile: DatasetProfile,
    pub charts: Vec<ChartSpec>,
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

/// The package manifest, written as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub title: String,
    pub question: String,
    pub dataset_format: String,
    pub row_count: usize,
    pub column_count: usize,
    pub truncated: bool,
    pub chart_count: usize,
    pub chart_kinds: Vec<String>,
    pub tools: Vec<ToolReport>,
    pub artifacts: Vec<Artifact>,
    pub verification: Verification,
}

impl Manifest {
    /// Consume the manifest, returning an error if verification did not pass.
    pub fn verified(self) -> CartographerResult<Self> {
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
            Err(CartographerError::verification(failed.join("; ")))
        }
    }
}

const ANALYSIS_JSON: &str = "analysis.json";
const DASHBOARD_HTML: &str = "dashboard.html";
const NARRATIVE_MD: &str = "narrative.md";
const APP_PY: &str = "app.py";
const MANIFEST_JSON: &str = "manifest.json";

fn write_file(path: &Path, contents: &str) -> CartographerResult<()> {
    std::fs::write(path, contents)
        .map_err(|e| CartographerError::io(format!("writing {}", path.display()), e))
}

fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Build a dashboard package from a brief file, resolving the dataset relative
/// to the brief's directory.
pub fn build_package(
    brief_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> CartographerResult<Manifest> {
    let brief = DashboardBrief::from_path(brief_path)?;
    let dataset_path = brief.resolved_dataset(brief_path);
    build_package_from_brief(&brief, &dataset_path, out_dir, options)
}

/// Build a dashboard package from an ad-hoc dataset path + question.
pub fn build_package_ad_hoc(
    dataset_path: &Path,
    question: &str,
    out_dir: &Path,
    options: BuildOptions,
) -> CartographerResult<Manifest> {
    let brief =
        DashboardBrief::from_dataset_and_question(dataset_path.to_string_lossy(), question)?;
    build_package_from_brief(&brief, dataset_path, out_dir, options)
}

/// Build a dashboard package from an already-parsed brief and a resolved
/// dataset path.
pub fn build_package_from_brief(
    brief: &DashboardBrief,
    dataset_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> CartographerResult<Manifest> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| CartographerError::io(format!("creating {}", out_dir.display()), e))?;

    // 1. Exploratory analysis: load + profile the dataset.
    let dataset = Dataset::load(dataset_path, brief.format(), brief.effective_max_rows())?;
    let profile = dataset.profile();

    // 2. Visualization design.
    let charts = viz::design_charts(brief, &dataset);

    // 3. Analysis JSON (machine-readable story + chart data).
    let analysis = AnalysisDocument {
        name: brief.name.clone(),
        question: brief.question.clone(),
        profile: profile.clone(),
        charts: charts.clone(),
    };
    let analysis_json = serde_json::to_string_pretty(&analysis)
        .map_err(|e| CartographerError::parse("analysis json", e.to_string()))?;
    let analysis_path = out_dir.join(ANALYSIS_JSON);
    write_file(&analysis_path, &analysis_json)?;

    // 4. Interactive dashboard (Plotly HTML).
    let html = dashboard::render_html(brief, &profile, &charts);
    let dashboard_path = out_dir.join(DASHBOARD_HTML);
    write_file(&dashboard_path, &html)?;

    // 5. Written narrative.
    let md = narrative::generate_narrative(brief, &profile, &charts);
    let narrative_path = out_dir.join(NARRATIVE_MD);
    write_file(&narrative_path, &md)?;

    // 6. Optional Streamlit alternate delivery.
    let mut app_present = false;
    let app_path = out_dir.join(APP_PY);
    if options.streamlit {
        let app = dashboard::render_streamlit_app(brief);
        write_file(&app_path, &app)?;
        app_present = true;
    }

    let mut artifacts = vec![
        artifact(&analysis_path, ANALYSIS_JSON, "analysis", None),
        artifact(&dashboard_path, DASHBOARD_HTML, "dashboard", None),
        artifact(&narrative_path, NARRATIVE_MD, "narrative", None),
    ];
    artifacts.push(if app_present {
        artifact(&app_path, APP_PY, "streamlit-app", None)
    } else {
        Artifact {
            file: APP_PY.to_string(),
            kind: "streamlit-app".to_string(),
            present: false,
            bytes: 0,
            detail: Some("streamlit app not requested".into()),
        }
    });

    let tools = drivers::probe_delivery_tools();

    let verification = verify(
        &charts,
        &dashboard_path,
        &narrative_path,
        &analysis_path,
        dataset.row_count,
    );

    let chart_kinds: Vec<String> = charts
        .iter()
        .map(|c| c.data.kind_label().to_string())
        .collect();

    let manifest = Manifest {
        name: brief.name.clone(),
        title: brief.title().to_string(),
        question: brief.question.clone(),
        dataset_format: brief.format().label().to_string(),
        row_count: dataset.row_count,
        column_count: profile.column_count,
        truncated: dataset.truncated,
        chart_count: charts.len(),
        chart_kinds,
        tools,
        artifacts,
        verification,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| CartographerError::parse("manifest json", e.to_string()))?;
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

fn file_ok(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Verify the produced package against the required deliverables.
fn verify(
    charts: &[ChartSpec],
    dashboard_path: &Path,
    narrative_path: &Path,
    analysis_path: &Path,
    row_count: usize,
) -> Verification {
    let mut checks = Vec::new();

    let data_ok = row_count > 0;
    checks.push(check(
        "dataset-loaded",
        data_ok,
        format!("{row_count} data rows profiled"),
    ));

    let charts_ok = !charts.is_empty();
    checks.push(check(
        "charts-present",
        charts_ok,
        format!("{} chart(s) designed", charts.len()),
    ));

    let dashboard_ok = file_ok(dashboard_path);
    checks.push(check(
        "dashboard-present",
        dashboard_ok,
        if dashboard_ok {
            "dashboard.html written".into()
        } else {
            "dashboard.html missing or empty".to_string()
        },
    ));

    let narrative_ok = file_ok(narrative_path);
    checks.push(check(
        "narrative-present",
        narrative_ok,
        if narrative_ok {
            "narrative.md written".into()
        } else {
            "narrative.md missing or empty".to_string()
        },
    ));

    let analysis_ok = file_ok(analysis_path);
    checks.push(check(
        "analysis-present",
        analysis_ok,
        if analysis_ok {
            "analysis.json written".into()
        } else {
            "analysis.json missing or empty".to_string()
        },
    ));

    let ok = data_ok && charts_ok && dashboard_ok && narrative_ok && analysis_ok;
    Verification { ok, checks }
}

/// Names of the checks that gate the aggregate `verification.ok`.
const REQUIRED_CHECKS: &[&str] = &[
    "dataset-loaded",
    "charts-present",
    "dashboard-present",
    "narrative-present",
    "analysis-present",
];

/// Read and re-verify an existing package manifest in `out_dir`.
///
/// `inspect` re-checks required artifacts against what is actually on disk so a
/// corrupted package cannot report a stale PASS. The dataset/chart counts are
/// trusted from the persisted manifest (they cannot change on disk); only the
/// artifact-presence checks are recomputed.
pub fn inspect(out_dir: &Path) -> CartographerResult<Manifest> {
    let manifest_path = out_dir.join(MANIFEST_JSON);
    let bytes = std::fs::read(&manifest_path)
        .map_err(|e| CartographerError::io(format!("reading {}", manifest_path.display()), e))?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| CartographerError::parse("manifest json", e.to_string()))?;

    // Re-confirm artifacts still exist on disk.
    for artifact in &mut manifest.artifacts {
        let path: PathBuf = out_dir.join(&artifact.file);
        let present = file_ok(&path);
        if artifact.present && !present {
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
    let dashboard_ok = present(DASHBOARD_HTML);
    let narrative_ok = present(NARRATIVE_MD);
    let analysis_ok = present(ANALYSIS_JSON);

    for check in &mut manifest.verification.checks {
        match check.name.as_str() {
            "dashboard-present" => {
                check.ok = dashboard_ok;
                check.detail = if dashboard_ok {
                    "dashboard.html present".into()
                } else {
                    "dashboard.html missing or empty at inspect time".into()
                };
            }
            "narrative-present" => {
                check.ok = narrative_ok;
                check.detail = if narrative_ok {
                    "narrative.md present".into()
                } else {
                    "narrative.md missing or empty at inspect time".into()
                };
            }
            "analysis-present" => {
                check.ok = analysis_ok;
                check.detail = if analysis_ok {
                    "analysis.json present".into()
                } else {
                    "analysis.json missing or empty at inspect time".into()
                };
            }
            _ => {}
        }
    }

    let required_ok = manifest
        .verification
        .checks
        .iter()
        .filter(|c| REQUIRED_CHECKS.contains(&c.name.as_str()))
        .all(|c| c.ok);
    manifest.verification.ok = required_ok;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "region,revenue,date\n\
        North,1200,2024-01-01\n\
        South,950,2024-01-02\n\
        North,1500,2024-01-03\n\
        East,700,2024-01-04\n";

    fn write_dataset(dir: &Path) -> PathBuf {
        let path = dir.join("data.csv");
        std::fs::write(&path, CSV).unwrap();
        path
    }

    fn brief_json(dataset: &str) -> String {
        format!(
            r#"{{"name":"Regional sales","question":"Which region earns most?","dataset":"{dataset}"}}"#
        )
    }

    #[test]
    fn build_produces_all_required_artifacts_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let dataset = write_dataset(dir.path());
        let out = dir.path().join("pkg");
        let manifest = build_package_ad_hoc(
            &dataset,
            "Which region earns most?",
            &out,
            BuildOptions::default(),
        )
        .unwrap();

        assert!(manifest.verification.ok, "manifest: {manifest:?}");
        assert!(manifest.chart_count >= 1);
        for f in [
            ANALYSIS_JSON,
            DASHBOARD_HTML,
            NARRATIVE_MD,
            APP_PY,
            MANIFEST_JSON,
        ] {
            assert!(
                file_ok(&out.join(f)),
                "{f} should be produced and non-empty"
            );
        }
    }

    #[test]
    fn dashboard_html_is_interactive() {
        let dir = tempfile::tempdir().unwrap();
        let dataset = write_dataset(dir.path());
        let out = dir.path().join("pkg");
        build_package_ad_hoc(&dataset, "Q?", &out, BuildOptions::default()).unwrap();
        let html = std::fs::read_to_string(out.join(DASHBOARD_HTML)).unwrap();
        assert!(html.contains("cdn.plot.ly"));
        assert!(html.contains("Plotly.newPlot"));
    }

    #[test]
    fn build_from_brief_file_resolves_dataset() {
        let dir = tempfile::tempdir().unwrap();
        write_dataset(dir.path());
        let brief_path = dir.path().join("brief.json");
        std::fs::write(&brief_path, brief_json("data.csv")).unwrap();
        let out = dir.path().join("pkg");
        let manifest = build_package(&brief_path, &out, BuildOptions::default()).unwrap();
        assert!(manifest.verification.ok);
        assert_eq!(manifest.title, "Regional sales");
    }

    #[test]
    fn no_streamlit_option_skips_app_py() {
        let dir = tempfile::tempdir().unwrap();
        let dataset = write_dataset(dir.path());
        let out = dir.path().join("pkg");
        build_package_ad_hoc(&dataset, "Q?", &out, BuildOptions { streamlit: false }).unwrap();
        assert!(!out.join(APP_PY).exists());
        // The package still verifies without the advisory Streamlit app.
        let manifest = inspect(&out).unwrap();
        assert!(manifest.verification.ok);
    }

    #[test]
    fn inspect_flips_ok_when_dashboard_removed() {
        let dir = tempfile::tempdir().unwrap();
        let dataset = write_dataset(dir.path());
        let out = dir.path().join("pkg");
        build_package_ad_hoc(&dataset, "Q?", &out, BuildOptions::default()).unwrap();
        assert!(inspect(&out).unwrap().verification.ok);

        std::fs::remove_file(out.join(DASHBOARD_HTML)).unwrap();
        let manifest = inspect(&out).unwrap();
        assert!(!manifest.verification.ok);
    }

    #[test]
    fn verified_returns_error_on_failure() {
        let manifest = Manifest {
            name: "x".into(),
            title: "x".into(),
            question: "q".into(),
            dataset_format: "csv".into(),
            row_count: 0,
            column_count: 0,
            truncated: false,
            chart_count: 0,
            chart_kinds: vec![],
            tools: vec![],
            artifacts: vec![],
            verification: Verification {
                ok: false,
                checks: vec![check("charts-present", false, "none")],
            },
        };
        assert!(manifest.verified().is_err());
    }

    #[test]
    fn missing_dataset_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pkg");
        let err = build_package_ad_hoc(
            &dir.path().join("nope.csv"),
            "Q?",
            &out,
            BuildOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, CartographerError::Io { .. }));
    }
}
