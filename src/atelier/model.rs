//! Parametric CAD source generation. Turns a [`ProductBrief`] into an
//! [OpenSCAD](https://openscad.org) program whose top-level parameters mirror
//! the brief, so a designer can tweak the model directly and re-export.
//!
//! OpenSCAD is used as the portable, text-first parametric kernel. The same
//! `.scad` file feeds STL export and PNG rendering in [`super::pipeline`], and
//! can be opened in FreeCAD for a STEP export.

use super::brief::{ProductBrief, ProductKind};
use super::fabrication::cut_list;

/// Generate an OpenSCAD program for the brief. The output is deterministic and
/// self-contained (no external `include`/`use`).
pub fn generate_openscad(brief: &ProductBrief) -> String {
    let mut s = String::new();
    s.push_str(&header(brief));
    s.push_str(&parameters(brief));
    s.push_str(PANEL_MODULE);
    match brief.kind {
        ProductKind::Table => s.push_str(TABLE_BODY),
        ProductKind::Shelf => s.push_str(SHELF_BODY),
        ProductKind::Box => s.push_str(BOX_BODY),
    }
    s
}

fn header(brief: &ProductBrief) -> String {
    format!(
        "// Simard Atelier — parametric model\n\
         // product: {}\n\
         // kind:    {}\n\
         // material:{}\n\
         // Generated from a product brief; edit parameters below and re-render.\n\n",
        comment_safe(&brief.name),
        brief.kind.label(),
        comment_safe(&brief.material),
    )
}

/// Flatten a brief-supplied string into a single-line, comment-safe token.
/// Newlines or control characters in an (untrusted) brief must never let the
/// text escape its `//` comment and inject arbitrary OpenSCAD code.
fn comment_safe(s: &str) -> String {
    let flattened: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "(unspecified)".to_string()
    } else {
        collapsed
    }
}

fn parameters(brief: &ProductBrief) -> String {
    // Emit the brief as first-class OpenSCAD variables so the model is truly
    // parametric. `shelves` is only consumed by the shelf body.
    format!(
        "width      = {w};   // outer X (mm)\n\
         depth      = {d};   // outer Y (mm)\n\
         height     = {h};   // outer Z (mm)\n\
         thickness  = {t};   // panel thickness (mm)\n\
         back_thick = {bt};  // back panel thickness (mm)\n\
         leg_section= {ls};  // table leg cross-section (mm)\n\
         shelf_count= {sc};  // interior shelves\n\
         $fn = 24;\n\n",
        w = fmt_num(brief.width_mm),
        d = fmt_num(brief.depth_mm),
        h = fmt_num(brief.height_mm),
        t = fmt_num(brief.panel_thickness_mm),
        bt = fmt_num(brief.panel_thickness_mm.min(6.0)),
        ls = fmt_num((brief.panel_thickness_mm * 3.0).max(40.0)),
        sc = brief.shelves,
    )
}

/// Format a float without a trailing `.0` noise for whole numbers, keeping the
/// generated source clean and stable.
fn fmt_num(v: f64) -> String {
    if (v.fract()).abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v:.3}")
    }
}

const PANEL_MODULE: &str = "\
// A rectangular panel placed with its near-bottom-left corner at [x,y,z].
module panel(x, y, z, lx, ly, lz) {
    translate([x, y, z]) cube([lx, ly, lz]);
}

";

const TABLE_BODY: &str = "\
module apron_ring() {
    // long aprons (front/back), inset by the leg section
    panel(leg_section, thickness, height - thickness - 100,
          width - 2*leg_section, thickness, 80);
    panel(leg_section, depth - 2*thickness, height - thickness - 100,
          width - 2*leg_section, thickness, 80);
    // short aprons (sides)
    panel(thickness, leg_section, height - thickness - 100,
          thickness, depth - 2*leg_section, 80);
    panel(width - 2*thickness, leg_section, height - thickness - 100,
          thickness, depth - 2*leg_section, 80);
}

module table() {
    // top
    panel(0, 0, height - thickness, width, depth, thickness);
    // four legs
    panel(0, 0, 0, leg_section, leg_section, height - thickness);
    panel(width - leg_section, 0, 0, leg_section, leg_section, height - thickness);
    panel(0, depth - leg_section, 0, leg_section, leg_section, height - thickness);
    panel(width - leg_section, depth - leg_section, 0,
          leg_section, leg_section, height - thickness);
    apron_ring();
}

table();
";

const SHELF_BODY: &str = "\
module carcass() {
    // sides
    panel(0, 0, 0, thickness, depth, height);
    panel(width - thickness, 0, 0, thickness, depth, height);
    // bottom + top (between the sides)
    panel(thickness, 0, 0, width - 2*thickness, depth, thickness);
    panel(thickness, 0, height - thickness, width - 2*thickness, depth, thickness);
    // back
    panel(0, depth - back_thick, 0, width, back_thick, height);
}

