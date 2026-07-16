//! A small but genuinely runnable fabrication engine.
//!
//! The engine turns a designed [`ProductConcept`](super::design::ProductConcept)
//! into the artifacts a workshop actually needs:
//! - a **cut list** (every stock piece, grouped by part),
//! - a **bill of materials** (structural stock plus joinery hardware, with
//!   estimated weight and cost), and
//! - **fabrication-ready exports**: an OpenSCAD script, an ASCII STL mesh, a
//!   STEP (ISO-10303-21) container, and an SVG front-elevation render.
//!
//! Everything is in-memory and deterministic, so a concept can be driven from a
//! brief all the way to exported models in a test or example without any
//! external CAD binary. The generated OpenSCAD script *is* the parametric model
//! that Blender (bpy) / FreeCAD / OpenSCAD tooling can consume downstream.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::AtelierError;
use super::design::ProductConcept;

/// A fabrication-ready export format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    /// Parametric OpenSCAD script (`.scad`).
    OpenScad,
    /// ASCII STL triangle mesh (`.stl`).
    Stl,
    /// STEP / ISO-10303-21 exchange container (`.step`).
    Step,
    /// SVG front-elevation render (`.svg`).
    SvgRender,
}

impl ExportFormat {
    /// File extension for the format (without the dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::OpenScad => "scad",
            Self::Stl => "stl",
            Self::Step => "step",
            Self::SvgRender => "svg",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenScad => "OpenSCAD model",
            Self::Stl => "STL mesh",
            Self::Step => "STEP (ISO-10303-21)",
            Self::SvgRender => "SVG render",
        }
    }
}

/// A generated export artifact with its content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportArtifact {
    pub format: ExportFormat,
    pub filename: String,
    pub content: String,
}

impl ExportArtifact {
    /// Byte length of the content.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.content.len()
    }
}

/// A single line in the cut list: identical stock pieces for one part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutPiece {
    pub part: String,
    pub quantity: u32,
    pub length_mm: u32,
    pub width_mm: u32,
    pub thickness_mm: u32,
}

/// A single line in the bill of materials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BomLine {
    pub item: String,
    pub detail: String,
    pub quantity: u32,
    pub volume_mm3: u64,
    pub weight_grams: u64,
    pub cost_cents: u64,
}

/// In-memory fabrication engine seeded from a designed concept.
#[derive(Clone, Debug)]
pub struct FabricationEngine {
    concept: ProductConcept,
    run_quantity: u32,
}

impl FabricationEngine {
    /// Seed a fabrication engine from a concept. The production run size is the
    /// concept's brief `quantity` (at least one unit).
    #[must_use]
    pub fn from_concept(concept: &ProductConcept) -> Self {
        Self {
            concept: concept.clone(),
            run_quantity: concept.brief.quantity.max(1),
        }
    }

    /// The concept this engine fabricates.
    #[must_use]
    pub fn concept(&self) -> &ProductConcept {
        &self.concept
    }

    /// Number of complete units in the production run.
    #[must_use]
    pub fn run_quantity(&self) -> u32 {
        self.run_quantity
    }

    /// Per-unit cut list, one line per part type, ordered by part name.
    #[must_use]
    pub fn cut_list(&self) -> Vec<CutPiece> {
        let mut pieces: Vec<CutPiece> = self
            .concept
            .parts
            .iter()
            .map(|part| CutPiece {
                part: part.name.clone(),
                quantity: part.quantity(),
                length_mm: part.length_mm,
                width_mm: part.width_mm,
                thickness_mm: part.thickness_mm,
            })
            .collect();
        pieces.sort_by(|a, b| a.part.cmp(&b.part));
        pieces
    }

    /// Total physical pieces across the whole run.
    #[must_use]
    pub fn total_pieces(&self) -> u64 {
        let per_unit: u64 = self
            .concept
            .parts
            .iter()
            .map(|p| u64::from(p.quantity()))
            .sum();
        per_unit * u64::from(self.run_quantity)
    }

