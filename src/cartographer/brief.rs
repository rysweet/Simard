//! Analysis-brief data model.
//!
//! A [`DashboardBrief`] is the untrusted input to the Cartographer pipeline: a
//! dataset to analyse plus the question the resulting dashboard should answer.
//! It is parsed from JSON, validated, and then drives exploratory analysis,
//! visualization design, and dashboard delivery.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{CartographerError, CartographerResult};

/// Hard upper bound on rows loaded from an untrusted dataset, guarding against
/// resource exhaustion. Dashboards summarise data; they do not need millions of
/// raw rows in the browser.
pub const MAX_ROWS_CAP: usize = 200_000;

/// Default number of rows loaded when the brief does not cap it.
pub const DEFAULT_MAX_ROWS: usize = 50_000;

/// Default number of points drawn per chart before down-sampling kicks in.
pub const DEFAULT_MAX_POINTS: usize = 5_000;

/// How a dataset file is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetFormat {
    /// Comma-separated values with a header row.
    Csv,
    /// A JSON array of flat objects.
    Json,
}

impl DatasetFormat {
    /// Infer a dataset format from a file extension, defaulting to CSV.
    pub fn from_extension(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("json" | "ndjson") => Self::Json,
            _ => Self::Csv,
        }
    }

    /// Parse an explicit format string from a brief.
    pub fn parse(raw: &str) -> CartographerResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            other => Err(CartographerError::invalid_brief(format!(
                "dataset_format must be 'csv' or 'json' (got '{other}')"
            ))),
        }
    }

    /// Stable label for manifests.
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// A dataset + question to turn into a narrated, interactive dashboard.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DashboardBrief {
    /// Human-readable name for the analysis / dashboard.
    pub name: String,
    /// The analytical question the dashboard should answer.
    pub question: String,
    /// Path to the dataset file (relative to the brief file, or absolute).
    pub dataset: String,
    /// Optional explicit dataset format; inferred from the extension otherwise.
    #[serde(default)]
    pub dataset_format: Option<String>,
    /// Optional dashboard title (defaults to `name`).
    #[serde(default)]
    pub title: Option<String>,
    /// Optional cap on rows to load (bounded by [`MAX_ROWS_CAP`]).
    #[serde(default)]
    pub max_rows: Option<usize>,
    /// Optional cap on points drawn per chart before down-sampling.
    #[serde(default)]
    pub max_points: Option<usize>,
}

