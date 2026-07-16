//! Visualization design.
//!
//! Turns a profiled [`Dataset`] and the brief's question into a small set of
//! [`ChartSpec`]s — the visualization-design phase. Chart selection is driven
//! by column types: distributions for numerics, ranked bars for categories,
//! trend lines for temporal series, and scatter + correlation for numeric
//! pairs. Every chart carries a plain-language caption used by the narrative.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::brief::DashboardBrief;
use super::dataset::{Column, ColumnType, Dataset, parse_number};

/// Maximum number of charts designed for one dashboard.
pub const MAX_CHARTS: usize = 6;

/// The data payload of a chart, tagged by kind for the front end.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ChartData {
    /// Distribution of a numeric column.
    Histogram { values: Vec<f64> },
    /// Ranked categories with an aggregated numeric value (or counts).
    Bar {
        labels: Vec<String>,
        values: Vec<f64>,
    },
    /// A numeric series ordered along a temporal axis.
    Line {
        labels: Vec<String>,
        values: Vec<f64>,
    },
    /// Two numeric columns plotted against each other.
    Scatter { x: Vec<f64>, y: Vec<f64> },
}

impl ChartData {
    /// Stable kind label for manifests and narrative.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Histogram { .. } => "histogram",
            Self::Bar { .. } => "bar",
            Self::Line { .. } => "line",
            Self::Scatter { .. } => "scatter",
        }
    }
}

/// A single designed chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartSpec {
    pub id: String,
    pub title: String,
    pub x_label: String,
    pub y_label: String,
    /// Plain-language insight this chart supports.
    pub caption: String,
    pub data: ChartData,
}

/// Design the dashboard's charts from the profiled dataset and the brief.
pub fn design_charts(brief: &DashboardBrief, dataset: &Dataset) -> Vec<ChartSpec> {
    let max_points = brief.effective_max_points();
    let mut charts: Vec<ChartSpec> = Vec::new();

    let numeric: Vec<&Column> = dataset.columns_of(ColumnType::Numeric);
    let categorical: Vec<&Column> = dataset.columns_of(ColumnType::Categorical);
    let temporal: Vec<&Column> = dataset.columns_of(ColumnType::Temporal);

    // 1. Trend: temporal axis vs the primary numeric measure.
    if let (Some(time), Some(measure)) = (temporal.first(), numeric.first())
        && let Some(chart) = line_chart(time, measure)
    {
        charts.push(chart);
    }

    // 2. Ranked categories by the primary numeric measure (or by frequency).
    if let Some(cat) = categorical.first() {
        let chart = match numeric.first() {
            Some(measure) => bar_chart_by_measure(cat, measure),
            None => bar_chart_by_count(cat),
        };
        if let Some(chart) = chart {
            charts.push(chart);
        }
    }

    // 3. Distributions of numeric columns.
    for measure in numeric.iter().take(2) {
        if let Some(chart) = histogram_chart(measure, max_points) {
            charts.push(chart);
        }
    }

    // 4. Relationship between the first two numeric columns.
    if numeric.len() >= 2
        && let Some(chart) = scatter_chart(numeric[0], numeric[1], max_points)
    {
        charts.push(chart);
    }

    // 5. Fallback: if nothing was designed (e.g. all-text data), rank the first
    // column by frequency so the dashboard is never empty.
    if charts.is_empty()
        && let Some(col) = dataset.columns.first()
        && let Some(chart) = bar_chart_by_count(col)
    {
        charts.push(chart);
    }

    charts.truncate(MAX_CHARTS);
    charts
}

fn fmt_num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

fn histogram_chart(col: &Column, max_points: usize) -> Option<ChartSpec> {
    let mut values = col.numeric_values();
    if values.len() < 2 {
        return None;
    }
    let n = values.len();
    let mut sum = 0.0;
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in &values {
        sum += v;
        min = min.min(v);
        max = max.max(v);
    }
    let mean = sum / n as f64;
    downsample(&mut values, max_points);
    Some(ChartSpec {
        id: format!("hist-{}", slug(&col.name)),
        title: format!("Distribution of {}", col.name),
        x_label: col.name.clone(),
        y_label: "count".to_string(),
        caption: format!(
            "{} ranges from {} to {} across {} values, averaging {}.",
            col.name,
            fmt_num(min),
            fmt_num(max),
            n,
            fmt_num(mean)
        ),
        data: ChartData::Histogram { values },
    })
}

fn aggregate_sum(cat: &Column, measure: &Column) -> Vec<(String, f64)> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    let rows = cat.cells.len().min(measure.cells.len());
    for i in 0..rows {
        let Some(label) = cat.cells[i].as_deref().map(str::trim) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        if let Some(v) = measure.numeric_at(i) {
            *sums.entry(label.to_string()).or_insert(0.0) += v;
        }
    }
    let mut pairs: Vec<(String, f64)> = sums.into_iter().collect();
    pairs.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    pairs
}

