//! Cut list and bill-of-materials computation.
//!
//! Derived deterministically from the [`Assembly`] geometry and the brief — no
//! external tools required, so this always runs even when CAD engines are
//! absent. Emits CSV for both artifacts and rolls up a material cost estimate.

use std::fmt::Write as _;

use super::brief::ProductBrief;
use super::geometry::Assembly;

/// One cut-list row: an identical panel and how many to cut.
#[derive(Debug, Clone, PartialEq)]
pub struct CutListRow {
    pub part: String,
    pub qty: u32,
    pub length_mm: f64,
    pub width_mm: f64,
    pub thickness_mm: f64,
    pub material: String,
    pub grain: &'static str,
}

/// The full cut list plus a sheet-stock estimate.
#[derive(Debug, Clone)]
pub struct CutList {
    pub rows: Vec<CutListRow>,
    /// Total in-plane panel area across all parts, mm².
    pub total_area_mm2: f64,
    /// Estimated full sheets required (area-based, with a waste allowance).
    pub sheets_required: u32,
    /// The largest single part footprint, for stock-fit verification.
    pub largest_part_mm: (f64, f64),
}

/// Waste allowance applied to the area-based sheet estimate (kerf + offcuts).
const WASTE_FACTOR: f64 = 1.20;

/// Build the cut list for an assembly.
pub fn build_cut_list(brief: &ProductBrief, assembly: &Assembly) -> CutList {
    let mut rows = Vec::new();
    let mut total_area = 0.0;
    let mut largest = (0.0_f64, 0.0_f64);

    for panel in &assembly.panels {
        let qty = panel.qty();
        if qty == 0 {
            continue;
        }
        total_area += panel.face_area_mm2() * qty as f64;
        if panel.face_area_mm2() > largest.0 * largest.1 {
            largest = (panel.length_mm, panel.width_mm);
        }
        rows.push(CutListRow {
            part: panel.label.clone(),
            qty,
            length_mm: panel.length_mm,
            width_mm: panel.width_mm,
            thickness_mm: panel.thickness_mm,
            material: panel.material.clone(),
            grain: panel.grain.label(),
        });
    }

    let sheet = brief.material.sheet();
    let sheet_area = sheet.length * sheet.width;
    let sheets_required = if sheet_area > 0.0 {
        ((total_area * WASTE_FACTOR) / sheet_area).ceil().max(1.0) as u32
    } else {
        1
    };

    CutList {
        rows,
        total_area_mm2: total_area,
        sheets_required,
        largest_part_mm: largest,
    }
}

impl CutList {
    /// Render the cut list as CSV.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("part,qty,length_mm,width_mm,thickness_mm,material,grain\n");
        for r in &self.rows {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{}",
                csv_field(&r.part),
                r.qty,
                trim(r.length_mm),
                trim(r.width_mm),
                trim(r.thickness_mm),
                csv_field(&r.material),
                r.grain,
            );
        }
        out
    }
}

/// One bill-of-materials line.
#[derive(Debug, Clone, PartialEq)]
pub struct BomRow {
    pub item: String,
    pub category: String,
    pub qty: f64,
    pub unit: String,
    pub unit_cost: Option<f64>,
}

impl BomRow {
    pub fn total_cost(&self) -> Option<f64> {
        self.unit_cost.map(|c| c * self.qty)
    }
}

/// A full bill of materials with a rolled-up cost.
#[derive(Debug, Clone)]
pub struct Bom {
    pub rows: Vec<BomRow>,
    /// Sum of known line totals (lines without a unit cost are excluded).
    pub total_cost: Option<f64>,
    /// True when a budget was supplied and the known total exceeds it.
    pub over_budget: bool,
}

/// Estimate joints as a heuristic: each additional placed part introduces a
/// connection, and each connection takes two fasteners.
fn estimate_fasteners(assembly: &Assembly) -> u32 {
    let instances = assembly.instance_count();
    if instances <= 1 {
        return 0;
    }
    (instances - 1) * 2
}

/// Build the bill of materials for an assembly + cut list.
pub fn build_bom(brief: &ProductBrief, assembly: &Assembly, cut_list: &CutList) -> Bom {
    let mut rows = Vec::new();

    // Sheet material.
    rows.push(BomRow {
        item: format!("{} sheet", brief.material.name),
        category: "material".into(),
        qty: cut_list.sheets_required as f64,
        unit: "sheet".into(),
        unit_cost: brief.material.cost_per_sheet,
    });

    // Hardware: brief-supplied lines, or a heuristic fastener line otherwise.
    if brief.hardware.is_empty() {
        let fasteners = estimate_fasteners(assembly);
        if fasteners > 0 {
            rows.push(BomRow {
                item: "Wood screw (assembly fasteners)".into(),
                category: "hardware".into(),
                qty: fasteners as f64,
                unit: "each".into(),
                unit_cost: Some(0.10),
            });
        }
    } else {
        for hw in &brief.hardware {
            rows.push(BomRow {
                item: hw.name.clone(),
                category: "hardware".into(),
                qty: hw.qty as f64,
                unit: "each".into(),
                unit_cost: hw.unit_cost,
            });
        }
    }

    // Finish.
    if let Some(finish) = &brief.finish {
        rows.push(BomRow {
            item: finish.clone(),
            category: "finish".into(),
            qty: 1.0,
            unit: "lot".into(),
            unit_cost: None,
        });
    }

    let mut total = 0.0;
    let mut any_cost = false;
    for r in &rows {
        if let Some(t) = r.total_cost() {
            total += t;
            any_cost = true;
        }
    }
    let total_cost = if any_cost { Some(total) } else { None };
    let over_budget = match (brief.budget, total_cost) {
        (Some(budget), Some(t)) => t > budget,
        _ => false,
    };

    Bom {
        rows,
        total_cost,
        over_budget,
    }
}

