//! Exploratory analysis: turn a profiled [`Dataset`] and the study question into
//! a small set of durable, quantitative *findings* that the narrative and the
//! dashboard are built around.
//!
//! The analysis is deterministic and dependency-free: shape overview, numeric
//! ranges, the strongest linear relationships between numeric columns, the
//! dominant category share, and the direction of a measure over time.

use serde::{Deserialize, Serialize};

use super::dataset::{Column, ColumnKind, Dataset};

/// Maximum number of numeric column pairs correlated (guards O(n²) blow-up on
/// very wide numeric tables).
const MAX_CORRELATION_COLUMNS: usize = 24;

/// The category of a finding, used for grouping in the narrative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    Overview,
    Distribution,
    Relationship,
    Composition,
    Trend,
}

/// A single quantitative observation about the dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub title: String,
    pub detail: String,
}

/// The full analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Findings {
    /// A one-line summary of the dataset shape.
    pub headline: String,
    pub items: Vec<Finding>,
}

impl Findings {
    /// Count of findings excluding the overview line.
    pub fn substantive_count(&self) -> usize {
        self.items
            .iter()
            .filter(|f| f.kind != FindingKind::Overview)
            .count()
    }
}

/// Compute Pearson's correlation coefficient over two columns aligned by row,
/// dropping any row where either value is null / non-numeric. Returns `None`
/// when fewer than three complete pairs exist or a series has zero variance.
pub fn correlation(a: &Column, b: &Column) -> Option<f64> {
    let pairs: Vec<(f64, f64)> = a
        .values
        .iter()
        .zip(b.values.iter())
        .filter_map(|(x, y)| {
            Some((
                super::dataset::parse_number(x)?,
                super::dataset::parse_number(y)?,
            ))
        })
        .collect();
    if pairs.len() < 3 {
        return None;
    }
    let n = pairs.len() as f64;
    let mean_x = pairs.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|p| p.1).sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    for (x, y) in &pairs {
        let dx = x - mean_x;
        let dy = y - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }
    if var_x <= f64::EPSILON || var_y <= f64::EPSILON {
        return None;
    }
    let r = cov / (var_x.sqrt() * var_y.sqrt());
    r.is_finite().then_some(r)
}

fn strength_word(r: f64) -> &'static str {
    match r.abs() {
        x if x >= 0.7 => "strong",
        x if x >= 0.4 => "moderate",
        x if x >= 0.2 => "weak",
        _ => "negligible",
    }
}

