//! Narrative generation: weave the study question, the dataset overview, the
//! findings, and the designed charts into a written Markdown story.
//!
//! The narrative is the *written* half of a data story — the dashboard is the
//! interactive half. It is deterministic prose assembled from the analysis, so
//! it always references the actual question, findings, and chart titles.

use super::analysis::{FindingKind, Findings};
use super::brief::StudyBrief;
use super::viz::ChartSpec;

/// Render the narrative for a study as a Markdown document.
pub fn render_narrative(brief: &StudyBrief, findings: &Findings, charts: &[ChartSpec]) -> String {
    let mut md = String::new();

    md.push_str(&format!("# {}\n\n", brief.title.trim()));
    md.push_str(&format!("**Question.** {}\n\n", brief.question.trim()));
    if let Some(audience) = brief
        .audience
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        md.push_str(&format!("**Audience.** {audience}\n\n"));
    }

    md.push_str("## Overview\n\n");
    md.push_str(&format!("The dataset holds {}.\n\n", findings.headline));

    md.push_str("## What the data shows\n\n");
    let substantive: Vec<_> = findings
        .items
        .iter()
        .filter(|f| f.kind != FindingKind::Overview)
        .collect();
    if substantive.is_empty() {
        md.push_str(
            "The dataset is thin on relationships to surface automatically; the dashboard below \
             lets you explore the raw distributions interactively.\n\n",
        );
    } else {
        for f in &substantive {
            md.push_str(&format!("- **{}.** {}\n", f.title, f.detail));
        }
        md.push('\n');
    }

    md.push_str("## How to read the dashboard\n\n");
    if charts.is_empty() {
        md.push_str("No charts could be designed for this dataset.\n\n");
    } else {
        md.push_str(
            "The dashboard renders the following interactive views (hover for values, drag to \
             zoom, click legend entries to filter):\n\n",
        );
        for (i, c) in charts.iter().enumerate() {
            md.push_str(&format!(
                "{}. **{}** ({}) — {}\n",
                i + 1,
                c.title,
                c.chart_type.label(),
                c.rationale
            ));
        }
        md.push('\n');
    }

    md.push_str("## Answer\n\n");
    md.push_str(&answer_paragraph(brief, findings));
    md.push('\n');

    md
}

/// Compose a closing paragraph that ties the strongest finding back to the
/// question.
fn answer_paragraph(brief: &StudyBrief, findings: &Findings) -> String {
    let lead = findings
        .items
        .iter()
        .find(|f| f.kind == FindingKind::Relationship)
        .or_else(|| findings.items.iter().find(|f| f.kind == FindingKind::Trend))
        .or_else(|| {
            findings
                .items
                .iter()
                .find(|f| f.kind == FindingKind::Composition)
        });

    match lead {
        Some(f) => format!(
            "In answer to “{}”: {} Explore the dashboard to test this against subgroups and \
             ranges of interest.\n",
            brief.question.trim(),
            f.detail
        ),
        None => format!(
            "The question “{}” cannot be answered decisively from this dataset alone; use the \
             interactive views to inspect the distributions and form hypotheses.\n",
            brief.question.trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::analysis::analyze;
    use crate::cartographer::brief::StudyBrief;
    use crate::cartographer::dataset::{Dataset, MAX_ROWS, parse_csv};
    use crate::cartographer::viz::design_charts;

    fn brief() -> StudyBrief {
        StudyBrief::from_json_bytes(
            br#"{"title":"Sales study","question":"How do sales relate to income?",
                 "dataset":{"csv":"region,sales,income\nN,100,50\nS,200,90\nE,300,120\n"}}"#,
        )
        .unwrap()
    }

    fn dataset() -> Dataset {
        let (h, rows) = parse_csv(
            "region,sales,income\nN,100,50\nS,200,90\nE,300,120\n",
            MAX_ROWS,
        )
        .unwrap();
        Dataset::from_table(h, rows).unwrap()
    }

    #[test]
    fn narrative_references_question_and_charts() {
        let b = brief();
        let d = dataset();
        let f = analyze(&d, &b.question);
        let charts = design_charts(&d, &b.hints);
        let md = render_narrative(&b, &f, &charts);
        assert!(md.contains("# Sales study"));
        assert!(md.contains("How do sales relate to income?"));
        assert!(md.contains("## Answer"));
        // Every chart title appears in the "how to read" section.
        for c in &charts {
            assert!(md.contains(&c.title), "missing chart {}", c.title);
        }
    }

    #[test]
    fn narrative_has_findings_section_content() {
        let b = brief();
        let d = dataset();
        let f = analyze(&d, &b.question);
        let charts = design_charts(&d, &b.hints);
        let md = render_narrative(&b, &f, &charts);
        assert!(md.contains("## What the data shows"));
        assert!(md.len() > 300);
    }
}