fn bar_chart_by_measure(cat: &Column, measure: &Column) -> Option<ChartSpec> {
    let pairs = aggregate_sum(cat, measure);
    if pairs.is_empty() {
        return None;
    }
    let top = pairs.first().cloned();
    let pairs: Vec<(String, f64)> = pairs.into_iter().take(20).collect();
    let labels: Vec<String> = pairs.iter().map(|(l, _)| l.clone()).collect();
    let values: Vec<f64> = pairs.iter().map(|(_, v)| *v).collect();
    let caption = match top {
        Some((label, value)) => format!(
            "{} leads {} with a total {} of {}.",
            label,
            cat.name,
            measure.name,
            fmt_num(value)
        ),
        None => format!("{} by {}", measure.name, cat.name),
    };
    Some(ChartSpec {
        id: format!("bar-{}-by-{}", slug(&measure.name), slug(&cat.name)),
        title: format!("{} by {}", measure.name, cat.name),
        x_label: cat.name.clone(),
        y_label: format!("total {}", measure.name),
        caption,
        data: ChartData::Bar { labels, values },
    })
}

fn bar_chart_by_count(col: &Column) -> Option<ChartSpec> {
    let counts = col.value_counts();
    if counts.is_empty() {
        return None;
    }
    let top = counts.first().cloned();
    let counts: Vec<(String, usize)> = counts.into_iter().take(20).collect();
    let labels: Vec<String> = counts.iter().map(|(l, _)| l.clone()).collect();
    let values: Vec<f64> = counts.iter().map(|(_, c)| *c as f64).collect();
    let caption = match top {
        Some((label, count)) => format!(
            "The most frequent {} is \"{}\" ({} rows).",
            col.name, label, count
        ),
        None => format!("Frequency of {}", col.name),
    };
    Some(ChartSpec {
        id: format!("count-{}", slug(&col.name)),
        title: format!("Most frequent {}", col.name),
        x_label: col.name.clone(),
        y_label: "count".to_string(),
        caption,
        data: ChartData::Bar { labels, values },
    })
}

fn line_chart(time: &Column, measure: &Column) -> Option<ChartSpec> {
    // Sum the measure per distinct time label, then order by label.
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    let rows = time.cells.len().min(measure.cells.len());
    for i in 0..rows {
        let Some(label) = time.cells[i].as_deref().map(str::trim) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        if let Some(v) = measure.numeric_at(i) {
            *sums.entry(label.to_string()).or_insert(0.0) += v;
        }
    }
    if sums.len() < 2 {
        return None;
    }
    // BTreeMap already orders labels ascending — good for ISO dates.
    let labels: Vec<String> = sums.keys().cloned().collect();
    let values: Vec<f64> = sums.values().copied().collect();
    let (peak_label, peak_value) = labels
        .iter()
        .cloned()
        .zip(values.iter().copied())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap();
    Some(ChartSpec {
        id: format!("trend-{}-over-{}", slug(&measure.name), slug(&time.name)),
        title: format!("{} over {}", measure.name, time.name),
        x_label: time.name.clone(),
        y_label: measure.name.clone(),
        caption: format!(
            "{} over {} peaks at {} on {}.",
            measure.name,
            time.name,
            fmt_num(peak_value),
            peak_label
        ),
        data: ChartData::Line { labels, values },
    })
}

fn scatter_chart(a: &Column, b: &Column, max_points: usize) -> Option<ChartSpec> {
    let rows = a.cells.len().min(b.cells.len());
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for i in 0..rows {
        if let (Some(x), Some(y)) = (a.numeric_at(i), b.numeric_at(i)) {
            xs.push(x);
            ys.push(y);
        }
    }
    if xs.len() < 3 {
        return None;
    }
    let r = pearson(&xs, &ys);
    // Down-sample both series in lock-step.
    if xs.len() > max_points {
        let stride = xs.len().div_ceil(max_points);
        let sx: Vec<f64> = xs.iter().step_by(stride).copied().collect();
        let sy: Vec<f64> = ys.iter().step_by(stride).copied().collect();
        xs = sx;
        ys = sy;
    }
    let strength = match r.abs() {
        x if x >= 0.7 => "a strong",
        x if x >= 0.4 => "a moderate",
        x if x >= 0.2 => "a weak",
        _ => "little to no",
    };
    let direction = if r >= 0.0 { "positive" } else { "negative" };
    Some(ChartSpec {
        id: format!("scatter-{}-vs-{}", slug(&a.name), slug(&b.name)),
        title: format!("{} vs {}", a.name, b.name),
        x_label: a.name.clone(),
        y_label: b.name.clone(),
        caption: format!(
            "{} and {} show {} {} relationship (r = {:.2}).",
            a.name, b.name, strength, direction, r
        ),
        data: ChartData::Scatter { x: xs, y: ys },
    })
}

