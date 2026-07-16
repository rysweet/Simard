//! Dashboard rendering: turn the dataset, chart specs, and narrative into a
//! self-contained **interactive** dashboard.
//!
//! * [`render_html`] produces a single `dashboard.html` that embeds the data and
//!   chart specs as JSON and renders them with **Plotly** (interactive: hover,
//!   zoom, legend filtering) plus a **D3**-rendered column-profile table and the
//!   written narrative. It is self-contained apart from the two CDN libraries.
//! * [`render_streamlit`] emits a runnable **Streamlit** `app.py` source.
//! * [`render_observable`] emits an **Observable**-flavoured notebook source.
//!
//! None of these renderers execute an interpreter; they generate source text.
//! The generated files are runtime outputs, never committed to the repository.

use serde_json::json;

use super::brief::StudyBrief;
use super::dataset::Dataset;
use super::viz::ChartSpec;

const PLOTLY_CDN: &str = "https://cdn.plot.ly/plotly-2.35.2.min.js";
const D3_CDN: &str = "https://cdn.jsdelivr.net/npm/d3@7";

/// HTML-escape text destined for element bodies / attributes.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Make a JSON string safe to embed inside a `<script>` block by neutralising
/// any `</` sequence (which would otherwise close the script element).
fn script_safe_json(value: &serde_json::Value) -> String {
    value.to_string().replace("</", "<\\/")
}

/// A tiny Markdown → HTML converter covering the constructs the narrative uses:
/// `#`/`##` headings, `-`/`1.` list items, `**bold**`, and paragraphs.
fn markdown_to_html(md: &str) -> String {
    let mut out = String::new();
    let mut in_ul = false;
    let mut in_ol = false;

    let close_lists = |out: &mut String, in_ul: &mut bool, in_ol: &mut bool| {
        if *in_ul {
            out.push_str("</ul>\n");
            *in_ul = false;
        }
        if *in_ol {
            out.push_str("</ol>\n");
            *in_ol = false;
        }
    };

    for raw in md.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            close_lists(&mut out, &mut in_ul, &mut in_ol);
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            close_lists(&mut out, &mut in_ul, &mut in_ol);
            out.push_str(&format!("<h2>{}</h2>\n", inline(rest)));
        } else if let Some(rest) = line.strip_prefix("# ") {
            close_lists(&mut out, &mut in_ul, &mut in_ol);
            out.push_str(&format!("<h1>{}</h1>\n", inline(rest)));
        } else if let Some(rest) = line.strip_prefix("- ") {
            if in_ol {
                out.push_str("</ol>\n");
                in_ol = false;
            }
            if !in_ul {
                out.push_str("<ul>\n");
                in_ul = true;
            }
            out.push_str(&format!("<li>{}</li>\n", inline(rest)));
        } else if let Some((_num, rest)) = split_ordered(line) {
            if in_ul {
                out.push_str("</ul>\n");
                in_ul = false;
            }
            if !in_ol {
                out.push_str("<ol>\n");
                in_ol = true;
            }
            out.push_str(&format!("<li>{}</li>\n", inline(rest)));
        } else {
            close_lists(&mut out, &mut in_ul, &mut in_ol);
            out.push_str(&format!("<p>{}</p>\n", inline(line)));
        }
    }
    close_lists(&mut out, &mut in_ul, &mut in_ol);
    out
}

/// Split `"3. text"` into `(3, "text")`.
fn split_ordered(line: &str) -> Option<(u32, &str)> {
    let dot = line.find(". ")?;
    let num: u32 = line[..dot].parse().ok()?;
    Some((num, &line[dot + 2..]))
}

/// Inline formatting: escape, then apply `**bold**` and “smart” passthrough.
fn inline(s: &str) -> String {
    let escaped = escape_html(s);
    let mut out = String::with_capacity(escaped.len());
    let mut bold = false;
    let bytes = escaped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            out.push_str(if bold { "</strong>" } else { "<strong>" });
            bold = !bold;
            i += 2;
        } else {
            out.push(escaped[i..].chars().next().unwrap());
            i += escaped[i..].chars().next().unwrap().len_utf8();
        }
    }
    if bold {
        out.push_str("</strong>");
    }
    out
}