    /// Bill of materials for the whole run: one structural line per part plus a
    /// joinery-hardware line. Weight and cost are estimated from material
    /// density and stock cost.
    #[must_use]
    pub fn bill_of_materials(&self) -> Vec<BomLine> {
        let material = self.concept.brief.material;
        let density = u64::from(material.density_kg_m3());
        let cost_per_m3 = material.cost_per_m3_cents();
        let run = u64::from(self.run_quantity);

        let mut lines: Vec<BomLine> = self
            .concept
            .parts
            .iter()
            .map(|part| {
                // Compute per-unit figures first, then scale by the run so the
                // reported totals are exactly `per_unit * run` (no rounding drift).
                let unit_volume = part.total_volume_mm3();
                BomLine {
                    item: part.name.clone(),
                    detail: material.label().to_string(),
                    quantity: part.quantity() * self.run_quantity,
                    volume_mm3: unit_volume * run,
                    // grams = mm^3 * (kg/m^3) / 1_000_000  (1 m^3 = 1e9 mm^3, kg->g = 1e3).
                    weight_grams: (unit_volume * density / 1_000_000) * run,
                    cost_cents: (unit_volume * cost_per_m3 / 1_000_000_000) * run,
                }
            })
            .collect();
        lines.sort_by(|a, b| a.item.cmp(&b.item));

        let (hardware, per_unit_count) = self.hardware_line();
        lines.push(BomLine {
            item: "Fasteners / joinery hardware".to_string(),
            detail: hardware,
            quantity: per_unit_count * self.run_quantity,
            volume_mm3: 0,
            weight_grams: 0,
            cost_cents: u64::from(per_unit_count) * u64::from(self.run_quantity) * 12,
        });
        lines
    }

    fn hardware_line(&self) -> (String, u32) {
        use super::design::Joinery::{
            BoltedFrame, Dado, Dowel, MortiseAndTenon, PocketScrew, WeldedFrame,
        };
        let joints = self.concept.total_parts().saturating_sub(1);
        match self.concept.joinery {
            MortiseAndTenon => ("mortise-and-tenon joints, glued".to_string(), joints * 2),
            Dowel => ("8mm dowels".to_string(), joints * 4),
            PocketScrew => ("pocket screws".to_string(), joints * 2),
            Dado => ("dado joints + brad nails".to_string(), joints * 3),
            WeldedFrame => ("MIG weld seams".to_string(), joints),
            BoltedFrame => ("M8 bolts + nyloc nuts".to_string(), joints * 2),
        }
    }

    /// Total estimated run weight, in grams.
    #[must_use]
    pub fn total_weight_grams(&self) -> u64 {
        self.bill_of_materials()
            .iter()
            .map(|l| l.weight_grams)
            .sum()
    }

    /// Total estimated run cost, in integer cents.
    #[must_use]
    pub fn total_cost_cents(&self) -> u64 {
        self.bill_of_materials().iter().map(|l| l.cost_cents).sum()
    }

    /// Generate all fabrication-ready exports (one model unit per export; the
    /// BOM/cost cover the full run).
    #[must_use]
    pub fn exports(&self) -> Vec<ExportArtifact> {
        let slug = slugify(&self.concept.brief.name);
        vec![
            ExportArtifact {
                format: ExportFormat::OpenScad,
                filename: format!("{slug}.scad"),
                content: self.openscad_source(),
            },
            ExportArtifact {
                format: ExportFormat::Stl,
                filename: format!("{slug}.stl"),
                content: self.stl_source(&slug),
            },
            ExportArtifact {
                format: ExportFormat::Step,
                filename: format!("{slug}.step"),
                content: self.step_source(&slug),
            },
            ExportArtifact {
                format: ExportFormat::SvgRender,
                filename: format!("{slug}-elevation.svg"),
                content: self.svg_render(),
            },
        ]
    }

