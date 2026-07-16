//! Dataset loading and exploratory profiling.
//!
//! Loads a CSV or JSON dataset, infers each column's type, and computes
//! per-column summary statistics. This is the exploratory-analysis phase of the
//! Cartographer pipeline: pure-Rust, dependency-free profiling that later drives
//! visualization design and narrative generation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::brief::DatasetFormat;
use super::error::{CartographerError, CartographerResult};

/// The inferred semantic type of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    /// Continuous / discrete numbers.
    Numeric,
    /// A low-cardinality set of labels.
    Categorical,
    /// Date / timestamp-like values.
    Temporal,
    /// Free text / high-cardinality identifiers.
    Text,
    /// Entirely empty column.
    Empty,
}

impl ColumnType {
    /// Stable label for manifests and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Numeric => "numeric",
            Self::Categorical => "categorical",
            Self::Temporal => "temporal",
            Self::Text => "text",
            Self::Empty => "empty",
        }
    }
}

/// A single loaded column: its name, inferred type, and raw string cells (with
/// `None` for missing values).
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub ctype: ColumnType,
    pub cells: Vec<Option<String>>,
}

impl Column {
    /// Parse the numeric value of a cell, tolerating `$`, `,`, and `%`.
    pub fn numeric_at(&self, row: usize) -> Option<f64> {
        self.cells
            .get(row)
            .and_then(|c| c.as_deref())
            .and_then(parse_number)
    }

    /// All numeric values in row order, dropping non-numeric / missing cells.
    pub fn numeric_values(&self) -> Vec<f64> {
        self.cells
            .iter()
            .filter_map(|c| c.as_deref().and_then(parse_number))
            .collect()
    }

    /// Count of each distinct non-missing value, most frequent first.
    pub fn value_counts(&self) -> Vec<(String, usize)> {
        let mut map: BTreeMap<String, usize> = BTreeMap::new();
        for cell in self.cells.iter().flatten() {
            let trimmed = cell.trim();
            if !trimmed.is_empty() {
                *map.entry(trimmed.to_string()).or_insert(0) += 1;
            }
        }
        let mut counts: Vec<(String, usize)> = map.into_iter().collect();
        // Most frequent first; ties broken by label for determinism.
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts
    }
}

/// A fully loaded, column-oriented dataset.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub columns: Vec<Column>,
    pub row_count: usize,
    pub truncated: bool,
}

impl Dataset {
    /// Load and profile a dataset from disk.
    pub fn load(path: &Path, format: DatasetFormat, max_rows: usize) -> CartographerResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| CartographerError::io(format!("reading dataset {}", path.display()), e))?;
        Self::from_bytes(&bytes, format, max_rows)
    }

    /// Load and profile a dataset from in-memory bytes.
    pub fn from_bytes(
        bytes: &[u8],
        format: DatasetFormat,
        max_rows: usize,
    ) -> CartographerResult<Self> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| CartographerError::parse("dataset", format!("not valid UTF-8: {e}")))?;
        let (headers, mut rows) = match format {
            DatasetFormat::Csv => parse_csv(text)?,
            DatasetFormat::Json => parse_json(text)?,
        };
        if headers.is_empty() {
            return Err(CartographerError::invalid_brief(
                "dataset has no columns / header row",
            ));
        }
        let truncated = rows.len() > max_rows;
        if truncated {
            rows.truncate(max_rows);
        }
        if rows.is_empty() {
            return Err(CartographerError::invalid_brief(
                "dataset has a header but no data rows",
            ));
        }

        let mut columns: Vec<Column> = headers
            .into_iter()
            .map(|name| Column {
                name,
                ctype: ColumnType::Empty,
                cells: Vec::with_capacity(rows.len()),
            })
            .collect();

        for row in &rows {
            for (i, col) in columns.iter_mut().enumerate() {
                let value = row.get(i).map(|s| s.as_str()).unwrap_or("");
                let trimmed = value.trim();
                col.cells.push(if trimmed.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                });
            }
        }

        for col in &mut columns {
            col.ctype = infer_type(col, rows.len());
        }

        Ok(Self {
            columns,
            row_count: rows.len(),
            truncated,
        })
    }

    /// Column indices whose inferred type matches `ctype`.
    pub fn columns_of(&self, ctype: ColumnType) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.ctype == ctype).collect()
    }

    /// Build the serializable profile for `analysis.json`.
    pub fn profile(&self) -> DatasetProfile {
        DatasetProfile {
            row_count: self.row_count,
            column_count: self.columns.len(),
            truncated: self.truncated,
            columns: self.columns.iter().map(ColumnProfile::of).collect(),
        }
    }
}

/// Serializable summary of a whole dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetProfile {
    pub row_count: usize,
    pub column_count: usize,
    pub truncated: bool,
    pub columns: Vec<ColumnProfile>,
}

/// Serializable summary of one column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnProfile {
    pub name: String,
    #[serde(rename = "type")]
    pub ctype: ColumnType,
    /// Number of non-missing cells.
    pub count: usize,
    /// Number of missing cells.
    pub missing: usize,
    /// Number of distinct non-missing values.
    pub distinct: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric: Option<NumericSummary>,
    /// Most frequent values (label + count), capped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_values: Vec<TopValue>,
}

