//! Fabrication planning: turn a [`ProductBrief`] into a deterministic **cut
//! list** (every part to cut, with dimensions and material) and a **bill of
//! materials** (sheet stock, hardware, glue, finish).
//!
//! All geometry here is a pure function of the brief, so the same brief always
//! yields the same cut list and BOM. Units are millimetres for parts and are
//! aggregated into square-metres / litres / counts for the BOM.

use serde::Serialize;

use super::brief::{ProductBrief, ProductKind};

/// One part to cut from stock. `length_mm` >= `width_mm` by convention.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    /// Human label, e.g. `"side"`, `"leg"`, `"shelf"`.
    pub name: String,
    /// How many identical copies of this part.
    pub count: u32,
    /// Longer in-plane dimension in millimetres.
    pub length_mm: f64,
    /// Shorter in-plane dimension in millimetres.
    pub width_mm: f64,
    /// Stock thickness in millimetres.
    pub thickness_mm: f64,
    /// Material label the part is cut from.
    pub material: String,
}

impl Part {
    fn new(
        name: &str,
        count: u32,
        a_mm: f64,
        b_mm: f64,
        thickness_mm: f64,
        material: &str,
    ) -> Self {
        let (length_mm, width_mm) = if a_mm >= b_mm {
            (a_mm, b_mm)
        } else {
            (b_mm, a_mm)
        };
        Self {
            name: name.to_string(),
            count,
            length_mm,
            width_mm,
            thickness_mm,
            material: material.to_string(),
        }
    }

    /// Face area (single face) of all copies of this part, in square metres.
    pub fn face_area_m2(&self) -> f64 {
        (self.length_mm * self.width_mm) / 1_000_000.0 * f64::from(self.count)
    }
}

/// The complete list of parts for a single finished unit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CutList {
    pub product: String,
    pub parts: Vec<Part>,
}

impl CutList {
    /// Total number of physical parts (summing `count`).
    pub fn total_parts(&self) -> u32 {
        self.parts.iter().map(|p| p.count).sum()
    }

    /// Total single-face area across every part, in square metres.
    pub fn total_face_area_m2(&self) -> f64 {
        self.parts.iter().map(Part::face_area_m2).sum()
    }

    /// Render the cut list as CSV (with a header row). Dimensions are rounded to
    /// one decimal millimetre.
    pub fn to_csv(&self) -> String {
        let mut out = String::from("part,count,length_mm,width_mm,thickness_mm,material\n");
        for p in &self.parts {
            out.push_str(&format!(
                "{},{},{:.1},{:.1},{:.1},{}\n",
                csv_escape(&p.name),
                p.count,
                p.length_mm,
                p.width_mm,
                p.thickness_mm,
                csv_escape(&p.material),
            ));
        }
        out
    }
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// One line of the bill of materials.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BomLine {
    pub item: String,
    /// Quantity in `unit`. Rounded up to a sensible purchasing quantity for
    /// discrete goods.
    pub quantity: f64,
    pub unit: String,
    pub note: String,
}

/// Bill of materials for the whole build (already multiplied by
/// `brief.quantity`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BillOfMaterials {
    pub product: String,
    pub units: u32,
    pub lines: Vec<BomLine>,
}

/// The linear cross-section (mm) of a table leg, derived from the panel
/// thickness but never thinner than a stable minimum so legs are buildable.
fn leg_section_mm(panel_thickness_mm: f64) -> f64 {
    (panel_thickness_mm * 3.0).max(40.0)
}

/// Height of a table/box apron (rail) in mm.
const APRON_HEIGHT_MM: f64 = 80.0;

/// Thickness of a thin back panel, capped by the carcass thickness.
fn back_thickness_mm(panel_thickness_mm: f64) -> f64 {
    panel_thickness_mm.min(6.0)
}