module shelves() {
    if (shelf_count > 0) {
        step = (height - 2*thickness) / (shelf_count + 1);
        for (i = [1 : shelf_count]) {
            panel(thickness, 0, thickness + i*step,
                  width - 2*thickness, depth - back_thick, thickness);
        }
    }
}

carcass();
shelves();
";

const BOX_BODY: &str = "\
module carcass() {
    // sides
    panel(0, 0, 0, thickness, depth, height);
    panel(width - thickness, 0, 0, thickness, depth, height);
    // bottom + top
    panel(thickness, 0, 0, width - 2*thickness, depth, thickness);
    panel(thickness, 0, height - thickness, width - 2*thickness, depth, thickness);
    // back
    panel(0, depth - back_thick, 0, width, back_thick, height);
}

carcass();
";

/// A short human-readable summary of the model geometry, used in artifact
/// manifests and CLI output.
pub fn geometry_summary(brief: &ProductBrief) -> String {
    let list = cut_list(brief);
    format!(
        "{} ({}): {:.0}x{:.0}x{:.0}mm, {} parts, {:.2} m2 panel face",
        brief.name,
        brief.kind.label(),
        brief.width_mm,
        brief.depth_mm,
        brief.height_mm,
        list.total_parts(),
        list.total_face_area_m2(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductKind;

    fn brief(kind: ProductKind) -> ProductBrief {
        ProductBrief {
            name: "Test Piece".into(),
            kind,
            width_mm: 800.0,
            depth_mm: 400.0,
            height_mm: 900.0,
            panel_thickness_mm: 18.0,
            material: "oak".into(),
            shelves: 3,
            quantity: 1,
            finish: "oil".into(),
        }
    }

    #[test]
    fn emits_parameters_from_brief() {
        let scad = generate_openscad(&brief(ProductKind::Shelf));
        assert!(scad.contains("width      = 800;"));
        assert!(scad.contains("depth      = 400;"));
        assert!(scad.contains("height     = 900;"));
        assert!(scad.contains("thickness  = 18;"));
        assert!(scad.contains("shelf_count= 3;"));
    }

    #[test]
    fn table_body_instantiates_table() {
        let scad = generate_openscad(&brief(ProductKind::Table));
        assert!(scad.contains("module table()"));
        assert!(scad.trim_end().ends_with("table();"));
    }

    #[test]
    fn shelf_body_has_shelves_loop() {
        let scad = generate_openscad(&brief(ProductKind::Shelf));
        assert!(scad.contains("module shelves()"));
        assert!(scad.contains("for (i = [1 : shelf_count])"));
    }

    #[test]
    fn box_body_has_no_shelves() {
        let scad = generate_openscad(&brief(ProductKind::Box));
        assert!(!scad.contains("module shelves()"));
        assert!(scad.contains("module carcass()"));
    }

    #[test]
    fn all_kinds_include_panel_module_and_header() {
        for kind in [ProductKind::Table, ProductKind::Shelf, ProductKind::Box] {
            let scad = generate_openscad(&brief(kind));
            assert!(scad.contains("module panel("));
            assert!(scad.contains("Simard Atelier"));
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let b = brief(ProductKind::Table);
        assert_eq!(generate_openscad(&b), generate_openscad(&b));
    }

    #[test]
    fn fmt_num_drops_trailing_zero() {
        assert_eq!(fmt_num(18.0), "18");
        assert_eq!(fmt_num(18.5), "18.500");
    }

    #[test]
    fn geometry_summary_mentions_parts() {
        let summary = geometry_summary(&brief(ProductKind::Table));
        assert!(summary.contains("parts"));
        assert!(summary.contains("table"));
    }

    #[test]
    fn header_neutralizes_comment_injection_from_name() {
        // A malicious/untrusted brief must not be able to break out of the
        // header comment and inject OpenSCAD code via a newline in the name.
        let mut b = brief(ProductKind::Table);
        b.name = "Desk\nmodule pwn(){cube(999);}".into();
        b.material = "oak\n// escaped".into();
        let scad = generate_openscad(&b);
        let header_block = scad.split("width").next().unwrap();
        // The real security invariant: every non-empty header line stays a
        // comment, so injected text can never become executable OpenSCAD.
        for line in header_block.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                line.trim_start().starts_with("//"),
                "non-comment line leaked into header: {line:?}"
            );
        }
        // The product line must be a single comment line (newline collapsed).
        assert!(header_block.contains("// product: Desk module pwn"));
    }

    #[test]
    fn comment_safe_collapses_whitespace_and_control_chars() {
        assert_eq!(comment_safe("a\nb\tc"), "a b c");
        assert_eq!(comment_safe("  spaced   out  "), "spaced out");
        assert_eq!(comment_safe("\n\t "), "(unspecified)");
    }
}