    /// A valid, parametric OpenSCAD script that models every part as a
    /// translated cube. Downstream tooling (OpenSCAD, FreeCAD, Blender bpy) can
    /// render or convert this directly.
    #[must_use]
    pub fn openscad_source(&self) -> String {
        let brief = &self.concept.brief;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "// {} — {}",
            brief.name, self.concept.aesthetic.tagline
        );
        let _ = writeln!(
            out,
            "// Category: {}  Material: {}  Joinery: {}  Finish: {}",
            brief.category.label(),
            brief.material.label(),
            self.concept.joinery.label(),
            self.concept.aesthetic.finish.label(),
        );
        let d = brief.dimensions;
        let _ = writeln!(
            out,
            "// Bounding box (mm): {} x {} x {}",
            d.length_mm, d.width_mm, d.height_mm
        );
        let _ = writeln!(out, "$fn = 32;");
        let _ = writeln!(out, "module part(name, size, pos) {{");
        let _ = writeln!(out, "  translate(pos) cube(size);");
        let _ = writeln!(out, "}}");
        let _ = writeln!(out, "module {}() {{", slugify_ident(&brief.name));
        for part in &self.concept.parts {
            let _ = writeln!(out, "  // {} (x{})", part.name, part.quantity());
            for pos in &part.placements {
                let _ = writeln!(
                    out,
                    "  part(\"{}\", [{}, {}, {}], [{}, {}, {}]);",
                    escape(&part.name),
                    part.length_mm,
                    part.width_mm,
                    part.thickness_mm,
                    pos[0],
                    pos[1],
                    pos[2],
                );
            }
        }
        let _ = writeln!(out, "}}");
        let _ = writeln!(out, "{}();", slugify_ident(&brief.name));
        out
    }

    /// A valid ASCII STL mesh of the assembled parts (each part instance is a
    /// closed box of 12 triangles).
    #[must_use]
    pub fn stl_source(&self, solid_name: &str) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "solid {solid_name}");
        for part in &self.concept.parts {
            let size = [
                f64::from(part.length_mm),
                f64::from(part.width_mm),
                f64::from(part.thickness_mm),
            ];
            for pos in &part.placements {
                let origin = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
                write_box_facets(&mut out, origin, size);
            }
        }
        let _ = writeln!(out, "endsolid {solid_name}");
        out
    }

    /// A STEP / ISO-10303-21 container. The header is standards-valid and the
    /// data section carries one product record per part so a CAD reader sees the
    /// assembly structure.
    #[must_use]
    pub fn step_source(&self, id: &str) -> String {
        let brief = &self.concept.brief;
        let mut out = String::new();
        out.push_str("ISO-10303-21;\n");
        out.push_str("HEADER;\n");
        let _ = writeln!(
            out,
            "FILE_DESCRIPTION(('Simard Atelier fabrication export for {}'),'2;1');",
            escape(&brief.name)
        );
        let _ = writeln!(
            out,
            "FILE_NAME('{id}.step','2026-01-01T00:00:00',('Simard Atelier'),('Simard'),'atelier','simard','');"
        );
        out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n");
        out.push_str("ENDSEC;\n");
        out.push_str("DATA;\n");
        let _ = writeln!(
            out,
            "#1=APPLICATION_CONTEXT('core data for automotive mechanical design processes');"
        );
        let _ = writeln!(
            out,
            "#2=PRODUCT('{}','{}','{}',(#1));",
            escape(&brief.name),
            brief.category.label(),
            escape(&self.concept.aesthetic.style)
        );
        for (offset, part) in self.concept.parts.iter().enumerate() {
            let entity = 10 + u32::try_from(offset).unwrap_or(0);
            let _ = writeln!(
                out,
                "#{entity}=PRODUCT_COMPONENT('{}',{},{},{}); /* qty {} */",
                escape(&part.name),
                part.length_mm,
                part.width_mm,
                part.thickness_mm,
                part.quantity()
            );
        }
        out.push_str("ENDSEC;\n");
        out.push_str("END-ISO-10303-21;\n");
        out
    }

    /// An SVG front-elevation (X–Z plane) render of the assembled parts.
    #[must_use]
    pub fn svg_render(&self) -> String {
        let d = self.concept.brief.dimensions;
        let margin = 40.0_f64;
        let target = 800.0_f64;
        let scale = (target - 2.0 * margin) / f64::from(d.length_mm.max(1));
        let width = f64::from(d.length_mm) * scale + 2.0 * margin;
        let height = f64::from(d.height_mm) * scale + 2.0 * margin;
        let stroke = &self.concept.aesthetic.palette[0];
        let fill = self
            .concept
            .aesthetic
            .palette
            .get(1)
            .map_or("#cccccc", String::as_str);

        let mut out = String::new();
        let _ = writeln!(
            out,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">"
        );
        let _ = writeln!(
            out,
            "  <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>"
        );
        let _ = writeln!(
            out,
            "  <title>{} front elevation</title>",
            escape(&self.concept.brief.name)
        );
        // Project each part instance onto the X-Z plane. SVG y grows downward, so
        // flip z: screen_y = height - margin - (z + dz) * scale.
        for part in &self.concept.parts {
            for pos in &part.placements {
                let x = margin + f64::from(pos[0]) * scale;
                let rect_w = f64::from(part.length_mm) * scale;
                let rect_h = f64::from(part.thickness_mm) * scale;
                let top_z = f64::from(pos[2] + i32::try_from(part.thickness_mm).unwrap_or(0));
                let y = height - margin - top_z * scale;
                let _ = writeln!(
                    out,
                    "  <rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{rect_w:.1}\" height=\"{rect_h:.1}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\"/>"
                );
            }
        }
        out.push_str("</svg>\n");
        out
    }

    /// Verify the fabrication artifacts against the concept and return any
    /// invariant violations. An empty result means every invariant held.
    #[must_use]
    pub fn verify(&self) -> Vec<String> {
        let mut failures = Vec::new();

        let cut_pieces: u32 = self.cut_list().iter().map(|c| c.quantity).sum();
        if cut_pieces != self.concept.total_parts() {
            failures.push(format!(
                "cut list per-unit pieces ({cut_pieces}) != concept parts ({})",
                self.concept.total_parts()
            ));
        }

        let bom = self.bill_of_materials();
        for part in &self.concept.parts {
            if !bom.iter().any(|l| l.item == part.name) {
                failures.push(format!("part '{}' missing from BOM", part.name));
            }
        }
        let structural_volume: u64 = bom.iter().map(|l| l.volume_mm3).sum();
        if structural_volume == 0 {
            failures.push("BOM structural volume is zero".to_string());
        }

        let (l, w, h) = self.concept.part_bounding_box_mm();
        let d = self.concept.brief.dimensions;
        if l > d.length_mm || w > d.width_mm || h > d.height_mm {
            failures.push(format!(
                "assembled bounding box ({l}x{w}x{h}) exceeds brief ({}x{}x{})",
                d.length_mm, d.width_mm, d.height_mm
            ));
        }
        if h != d.height_mm {
            failures.push(format!(
                "model height ({h}) does not reach brief height ({})",
                d.height_mm
            ));
        }

        let exports = self.exports();
        for format in [
            ExportFormat::OpenScad,
            ExportFormat::Stl,
            ExportFormat::Step,
            ExportFormat::SvgRender,
        ] {
            match exports.iter().find(|e| e.format == format) {
                None => failures.push(format!("missing export: {}", format.label())),
                Some(export) if export.content.trim().is_empty() => {
                    failures.push(format!("empty export: {}", format.label()));
                }
                Some(export) if !export_is_well_formed(export) => {
                    failures.push(format!("malformed export: {}", format.label()));
                }
                Some(_) => {}
            }
        }

        failures
    }
}

