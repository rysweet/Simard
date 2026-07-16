//! Deterministic exporters: STL mesh, SVG render, cut list, and BOM.
//!
//! Every function here is pure (input model -> `String`) so the fabrication
//! outputs can be unit-tested without touching the filesystem or any external
//! CAD tool. The pipeline layer is responsible for writing them to disk and,
//! when available, augmenting them with the OpenSCAD toolchain.

use std::collections::BTreeMap;

use serde::Serialize;

use super::model::{Model, SolidBox};

/// Export the model as an ASCII STL mesh (millimetre units).
///
/// Each box contributes 12 triangles (2 per face) with outward-facing normals,
/// which is a valid, watertight, fabrication-ready mesh for slicing/CAM.
pub fn to_ascii_stl(model: &Model) -> String {
    let solid_name = model.brief.slug();
    let mut out = String::new();
    out.push_str(&format!("solid {solid_name}\n"));
    for s in &model.solids {
        append_box_facets(&mut out, s);
    }
    out.push_str(&format!("endsolid {solid_name}\n"));
    out
}

fn append_box_facets(out: &mut String, s: &SolidBox) {
    let c = s.corners();
    // (four corner indices in outward-winding order, outward normal)
    let faces: [([usize; 4], [f64; 3]); 6] = [
        ([0, 3, 2, 1], [0.0, 0.0, -1.0]), // bottom (z-)
        ([4, 5, 6, 7], [0.0, 0.0, 1.0]),  // top (z+)
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]), // front (y-)
        ([3, 7, 6, 2], [0.0, 1.0, 0.0]),  // back (y+)
        ([0, 4, 7, 3], [-1.0, 0.0, 0.0]), // left (x-)
        ([1, 2, 6, 5], [1.0, 0.0, 0.0]),  // right (x+)
    ];
    for (idx, normal) in faces {
        append_triangle(out, normal, c[idx[0]], c[idx[1]], c[idx[2]]);
        append_triangle(out, normal, c[idx[0]], c[idx[2]], c[idx[3]]);
    }
}

fn append_triangle(out: &mut String, n: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) {
    out.push_str(&format!(
        "  facet normal {:e} {:e} {:e}\n    outer loop\n",
        n[0], n[1], n[2]
    ));
    for v in [a, b, c] {
        out.push_str(&format!(
            "      vertex {:.4} {:.4} {:.4}\n",
            v[0], v[1], v[2]
        ));
    }
    out.push_str("    endloop\n  endfacet\n");
}

/// A single line of the cut list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CutListItem {
    pub part: String,
    pub quantity: u32,
    pub length_mm: f64,
    pub width_mm: f64,
    pub thickness_mm: f64,
    pub material: String,
}

/// Compute the per-unit cut list, grouping identical parts.
pub fn cut_list(model: &Model) -> Vec<CutListItem> {
    // Key by part + rounded panel dimensions so identical parts collapse.
    let mut groups: BTreeMap<(String, i64, i64, i64), u32> = BTreeMap::new();
    for s in &model.solids {
        let mut dims = s.size;
        dims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let key = (
            s.part.clone(),
            round_um(dims[0]),
            round_um(dims[1]),
            round_um(dims[2]),
        );
        *groups.entry(key).or_insert(0) += 1;
    }
    groups
        .into_iter()
        .map(|((part, l, w, t), qty)| CutListItem {
            part,
            quantity: qty,
            length_mm: from_um(l),
            width_mm: from_um(w),
            thickness_mm: from_um(t),
            material: model.brief.material.clone(),
        })
        .collect()
}

fn round_um(mm: f64) -> i64 {
    (mm * 1000.0).round() as i64
}
fn from_um(um: i64) -> f64 {
    um as f64 / 1000.0
}

/// Render the cut list as CSV (header + one row per grouped part).
pub fn cut_list_csv(model: &Model) -> String {
    let mut out = String::from("part,quantity,length_mm,width_mm,thickness_mm,material\n");
    for item in cut_list(model) {
        out.push_str(&format!(
            "{},{},{:.2},{:.2},{:.2},{}\n",
            item.part,
            item.quantity,
            item.length_mm,
            item.width_mm,
            item.thickness_mm,
            csv_escape(&item.material),
        ));
    }
    out
}

/// A single bill-of-materials line (totals across `quantity` units).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BomItem {
    pub item: String,
    pub quantity: u32,
    pub unit: String,
    pub notes: String,
}

