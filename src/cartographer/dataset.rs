//! Dataset loading and profiling.
//!
//! Cartographer reads a tabular dataset (CSV or JSON array-of-objects), infers
//! each column's kind (numeric / temporal / categorical / text), and computes a
//! compact profile used by analysis, visualization design, and the dashboard.
//!
//! The CSV reader is a small, dependency-free RFC 4180 parser (quoted fields,
//! `""` escapes, `CRLF`/`LF`). All ingestion is bounded (rows, columns, cell
//! length, category cardinality) so an untrusted dataset cannot exhaust memory.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::brief::DatasetSource;
use super::error::{CartographerError, CartographerResult};

/// Hard cap on ingested data rows.
pub const MAX_ROWS: usize = 1_000_000;
/// Hard cap on columns.
pub const MAX_COLS: usize = 1_024;
/// Hard cap on a single cell's length in bytes.
pub const MAX_CELL_BYTES: usize = 100_000;
/// A column with at most this many distinct non-null values is categorical.
pub const CATEGORICAL_MAX_DISTINCT: usize = 50;
/// Number of top categories retained in a categorical profile.
pub const TOP_CATEGORIES: usize = 12;

/// The inferred kind of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    /// Continuous or discrete numbers.
    Numeric,
    /// ISO-style dates (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`).
    Temporal,
    /// Low-cardinality labels.
    Categorical,
    /// Free-form / high-cardinality text.
    Text,
}

impl ColumnKind {
    /// Stable label for manifests and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Numeric => "numeric",
            Self::Temporal => "temporal",
            Self::Categorical => "categorical",
            Self::Text => "text",
        }
    }
}

/// A `(value, count)` pair for a categorical column's most common labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryCount {
    pub value: String,
    pub count: usize,
}

/// Numeric summary statistics for a numeric column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub std_dev: f64,
    pub count: usize,
}

/// A profiled column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub kind: ColumnKind,
    pub null_count: usize,
    pub distinct_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric: Option<NumericStats>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_categories: Vec<CategoryCount>,
    /// Raw cell values (empty string == null), kept for chart rendering.
    #[serde(skip)]
    pub values: Vec<String>,
}

impl Column {
    /// Parsed numeric values with nulls dropped, in row order.
    pub fn numeric_values(&self) -> Vec<f64> {
        self.values.iter().filter_map(|v| parse_number(v)).collect()
    }
}

/// A loaded, profiled dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    pub row_count: usize,
    pub columns: Vec<Column>,
}

impl Dataset {
    /// Load and profile a dataset described by a [`DatasetSource`].
    ///
    /// `base_dir` resolves a relative `path` (typically the brief's directory).
    pub fn load(source: &DatasetSource, base_dir: &Path) -> CartographerResult<Self> {
        let max_rows = source.max_rows.unwrap_or(MAX_ROWS).min(MAX_ROWS);
        let (headers, rows) = if let Some(csv) = &source.csv {
            parse_csv(csv, max_rows)?
        } else if let Some(path) = &source.path {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                base_dir.join(path)
            };
            let text = std::fs::read_to_string(&resolved).map_err(|e| {
                CartographerError::io(format!("reading dataset {}", resolved.display()), e)
            })?;
            let format = source
                .format
                .clone()
                .unwrap_or_else(|| infer_format(&resolved));
            match format.trim().to_ascii_lowercase().as_str() {
                "json" => parse_json_records(&text, max_rows)?,
                _ => parse_csv(&text, max_rows)?,
            }
        } else {
            return Err(CartographerError::invalid_dataset(
                "dataset source has neither inline csv nor a path",
            ));
        };

