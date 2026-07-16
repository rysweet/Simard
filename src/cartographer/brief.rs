//! Study-brief data model.
//!
//! A [`StudyBrief`] is the untrusted input to the Cartographer pipeline: a
//! dataset reference plus the analytical *question* the reader wants answered.
//! It is parsed from JSON, validated, and then drives dataset profiling,
//! visualization design, and dashboard delivery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{CartographerError, CartographerResult};

/// Upper bound on the brief's declared row cap, guarding against a hostile brief
/// requesting an unbounded in-memory table.
pub const MAX_DECLARED_ROWS: usize = 5_000_000;

/// The delivery target for the served application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTarget {
    /// A self-contained interactive HTML dashboard (Plotly + D3). Always built.
    #[default]
    Html,
    /// Additionally emit a Streamlit `app.py` source (runnable where Streamlit
    /// is installed).
    Streamlit,
    /// Additionally emit an Observable-flavoured notebook source.
    Observable,
}

impl AppTarget {
    /// Map a free-form target string to a delivery target. Unknown values fall
    /// back to the always-available HTML dashboard.
    pub fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "streamlit" | "app" | "py" => Self::Streamlit,
            "observable" | "ojs" | "notebook" => Self::Observable,
            _ => Self::Html,
        }
    }

    /// Stable label for manifests and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Streamlit => "streamlit",
            Self::Observable => "observable",
        }
    }
}

/// Where the dataset comes from: a file on disk, or inline CSV text embedded in
/// the brief (handy for small, fully self-describing studies and tests).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetSource {
    /// Path to a `.csv` or `.json` dataset file.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Explicit format override (`csv` or `json`); inferred from the extension
    /// when absent.
    #[serde(default)]
    pub format: Option<String>,
    /// Inline CSV text (first row is the header). Mutually exclusive with
    /// `path`.
    #[serde(default)]
    pub csv: Option<String>,
    /// Optional cap on the number of data rows read from the dataset.
    #[serde(default)]
    pub max_rows: Option<usize>,
}

/// Optional operator hints steering which columns to visualize.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Hints {
    /// Preferred x-axis column.
    #[serde(default)]
    pub x: Option<String>,
    /// Preferred y-axis (measure) column.
    #[serde(default)]
    pub y: Option<String>,
    /// Preferred grouping / colour column.
    #[serde(default)]
    pub color: Option<String>,
    /// Preferred temporal column for trend charts.
    #[serde(default)]
    pub time: Option<String>,
    /// Any additional hints, preserved for documentation in the manifest.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Hints {
    /// All explicitly-named column hints, for validation against the dataset.
    pub fn named_columns(&self) -> Vec<&str> {
        [&self.x, &self.y, &self.color, &self.time]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }
}

/// A data-storytelling study to be turned into a dashboard, parsed from a brief.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StudyBrief {
    /// Human-readable study title.
    pub title: String,
    /// The analytical question the dashboard must help answer.
    pub question: String,
    /// The dataset to analyse.
    pub dataset: DatasetSource,
    /// Delivery target. Defaults to the self-contained HTML dashboard.
    #[serde(default)]
    pub app_target: Option<String>,
    /// Optional column hints.
    #[serde(default)]
    pub hints: Hints,
    /// Optional audience note woven into the narrative.
    #[serde(default)]
    pub audience: Option<String>,
}

