//! Narrative generation.
//!
//! Produces the written story (`narrative.md`) that accompanies the dashboard:
//! it restates the question, summarises the dataset, and turns each designed
//! chart's caption into a plain-language finding. The narrative is a required
//! deliverable — a dashboard without a story is only half the answer.

use super::brief::DashboardBrief;
use super::dataset::{DatasetProfile, NumericSummary};
use super::viz::ChartSpec;

/// Render the Markdown narrative for a dashboard.
pub fn generate_narrative(
    brief: &DashboardBrief,
    profile: &DatasetProfile,
    charts: &[ChartSpec],
) -> String {
    let mut out = String::new();

    out.push_str(&format!("# {}\n\n", brief.title()));
    out.push_str(&format!("**Question.** {}\n\n", brief.question.trim()));

    out.push_str("## Dataset overview\n\n");
    out.push_str(&format!(
        "The dataset has **{} rows** across **{} columns**{}.\n\n",
        profile.row_count,
        profile.column_count,
        if profile.truncated {
            " (loaded up to the configured row cap)"
        } else {
            ""
        }
    ));

    out.push_str("| Column | Type | Non-missing | Distinct | Summary |\n");
    out.push_str("| ------ | ---- | ----------- | -------- | ------- |\n");
    for col in &profile.columns {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            escape_pipe(&col.name),
            col.ctype.label(),
            col.count,
            col.distinct,
            escape_pipe(&column_summary(
                col.numeric.as_ref(),
                &col.top_values_labels()
            )),
        ));
    }
    out.push('\n');

    out.push_str("## Key findings\n\n");
    if charts.is_empty() {
        out.push_str(
            "No chartable structure was found in this dataset, so no findings could be \
             derived automatically.\n\n",
        );
    } else {
        for chart in charts {
            out.push_str(&format!("- {}\n", chart.caption));
        }
        out.push('\n');
    }

    out.push_str("## Dashboard\n\n");
    out.push_str(&format!(
        "The interactive dashboard `dashboard.html` presents {} chart{} answering the \
         question above. Open it in a browser, or serve it with `simard cartographer \
         serve --out <dir>`.\n\n",
        charts.len(),
        if charts.len() == 1 { "" } else { "s" }
    ));
    for chart in charts {
        out.push_str(&format!(
            "### {} ({})\n\n{}\n\n",
            chart.title,
            chart.data.kind_label(),
            chart.caption
        ));
    }

    out
}

fn column_summary(numeric: Option<&NumericSummary>, top_labels: &[String]) -> String {
    if let Some(n) = numeric {
        return format!(
            "min {}, mean {}, max {}",
            trim_num(n.min),
            trim_num(n.mean),
            trim_num(n.max)
        );
    }
    if top_labels.is_empty() {
        return "—".to_string();
    }
    let shown: Vec<String> = top_labels.iter().take(3).cloned().collect();
    format!("top: {}", shown.join(", "))
}

fn trim_num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

fn escape_pipe(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// Helper on the profile column to pull its top-value labels.
trait TopLabels {
    fn top_values_labels(&self) -> Vec<String>;
}

impl TopLabels for super::dataset::ColumnProfile {
    fn top_values_labels(&self) -> Vec<String> {
        self.top_values.iter().map(|t| t.value.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::brief::DatasetFormat;
    use crate::cartographer::dataset::Dataset;
    use crate::cartographer::viz::design_charts;

    const CSV: &str = "region,revenue,date\n\
        North,1200,2024-01-01\n\
        South,950,2024-01-02\n\
        North,1500,2024-01-03\n";

    fn setup() -> (DashboardBrief, DatasetProfile, Vec<ChartSpec>) {
        let brief =
            DashboardBrief::from_dataset_and_question("d.csv", "Which region earns most?").unwrap();
        let ds = Dataset::from_bytes(CSV.as_bytes(), DatasetFormat::Csv, 1000).unwrap();
        let profile = ds.profile();
        let charts = design_charts(&brief, &ds);
        (brief, profile, charts)
    }

    #[test]
    fn narrative_has_expected_sections() {
        let (brief, profile, charts) = setup();
        let md = generate_narrative(&brief, &profile, &charts);
        assert!(md.contains("## Dataset overview"));
        assert!(md.contains("## Key findings"));
        assert!(md.contains("## Dashboard"));
    }

    #[test]
    fn narrative_restates_the_question() {
        let (brief, profile, charts) = setup();
        let md = generate_narrative(&brief, &profile, &charts);
        assert!(md.contains("Which region earns most?"));
    }

    #[test]
    fn narrative_lists_columns_and_findings() {
        let (brief, profile, charts) = setup();
        let md = generate_narrative(&brief, &profile, &charts);
        assert!(md.contains("| region |"));
        assert!(md.contains("| revenue |"));
        // At least one chart caption should appear as a finding.
        assert!(md.contains("North") || md.contains("revenue"));
    }

    #[test]
    fn narrative_handles_no_charts() {
        let brief = DashboardBrief::from_dataset_and_question("d.csv", "Q?").unwrap();
        let ds = Dataset::from_bytes(CSV.as_bytes(), DatasetFormat::Csv, 1000).unwrap();
        let profile = ds.profile();
        let md = generate_narrative(&brief, &profile, &[]);
        assert!(md.contains("No chartable structure"));
    }

    #[test]
    fn escape_pipe_protects_table() {
        assert_eq!(escape_pipe("a|b"), "a\\|b");
    }
}