/// Run the full exploratory analysis.
pub fn analyze(dataset: &Dataset, question: &str) -> Findings {
    let numeric = dataset.columns_of_kind(ColumnKind::Numeric);
    let categorical = dataset.columns_of_kind(ColumnKind::Categorical);
    let temporal = dataset.columns_of_kind(ColumnKind::Temporal);

    let headline = format!(
        "{} rows × {} columns ({} numeric, {} categorical, {} temporal, {} text)",
        dataset.row_count,
        dataset.columns.len(),
        numeric.len(),
        categorical.len(),
        temporal.len(),
        dataset.columns_of_kind(ColumnKind::Text).len(),
    );

    let mut items = vec![Finding {
        kind: FindingKind::Overview,
        title: "Dataset shape".to_string(),
        detail: format!(
            "The study asks: “{}”. The dataset holds {} and is analysed below.",
            question.trim(),
            headline
        ),
    }];

    // Numeric ranges (a couple of the widest-varying measures).
    let mut ranged: Vec<(&Column, f64)> = numeric
        .iter()
        .filter_map(|c| c.numeric.as_ref().map(|s| (*c, s.max - s.min)))
        .collect();
    ranged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (col, _) in ranged.iter().take(2) {
        if let Some(s) = &col.numeric {
            items.push(Finding {
                kind: FindingKind::Distribution,
                title: format!("{} ranges {} – {}", col.name, trim(s.min), trim(s.max)),
                detail: format!(
                    "“{}” spans {} to {} with a mean of {} (σ {}) over {} values.",
                    col.name,
                    trim(s.min),
                    trim(s.max),
                    trim(s.mean),
                    trim(s.std_dev),
                    s.count
                ),
            });
        }
    }

    // Strongest linear relationship among numeric columns.
    if numeric.len() >= 2 {
        let capped = &numeric[..numeric.len().min(MAX_CORRELATION_COLUMNS)];
        let mut best: Option<(&Column, &Column, f64)> = None;
        for i in 0..capped.len() {
            for j in (i + 1)..capped.len() {
                if let Some(r) = correlation(capped[i], capped[j])
                    && best.map(|(_, _, br)| r.abs() > br.abs()).unwrap_or(true)
                {
                    best = Some((capped[i], capped[j], r));
                }
            }
        }
        if let Some((a, b, r)) = best {
            let dir = if r >= 0.0 { "positive" } else { "negative" };
            items.push(Finding {
                kind: FindingKind::Relationship,
                title: format!(
                    "{} vs {}: {} {} correlation",
                    a.name,
                    b.name,
                    strength_word(r),
                    dir
                ),
                detail: format!(
                    "“{}” and “{}” move together with Pearson r = {:.2} ({} {}). This is the \
                     strongest linear relationship among the numeric columns and anchors the \
                     scatter view.",
                    a.name,
                    b.name,
                    r,
                    strength_word(r),
                    dir
                ),
            });
        }
    }

    // Dominant category composition.
    if let Some(cat) = categorical.first()
        && let Some(top) = cat.top_categories.first()
    {
        let share = if dataset.row_count > 0 {
            100.0 * top.count as f64 / dataset.row_count as f64
        } else {
            0.0
        };
        items.push(Finding {
            kind: FindingKind::Composition,
            title: format!("“{}” is led by {}", cat.name, top.value),
            detail: format!(
                "Across {} groups of “{}”, “{}” is the most common at {} rows ({:.0}% of the \
                 data).",
                cat.distinct_count, cat.name, top.value, top.count, share
            ),
        });
    }

    // Trend of the first numeric measure over the first temporal column.
    if let (Some(time), Some(measure)) = (temporal.first(), numeric.first())
        && let Some((first, last)) = first_last_by_time(time, measure)
    {
        let (dir, pct) = if first.abs() > f64::EPSILON {
            let change = 100.0 * (last - first) / first.abs();
            (if change >= 0.0 { "rose" } else { "fell" }, change.abs())
        } else if last > first {
            ("rose", f64::NAN)
        } else {
            ("held", 0.0)
        };
        let pct_txt = if pct.is_nan() {
            String::new()
        } else {
            format!(" by {:.0}%", pct)
        };
        items.push(Finding {
            kind: FindingKind::Trend,
            title: format!("“{}” {} over “{}”", measure.name, dir, time.name),
            detail: format!(
                "From the earliest to the latest “{}”, “{}” {}{} (from {} to {}).",
                time.name,
                measure.name,
                dir,
                pct_txt,
                trim(first),
                trim(last)
            ),
        });
    }

    Findings { headline, items }
}

/// First and last numeric measure value ordered by the temporal column's string
/// order (ISO dates sort chronologically as strings).
fn first_last_by_time(time: &Column, measure: &Column) -> Option<(f64, f64)> {
    let mut pairs: Vec<(&String, f64)> = time
        .values
        .iter()
        .zip(measure.values.iter())
        .filter_map(|(t, m)| {
            if t.is_empty() {
                None
            } else {
                super::dataset::parse_number(m).map(|v| (t, v))
            }
        })
        .collect();
    if pairs.len() < 2 {
        return None;
    }
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    Some((pairs.first().unwrap().1, pairs.last().unwrap().1))
}

/// Format a float compactly: integers print without a fractional part.
pub fn trim(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
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
    fn correlation_detects_perfect_line() {
        let d = ds("x,y\n1,2\n2,4\n3,6\n4,8\n");
        let r = correlation(d.column("x").unwrap(), d.column("y").unwrap()).unwrap();
        assert!((r - 1.0).abs() < 1e-9);
    }

    #[test]
    fn correlation_none_on_constant() {
        let d = ds("x,y\n1,5\n2,5\n3,5\n");
        assert!(correlation(d.column("x").unwrap(), d.column("y").unwrap()).is_none());
    }

    #[test]
    fn analyze_produces_overview_and_findings() {
        let d = ds("region,sales,income\nNorth,100,50\nSouth,200,90\nNorth,150,70\nEast,300,120\n");
        let f = analyze(&d, "How do sales relate to income?");
        assert!(f.headline.contains("rows"));
        assert!(f.substantive_count() >= 2);
        assert!(f.items.iter().any(|i| i.kind == FindingKind::Relationship));
        assert!(f.items.iter().any(|i| i.kind == FindingKind::Composition));
    }

    #[test]
    fn analyze_detects_trend_over_time() {
        let d = ds("month,users\n2024-01,100\n2024-02,150\n2024-03,220\n");
        let f = analyze(&d, "How do users change over time?");
        assert!(f.items.iter().any(|i| i.kind == FindingKind::Trend));
        let trend = f
            .items
            .iter()
            .find(|i| i.kind == FindingKind::Trend)
            .unwrap();
        assert!(trend.detail.contains("rose"));
    }

    #[test]
    fn trim_formats_ints_and_floats() {
        assert_eq!(trim(3.0), "3");
        assert_eq!(trim(1.23456), "1.23");
    }
}