        Self::from_table(headers, rows)
    }

    /// Build a profiled dataset from a header row and data rows.
    pub fn from_table(headers: Vec<String>, rows: Vec<Vec<String>>) -> CartographerResult<Self> {
        if headers.is_empty() {
            return Err(CartographerError::invalid_dataset("no columns in dataset"));
        }
        if headers.len() > MAX_COLS {
            return Err(CartographerError::invalid_dataset(format!(
                "too many columns: {} (max {MAX_COLS})",
                headers.len()
            )));
        }
        if rows.is_empty() {
            return Err(CartographerError::invalid_dataset(
                "dataset has a header but no data rows",
            ));
        }

        let row_count = rows.len();
        let mut columns = Vec::with_capacity(headers.len());
        for (idx, name) in headers.iter().enumerate() {
            let values: Vec<String> = rows
                .iter()
                .map(|r| r.get(idx).cloned().unwrap_or_default())
                .collect();
            columns.push(profile_column(name, values));
        }
        Ok(Self { row_count, columns })
    }

    /// Look up a column by (case-insensitive) name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Column names of a given kind, in dataset order.
    pub fn columns_of_kind(&self, kind: ColumnKind) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.kind == kind).collect()
    }

    /// Serialize columns as JSON arrays for embedding in the dashboard: numeric
    /// columns become number arrays (nulls preserved), others string arrays.
    pub fn to_json_columns(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        for col in &self.columns {
            let arr: Vec<Value> = col
                .values
                .iter()
                .map(|v| {
                    if v.is_empty() {
                        Value::Null
                    } else if col.kind == ColumnKind::Numeric {
                        parse_number(v)
                            .and_then(serde_json::Number::from_f64)
                            .map(Value::Number)
                            .unwrap_or(Value::Null)
                    } else {
                        Value::String(v.clone())
                    }
                })
                .collect();
            map.insert(col.name.clone(), Value::Array(arr));
        }
        map
    }
}

fn infer_format(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("json") => "json".to_string(),
        _ => "csv".to_string(),
    }
}

/// Parse a number, tolerating surrounding whitespace, thousands separators, a
/// leading currency symbol, and a trailing percent sign.
pub fn parse_number(raw: &str) -> Option<f64> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let cleaned: String = t
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | '£' | '€' | '%' | '_'))
        .collect();
    cleaned.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// True if `raw` looks like an ISO date (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`).