/// Compute the cut list for a single finished unit of the brief.
pub fn cut_list(brief: &ProductBrief) -> CutList {
    let t = brief.panel_thickness_mm;
    let m = brief.material.as_str();
    let w = brief.width_mm;
    let d = brief.depth_mm;
    let h = brief.height_mm;
    let back_t = back_thickness_mm(t);

    let parts = match brief.kind {
        ProductKind::Table => {
            let ls = leg_section_mm(t);
            let leg_len = (h - t).max(t);
            vec![
                Part::new("top", 1, w, d, t, m),
                Part::new("leg", 4, leg_len, ls, ls, m),
                // Long aprons span the width between legs; short aprons the depth.
                Part::new(
                    "apron-long",
                    2,
                    (w - 2.0 * ls).max(0.0),
                    APRON_HEIGHT_MM,
                    t,
                    m,
                ),
                Part::new(
                    "apron-short",
                    2,
                    (d - 2.0 * ls).max(0.0),
                    APRON_HEIGHT_MM,
                    t,
                    m,
                ),
            ]
        }
        ProductKind::Shelf => {
            let inner_w = (w - 2.0 * t).max(0.0);
            let shelf_depth = (d - back_t).max(0.0);
            let mut parts = vec![
                Part::new("side", 2, h, d, t, m),
                Part::new("top", 1, inner_w, d, t, m),
                Part::new("bottom", 1, inner_w, d, t, m),
                Part::new("back", 1, w, h, back_t, m),
            ];
            if brief.shelves > 0 {
                parts.push(Part::new(
                    "shelf",
                    brief.shelves,
                    inner_w,
                    shelf_depth,
                    t,
                    m,
                ));
            }
            parts
        }
        ProductKind::Box => {
            let inner_w = (w - 2.0 * t).max(0.0);
            vec![
                Part::new("side", 2, h, d, t, m),
                Part::new("top", 1, inner_w, d, t, m),
                Part::new("bottom", 1, inner_w, d, t, m),
                Part::new("back", 1, w, h, back_t, m),
            ]
        }
    };

    CutList {
        product: brief.name.clone(),
        parts,
    }
}