/// Numeric summary statistics for a numeric column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericSummary {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub stddev: f64,
}

/// One (value, count) pair in a column's top-values list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopValue {
    pub value: String,
    pub count: usize,
}

/// Maximum number of top values recorded per column.
pub const TOP_VALUES_LIMIT: usize = 12;

impl ColumnProfile {
    fn of(col: &Column) -> Self {
        let count = col.cells.iter().filter(|c| c.is_some()).count();
        let missing = col.cells.len() - count;
        let counts = col.value_counts();
        let distinct = counts.len();

        let numeric = if col.ctype == ColumnType::Numeric {
            numeric_summary(&col.numeric_values())
        } else {
            None
        };

        let top_values = if matches!(
            col.ctype,
            ColumnType::Categorical | ColumnType::Temporal | ColumnType::Text
        ) {
            counts
                .into_iter()
                .take(TOP_VALUES_LIMIT)
                .map(|(value, count)| TopValue { value, count })
                .collect()
        } else {
            Vec::new()
        };

        Self {
            name: col.name.clone(),
            ctype: col.ctype,
            count,
            missing,
            distinct,
            numeric,
            top_values,
        }
    }
}

fn numeric_summary(values: &[f64]) -> Option<NumericSummary> {
    if values.is_empty() {
        return None;
    }
    let n = values.len() as f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &v in values {
        min = min.min(v);
        max = max.max(v);
        sum += v;
    }
    let mean = sum / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    let stddev = variance.sqrt();

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        let mid = sorted.len() / 2;
        (sorted[mid - 1] + sorted[mid]) / 2.0
    };

    Some(NumericSummary {
        min,
        max,
        mean,
        median,
        stddev,
    })
}

/// Parse a possibly-formatted number: strips a single leading currency symbol,
/// thousands separators, surrounding whitespace, and a trailing `%`.
pub fn parse_number(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let mut cleaned = String::with_capacity(s.len());
    let mut percent = false;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            '$' | '€' | '£' | '¥' if i == 0 => {}
            ',' | '_' => {}
            '%' => percent = true,
            other => cleaned.push(other),
        }
    }
    let value: f64 = cleaned.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(if percent { value / 100.0 } else { value })
}

/// Return true if a string looks like a date or timestamp.
fn looks_temporal(s: &str) -> bool {
    let t = s.trim();
    let bytes = t.as_bytes();
    // ISO date: YYYY-MM-DD (optionally with a time suffix).
    let iso = t.len() >= 10
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[7] == b'-';
    // Slash date: MM/DD/YYYY or DD/MM/YYYY.
    let slash = t.len() >= 8
        && t.matches('/').count() == 2
        && t.chars().all(|c| c.is_ascii_digit() || c == '/');
    iso || slash
}

/// Infer a column's semantic type from its cells.
fn infer_type(col: &Column, _rows: usize) -> ColumnType {
    let non_empty: Vec<&str> = col
        .cells
        .iter()
        .filter_map(|c| c.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if non_empty.is_empty() {
        return ColumnType::Empty;
    }

    let numeric = non_empty
        .iter()
        .filter(|s| parse_number(s).is_some())
        .count();
    if numeric == non_empty.len() {
        return ColumnType::Numeric;
    }

    let temporal = non_empty.iter().filter(|s| looks_temporal(s)).count();
    if temporal * 10 >= non_empty.len() * 9 {
        return ColumnType::Temporal;
    }

    let distinct = col.value_counts().len();
    // Low-cardinality strings are categorical; otherwise free text.
    let small_absolute = distinct <= 50;
    let small_relative = distinct * 2 <= non_empty.len();
    if distinct >= 1 && (small_absolute || small_relative) {
        ColumnType::Categorical
    } else {
        ColumnType::Text
    }
}

/// Parse CSV text (RFC4180-ish) into a header row and data rows.
fn parse_csv(text: &str) -> CartographerResult<(Vec<String>, Vec<Vec<String>>)> {
    let mut records = parse_csv_records(text);
    if records.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let headers = records.remove(0);
    if headers.iter().all(|h| h.trim().is_empty()) {
        return Err(CartographerError::parse(
            "dataset csv",
            "header row is empty",
        ));
    }
    Ok((headers, records))
}

/// Split CSV text into records of fields, honouring quotes and escaped quotes.
fn parse_csv_records(text: &str) -> Vec<Vec<String>> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut saw_any = false;

    while let Some(ch) = chars.next() {
        saw_any = true;
        if in_quotes {
            match ch {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                other => field.push(other),
            }
        } else {
            match ch {
                '"' => in_quotes = true,
                ',' => {
                    record.push(std::mem::take(&mut field));
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                other => field.push(other),
            }
        }
    }

    if saw_any && (!field.is_empty() || !record.is_empty()) {
        record.push(field);
        records.push(record);
    }

    // Drop trailing fully-empty records (e.g. a trailing newline).
    records.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    records
}

/// Parse a JSON array of flat objects into a header row and data rows. Column
/// order is the union of keys in first-seen order.
fn parse_json(text: &str) -> CartographerResult<(Vec<String>, Vec<Vec<String>>)> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| CartographerError::parse("dataset json", e.to_string()))?;
    let array = value.as_array().ok_or_else(|| {
        CartographerError::invalid_brief("JSON dataset must be an array of objects")
    })?;

    let mut headers: Vec<String> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for item in array {
        if let Some(obj) = item.as_object() {
            for key in obj.keys() {
                if !seen.contains_key(key) {
                    seen.insert(key.clone(), headers.len());
                    headers.push(key.clone());
                }
            }
        }
    }
    if headers.is_empty() {
        return Err(CartographerError::invalid_brief(
            "JSON dataset objects have no fields",
        ));
    }

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(array.len());
    for item in array {
        let obj = item.as_object().ok_or_else(|| {
            CartographerError::invalid_brief("every JSON dataset element must be an object")
        })?;
        let mut row = vec![String::new(); headers.len()];
        for (key, idx) in &seen {
            if let Some(v) = obj.get(key) {
                row[*idx] = json_scalar(v);
            }
        }
        rows.push(row);
    }
    Ok((headers, rows))
}