fn looks_temporal(raw: &str) -> bool {
    let t = raw.trim();
    let parts: Vec<&str> = t.split('-').collect();
    if parts.is_empty() || parts.len() > 3 {
        return false;
    }
    // A bare 4-digit token is treated as numeric (a year measure), not temporal,
    // so it stays plottable on a numeric axis. Require at least YYYY-MM.
    if parts.len() < 2 {
        return false;
    }
    let widths = [4usize, 2, 2];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != widths[i] || !p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

fn profile_column(name: &str, values: Vec<String>) -> Column {
    let non_null: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
    let null_count = values.len() - non_null.len();

    let mut distinct: BTreeMap<&str, usize> = BTreeMap::new();
    for v in &non_null {
        *distinct.entry(v.as_str()).or_insert(0) += 1;
    }
    let distinct_count = distinct.len();

    let all_numeric = !non_null.is_empty() && non_null.iter().all(|v| parse_number(v).is_some());
    let all_temporal = !non_null.is_empty() && non_null.iter().all(|v| looks_temporal(v));

    let kind = if all_temporal {
        ColumnKind::Temporal
    } else if all_numeric {
        ColumnKind::Numeric
    } else if distinct_count <= CATEGORICAL_MAX_DISTINCT {
        ColumnKind::Categorical
    } else {
        ColumnKind::Text
    };

    let numeric = if kind == ColumnKind::Numeric {
        Some(numeric_stats(&non_null))
    } else {
        None
    };

    let top_categories = if kind == ColumnKind::Categorical {
        let mut counts: Vec<CategoryCount> = distinct
            .into_iter()
            .map(|(value, count)| CategoryCount {
                value: value.to_string(),
                count,
            })
            .collect();
        counts.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
        counts.truncate(TOP_CATEGORIES);
        counts
    } else {
        Vec::new()
    };

    Column {
        name: name.to_string(),
        kind,
        null_count,
        distinct_count,
        numeric,
        top_categories,
        values,
    }
}

fn numeric_stats(non_null: &[&String]) -> NumericStats {
    let nums: Vec<f64> = non_null.iter().filter_map(|v| parse_number(v)).collect();
    let count = nums.len();
    if count == 0 {
        return NumericStats {
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            std_dev: 0.0,
            count: 0,
        };
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &n in &nums {
        min = min.min(n);
        max = max.max(n);
        sum += n;
    }
    let mean = sum / count as f64;
    let variance = nums.iter().map(|n| (n - mean).powi(2)).sum::<f64>() / count as f64;
    NumericStats {
        min,
        max,
        mean,
        std_dev: variance.sqrt(),
        count,
    }
}

/// A dependency-free RFC 4180 CSV parser. Returns `(headers, rows)`.
pub fn parse_csv(
    text: &str,
    max_rows: usize,
) -> CartographerResult<(Vec<String>, Vec<Vec<String>>)> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut saw_any = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    let push_field = |field: &mut String, record: &mut Vec<String>| -> CartographerResult<()> {
        if field.len() > MAX_CELL_BYTES {
            return Err(CartographerError::invalid_dataset(format!(
                "a cell exceeds {MAX_CELL_BYTES} bytes"
            )));
        }
        record.push(std::mem::take(field));
        Ok(())
    };

    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    field.push('"');
                    i += 2;
                    continue;
                }
                in_quotes = false;
                i += 1;
                continue;
            }
            field.push(c);
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_quotes = true;
                saw_any = true;
                i += 1;
            }
            ',' => {
                saw_any = true;
                push_field(&mut field, &mut record)?;
                i += 1;
            }
            '\r' => {
                // Swallow CR; the following LF (if any) ends the record.
                i += 1;
            }
            '\n' => {
                push_field(&mut field, &mut record)?;
                records.push(std::mem::take(&mut record));
                saw_any = false;
                i += 1;
                // Bounded ingestion: once the header plus `max_rows` data rows
                // are captured, stop reading and truncate the rest gracefully.
                if records.len() > max_rows {
                    field.clear();
                    break;
                }
            }
            _ => {
                saw_any = true;
                field.push(c);
                i += 1;
            }
        }
    }
    // Flush a trailing record with no terminating newline.
    if saw_any || !field.is_empty() || !record.is_empty() {
        push_field(&mut field, &mut record)?;
        records.push(record);
    }

    if records.is_empty() {
        return Err(CartographerError::invalid_dataset("empty CSV input"));
    }
    let headers = records.remove(0);
    if headers.iter().all(|h| h.trim().is_empty()) {
        return Err(CartographerError::invalid_dataset(
            "CSV header row is blank",
        ));
    }
    let headers: Vec<String> = headers
        .into_iter()
        .enumerate()
        .map(|(idx, h)| {
            let h = h.trim().to_string();
            if h.is_empty() {
                format!("column_{}", idx + 1)
            } else {
                h
            }
        })
        .collect();

    // Drop blank filler rows (a single empty field) only when the dataset has
    // more than one column, where such a row is a formatting artifact. For a
    // single-column dataset a blank line is a genuine null value and is kept.
    if headers.len() > 1 {
        records.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    }
    records.truncate(max_rows);
    Ok((headers, records))
}