/// Render the interactive HTML dashboard.
pub fn render_html(
    brief: &StudyBrief,
    dataset: &Dataset,
    charts: &[ChartSpec],
    narrative_md: &str,
) -> String {
    let data_json = json!({ "columns": dataset.to_json_columns() });
    let charts_json = serde_json::to_value(charts).unwrap_or(json!([]));
    let profile_json = json!({
        "row_count": dataset.row_count,
        "columns": dataset.columns.iter().map(|c| json!({
            "name": c.name,
            "kind": c.kind.label(),
            "nulls": c.null_count,
            "distinct": c.distinct_count,
        })).collect::<Vec<_>>(),
    });

    let narrative_html = markdown_to_html(narrative_md);
    let title = escape_html(brief.title.trim());
    let question = escape_html(brief.question.trim());

    let chart_divs: String = charts
        .iter()
        .map(|c| {
            format!(
                "<figure class=\"chart\"><div id=\"chart-{id}\" class=\"plot\"></div>\
                 <figcaption>{cap}</figcaption></figure>\n",
                id = escape_html(&c.id),
                cap = escape_html(&c.rationale)
            )
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — Cartographer dashboard</title>
<script src="{plotly}"></script>
<script src="{d3}"></script>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
         margin: 0; line-height: 1.55; color: #1b1f24; background: #fbfcfe; }}
  header {{ background: #0d3b66; color: #fff; padding: 1.5rem 2rem; }}
  header h1 {{ margin: 0 0 .25rem; font-size: 1.6rem; }}
  header p {{ margin: 0; opacity: .9; }}
  main {{ max-width: 1080px; margin: 0 auto; padding: 1.5rem 2rem 4rem; }}
  section {{ margin-bottom: 2.5rem; }}
  .charts {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(420px, 1fr)); gap: 1.5rem; }}
  figure.chart {{ margin: 0; background: #fff; border: 1px solid #e2e8f0; border-radius: 10px;
                 padding: .5rem .75rem 1rem; box-shadow: 0 1px 2px rgba(0,0,0,.04); }}
  figure.chart figcaption {{ font-size: .85rem; color: #475569; padding: 0 .25rem; }}
  .plot {{ width: 100%; height: 360px; }}
  table.profile {{ border-collapse: collapse; width: 100%; font-size: .9rem; }}
  table.profile th, table.profile td {{ border-bottom: 1px solid #e2e8f0; padding: .35rem .6rem; text-align: left; }}
  table.profile th {{ background: #eef2f7; }}
  .narrative h1 {{ display: none; }}
  .badge {{ display: inline-block; font-size: .75rem; background: #e0ecff; color: #0d3b66;
           border-radius: 999px; padding: .1rem .6rem; }}
</style>
</head>
<body>
<header>
  <h1>{title}</h1>
  <p><span class="badge">Cartographer</span> {question}</p>
</header>
<main>
  <section class="narrative">{narrative}</section>
  <section>
    <h2>Interactive views</h2>
    <div class="charts">
{chart_divs}    </div>
  </section>
  <section>
    <h2>Column profile</h2>
    <div id="profile"></div>
  </section>
</main>

<script type="application/json" id="cartographer-data">{data}</script>
<script type="application/json" id="cartographer-charts">{charts}</script>
<script type="application/json" id="cartographer-profile">{profile}</script>
<script>
const DATA = JSON.parse(document.getElementById('cartographer-data').textContent).columns;
const CHARTS = JSON.parse(document.getElementById('cartographer-charts').textContent);
const PROFILE = JSON.parse(document.getElementById('cartographer-profile').textContent);

function col(name) {{ return DATA[name] || []; }}

function groupBy(colorName) {{
  const groups = new Map();
  const colors = col(colorName);
  colors.forEach((v, i) => {{
    const key = v == null ? '(null)' : String(v);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(i);
  }});
  return groups;
}}

function aggregate(spec) {{
  const xs = col(spec.x), ys = spec.y ? col(spec.y) : null;
  const buckets = new Map();
  xs.forEach((xv, i) => {{
    const key = xv == null ? '(null)' : String(xv);
    if (!buckets.has(key)) buckets.set(key, []);
    if (ys) {{ if (ys[i] != null) buckets.get(key).push(ys[i]); }}
    else buckets.get(key).push(1);
  }});
  const cats = [...buckets.keys()].sort();
  const vals = cats.map(k => {{
    const arr = buckets.get(k);
    if (spec.aggregate === 'sum' || spec.aggregate === 'count') return arr.reduce((a,b)=>a+b,0);
    return arr.length ? arr.reduce((a,b)=>a+b,0)/arr.length : 0;
  }});
  return {{cats, vals}};
}}

function renderChart(spec) {{
  const el = 'chart-' + spec.id;
  const layout = {{ title: spec.title, margin: {{t: 40, r: 16, b: 44, l: 56}},
                   xaxis: {{title: spec.x}}, yaxis: {{title: spec.y || 'count'}} }};
  let traces = [];
  if (spec.chart_type === 'scatter' || spec.chart_type === 'line') {{
    const mode = spec.chart_type === 'line' ? 'lines+markers' : 'markers';
    if (spec.color) {{
      for (const [key, idx] of groupBy(spec.color)) {{
        traces.push({{ x: idx.map(i => col(spec.x)[i]), y: idx.map(i => col(spec.y)[i]),
                      mode, type: 'scatter', name: key }});
      }}
    }} else {{
      traces.push({{ x: col(spec.x), y: col(spec.y), mode, type: 'scatter', name: spec.y }});
    }}
  }} else if (spec.chart_type === 'bar') {{
    const {{cats, vals}} = aggregate(spec);
    traces.push({{ x: cats, y: vals, type: 'bar', name: spec.title }});
  }} else if (spec.chart_type === 'histogram') {{
    traces.push({{ x: col(spec.x), type: 'histogram', name: spec.x }});
  }}
  Plotly.newPlot(el, traces, layout, {{responsive: true, displayModeBar: true}});
}}

CHARTS.forEach(renderChart);

// D3-rendered column-profile table.
const rows = PROFILE.columns;
const table = d3.select('#profile').append('table').attr('class', 'profile');
table.append('thead').append('tr').selectAll('th')
  .data(['Column', 'Kind', 'Nulls', 'Distinct']).enter().append('th').text(d => d);
const tbody = table.append('tbody');
const tr = tbody.selectAll('tr').data(rows).enter().append('tr');
tr.append('td').text(d => d.name);
tr.append('td').text(d => d.kind);
tr.append('td').text(d => d.nulls);
tr.append('td').text(d => d.distinct);
d3.select('#profile').append('p')
  .style('font-size', '.85rem').style('color', '#475569')
  .text(`${{PROFILE.row_count}} rows profiled.`);
</script>
</body>
</html>
"#,
        title = title,
        question = question,
        plotly = PLOTLY_CDN,
        d3 = D3_CDN,
        narrative = narrative_html,
        chart_divs = chart_divs,
        data = script_safe_json(&data_json),
        charts = script_safe_json(&charts_json),
        profile = script_safe_json(&profile_json),
    )
}

/// Emit a runnable Streamlit `app.py` source that reproduces the dashboard.
pub fn render_streamlit(brief: &StudyBrief, charts: &[ChartSpec]) -> String {
    let charts_py = serde_json::to_string(charts).unwrap_or_else(|_| "[]".to_string());
    let title = brief.title.trim().replace('"', "'");
    let question = brief.question.trim().replace('"', "'");
    // Generated at runtime, never tracked. Reads dataset.csv sibling file.
    format!(
        r#"# Streamlit delivery generated by Simard Cartographer.
# Run with:  streamlit run app.py
import json
import pandas as pd
import plotly.express as px
import streamlit as st

st.set_page_config(page_title="{title}", layout="wide")
st.title("{title}")
st.caption("{question}")

with open("narrative.md", "r", encoding="utf-8") as fh:
    st.markdown(fh.read())

df = pd.read_csv("dataset.csv")
CHARTS = json.loads(r'''{charts}''')

st.header("Interactive views")
for spec in CHARTS:
    st.subheader(spec["title"])
    st.caption(spec.get("rationale", ""))
    kind = spec["chart_type"]
    if kind in ("scatter", "line"):
        fn = px.scatter if kind == "scatter" else px.line
        fig = fn(df, x=spec["x"], y=spec.get("y"), color=spec.get("color"))
    elif kind == "bar":
        agg = spec.get("aggregate", "mean")
        grouped = df.groupby(spec["x"])[spec["y"]]
        series = grouped.sum() if agg == "sum" else grouped.mean()
        fig = px.bar(x=series.index, y=series.values, labels={{"x": spec["x"], "y": spec.get("y")}})
    else:
        fig = px.histogram(df, x=spec["x"])
    st.plotly_chart(fig, use_container_width=True)

st.header("Column profile")
st.dataframe(df.describe(include="all").transpose())
"#,
        title = title,
        question = question,
        charts = charts_py,
    )
}

/// Emit an Observable-flavoured notebook source (Markdown + `ojs` cells).
pub fn render_observable(brief: &StudyBrief, charts: &[ChartSpec]) -> String {
    let charts_json = serde_json::to_string(charts).unwrap_or_else(|_| "[]".to_string());
    let mut ojs = String::new();
    ojs.push_str(&format!("# {}\n\n", brief.title.trim()));
    ojs.push_str(&format!("_{}_\n\n", brief.question.trim()));
    ojs.push_str("```js\ndata = FileAttachment(\"dataset.csv\").csv({typed: true})\n```\n\n");
    ojs.push_str(&format!("```js\ncharts = {charts_json}\n```\n\n"));
    ojs.push_str(
        "```js\nimport {Plot} from \"@observablehq/plot\"\n```\n\n\
         ```js\ncharts.map(spec => {\n\
         \x20 if (spec.chart_type === \"bar\")\n\
         \x20   return Plot.plot({marks: [Plot.barY(data, {x: spec.x, y: spec.y})]});\n\
         \x20 if (spec.chart_type === \"histogram\")\n\
         \x20   return Plot.plot({marks: [Plot.rectY(data, Plot.binX({y: \"count\"}, {x: spec.x}))]});\n\
         \x20 return Plot.plot({marks: [Plot.dot(data, {x: spec.x, y: spec.y, stroke: spec.color})]});\n\
         })\n```\n",
    );
    ojs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartographer::analysis::Findings;
    use crate::cartographer::analysis::analyze;
    use crate::cartographer::brief::StudyBrief;
    use crate::cartographer::dataset::{Dataset, MAX_ROWS, parse_csv};
    use crate::cartographer::viz::design_charts;

    fn setup() -> (StudyBrief, Dataset, Findings, Vec<ChartSpec>) {
        let brief = StudyBrief::from_json_bytes(
            br#"{"title":"Study","question":"How do sales relate to income?",
                 "dataset":{"csv":"region,sales,income\nN,100,50\nS,200,90\nE,300,120\n"}}"#,
        )
        .unwrap();
        let (h, rows) = parse_csv(
            "region,sales,income\nN,100,50\nS,200,90\nE,300,120\n",
            MAX_ROWS,
        )
        .unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        let findings = analyze(&ds, &brief.question);
        let charts = design_charts(&ds, &brief.hints);
        (brief, ds, findings, charts)
    }

    #[test]
    fn html_is_self_contained_and_embeds_data() {
        let (b, d, f, charts) = setup();
        let md = crate::cartographer::narrative::render_narrative(&b, &f, &charts);
        let html = render_html(&b, &d, &charts, &md);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("cdn.plot.ly"));
        assert!(html.contains("d3@7"));
        assert!(html.contains("cartographer-data"));
        assert!(html.contains("Plotly.newPlot"));
        // Data is embedded, not fetched.
        assert!(html.contains("\"sales\""));
        // Each chart div is present.
        for c in &charts {
            assert!(html.contains(&format!("chart-{}", c.id)));
        }
    }

    #[test]
    fn html_escapes_and_neutralises_script_close() {
        let brief = StudyBrief::from_json_bytes(
            br#"{"title":"</script><b>x","question":"q?","dataset":{"csv":"a\nx</script>\n"}}"#,
        )
        .unwrap();
        let (h, rows) = parse_csv("a\nx</script>\n", MAX_ROWS).unwrap();
        let ds = Dataset::from_table(h, rows).unwrap();
        let f = analyze(&ds, &brief.question);
        let charts = design_charts(&ds, &brief.hints);
        let md = crate::cartographer::narrative::render_narrative(&brief, &f, &charts);
        let html = render_html(&brief, &ds, &charts, &md);
        assert!(
            html.contains("&lt;/script&gt;<b>x".replace("<b>", "&lt;b&gt;").as_str())
                || html.contains("&lt;/script&gt;")
        );
        // The embedded JSON must not contain a raw </script>.
        let data_block = &html[html.find("cartographer-data").unwrap()..];
        let data_block = &data_block[..data_block.find("</script>").unwrap()];
        assert!(!data_block.contains("</script"));
    }

    #[test]
    fn streamlit_source_is_generated() {
        let (b, _d, _f, charts) = setup();
        let py = render_streamlit(&b, &charts);
        assert!(py.contains("import streamlit as st"));
        assert!(py.contains("st.plotly_chart"));
        assert!(py.contains("dataset.csv"));
    }

    #[test]
    fn observable_source_is_generated() {
        let (b, _d, _f, charts) = setup();
        let ojs = render_observable(&b, &charts);
        assert!(ojs.contains("FileAttachment"));
        assert!(ojs.contains("@observablehq/plot"));
    }

    #[test]
    fn markdown_converts_headings_lists_and_bold() {
        let html = markdown_to_html("# Title\n\n## Section\n\n- **a** item\n\n1. first\n\npara\n");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<h2>Section</h2>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<strong>a</strong>"));
        assert!(html.contains("<ol>"));
        assert!(html.contains("<p>para</p>"));
    }
}
