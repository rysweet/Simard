//! Package orchestration, manifest, and verification.
//!
//! [`build_package`] is the end-to-end entry point: brief → profiled dataset →
//! findings → chart specs → narrative → interactive `dashboard.html` (+ optional
//! Streamlit / Observable sources) → verified `manifest.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::analysis::{self, Findings};
use super::brief::{AppTarget, StudyBrief};
use super::dashboard;
use super::dataset::{Column, ColumnKind, Dataset};
use super::drivers::{self, ToolReport};
use super::error::{CartographerError, CartographerResult};
use super::viz::{self, ChartSpec};

const DATASET_CSV: &str = "dataset.csv";
const NARRATIVE_MD: &str = "narrative.md";
const CHARTS_JSON: &str = "charts.json";
const SUMMARY_JSON: &str = "summary.json";
const DASHBOARD_HTML: &str = "dashboard.html";
const STREAMLIT_APP: &str = "app.py";
const OBSERVABLE_NB: &str = "dashboard.ojs";
const MANIFEST_JSON: &str = "manifest.json";

/// Options controlling the build.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Override the brief's delivery target.
    pub app_target: Option<AppTarget>,
}

/// One produced (or skipped) artifact.
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

/// Compact column descriptor persisted in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub kind: String,
}

/// Compact chart descriptor persisted in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartMeta {
    pub id: String,
    pub chart_type: String,
    pub title: String,
}

/// The package manifest, written as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub title: String,
    pub question: String,
    pub app_target: String,
    pub row_count: usize,
    pub column_count: usize,
    pub columns: Vec<ColumnMeta>,
    pub finding_count: usize,
    pub chart_count: usize,
    pub charts: Vec<ChartMeta>,
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

fn write_file(path: &Path, contents: &str) -> CartographerResult<()> {
    std::fs::write(path, contents)
        .map_err(|e| CartographerError::io(format!("writing {}", path.display()), e))
}

fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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