fn json_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "region,revenue,date,note\n\
        North,1200,2024-01-01,\"a, quoted note\"\n\
        South,950,2024-01-02,plain\n\
        North,1500,2024-01-03,\"line\nbreak\"\n\
        East,,2024-01-04,x\n";

    fn load_csv() -> Dataset {
        Dataset::from_bytes(CSV.as_bytes(), DatasetFormat::Csv, 1000).unwrap()
    }

    #[test]
    fn parses_csv_columns_and_rows() {
        let ds = load_csv();
        assert_eq!(ds.row_count, 4);
        assert_eq!(ds.columns.len(), 4);
        assert_eq!(ds.columns[0].name, "region");
    }

    #[test]
    fn quoted_fields_with_commas_and_newlines() {
        let ds = load_csv();
        let note = &ds.columns[3];
        assert_eq!(note.cells[0].as_deref(), Some("a, quoted note"));
        assert_eq!(note.cells[2].as_deref(), Some("line\nbreak"));
    }

    #[test]
    fn infers_numeric_column() {
        let ds = load_csv();
        assert_eq!(ds.columns[1].ctype, ColumnType::Numeric);
    }

    #[test]
    fn infers_temporal_column() {
        let ds = load_csv();
        assert_eq!(ds.columns[2].ctype, ColumnType::Temporal);
    }

    #[test]
    fn infers_categorical_column() {
        let ds = load_csv();
        assert_eq!(ds.columns[0].ctype, ColumnType::Categorical);
    }

    #[test]
    fn missing_value_is_none() {
        let ds = load_csv();
        assert_eq!(ds.columns[1].cells[3], None);
    }

    #[test]
    fn numeric_summary_is_computed() {
        let ds = load_csv();
        let profile = ds.profile();
        let revenue = profile
            .columns
            .iter()
            .find(|c| c.name == "revenue")
            .unwrap();
        let n = revenue.numeric.as_ref().unwrap();
        assert_eq!(n.min, 950.0);
        assert_eq!(n.max, 1500.0);
        assert_eq!(revenue.missing, 1);
        assert_eq!(revenue.count, 3);
    }

    #[test]
    fn value_counts_most_frequent_first() {
        let ds = load_csv();
        let counts = ds.columns[0].value_counts();
        assert_eq!(counts[0], ("North".to_string(), 2));
    }

    #[test]
    fn parse_number_handles_currency_and_separators() {
        assert_eq!(parse_number("$1,200"), Some(1200.0));
        assert_eq!(parse_number("  3.5 "), Some(3.5));
        assert_eq!(parse_number("50%"), Some(0.5));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn loads_json_array_of_objects() {
        let json = r#"[
            {"city":"NYC","pop":8000000},
            {"city":"LA","pop":4000000},
            {"city":"SF","pop":880000}
        ]"#;
        let ds = Dataset::from_bytes(json.as_bytes(), DatasetFormat::Json, 1000).unwrap();
        assert_eq!(ds.row_count, 3);
        assert_eq!(ds.columns.len(), 2);
        assert_eq!(ds.columns[1].ctype, ColumnType::Numeric);
    }

    #[test]
    fn truncates_beyond_max_rows() {
        let ds = Dataset::from_bytes(CSV.as_bytes(), DatasetFormat::Csv, 2).unwrap();
        assert_eq!(ds.row_count, 2);
        assert!(ds.truncated);
    }

    #[test]
    fn empty_data_rows_is_error() {
        let err = Dataset::from_bytes(b"a,b,c\n", DatasetFormat::Csv, 10).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn non_object_json_is_error() {
        let err = Dataset::from_bytes(b"[1,2,3]", DatasetFormat::Json, 10).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidBrief { .. }));
    }

    #[test]
    fn columns_of_filters_by_type() {
        let ds = load_csv();
        assert_eq!(ds.columns_of(ColumnType::Numeric).len(), 1);
        assert_eq!(ds.columns_of(ColumnType::Temporal).len(), 1);
    }
}