impl DashboardBrief {
    /// Read and validate a brief from a JSON file.
    pub fn from_path(path: &Path) -> CartographerResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| CartographerError::io(format!("reading brief {}", path.display()), e))?;
        Self::from_json_bytes(&bytes)
    }

    /// Parse and validate a brief from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> CartographerResult<Self> {
        let brief: DashboardBrief = serde_json::from_slice(bytes)
            .map_err(|e| CartographerError::parse("brief json", e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// Construct a brief directly from a dataset path and a question, for the
    /// ad-hoc `--dataset ... --question ...` CLI form.
    pub fn from_dataset_and_question(
        dataset: impl Into<String>,
        question: impl Into<String>,
    ) -> CartographerResult<Self> {
        let brief = Self {
            name: "Ad-hoc analysis".to_string(),
            question: question.into(),
            dataset: dataset.into(),
            dataset_format: None,
            title: None,
            max_rows: None,
            max_points: None,
        };
        brief.validate()?;
        Ok(brief)
    }

    /// Reject malformed briefs.
    pub fn validate(&self) -> CartographerResult<()> {
        if self.name.trim().is_empty() {
            return Err(CartographerError::invalid_brief("name must not be empty"));
        }
        if self.question.trim().is_empty() {
            return Err(CartographerError::invalid_brief(
                "question must not be empty",
            ));
        }
        if self.dataset.trim().is_empty() {
            return Err(CartographerError::invalid_brief(
                "dataset path must not be empty",
            ));
        }
        if let Some(fmt) = &self.dataset_format {
            DatasetFormat::parse(fmt)?;
        }
        if let Some(rows) = self.max_rows
            && rows == 0
        {
            return Err(CartographerError::invalid_brief(
                "max_rows must be greater than zero",
            ));
        }
        if let Some(points) = self.max_points
            && points == 0
        {
            return Err(CartographerError::invalid_brief(
                "max_points must be greater than zero",
            ));
        }
        Ok(())
    }

    /// The dashboard title, falling back to the brief name.
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// Effective row cap, bounded by [`MAX_ROWS_CAP`].
    pub fn effective_max_rows(&self) -> usize {
        self.max_rows.unwrap_or(DEFAULT_MAX_ROWS).min(MAX_ROWS_CAP)
    }

    /// Effective per-chart point cap.
    pub fn effective_max_points(&self) -> usize {
        self.max_points.unwrap_or(DEFAULT_MAX_POINTS).max(1)
    }

    /// Resolve the dataset path relative to the brief file's directory.
    pub fn resolved_dataset(&self, brief_path: &Path) -> PathBuf {
        let raw = PathBuf::from(&self.dataset);
        if raw.is_absolute() {
            return raw;
        }
        match brief_path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.join(raw),
            _ => raw,
        }
    }

    /// The dataset format, from the explicit field or the file extension.
    pub fn format(&self) -> DatasetFormat {
        match &self.dataset_format {
            Some(raw) => DatasetFormat::parse(raw).unwrap_or(DatasetFormat::Csv),
            None => DatasetFormat::from_extension(Path::new(&self.dataset)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "name": "Sales by region",
            "question": "Which region drives the most revenue?",
            "dataset": "sales.csv",
            "title": "Regional Sales Dashboard",
            "max_rows": 10000
        }"#
    }

    #[test]
    fn parses_and_validates_sample() {
        let brief = DashboardBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.name, "Sales by region");
        assert_eq!(brief.title(), "Regional Sales Dashboard");
        assert_eq!(brief.format(), DatasetFormat::Csv);
        assert_eq!(brief.effective_max_rows(), 10000);
    }

    #[test]
    fn title_defaults_to_name() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.csv"}"#;
        let brief = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(brief.title(), "X");
    }

    #[test]
    fn rejects_empty_question() {
        let json = r#"{"name":"X","question":"   ","dataset":"d.csv"}"#;
        let err = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_empty_dataset() {
        let json = r#"{"name":"X","question":"Q?","dataset":""}"#;
        let err = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_bad_format() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.dat","dataset_format":"xml"}"#;
        let err = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_zero_max_rows() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.csv","max_rows":0}"#;
        let err = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn max_rows_capped() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.csv","max_rows":999999999}"#;
        let brief = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(brief.effective_max_rows(), MAX_ROWS_CAP);
    }

    #[test]
    fn format_inferred_from_extension() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.json"}"#;
        let brief = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(brief.format(), DatasetFormat::Json);
    }

    #[test]
    fn explicit_format_overrides_extension() {
        let json = r#"{"name":"X","question":"Q?","dataset":"d.txt","dataset_format":"json"}"#;
        let brief = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap();
        assert_eq!(brief.format(), DatasetFormat::Json);
    }

    #[test]
    fn resolves_relative_dataset_against_brief_dir() {
        let brief = DashboardBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        let resolved = brief.resolved_dataset(Path::new("/data/briefs/b.json"));
        assert_eq!(resolved, PathBuf::from("/data/briefs/sales.csv"));
    }

    #[test]
    fn keeps_absolute_dataset() {
        let json = r#"{"name":"X","question":"Q?","dataset":"/abs/d.csv"}"#;
        let brief = DashboardBrief::from_json_bytes(json.as_bytes()).unwrap();
        let resolved = brief.resolved_dataset(Path::new("/data/briefs/b.json"));
        assert_eq!(resolved, PathBuf::from("/abs/d.csv"));
    }

    #[test]
    fn ad_hoc_constructor_builds_valid_brief() {
        let brief = DashboardBrief::from_dataset_and_question("d.csv", "What?").unwrap();
        assert_eq!(brief.question, "What?");
        assert!(brief.validate().is_ok());
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = DashboardBrief::from_json_bytes(b"{not json").unwrap_err();
        assert!(matches!(err, CartographerError::Parse { .. }));
    }
}