impl Bom {
    /// Render the BOM as CSV.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("item,category,qty,unit,unit_cost,total_cost\n");
        for r in &self.rows {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{}",
                csv_field(&r.item),
                csv_field(&r.category),
                trim(r.qty),
                csv_field(&r.unit),
                r.unit_cost.map(trim).unwrap_or_default(),
                r.total_cost().map(trim).unwrap_or_default(),
            );
        }
        out
    }
}

fn trim(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Minimal CSV escaping with spreadsheet-formula-injection defence.
///
/// Fields containing structural CSV characters are RFC-4180 quoted. In
/// addition, any field whose first character is a spreadsheet formula trigger
/// (`=`, `+`, `-`, `@`, tab, or carriage return) is prefixed with a single
/// quote so that Excel/LibreOffice treat it as literal text rather than a
/// formula — the material/hardware/finish names originate from an untrusted
/// brief and flow into these deliverable CSVs.
fn csv_field(s: &str) -> String {
    let needs_formula_guard = s
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
    let guarded = if needs_formula_guard {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if guarded.contains([',', '"', '\n']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductBrief;
    use crate::atelier::geometry::generate;

    fn brief(extra: &str) -> ProductBrief {
        let json = format!(
            r#"{{"name":"Bookcase","kind":"bookcase",
                "dimensions_mm":{{"width":800,"depth":300,"height":1000}},
                "material":{{"name":"Birch ply","thickness_mm":18,"grain":true,"cost_per_sheet":55}},
                "parameters":{{"shelves":2,"back_panel":true}}{extra}}}"#
        );
        ProductBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn cut_list_covers_every_panel() {
        let b = brief("");
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        assert_eq!(cl.rows.len(), a.panels.len());
        assert!(cl.total_area_mm2 > 0.0);
        assert!(cl.sheets_required >= 1);
        let csv = cl.to_csv();
        assert!(csv.starts_with("part,qty,length_mm"));
        assert_eq!(csv.lines().count(), a.panels.len() + 1);
    }

    #[test]
    fn bom_includes_material_and_hardware_and_finish() {
        let b = brief(
            r#","hardware":[{"name":"Confirmat","qty":24,"unit_cost":0.15}],"finish":"lacquer""#,
        );
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        let bom = build_bom(&b, &a, &cl);
        assert!(bom.rows.iter().any(|r| r.category == "material"));
        assert!(bom.rows.iter().any(|r| r.item == "Confirmat"));
        assert!(bom.rows.iter().any(|r| r.category == "finish"));
        assert!(bom.total_cost.unwrap() > 0.0);
    }

    #[test]
    fn bom_adds_heuristic_fasteners_when_no_hardware() {
        let b = brief("");
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        let bom = build_bom(&b, &a, &cl);
        let screws = bom.rows.iter().find(|r| r.category == "hardware").unwrap();
        assert!(screws.qty > 0.0);
    }

    #[test]
    fn over_budget_flag_trips() {
        let b = brief(r#","budget":1.0"#);
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        let bom = build_bom(&b, &a, &cl);
        assert!(bom.over_budget);
    }

    #[test]
    fn under_budget_does_not_trip() {
        let b = brief(r#","budget":100000.0"#);
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        let bom = build_bom(&b, &a, &cl);
        assert!(!bom.over_budget);
    }

    #[test]
    fn csv_field_escapes_commas() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_field_neutralizes_formula_injection() {
        // Leading formula triggers are prefixed with a quote so spreadsheets
        // treat them as text rather than executing them. No comma/quote/newline
        // means no RFC-4180 wrapping is added.
        assert_eq!(csv_field("=cmd|'/c calc'!A1"), "'=cmd|'/c calc'!A1");
        assert_eq!(csv_field("+1+1"), "'+1+1");
        assert_eq!(csv_field("-2"), "'-2");
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("\tval"), "'\tval");
        // Formula trigger AND a comma → prefixed then RFC-4180 quoted.
        assert_eq!(csv_field("=a,b"), "\"'=a,b\"");
        // A hyphen mid-string (not leading) is untouched.
        assert_eq!(csv_field("A-B"), "A-B");
    }

    #[test]
    fn malicious_material_name_cannot_inject_formula_into_csv() {
        let json = r#"{"name":"Bookcase","kind":"bookcase",
            "dimensions_mm":{"width":800,"depth":300,"height":1000},
            "material":{"name":"=1+1","thickness_mm":18,"grain":true},
            "parameters":{"shelves":2,"back_panel":true},"finish":"@evil"}"#;
        let b = ProductBrief::from_json_bytes(json.as_bytes()).unwrap();
        let a = generate(&b);
        let cl = build_cut_list(&b, &a);
        let csv = cl.to_csv();
        assert!(!csv.lines().any(|l| l.contains(",=1+1")));
        assert!(csv.contains("'=1+1"));
        let bom = build_bom(&b, &a, &cl);
        let bcsv = bom.to_csv();
        assert!(bcsv.contains("'@evil"));
        assert!(!bcsv.lines().any(|l| l.split(',').any(|f| f == "@evil")));
    }
}