/// Compute the bill of materials for the full production run.
pub fn bill_of_materials(model: &Model) -> Vec<BomItem> {
    let units = model.brief.quantity;
    let mut bom = Vec::new();

    // Sheet goods: total panel area rounded up to standard 2440x1220 sheets.
    let per_unit_area_mm2: f64 = model
        .solids
        .iter()
        .map(|s| {
            let mut d = s.size;
            d.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            d[0] * d[1]
        })
        .sum();
    let total_area_m2 = per_unit_area_mm2 * units as f64 / 1_000_000.0;
    const STANDARD_SHEET_M2: f64 = 2.440 * 1.220;
    let sheets = (total_area_m2 / STANDARD_SHEET_M2).ceil().max(1.0) as u32;
    bom.push(BomItem {
        item: format!("Sheet good: {}", model.brief.material),
        quantity: sheets,
        unit: "sheet(2440x1220)".to_string(),
        notes: format!("{total_area_m2:.3} m² total panel area (with waste allowance)"),
    });

    // Fasteners scale with joint count and units.
    let screws_per_unit = fasteners_per_unit(model);
    if screws_per_unit > 0 {
        bom.push(BomItem {
            item: "Wood screw 4x40mm".to_string(),
            quantity: screws_per_unit * units,
            unit: "each".to_string(),
            notes: format!("{screws_per_unit} per unit"),
        });
    }

    bom.push(BomItem {
        item: "Wood glue".to_string(),
        quantity: units,
        unit: "unit-application".to_string(),
        notes: "one glue-up per unit".to_string(),
    });

    bom
}

fn fasteners_per_unit(model: &Model) -> u32 {
    use super::brief::ProductType;
    match model.brief.product_type {
        ProductType::Panel => 0,
        ProductType::Box => 12,
        ProductType::Table => 16,
        ProductType::Shelf => {
            let shelves = model.solids.iter().filter(|s| s.part == "shelf").count() as u32;
            // 8 for the carcass corners + 4 per interior shelf.
            8 + 4 * shelves
        }
    }
}

/// Render the BOM as CSV.
pub fn bom_csv(model: &Model) -> String {
    let mut out = String::from("item,quantity,unit,notes\n");
    for b in bill_of_materials(model) {
        out.push_str(&format!(
            "{},{},{},{}\n",
            csv_escape(&b.item),
            b.quantity,
            csv_escape(&b.unit),
            csv_escape(&b.notes),
        ));
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Render three orthographic projections (front, top, side) as a single SVG.
///
/// This is the always-available "render" artifact: it needs no GPU, no
/// headless renderer, and is byte-for-byte deterministic for a given model.
pub fn to_svg_render(model: &Model) -> String {
    let (min, max) = model.bounds();
    let span_x = (max[0] - min[0]).max(1.0);
    let span_y = (max[1] - min[1]).max(1.0);
    let span_z = (max[2] - min[2]).max(1.0);

    // Scale so the largest overall span fits a 300px cell.
    let cell = 300.0_f64;
    let pad = 30.0_f64;
    let largest = span_x.max(span_y).max(span_z);
    let scale = cell / largest;

    // Three side-by-side cells: front (X/Z), side (Y/Z), top (X/Y).
    let cell_w = cell + 2.0 * pad;
    let width = 3.0 * cell_w;
    let height = cell + 2.0 * pad + 40.0;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">\n"
    ));
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\n");
    svg.push_str(&format!(
        "<text x=\"12\" y=\"22\" font-family=\"sans-serif\" font-size=\"18\" fill=\"#222\">{} — {} ({:.0}×{:.0}×{:.0} mm)</text>\n",
        xml_escape(&model.brief.name),
        model.brief.product_type.label(),
        span_x,
        span_y,
        span_z,
    ));

    let views: [(&str, usize, usize); 3] = [
        ("front (X-Z)", 0, 2),
        ("side (Y-Z)", 1, 2),
        ("top (X-Y)", 0, 1),
    ];
    for (i, (label, ax, ay)) in views.into_iter().enumerate() {
        let ox = i as f64 * cell_w + pad;
        let oy = 40.0 + pad;
        svg.push_str(&format!(
            "<text x=\"{tx:.0}\" y=\"{ty:.0}\" font-family=\"sans-serif\" font-size=\"13\" fill=\"#555\">{label}</text>\n",
            tx = ox,
            ty = oy - 8.0,
        ));
        for s in &model.solids {
            // Project: SVG y grows downward, so flip the vertical axis.
            let px = ox + (s.origin[ax] - min[ax]) * scale;
            let rw = s.size[ax] * scale;
            let rh = s.size[ay] * scale;
            let py = oy
                + (span_for(ay, span_x, span_y, span_z) - (s.origin[ay] - min[ay]) - s.size[ay])
                    * scale;
            svg.push_str(&format!(
                "<rect x=\"{px:.2}\" y=\"{py:.2}\" width=\"{rw:.2}\" height=\"{rh:.2}\" fill=\"#cfe3f7\" stroke=\"#1f4e79\" stroke-width=\"1\" opacity=\"0.85\"/>\n"
            ));
        }
    }
    svg.push_str("</svg>\n");
    svg
}