/// Re-emit the dataset as a normalized CSV so the Streamlit / Observable sources
/// have a stable data file to read.
fn dataset_to_csv(dataset: &Dataset) -> String {
    let escape = |s: &str| -> String {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::new();
    let headers: Vec<String> = dataset.columns.iter().map(|c| escape(&c.name)).collect();
    out.push_str(&headers.join(","));
    out.push('\n');
    for row in 0..dataset.row_count {
        let cells: Vec<String> = dataset
            .columns
            .iter()
            .map(|c| escape(c.values.get(row).map(String::as_str).unwrap_or("")))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// Build the full dashboard package for `brief_path` into `out_dir`.
pub fn build_package(
    brief_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> CartographerResult<Manifest> {
    let brief = StudyBrief::from_path(brief_path)?;
    let base_dir = brief_path.parent().unwrap_or(Path::new("."));
    build_package_from_brief(&brief, base_dir, out_dir, options)
}

/// Build a package from an already-parsed brief. `base_dir` resolves a relative
/// dataset path.
pub fn build_package_from_brief(
    brief: &StudyBrief,
    base_dir: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> CartographerResult<Manifest> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| CartographerError::io(format!("creating {}", out_dir.display()), e))?;

    let dataset = Dataset::load(&brief.dataset, base_dir)?;

    // Validate column hints against the actual dataset now that it is loaded.
    for hint in brief.hints.named_columns() {
        if dataset.column(hint).is_none() {
            return Err(CartographerError::invalid_brief(format!(
                "hint column '{hint}' is not present in the dataset"
            )));
        }
    }

    let findings = analysis::analyze(&dataset, &brief.question);
    let charts = viz::design_charts(&dataset, &brief.hints);
    let narrative = super::narrative::render_narrative(brief, &findings, &charts);
    let html = dashboard::render_html(brief, &dataset, &charts, &narrative);

    let target = options.app_target.unwrap_or_else(|| brief.target());

    // Core artifacts (always written).
    write_file(&out_dir.join(DATASET_CSV), &dataset_to_csv(&dataset))?;
    write_file(&out_dir.join(NARRATIVE_MD), &narrative)?;
    write_file(
        &out_dir.join(CHARTS_JSON),
        &serde_json::to_string_pretty(&charts)
            .map_err(|e| CartographerError::parse("charts json", e.to_string()))?,
    )?;
    write_file(
        &out_dir.join(SUMMARY_JSON),
        &summary_json(&dataset, &findings)?,
    )?;
    write_file(&out_dir.join(DASHBOARD_HTML), &html)?;

    let mut artifacts = vec![
        artifact(&out_dir.join(DATASET_CSV), DATASET_CSV, "data", None),
        artifact(&out_dir.join(NARRATIVE_MD), NARRATIVE_MD, "narrative", None),
        artifact(&out_dir.join(CHARTS_JSON), CHARTS_JSON, "chart-specs", None),
        artifact(&out_dir.join(SUMMARY_JSON), SUMMARY_JSON, "profile", None),
        artifact(
            &out_dir.join(DASHBOARD_HTML),
            DASHBOARD_HTML,
            "dashboard",
            None,
        ),
    ];

    let tools = drivers::probe_delivery_runtimes();

    // Optional delivery-target source.
    match target {
        AppTarget::Streamlit => {
            let path = out_dir.join(STREAMLIT_APP);
            write_file(&path, &dashboard::render_streamlit(brief, &charts))?;
            let runnable = tools
                .iter()
                .find(|t| t.name == "streamlit")
                .map(|t| t.available)
                .unwrap_or(false);
            artifacts.push(artifact(
                &path,
                STREAMLIT_APP,
                "streamlit-app",
                Some(if runnable {
                    "streamlit installed — run `streamlit run app.py`".to_string()
                } else {
                    "streamlit source emitted; runtime not installed on this host".to_string()
                }),
            ));
        }
        AppTarget::Observable => {
            let path = out_dir.join(OBSERVABLE_NB);
            write_file(&path, &dashboard::render_observable(brief, &charts))?;
            artifacts.push(artifact(
                &path,
                OBSERVABLE_NB,
                "observable-notebook",
                Some("Observable notebook source; import into observablehq.com".to_string()),
            ));
        }
        AppTarget::Html => {}
    }

    let verification = verify(&dataset, &charts, &narrative, &html);

    let manifest = Manifest {
        title: brief.title.clone(),
        question: brief.question.clone(),
        app_target: target.label().to_string(),
        row_count: dataset.row_count,
        column_count: dataset.columns.len(),
        columns: dataset
            .columns
            .iter()
            .map(|c| ColumnMeta {
                name: c.name.clone(),
                kind: c.kind.label().to_string(),
            })
            .collect(),
        finding_count: findings.substantive_count(),
        chart_count: charts.len(),
        charts: charts
            .iter()
            .map(|c| ChartMeta {
                id: c.id.clone(),
                chart_type: c.chart_type.label().to_string(),
                title: c.title.clone(),
            })
            .collect(),
        tools,
        artifacts,
        verification,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| CartographerError::parse("manifest json", e.to_string()))?;
    write_file(&out_dir.join(MANIFEST_JSON), &manifest_json)?;

    Ok(manifest)
}

fn summary_json(dataset: &Dataset, findings: &Findings) -> CartographerResult<String> {
    let value = serde_json::json!({
        "dataset": dataset,
        "findings": findings,
    });
    serde_json::to_string_pretty(&value)
        .map_err(|e| CartographerError::parse("summary json", e.to_string()))
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

/// Verify the produced package.
fn verify(dataset: &Dataset, charts: &[ChartSpec], narrative: &str, html: &str) -> Verification {
    let mut checks = Vec::new();

    let dataset_ok = dataset.row_count > 0 && !dataset.columns.is_empty();
    checks.push(check(
        "dataset-loaded",
        dataset_ok,
        format!(
            "{} rows × {} columns",
            dataset.row_count,
            dataset.columns.len()
        ),
    ));

    let dangling: Vec<String> = charts
        .iter()
        .flat_map(|c| c.referenced_columns().into_iter().map(|s| s.to_string()))
        .filter(|c| dataset.column(c).is_none())
        .collect();
    let charts_ok = !charts.is_empty() && dangling.is_empty();
    checks.push(check(
        "charts-designed",
        charts_ok,
        if charts.is_empty() {
            "no charts designed".to_string()
        } else if !dangling.is_empty() {
            format!("charts reference missing columns: {}", dangling.join(", "))
        } else {
            format!(
                "{} interactive chart(s) grounded in the dataset",
                charts.len()
            )
        },
    ));

    let narrative_ok = narrative.len() > 200 && narrative.contains("## Answer");
    checks.push(check(
        "narrative-present",
        narrative_ok,
        if narrative_ok {
            "written narrative with an explicit Answer section".to_string()
        } else {
            "narrative missing or lacks an Answer section".to_string()
        },
    ));

    let dashboard_ok =
        html.contains("Plotly.newPlot") && html.contains("cartographer-data") && html.len() > 1000;
    checks.push(check(
        "dashboard-interactive",
        dashboard_ok,
        if dashboard_ok {
            "dashboard.html embeds data and renders interactive Plotly views".to_string()
        } else {
            "dashboard.html is missing interactive chart wiring".to_string()
        },
    ));

    // Advisory: does the dataset carry a plottable numeric measure?
    let has_measure = !dataset.columns_of_kind(ColumnKind::Numeric).is_empty();
    checks.push(check(
        "has-measure",
        has_measure,
        if has_measure {
            "dataset has at least one numeric measure".to_string()
        } else {
            "dataset has no numeric measure (categorical-only story)".to_string()
        },
    ));

    // Required minimum. `has-measure` is advisory.
    let ok = dataset_ok && charts_ok && narrative_ok && dashboard_ok;
    Verification { ok, checks }
}

/// Read and re-check an existing package manifest in `out_dir`.
///
/// `inspect` re-scans every artifact on disk and, when a **required** artifact
/// (dataset, narrative, dashboard, chart specs) has gone missing or empty since
/// build time, flips the corresponding presence check and the aggregate
/// `verification.ok` to `false`, so a corrupted package cannot report a stale
/// PASS.
pub fn inspect(out_dir: &Path) -> CartographerResult<Manifest> {
    let manifest_path = out_dir.join(MANIFEST_JSON);
    let bytes = std::fs::read(&manifest_path)
        .map_err(|e| CartographerError::io(format!("reading {}", manifest_path.display()), e))?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| CartographerError::parse("manifest json", e.to_string()))?;

    for artifact in &mut manifest.artifacts {
        let path: PathBuf = out_dir.join(&artifact.file);
        let bytes = bytes_of(&path);
        if artifact.present && bytes == 0 {
            artifact.present = false;
            artifact.detail = Some("artifact missing on disk at inspect time".into());
        }
        artifact.bytes = bytes;
    }

    let present = |file: &str| -> bool {
        manifest
            .artifacts
            .iter()
            .any(|a| a.file == file && a.present)
    };
    let dataset_present = present(DATASET_CSV);
    let narrative_present = present(NARRATIVE_MD);
    let dashboard_present = present(DASHBOARD_HTML);
    let charts_present = present(CHARTS_JSON);

    for c in &mut manifest.verification.checks {
        match c.name.as_str() {
            "dataset-loaded" if !dataset_present => {
                c.ok = false;
                c.detail = "dataset.csv missing on disk at inspect time".into();
            }
            "narrative-present" if !narrative_present => {
                c.ok = false;
                c.detail = "narrative.md missing on disk at inspect time".into();
            }
            "dashboard-interactive" if !dashboard_present => {
                c.ok = false;
                c.detail = "dashboard.html missing on disk at inspect time".into();
            }
            "charts-designed" if !charts_present => {
                c.ok = false;
                c.detail = "charts.json missing on disk at inspect time".into();
            }
            _ => {}
        }
    }

    let required_ok = [
        "dataset-loaded",
        "charts-designed",
        "narrative-present",
        "dashboard-interactive",
    ];
    manifest.verification.ok = manifest
        .verification
        .checks
        .iter()
        .filter(|c| required_ok.contains(&c.name.as_str()))
        .all(|c| c.ok);

    Ok(manifest)
}

/// Helper exposing a column lookup for external verification helpers/tests.
pub fn columns_of(dataset: &Dataset, kind: ColumnKind) -> Vec<&Column> {
    dataset.columns_of_kind(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(csv: &str, target: &str) -> StudyBrief {
        let json = format!(
            r#"{{"title":"Study","question":"How do the numbers relate?",
                "app_target":"{target}","dataset":{{"csv":{}}}}}"#,
            serde_json::to_string(csv).unwrap()
        );
        StudyBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn build_produces_verified_package() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief(
            "region,sales,income\nN,100,50\nS,200,90\nE,300,120\n",
            "html",
        );
        let manifest =
            build_package_from_brief(&b, Path::new("."), dir.path(), BuildOptions::default())
                .unwrap();
        assert!(manifest.verification.ok, "manifest: {manifest:?}");
        assert!(manifest.chart_count >= 1);
        for f in [
            DATASET_CSV,
            NARRATIVE_MD,
            CHARTS_JSON,
            SUMMARY_JSON,
            DASHBOARD_HTML,
            MANIFEST_JSON,
        ] {
            assert!(dir.path().join(f).exists(), "missing {f}");
        }
        manifest.verified().unwrap();
    }

    #[test]
    fn streamlit_target_emits_app_py() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief("region,sales\nN,100\nS,200\nE,300\n", "streamlit");
        build_package_from_brief(&b, Path::new("."), dir.path(), BuildOptions::default()).unwrap();
        assert!(dir.path().join(STREAMLIT_APP).exists());
    }

    #[test]
    fn observable_target_emits_ojs() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief("region,sales\nN,100\nS,200\nE,300\n", "observable");
        build_package_from_brief(&b, Path::new("."), dir.path(), BuildOptions::default()).unwrap();
        assert!(dir.path().join(OBSERVABLE_NB).exists());
    }

    #[test]
    fn inspect_reverifies_and_detects_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief(
            "region,sales,income\nN,100,50\nS,200,90\nE,300,120\n",
            "html",
        );
        build_package_from_brief(&b, Path::new("."), dir.path(), BuildOptions::default()).unwrap();
        let m = inspect(dir.path()).unwrap();
        assert!(m.verification.ok);

        std::fs::remove_file(dir.path().join(DASHBOARD_HTML)).unwrap();
        let m2 = inspect(dir.path()).unwrap();
        assert!(
            !m2.verification.ok,
            "deleting the dashboard must fail inspect"
        );
    }

    #[test]
    fn hint_referencing_missing_column_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"title":"t","question":"q?","dataset":{"csv":"a,b\n1,2\n3,4\n"},
                       "hints":{"x":"nonexistent"}}"#;
        let b = StudyBrief::from_json_bytes(json.as_bytes()).unwrap();
        let err = build_package_from_brief(&b, Path::new("."), dir.path(), BuildOptions::default())
            .unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn dataset_csv_roundtrips_quoting() {
        let (h, rows) = super::super::dataset::parse_csv(
            "name,note\n\"Doe, Jane\",\"hi \"\"there\"\"\"\n",
            super::super::dataset::MAX_ROWS,
        )
        .unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        let csv = dataset_to_csv(&ds);
        // Re-parse and confirm the awkward cell survives a round trip.
        let (h2, rows2) =
            super::super::dataset::parse_csv(&csv, super::super::dataset::MAX_ROWS).unwrap();
        let ds2 = Dataset::from_table(h2, rows2).unwrap();
        assert_eq!(ds2.column("name").unwrap().values[0], "Doe, Jane");
        assert_eq!(ds2.column("note").unwrap().values[0], "hi \"there\"");
    }
}