impl StudyBrief {
    /// Read and validate a brief from a JSON file.
    pub fn from_path(path: &Path) -> CartographerResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| CartographerError::io(format!("reading brief {}", path.display()), e))?;
        Self::from_json_bytes(&bytes)
    }

    /// Parse and validate a brief from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> CartographerResult<Self> {
        let brief: StudyBrief = serde_json::from_slice(bytes)
            .map_err(|e| CartographerError::parse("brief json", e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// The resolved delivery target.
    pub fn target(&self) -> AppTarget {
        self.app_target
            .as_deref()
            .map(AppTarget::classify)
            .unwrap_or_default()
    }

    /// Reject malformed briefs before any work is done. Column-hint existence is
    /// checked later against the loaded dataset (see `manifest::build`).
    pub fn validate(&self) -> CartographerResult<()> {
        if self.title.trim().is_empty() {
            return Err(CartographerError::invalid_brief("title must not be empty"));
        }
        if self.question.trim().is_empty() {
            return Err(CartographerError::invalid_brief(
                "question must not be empty",
            ));
        }
        match (&self.dataset.path, &self.dataset.csv) {
            (None, None) => {
                return Err(CartographerError::invalid_brief(
                    "dataset must provide either a `path` or inline `csv` text",
                ));
            }
            (Some(_), Some(_)) => {
                return Err(CartographerError::invalid_brief(
                    "dataset must not set both `path` and inline `csv`",
                ));
            }
            _ => {}
        }
        if let Some(fmt) = &self.dataset.format {
            let f = fmt.trim().to_ascii_lowercase();
            if f != "csv" && f != "json" {
                return Err(CartographerError::invalid_brief(format!(
                    "dataset.format must be 'csv' or 'json' (got '{fmt}')"
                )));
            }
        }
        if let Some(cap) = self.dataset.max_rows {
            if cap == 0 {
                return Err(CartographerError::invalid_brief(
                    "dataset.max_rows must be greater than zero",
                ));
            }
            if cap > MAX_DECLARED_ROWS {
                return Err(CartographerError::invalid_brief(format!(
                    "dataset.max_rows must not exceed {MAX_DECLARED_ROWS} (got {cap})"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "title": "Life expectancy vs income",
            "question": "How does life expectancy relate to income across continents?",
            "dataset": { "path": "gapminder.csv" },
            "app_target": "html",
            "hints": { "x": "income", "y": "life_expectancy", "color": "continent" }
        }"#
    }

    #[test]
    fn parses_and_validates_sample() {
        let brief = StudyBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.title, "Life expectancy vs income");
        assert_eq!(brief.target(), AppTarget::Html);
        assert_eq!(brief.hints.x.as_deref(), Some("income"));
        assert_eq!(brief.hints.named_columns().len(), 3);
    }

    #[test]
    fn inline_csv_is_accepted() {
        let json = r#"{"title":"t","question":"q?","dataset":{"csv":"a,b\n1,2\n"}}"#;
        let brief = StudyBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert!(brief.dataset.csv.is_some());
    }

    #[test]
    fn rejects_empty_question() {
        let json = r#"{"title":"t","question":"   ","dataset":{"path":"x.csv"}}"#;
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_missing_dataset_source() {
        let json = r#"{"title":"t","question":"q?","dataset":{}}"#;
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_dual_dataset_source() {
        let json = r#"{"title":"t","question":"q?","dataset":{"path":"x.csv","csv":"a,b\n1,2\n"}}"#;
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_bad_format() {
        let json = r#"{"title":"t","question":"q?","dataset":{"path":"x","format":"parquet"}}"#;
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_max_rows() {
        let json = format!(
            r#"{{"title":"t","question":"q?","dataset":{{"path":"x.csv","max_rows":{}}}}}"#,
            MAX_DECLARED_ROWS + 1
        );
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_zero_max_rows() {
        let json = r#"{"title":"t","question":"q?","dataset":{"path":"x.csv","max_rows":0}}"#;
        let err = StudyBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn app_target_classification_is_stable() {
        assert_eq!(AppTarget::classify("Streamlit"), AppTarget::Streamlit);
        assert_eq!(AppTarget::classify("OJS"), AppTarget::Observable);
        assert_eq!(AppTarget::classify("html"), AppTarget::Html);
        assert_eq!(AppTarget::classify("whatever"), AppTarget::Html);
        assert_eq!(AppTarget::Streamlit.label(), "streamlit");
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = StudyBrief::from_json_bytes(b"{not json").unwrap_err();
        assert!(matches!(err, CartographerError::Parse { .. }));
    }
}