/// Whether an export artifact passes a lightweight format sanity check.
#[must_use]
pub fn export_is_well_formed(export: &ExportArtifact) -> bool {
    let c = &export.content;
    match export.format {
        ExportFormat::OpenScad => c.contains("module ") && c.contains("cube("),
        ExportFormat::Stl => {
            c.starts_with("solid ") && c.contains("facet normal") && c.contains("endsolid ")
        }
        ExportFormat::Step => {
            c.starts_with("ISO-10303-21;")
                && c.contains("FILE_SCHEMA")
                && c.contains("DATA;")
                && c.trim_end().ends_with("END-ISO-10303-21;")
        }
        ExportFormat::SvgRender => {
            c.contains("<svg") && c.contains("</svg>") && c.contains("<rect")
        }
    }
}

/// Turn a fabrication run into a verified result or an error if any invariant
/// fails.
///
/// # Errors
/// Returns [`AtelierError::VerificationFailed`] listing every violated
/// invariant.
pub fn verify_engine(engine: &FabricationEngine) -> Result<(), AtelierError> {
    let failures = engine.verify();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AtelierError::VerificationFailed {
            reason: failures.join("; "),
        })
    }
}

fn write_box_facets(out: &mut String, origin: [f64; 3], size: [f64; 3]) {
    let [x, y, z] = origin;
    let [dx, dy, dz] = size;
    // 8 corners of the box.
    let v = [
        [x, y, z],
        [x + dx, y, z],
        [x + dx, y + dy, z],
        [x, y + dy, z],
        [x, y, z + dz],
        [x + dx, y, z + dz],
        [x + dx, y + dy, z + dz],
        [x, y + dy, z + dz],
    ];
    // Each face: (normal, [a,b,c,d]) with CCW winding as seen from outside.
    let faces: [([f64; 3], [usize; 4]); 6] = [
        ([0.0, 0.0, -1.0], [0, 3, 2, 1]), // bottom
        ([0.0, 0.0, 1.0], [4, 5, 6, 7]),  // top
        ([0.0, -1.0, 0.0], [0, 1, 5, 4]), // front
        ([0.0, 1.0, 0.0], [3, 7, 6, 2]),  // back
        ([-1.0, 0.0, 0.0], [0, 4, 7, 3]), // left
        ([1.0, 0.0, 0.0], [1, 2, 6, 5]),  // right
    ];
    for (normal, quad) in faces {
        for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
            let _ = writeln!(
                out,
                "  facet normal {:e} {:e} {:e}",
                normal[0], normal[1], normal[2]
            );
            out.push_str("    outer loop\n");
            for idx in tri {
                let p = v[idx];
                let _ = writeln!(out, "      vertex {:e} {:e} {:e}", p[0], p[1], p[2]);
            }
            out.push_str("    endloop\n");
            out.push_str("  endfacet\n");
        }
    }
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let mut collapsed = String::new();
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push(c);
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    if collapsed.is_empty() {
        "atelier-product".to_string()
    } else {
        collapsed
    }
}

