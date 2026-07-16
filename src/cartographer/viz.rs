//! Visualization design: choose a small, well-typed set of chart specifications
//! from the dataset's column kinds, the operator hints, and the question.
//!
//! A [`ChartSpec`] is a declarative encoding (chart type + column roles) that
//! the dashboard renderer turns into an interactive Plotly/D3 view. Column
//! selection is deterministic and always references columns that exist in the
//! dataset, so a spec can never dangle.

use serde::{Deserialize, Serialize};

use super::brief::Hints;
use super::dataset::{Column, ColumnKind, Dataset};

/// The kinds of interactive chart Cartographer can design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartType {
    /// Two numeric measures, optionally coloured by a category.
    Scatter,
    /// A measure over a temporal axis, optionally split by a category.
    Line,
    /// A categorical breakdown of an aggregated measure.
    Bar,
    /// The distribution of a single numeric measure.
    Histogram,
}

impl ChartType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Scatter => "scatter",
            Self::Line => "line",
            Self::Bar => "bar",
            Self::Histogram => "histogram",
        }
    }
}

/// A declarative, dataset-grounded chart encoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartSpec {
    pub id: String,
    pub chart_type: ChartType,
    pub title: String,
    pub x: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// How `y` is aggregated per `x` bucket for bar charts (`mean` or `sum`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<String>,
    /// One-line rationale for why this chart answers part of the question.
    pub rationale: String,
}

impl ChartSpec {
    /// All dataset column names this spec references.
    pub fn referenced_columns(&self) -> Vec<&str> {
        let mut cols = vec![self.x.as_str()];
        if let Some(y) = &self.y {
            cols.push(y);
        }
        if let Some(c) = &self.color {
            cols.push(c);
        }
        cols
    }
}

fn hint_column<'a>(dataset: &'a Dataset, name: &Option<String>) -> Option<&'a Column> {
    name.as_deref().and_then(|n| dataset.column(n))
}

/// Pick a numeric column, preferring `hint`, then the first numeric that is not
/// `exclude`.
fn pick_numeric<'a>(
    dataset: &'a Dataset,
    hint: Option<&'a Column>,
    exclude: Option<&str>,
) -> Option<&'a Column> {
    if let Some(h) = hint
        && h.kind == ColumnKind::Numeric
        && Some(h.name.as_str()) != exclude
    {
        return Some(h);
    }
    dataset
        .columns_of_kind(ColumnKind::Numeric)
        .into_iter()
        .find(|c| Some(c.name.as_str()) != exclude)
}