/// Compute the bill of materials for the whole build.
pub fn bill_of_materials(brief: &ProductBrief) -> BillOfMaterials {
    let list = cut_list(brief);
    let units = f64::from(brief.quantity);

    // Sheet stock: single-face area of every panel, plus a 15% offcut waste
    // allowance, rounded to a whole number of sheets is out of scope — we sell
    // area. Keep the two faces separate: joinery/finish care about both faces,
    // but stock is bought by area, so use single-face area with waste.
    let sheet_area_m2 = list.total_face_area_m2() * 1.15 * units;

    // Hardware: every part is fastened at its joints. Use a per-part fastener
    // budget that scales with part count — deterministic and easy to reason
    // about on the shop floor.
    let total_parts = f64::from(list.total_parts());
    let screws = (total_parts * 8.0 * units).ceil();
    let dowels = (total_parts * 4.0 * units).ceil();

    // Glue: ~25 ml per square metre of joinery face area.
    let glue_ml = (list.total_face_area_m2() * 25.0 * units).ceil();

    let mut lines = vec![
        BomLine {
            item: format!("{} sheet stock", brief.material),
            quantity: round1(sheet_area_m2),
            unit: "m2".into(),
            note: "single-face area incl. 15% offcut waste".into(),
        },
        BomLine {
            item: "wood screws".into(),
            quantity: screws,
            unit: "pcs".into(),
            note: "8 per part".into(),
        },
        BomLine {
            item: "dowels".into(),
            quantity: dowels,
            unit: "pcs".into(),
            note: "4 per part".into(),
        },
        BomLine {
            item: "wood glue".into(),
            quantity: round1(glue_ml / 1000.0),
            unit: "L".into(),
            note: "~25 ml per m2 of joinery".into(),
        },
    ];

    // Finish: both faces, two coats, ~8 m2 per litre per coat.
    if !brief.finish.eq_ignore_ascii_case("none") {
        let surface_m2 = list.total_face_area_m2() * 2.0 * units;
        let finish_l = surface_m2 * 2.0 / 8.0;
        lines.push(BomLine {
            item: format!("{} finish", brief.finish),
            quantity: round1(finish_l),
            unit: "L".into(),
            note: "both faces, 2 coats @ 8 m2/L".into(),
        });
    }

    BillOfMaterials {
        product: brief.name.clone(),
        units: brief.quantity,
        lines,
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> ProductBrief {
        ProductBrief {
            name: "Desk".into(),
            kind: ProductKind::Table,
            width_mm: 1200.0,
            depth_mm: 600.0,
            height_mm: 740.0,
            panel_thickness_mm: 18.0,
            material: "oak".into(),
            shelves: 0,
            quantity: 1,
            finish: "oil".into(),
        }
    }

    fn shelf() -> ProductBrief {
        ProductBrief {
            name: "Bookcase".into(),
            kind: ProductKind::Shelf,
            width_mm: 800.0,
            depth_mm: 300.0,
            height_mm: 1800.0,
            panel_thickness_mm: 18.0,
            material: "birch-plywood".into(),
            shelves: 4,
            quantity: 1,
            finish: "lacquer".into(),
        }
    }

    #[test]
    fn table_cut_list_has_top_legs_and_aprons() {
        let list = cut_list(&table());
        let names: Vec<&str> = list.parts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"top"));
        assert!(names.contains(&"leg"));
        assert!(names.contains(&"apron-long"));
        assert!(names.contains(&"apron-short"));
        // 1 top + 4 legs + 2 + 2 aprons = 9 physical parts.
        assert_eq!(list.total_parts(), 9);
    }

    #[test]
    fn table_top_dimensions_match_outer_box() {
        let list = cut_list(&table());
        let top = list.parts.iter().find(|p| p.name == "top").unwrap();
        assert_eq!(top.length_mm, 1200.0);
        assert_eq!(top.width_mm, 600.0);
        assert_eq!(top.thickness_mm, 18.0);
    }

    #[test]
    fn part_orders_length_ge_width() {
        // depth > width still yields length >= width.
        let p = Part::new("x", 1, 300.0, 1200.0, 18.0, "oak");
        assert!(p.length_mm >= p.width_mm);
        assert_eq!(p.length_mm, 1200.0);
    }

    #[test]
    fn shelf_cut_list_includes_interior_shelves() {
        let list = cut_list(&shelf());
        let shelf_part = list.parts.iter().find(|p| p.name == "shelf").unwrap();
        assert_eq!(shelf_part.count, 4);
        // sides(2) + top + bottom + back + shelves(4) = 9
        assert_eq!(list.total_parts(), 9);
    }

    #[test]
    fn shelf_with_zero_shelves_omits_shelf_part() {
        let mut b = shelf();
        b.shelves = 0;
        let list = cut_list(&b);
        assert!(list.parts.iter().all(|p| p.name != "shelf"));
    }

    #[test]
    fn box_is_five_panels() {
        let mut b = shelf();
        b.kind = ProductKind::Box;
        let list = cut_list(&b);
        // sides(2)+top+bottom+back = 5
        assert_eq!(list.total_parts(), 5);
    }

    #[test]
    fn back_panel_is_thin() {
        let list = cut_list(&shelf());
        let back = list.parts.iter().find(|p| p.name == "back").unwrap();
        assert_eq!(back.thickness_mm, 6.0);
    }

    #[test]
    fn csv_has_header_and_row_per_part() {
        let list = cut_list(&table());
        let csv = list.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "part,count,length_mm,width_mm,thickness_mm,material"
        );
        assert_eq!(lines.len(), 1 + list.parts.len());
    }

    #[test]
    fn bom_includes_sheet_hardware_glue_and_finish() {
        let bom = bill_of_materials(&table());
        let items: Vec<&str> = bom.lines.iter().map(|l| l.item.as_str()).collect();
        assert!(items.iter().any(|i| i.contains("sheet stock")));
        assert!(items.contains(&"wood screws"));
        assert!(items.contains(&"dowels"));
        assert!(items.contains(&"wood glue"));
        assert!(items.iter().any(|i| i.contains("finish")));
    }

    #[test]
    fn bom_omits_finish_when_none() {
        let mut b = table();
        b.finish = "none".into();
        let bom = bill_of_materials(&b);
        assert!(bom.lines.iter().all(|l| !l.item.contains("finish")));
    }

    #[test]
    fn bom_scales_with_quantity() {
        let mut b = table();
        b.quantity = 1;
        let one = bill_of_materials(&b);
        b.quantity = 3;
        let three = bill_of_materials(&b);
        let screws_one = one
            .lines
            .iter()
            .find(|l| l.item == "wood screws")
            .unwrap()
            .quantity;
        let screws_three = three
            .lines
            .iter()
            .find(|l| l.item == "wood screws")
            .unwrap()
            .quantity;
        assert_eq!(screws_three, screws_one * 3.0);
    }

    #[test]
    fn cut_list_is_deterministic() {
        assert_eq!(cut_list(&table()), cut_list(&table()));
        assert_eq!(bill_of_materials(&shelf()), bill_of_materials(&shelf()));
    }
}