/// Pearson correlation coefficient of two equal-length numeric series.
pub fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n == 0 {
        return 0.0;
    }
    let nf = n as f64;
    let mean_x = x.iter().take(n).sum::<f64>() / nf;
    let mean_y = y.iter().take(n).sum::<f64>() / nf;
    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for i in 0..n {
        let dx = x[i] - mean_x;
        let dy = y[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    let denom = (var_x * var_y).sqrt();
    if denom == 0.0 { 0.0 } else { cov / denom }
}

fn downsample(values: &mut Vec<f64>, max_points: usize) {
    if values.len() > max_points {
        let stride = values.len().div_ceil(max_points);
        *values = values.iter().step_by(stride).copied().collect();
    }
}

/// A filesystem/DOM-safe slug for chart ids.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = s.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "col".to_string()
    } else {
        trimmed
    }
}

/// Number-parse helper re-exported for callers building ad-hoc series.
pub fn as_number(raw: &str) -> Option<f64> {
    parse_number(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::brief::DatasetFormat;

    const CSV: &str = "region,revenue,units,date\n\
        North,1200,10,2024-01-01\n\
        South,950,8,2024-01-02\n\
        North,1500,12,2024-01-03\n\
        East,700,6,2024-01-04\n\
        South,1100,9,2024-01-05\n";

    fn dataset() -> Dataset {
        Dataset::from_bytes(CSV.as_bytes(), DatasetFormat::Csv, 1000).unwrap()
    }

    fn brief() -> DashboardBrief {
        DashboardBrief::from_dataset_and_question("d.csv", "Which region earns most?").unwrap()
    }

    #[test]
    fn designs_multiple_chart_kinds() {
        let charts = design_charts(&brief(), &dataset());
        assert!(!charts.is_empty());
        let kinds: Vec<&str> = charts.iter().map(|c| c.data.kind_label()).collect();
        assert!(kinds.contains(&"line"));
        assert!(kinds.contains(&"bar"));
        assert!(kinds.contains(&"histogram"));
        assert!(kinds.contains(&"scatter"));
    }

    #[test]
    fn bar_chart_ranks_by_measure() {
        let charts = design_charts(&brief(), &dataset());
        let bar = charts
            .iter()
            .find(|c| matches!(c.data, ChartData::Bar { .. }))
            .unwrap();
        if let ChartData::Bar { labels, values } = &bar.data {
            // North total = 2700, South = 2050, East = 700.
            assert_eq!(labels[0], "North");
            assert!((values[0] - 2700.0).abs() < 1e-9);
        }
        assert!(bar.caption.contains("North"));
    }

    #[test]
    fn line_chart_is_ordered_by_time() {
        let charts = design_charts(&brief(), &dataset());
        let line = charts
            .iter()
            .find(|c| matches!(c.data, ChartData::Line { .. }))
            .unwrap();
        if let ChartData::Line { labels, .. } = &line.data {
            assert_eq!(labels.first().unwrap(), "2024-01-01");
            assert!(labels.windows(2).all(|w| w[0] <= w[1]));
        }
    }

    #[test]
    fn scatter_reports_correlation() {
        let charts = design_charts(&brief(), &dataset());
        let scatter = charts
            .iter()
            .find(|c| matches!(c.data, ChartData::Scatter { .. }))
            .unwrap();
        assert!(scatter.caption.contains("r ="));
    }

    #[test]
    fn pearson_perfect_positive() {
        let r = pearson(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]);
        assert!((r - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pearson_handles_zero_variance() {
        let r = pearson(&[1.0, 1.0, 1.0], &[2.0, 4.0, 6.0]);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn charts_are_capped() {
        let charts = design_charts(&brief(), &dataset());
        assert!(charts.len() <= MAX_CHARTS);
    }

    #[test]
    fn all_text_dataset_still_yields_a_chart() {
        let csv = "a,b\nx,foo\ny,bar\nz,baz\nx,foo\n";
        let ds = Dataset::from_bytes(csv.as_bytes(), DatasetFormat::Csv, 100).unwrap();
        let charts = design_charts(&brief(), &ds);
        assert!(!charts.is_empty());
    }

    #[test]
    fn slug_is_dom_safe() {
        assert_eq!(slug("Total Revenue ($)"), "total-revenue");
        assert_eq!(slug("!!!"), "col");
    }

    #[test]
    fn as_number_reexport_parses() {
        assert_eq!(as_number("$3,000"), Some(3000.0));
    }
}