/// Design the chart set. Always returns at least one chart when the dataset has
/// any numeric or categorical column; an empty vec only for degenerate data.
pub fn design_charts(dataset: &Dataset, hints: &Hints) -> Vec<ChartSpec> {
    let mut charts: Vec<ChartSpec> = Vec::new();

    let hx = hint_column(dataset, &hints.x);
    let hy = hint_column(dataset, &hints.y);
    let hcolor = hint_column(dataset, &hints.color).filter(|c| {
        matches!(c.kind, ColumnKind::Categorical | ColumnKind::Text) && c.distinct_count <= 20
    });
    let htime = hint_column(dataset, &hints.time).filter(|c| c.kind == ColumnKind::Temporal);

    let numeric = dataset.columns_of_kind(ColumnKind::Numeric);
    let categorical = dataset.columns_of_kind(ColumnKind::Categorical);
    let temporal = dataset.columns_of_kind(ColumnKind::Temporal);

    // 1. Scatter: relationship between two numeric measures (the headline view).
    if numeric.len() >= 2 {
        let x = pick_numeric(dataset, hx, None);
        let y = pick_numeric(dataset, hy, x.map(|c| c.name.as_str()));
        if let (Some(x), Some(y)) = (x, y) {
            charts.push(ChartSpec {
                id: "relationship".to_string(),
                chart_type: ChartType::Scatter,
                title: format!("{} vs {}", y.name, x.name),
                x: x.name.clone(),
                y: Some(y.name.clone()),
                color: hcolor.map(|c| c.name.clone()),
                aggregate: None,
                rationale: format!(
                    "Shows how “{}” responds to “{}” row by row{}.",
                    y.name,
                    x.name,
                    hcolor
                        .map(|c| format!(", coloured by “{}”", c.name))
                        .unwrap_or_default()
                ),
            });
        }
    }

    // 2. Line: a measure over time.
    if let Some(measure) = pick_numeric(dataset, hy.or(hx), None) {
        let time = htime.or_else(|| temporal.first().copied());
        if let Some(time) = time {
            charts.push(ChartSpec {
                id: "trend".to_string(),
                chart_type: ChartType::Line,
                title: format!("{} over {}", measure.name, time.name),
                x: time.name.clone(),
                y: Some(measure.name.clone()),
                color: hcolor.map(|c| c.name.clone()),
                aggregate: None,
                rationale: format!(
                    "Traces how “{}” evolves along “{}”.",
                    measure.name, time.name
                ),
            });
        }
    }

    // 3. Bar: a measure aggregated by category.
    if let (Some(cat), Some(measure)) = (categorical.first(), pick_numeric(dataset, hy, None)) {
        charts.push(ChartSpec {
            id: "composition".to_string(),
            chart_type: ChartType::Bar,
            title: format!("Mean {} by {}", measure.name, cat.name),
            x: cat.name.clone(),
            y: Some(measure.name.clone()),
            color: None,
            aggregate: Some("mean".to_string()),
            rationale: format!(
                "Compares the average “{}” across each “{}”.",
                measure.name, cat.name
            ),
        });
    }

    // 4. Histogram: distribution of the primary measure (fallback + always
    //    useful). Guarantees at least one chart when any numeric column exists.
    if let Some(measure) = pick_numeric(dataset, hy.or(hx), None) {
        charts.push(ChartSpec {
            id: "distribution".to_string(),
            chart_type: ChartType::Histogram,
            title: format!("Distribution of {}", measure.name),
            x: measure.name.clone(),
            y: None,
            color: None,
            aggregate: None,
            rationale: format!("Reveals the shape and spread of “{}”.", measure.name),
        });
    }

    // Degenerate fallback: no numeric column at all → count the first category.
    if charts.is_empty()
        && let Some(cat) = categorical.first()
    {
        charts.push(ChartSpec {
            id: "composition".to_string(),
            chart_type: ChartType::Bar,
            title: format!("Count by {}", cat.name),
            x: cat.name.clone(),
            y: None,
            color: None,
            aggregate: Some("count".to_string()),
            rationale: format!("Counts rows in each “{}”.", cat.name),
        });
    }

    charts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::dataset::{Dataset, MAX_ROWS, parse_csv};

    fn ds(csv: &str) -> Dataset {
        let (h, rows) = parse_csv(csv, MAX_ROWS).unwrap();
        Dataset::from_table(h, rows).unwrap()
    }

    #[test]
    fn scatter_bar_and_histogram_for_mixed_data() {
        let d = ds("region,sales,income\nNorth,100,50\nSouth,200,90\nEast,300,120\nWest,250,110\n");
        let charts = design_charts(&d, &Hints::default());
        assert!(charts.iter().any(|c| c.chart_type == ChartType::Scatter));
        assert!(charts.iter().any(|c| c.chart_type == ChartType::Bar));
        assert!(charts.iter().any(|c| c.chart_type == ChartType::Histogram));
    }

    #[test]
    fn line_chart_for_temporal_data() {
        let d = ds("month,users\n2024-01,100\n2024-02,150\n2024-03,220\n");
        let charts = design_charts(&d, &Hints::default());
        assert!(charts.iter().any(|c| c.chart_type == ChartType::Line));
    }

    #[test]
    fn every_spec_references_existing_columns() {
        let d = ds("a,b,c\n1,2,x\n3,4,y\n5,6,z\n");
        for spec in design_charts(&d, &Hints::default()) {
            for col in spec.referenced_columns() {
                assert!(d.column(col).is_some(), "dangling column {col}");
            }
        }
    }

    #[test]
    fn hints_steer_scatter_axes_and_color() {
        let d = ds("income,life,continent\n50,60,Asia\n90,72,Europe\n120,80,Europe\n40,55,Asia\n");
        let hints = Hints {
            x: Some("income".into()),
            y: Some("life".into()),
            color: Some("continent".into()),
            ..Hints::default()
        };
        let charts = design_charts(&d, &hints);
        let scatter = charts
            .iter()
            .find(|c| c.chart_type == ChartType::Scatter)
            .unwrap();
        assert_eq!(scatter.x, "income");
        assert_eq!(scatter.y.as_deref(), Some("life"));
        assert_eq!(scatter.color.as_deref(), Some("continent"));
    }

    #[test]
    fn categorical_only_data_still_yields_a_chart() {
        let d = ds("team\nred\nblue\nred\ngreen\n");
        let charts = design_charts(&d, &Hints::default());
        assert!(!charts.is_empty());
        assert_eq!(charts[0].chart_type, ChartType::Bar);
    }
}