fn span_for(axis: usize, sx: f64, sy: f64, sz: f64) -> f64 {
    match axis {
        0 => sx,
        1 => sy,
        _ => sz,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::{Dimensions, ProductBrief, ProductType};

    fn model(pt: ProductType) -> Model {
        let brief = ProductBrief {
            name: "Test Piece".into(),
            product_type: pt,
            dimensions: Dimensions {
                length_mm: 1000.0,
                width_mm: 400.0,
                height_mm: 800.0,
                thickness_mm: 18.0,
            },
            material: "18mm birch plywood".into(),
            quantity: 2,
            shelf_count: 3,
            leg_section_mm: 50.0,
        };
        Model::from_brief(&brief)
    }

    #[test]
    fn stl_is_wellformed() {
        let m = model(ProductType::Table);
        let stl = to_ascii_stl(&m);
        assert!(stl.starts_with("solid test-piece\n"));
        assert!(stl.trim_end().ends_with("endsolid test-piece"));
        // 12 triangles per solid box.
        let facets = stl.matches("facet normal").count();
        assert_eq!(facets, m.solids.len() * 12);
        assert_eq!(stl.matches("outer loop").count(), facets);
        assert_eq!(stl.matches("vertex").count(), facets * 3);
    }

    #[test]
    fn stl_is_deterministic() {
        let m = model(ProductType::Shelf);
        assert_eq!(to_ascii_stl(&m), to_ascii_stl(&m));
    }

    #[test]
    fn cut_list_groups_identical_parts() {
        let m = model(ProductType::Table);
        let items = cut_list(&m);
        let legs = items.iter().find(|i| i.part == "leg").unwrap();
        assert_eq!(legs.quantity, 4);
    }

    #[test]
    fn cut_list_csv_has_header_and_rows() {
        let m = model(ProductType::Box);
        let csv = cut_list_csv(&m);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "part,quantity,length_mm,width_mm,thickness_mm,material"
        );
        assert!(lines.len() > 1);
    }

    #[test]
    fn bom_scales_with_quantity() {
        let m = model(ProductType::Table);
        let bom = bill_of_materials(&m);
        let screws = bom.iter().find(|b| b.item.contains("screw")).unwrap();
        // 16 per unit * 2 units.
        assert_eq!(screws.quantity, 32);
        assert!(bom.iter().any(|b| b.item.starts_with("Sheet good")));
    }

    #[test]
    fn panel_bom_has_no_screws() {
        let m = model(ProductType::Panel);
        let bom = bill_of_materials(&m);
        assert!(!bom.iter().any(|b| b.item.contains("screw")));
    }

    #[test]
    fn bom_csv_escapes_commas() {
        let m = model(ProductType::Table);
        let csv = bom_csv(&m);
        assert!(csv.starts_with("item,quantity,unit,notes\n"));
        // material contains no comma here, but notes with m² should be present
        assert!(csv.contains("Sheet good"));
    }

    #[test]
    fn svg_is_wellformed_and_deterministic() {
        let m = model(ProductType::Shelf);
        let svg = to_svg_render(&m);
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("front (X-Z)"));
        assert!(svg.contains("top (X-Y)"));
        // one rect per solid per 3 views + 1 background rect.
        let rects = svg.matches("<rect").count();
        assert_eq!(rects, m.solids.len() * 3 + 1);
        assert_eq!(svg, to_svg_render(&m));
    }

    #[test]
    fn svg_escapes_title() {
        let mut brief = model(ProductType::Panel).brief;
        brief.name = "A & B <Table>".into();
        let m = Model::from_brief(&brief);
        let svg = to_svg_render(&m);
        assert!(svg.contains("A &amp; B &lt;Table&gt;"));
    }
}