/// Parse a JSON array of flat objects into `(headers, rows)`. Column order is
/// the union of keys in first-seen order.
pub fn parse_json_records(
    text: &str,
    max_rows: usize,
) -> CartographerResult<(Vec<String>, Vec<Vec<String>>)> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| CartographerError::parse("dataset json", e.to_string()))?;
    let array = value
        .as_array()
        .ok_or_else(|| CartographerError::invalid_dataset("dataset JSON must be an array"))?;

    let mut headers: Vec<String> = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for item in array.iter().take(max_rows) {
        let obj = item.as_object().ok_or_else(|| {
            CartographerError::invalid_dataset("dataset JSON array must contain objects")
        })?;
        for key in obj.keys() {
            if seen.insert(key.clone(), ()).is_none() {
                headers.push(key.clone());
                if headers.len() > MAX_COLS {
                    return Err(CartographerError::invalid_dataset(format!(
                        "too many columns: >{MAX_COLS}"
                    )));
                }
            }
        }
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for item in array.iter().take(max_rows) {
        let obj = item.as_object().unwrap();
        let row: Vec<String> = headers
            .iter()
            .map(|h| match obj.get(h) {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Number(n)) => n.to_string(),
                Some(other) => other.to_string(),
            })
            .collect();
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(CartographerError::invalid_dataset(
            "dataset JSON array is empty",
        ));
    }
    Ok((headers, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_csv() {
        let (h, rows) = parse_csv("a,b,c\n1,2,3\n4,5,6\n", MAX_ROWS).unwrap();
        assert_eq!(h, vec!["a", "b", "c"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["4", "5", "6"]);
    }

    #[test]
    fn parses_quoted_fields_with_commas_and_escapes() {
        let (h, rows) = parse_csv(
            "name,note\n\"Doe, Jane\",\"she said \"\"hi\"\"\"\n",
            MAX_ROWS,
        )
        .unwrap();
        assert_eq!(h, vec!["name", "note"]);
        assert_eq!(rows[0][0], "Doe, Jane");
        assert_eq!(rows[0][1], "she said \"hi\"");
    }

    #[test]
    fn handles_crlf_and_no_trailing_newline() {
        let (_h, rows) = parse_csv("a,b\r\n1,2\r\n3,4", MAX_ROWS).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["3", "4"]);
    }

    #[test]
    fn blank_header_column_is_named() {
        let (h, _rows) = parse_csv("a,,c\n1,2,3\n", MAX_ROWS).unwrap();
        assert_eq!(h[1], "column_2");
    }

    #[test]
    fn row_cap_truncates_gracefully() {
        // Bounded ingestion truncates to `max_rows` data rows without erroring.
        let (_h, rows) = parse_csv("a\n1\n2\n3\n", 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1"]);
        assert_eq!(rows[1], vec!["2"]);
    }

    #[test]
    fn profiles_numeric_categorical_and_temporal() {
        let csv = "region,sales,month\nNorth,100,2024-01\nSouth,200,2024-02\nNorth,150,2024-03\n";
        let (h, rows) = parse_csv(csv, MAX_ROWS).unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        assert_eq!(ds.row_count, 3);
        assert_eq!(ds.column("sales").unwrap().kind, ColumnKind::Numeric);
        assert_eq!(ds.column("region").unwrap().kind, ColumnKind::Categorical);
        assert_eq!(ds.column("month").unwrap().kind, ColumnKind::Temporal);
        let sales = ds.column("sales").unwrap();
        let stats = sales.numeric.as_ref().unwrap();
        assert_eq!(stats.min, 100.0);
        assert_eq!(stats.max, 200.0);
        assert!((stats.mean - 150.0).abs() < 1e-9);
    }

    #[test]
    fn null_and_distinct_counts() {
        let csv = "x\n1\n\n1\n2\n";
        let (h, rows) = parse_csv(csv, MAX_ROWS).unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        let x = ds.column("x").unwrap();
        assert_eq!(x.null_count, 1);
        assert_eq!(x.distinct_count, 2);
    }

    #[test]
    fn currency_and_percent_numbers_parse() {
        assert_eq!(parse_number("$1,200.50"), Some(1200.50));
        assert_eq!(parse_number("42%"), Some(42.0));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("abc"), None);
    }

    #[test]
    fn json_records_parse_and_profile() {
        let text = r#"[{"a":1,"b":"x"},{"a":2,"b":"y"}]"#;
        let (h, rows) = parse_json_records(text, MAX_ROWS).unwrap();
        assert_eq!(h, vec!["a", "b"]);
        let ds = Dataset::from_table(h, rows).unwrap();
        assert_eq!(ds.column("a").unwrap().kind, ColumnKind::Numeric);
    }

    #[test]
    fn empty_dataset_is_rejected() {
        let err = Dataset::from_table(vec!["a".into()], vec![]).unwrap_err();
        assert!(matches!(err, CartographerError::InvalidDataset { .. }));
    }

    #[test]
    fn json_columns_preserve_numeric_and_null() {
        let csv = "x,y\n1,a\n,b\n";
        let (h, rows) = parse_csv(csv, MAX_ROWS).unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        let cols = ds.to_json_columns();
        let x = cols.get("x").unwrap().as_array().unwrap();
        assert!(x[0].is_number());
        assert!(x[1].is_null());
    }

    #[test]
    fn bare_year_is_numeric_not_temporal() {
        let csv = "year\n2020\n2021\n";
        let (h, rows) = parse_csv(csv, MAX_ROWS).unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        assert_eq!(ds.column("year").unwrap().kind, ColumnKind::Numeric);
    }
}