fn slugify_ident(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    ident = ident.trim_matches('_').to_string();
    if ident.is_empty() || ident.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("part_{ident}")
    } else {
        ident
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "").replace('"', "'").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::design::{
        Dimensions, Material, ProductBrief, ProductCategory, design_product,
    };

    fn table_engine() -> FabricationEngine {
        let brief = ProductBrief::new(
            "Oak Dining Table",
            ProductCategory::Table,
            Material::SolidOak,
            Dimensions::new(1800, 900, 740),
            1,
            "trestle",
        );
        let concept = design_product(&brief).unwrap();
        FabricationEngine::from_concept(&concept)
    }

    #[test]
    fn cut_list_covers_every_part() {
        let engine = table_engine();
        let cut = engine.cut_list();
        let total: u32 = cut.iter().map(|c| c.quantity).sum();
        assert_eq!(total, engine.concept().total_parts());
        assert!(cut.iter().any(|c| c.part == "Leg" && c.quantity == 4));
    }

    #[test]
    fn bom_has_a_line_per_part_plus_hardware() {
        let engine = table_engine();
        let bom = engine.bill_of_materials();
        for part in &engine.concept().parts {
            assert!(bom.iter().any(|l| l.item == part.name));
        }
        assert!(bom.iter().any(|l| l.item.contains("Fasteners")));
        assert!(engine.total_weight_grams() > 0);
        assert!(engine.total_cost_cents() > 0);
    }

    #[test]
    fn run_quantity_scales_bom() {
        let brief = ProductBrief::new(
            "Batch Stool",
            ProductCategory::Stool,
            Material::Pine,
            Dimensions::new(360, 360, 650),
            10,
            "cafe",
        );
        let concept = design_product(&brief).unwrap();
        let engine = FabricationEngine::from_concept(&concept);
        assert_eq!(engine.run_quantity(), 10);
        let single = {
            let mut b = concept.clone();
            b.brief.quantity = 1;
            FabricationEngine::from_concept(&b)
        };
        assert_eq!(engine.total_cost_cents(), single.total_cost_cents() * 10);
        assert_eq!(engine.total_pieces(), single.total_pieces() * 10);
    }

    #[test]
    fn all_exports_are_generated_and_well_formed() {
        let engine = table_engine();
        let exports = engine.exports();
        assert_eq!(exports.len(), 4);
        for export in &exports {
            assert!(export.byte_len() > 0);
            assert!(
                export_is_well_formed(export),
                "{} should be well-formed",
                export.format.label()
            );
        }
    }

    #[test]
    fn openscad_is_valid_and_has_a_cube_per_placement() {
        let engine = table_engine();
        let scad = engine.openscad_source();
        assert!(scad.contains("module "));
        // One `part("...")` call is emitted per placed instance.
        let calls = scad.matches("part(\"").count();
        let placements: usize = engine
            .concept()
            .parts
            .iter()
            .map(|p| p.placements.len())
            .sum();
        assert_eq!(calls, placements);
    }

    #[test]
    fn stl_is_a_closed_mesh() {
        let engine = table_engine();
        let stl = engine.stl_source("t");
        assert!(stl.starts_with("solid t"));
        assert!(stl.trim_end().ends_with("endsolid t"));
        // 12 triangles per box instance.
        let placements: usize = engine
            .concept()
            .parts
            .iter()
            .map(|p| p.placements.len())
            .sum();
        assert_eq!(stl.matches("facet normal").count(), placements * 12);
    }

    #[test]
    fn step_has_valid_envelope() {
        let engine = table_engine();
        let step = engine.step_source("t");
        assert!(step.starts_with("ISO-10303-21;"));
        assert!(step.contains("FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));"));
        assert!(step.trim_end().ends_with("END-ISO-10303-21;"));
        assert_eq!(
            step.matches("PRODUCT_COMPONENT").count(),
            engine.concept().parts.len()
        );
    }

    #[test]
    fn svg_render_is_well_formed_with_dimensions() {
        let engine = table_engine();
        let svg = engine.svg_render();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("width=\""));
        assert!(svg.contains("</svg>"));
        assert!(svg.matches("<rect").count() >= engine.concept().total_parts() as usize);
    }

    #[test]
    fn verify_passes_for_a_sound_concept() {
        let engine = table_engine();
        assert!(engine.verify().is_empty());
        assert!(verify_engine(&engine).is_ok());
    }

    #[test]
    fn slugify_is_filesystem_safe() {
        assert_eq!(slugify("Oak Dining Table!"), "oak-dining-table");
        assert_eq!(slugify("  --- "), "atelier-product");
        assert_eq!(slugify_ident("123 Chair"), "part_123_chair");
    }
}
