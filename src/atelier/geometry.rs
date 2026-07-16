//! Parametric geometry generation.
//!
//! Turns a validated [`ProductBrief`] into an [`Assembly`]: a set of rectangular
//! [`Panel`]s, each with cut dimensions (for the cut list) and one or more
//! placed [`Instance`]s (axis-aligned boxes, for the model + render).
//!
//! Coordinate system (millimetres): `x` = width, `y` = depth, `z` = height.
//! Every panel instance is an axis-aligned box given by its minimum corner
//! (`origin`) and its `size`. This keeps geometry a single source of truth,
//! shared by the OpenSCAD model, the STEP solid, the cut list, and verification.

use super::brief::{Dimensions, ProductBrief, ProductKind};

/// Grain direction of a cut panel relative to its longest in-plane edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grain {
    /// Grain runs along the panel length (the default for directional stock).
    Length,
    /// Material has no directional grain.
    None,
}

impl Grain {
    pub fn label(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::None => "none",
        }
    }
}

/// A single placed box instance of a panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    /// Minimum corner (x, y, z) in millimetres.
    pub origin: [f64; 3],
    /// Box size (sx, sy, sz) in millimetres.
    pub size: [f64; 3],
}

/// A rectangular part cut from sheet/board stock.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// Role of the panel in the assembly (e.g. "side", "shelf", "leg").
    pub label: String,
    /// Longest in-plane cut dimension (grain direction), millimetres.
    pub length_mm: f64,
    /// Shorter in-plane cut dimension, millimetres.
    pub width_mm: f64,
    /// Panel thickness, millimetres.
    pub thickness_mm: f64,
    /// Material name (carried through to the cut list / BOM).
    pub material: String,
    /// Grain direction.
    pub grain: Grain,
    /// Placed instances of this identical panel.
    pub instances: Vec<Instance>,
}

impl Panel {
    /// Number of identical copies of this panel.
    pub fn qty(&self) -> u32 {
        self.instances.len() as u32
    }

    /// In-plane area of one panel face in square millimetres.
    pub fn face_area_mm2(&self) -> f64 {
        self.length_mm * self.width_mm
    }
}

/// A generated product: its overall dimensions and constituent panels.
#[derive(Debug, Clone)]
pub struct Assembly {
    pub product_name: String,
    pub kind: ProductKind,
    pub dimensions_mm: Dimensions,
    pub panels: Vec<Panel>,
}

impl Assembly {
    /// Total number of placed panel instances across the assembly.
    pub fn instance_count(&self) -> u32 {
        self.panels.iter().map(Panel::qty).sum()
    }

    /// Bounding box (max corner) of all instances, for render framing / checks.
    pub fn bounds_mm(&self) -> [f64; 3] {
        let mut max = [0.0_f64; 3];
        for panel in &self.panels {
            for inst in &panel.instances {
                for (axis, m) in max.iter_mut().enumerate() {
                    *m = m.max(inst.origin[axis] + inst.size[axis]);
                }
            }
        }
        max
    }
}

/// Generate the parametric assembly for a validated brief.
pub fn generate(brief: &ProductBrief) -> Assembly {
    let kind = brief.normalized_kind();
    let panels = match kind {
        ProductKind::Bookcase => bookcase(brief),
        ProductKind::Table => table(brief),
        ProductKind::Stool => stool(brief),
        ProductKind::Carcass => carcass(brief),
    };
    Assembly {
        product_name: brief.name.clone(),
        kind,
        dimensions_mm: brief.dimensions_mm,
        panels,
    }
}

fn grain_of(brief: &ProductBrief) -> Grain {
    if brief.material.grain {
        Grain::Length
    } else {
        Grain::None
    }
}

/// Order two in-plane dimensions as (length, width) with length ≥ width.
fn cut_dims(a: f64, b: f64) -> (f64, f64) {
    if a >= b { (a, b) } else { (b, a) }
}

fn bookcase(brief: &ProductBrief) -> Vec<Panel> {
    let Dimensions {
        width: w,
        depth: d,
        height: h,
    } = brief.dimensions_mm;
    let t = brief.material.thickness_mm;
    let mat = brief.material.name.clone();
    let grain = grain_of(brief);
    let back = brief.parameters.back_panel.unwrap_or(true);
    let shelf_count = brief.parameters.shelves.unwrap_or(2);

    // Shelves lose the back-panel depth when a back is fitted.
    let shelf_depth = if back { (d - t).max(t) } else { d };
    let inner_w = (w - 2.0 * t).max(t);

    let mut panels = Vec::new();

    // Two vertical sides (full height, full depth).
    let (sl, sw) = cut_dims(h, d);
    panels.push(Panel {
        label: "side".into(),
        length_mm: sl,
        width_mm: sw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![
            Instance {
                origin: [0.0, 0.0, 0.0],
                size: [t, d, h],
            },
            Instance {
                origin: [w - t, 0.0, 0.0],
                size: [t, d, h],
            },
        ],
    });

    // Top and bottom span between the sides.
    let (tl, tw) = cut_dims(inner_w, d);
    panels.push(Panel {
        label: "top-bottom".into(),
        length_mm: tl,
        width_mm: tw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![
            Instance {
                origin: [t, 0.0, 0.0],
                size: [inner_w, d, t],
            },
            Instance {
                origin: [t, 0.0, h - t],
                size: [inner_w, d, t],
            },
        ],
    });

    // Evenly spaced fixed shelves between bottom and top.
    if shelf_count > 0 {
        let span_bottom = t;
        let span_top = h - t;
        let gaps = shelf_count + 1;
        let mut instances = Vec::new();
        for i in 1..=shelf_count {
            let z = span_bottom + (span_top - span_bottom) * (i as f64) / (gaps as f64) - t / 2.0;
            instances.push(Instance {
                origin: [t, 0.0, z],
                size: [inner_w, shelf_depth, t],
            });
        }
        let (shl, shw) = cut_dims(inner_w, shelf_depth);
        panels.push(Panel {
            label: "shelf".into(),
            length_mm: shl,
            width_mm: shw,
            thickness_mm: t,
            material: mat.clone(),
            grain,
            instances,
        });
    }

    // Inset back panel.
    if back {
        let inner_h = (h - 2.0 * t).max(t);
        let (bl, bw) = cut_dims(inner_w, inner_h);
        panels.push(Panel {
            label: "back".into(),
            length_mm: bl,
            width_mm: bw,
            thickness_mm: t,
            material: mat,
            grain,
            instances: vec![Instance {
                origin: [t, d - t, t],
                size: [inner_w, t, inner_h],
            }],
        });
    }

    panels
}

fn table(brief: &ProductBrief) -> Vec<Panel> {
    let Dimensions {
        width: w,
        depth: d,
        height: h,
    } = brief.dimensions_mm;
    let t = brief.material.thickness_mm;
    let mat = brief.material.name.clone();
    let grain = grain_of(brief);
    let leg = brief
        .parameters
        .leg_size_mm
        .unwrap_or((t * 2.0).max(40.0))
        .min(w.min(d) / 3.0);
    let overhang = brief
        .parameters
        .top_overhang_mm
        .unwrap_or(leg)
        .min(w.min(d) / 4.0);
    let apron = brief.parameters.apron.unwrap_or(true);

    let mut panels = Vec::new();

    // Table top.
    let (tl, tw) = cut_dims(w, d);
    panels.push(Panel {
        label: "top".into(),
        length_mm: tl,
        width_mm: tw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![Instance {
            origin: [0.0, 0.0, h - t],
            size: [w, d, t],
        }],
    });

    // Four legs, inset by the overhang.
    let leg_h = h - t;
    let inset = overhang;
    let leg_positions = [
        [inset, inset],
        [w - inset - leg, inset],
        [inset, d - inset - leg],
        [w - inset - leg, d - inset - leg],
    ];
    let leg_instances = leg_positions
        .iter()
        .map(|&[x, y]| Instance {
            origin: [x, y, 0.0],
            size: [leg, leg, leg_h],
        })
        .collect();
    panels.push(Panel {
        label: "leg".into(),
        length_mm: leg_h,
        width_mm: leg,
        thickness_mm: leg,
        material: mat.clone(),
        grain,
        instances: leg_instances,
    });

    // Optional aprons: rails just under the top connecting the legs.
    if apron {
        let apron_h = (leg * 2.0).min(leg_h / 2.0).max(t);
        let apron_z = h - t - apron_h;
        let inner_x0 = inset + leg;
        let inner_x1 = w - inset - leg;
        let inner_y0 = inset + leg;
        let inner_y1 = d - inset - leg;
        let long_len = (inner_x1 - inner_x0).max(t);
        let short_len = (inner_y1 - inner_y0).max(t);
        let (al, aw) = cut_dims(long_len.max(short_len), apron_h);
        panels.push(Panel {
            label: "apron".into(),
            length_mm: al,
            width_mm: aw,
            thickness_mm: t,
            material: mat,
            grain,
            instances: vec![
                Instance {
                    origin: [inner_x0, inset, apron_z],
                    size: [long_len, t, apron_h],
                },
                Instance {
                    origin: [inner_x0, d - inset - t, apron_z],
                    size: [long_len, t, apron_h],
                },
                Instance {
                    origin: [inset, inner_y0, apron_z],
                    size: [t, short_len, apron_h],
                },
                Instance {
                    origin: [w - inset - t, inner_y0, apron_z],
                    size: [t, short_len, apron_h],
                },
            ],
        });
    }

    panels
}

fn stool(brief: &ProductBrief) -> Vec<Panel> {
    let Dimensions {
        width: w,
        depth: d,
        height: h,
    } = brief.dimensions_mm;
    let t = brief.material.thickness_mm;
    let mat = brief.material.name.clone();
    let grain = grain_of(brief);
    let leg = brief
        .parameters
        .leg_size_mm
        .unwrap_or((t * 2.0).max(35.0))
        .min(w.min(d) / 3.0);

    let mut panels = Vec::new();

    // Seat.
    let (sl, sw) = cut_dims(w, d);
    panels.push(Panel {
        label: "seat".into(),
        length_mm: sl,
        width_mm: sw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![Instance {
            origin: [0.0, 0.0, h - t],
            size: [w, d, t],
        }],
    });

    // Four legs at the corners.
    let leg_h = h - t;
    let inset = leg / 2.0;
    let positions = [
        [inset, inset],
        [w - inset - leg, inset],
        [inset, d - inset - leg],
        [w - inset - leg, d - inset - leg],
    ];
    let instances = positions
        .iter()
        .map(|&[x, y]| Instance {
            origin: [x, y, 0.0],
            size: [leg, leg, leg_h],
        })
        .collect();
    panels.push(Panel {
        label: "leg".into(),
        length_mm: leg_h,
        width_mm: leg,
        thickness_mm: leg,
        material: mat,
        grain,
        instances,
    });

    panels
}

fn carcass(brief: &ProductBrief) -> Vec<Panel> {
    let Dimensions {
        width: w,
        depth: d,
        height: h,
    } = brief.dimensions_mm;
    let t = brief.material.thickness_mm;
    let mat = brief.material.name.clone();
    let grain = grain_of(brief);
    let open_front = brief.parameters.open_front.unwrap_or(false);

    let inner_h = (h - 2.0 * t).max(t);
    let inner_w = (w - 2.0 * t).max(t);

    let mut panels = Vec::new();

    // Bottom + top (full footprint).
    let (tbl, tbw) = cut_dims(w, d);
    panels.push(Panel {
        label: "top-bottom".into(),
        length_mm: tbl,
        width_mm: tbw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![
            Instance {
                origin: [0.0, 0.0, 0.0],
                size: [w, d, t],
            },
            Instance {
                origin: [0.0, 0.0, h - t],
                size: [w, d, t],
            },
        ],
    });

    // Left + right sides (between top and bottom).
    let (sl, sw) = cut_dims(inner_h, d);
    panels.push(Panel {
        label: "side".into(),
        length_mm: sl,
        width_mm: sw,
        thickness_mm: t,
        material: mat.clone(),
        grain,
        instances: vec![
            Instance {
                origin: [0.0, 0.0, t],
                size: [t, d, inner_h],
            },
            Instance {
                origin: [w - t, 0.0, t],
                size: [t, d, inner_h],
            },
        ],
    });

    // Back panel (inset between sides).
    let (bl, bw) = cut_dims(inner_w, inner_h);
    let mut face_instances = vec![Instance {
        origin: [t, d - t, t],
        size: [inner_w, t, inner_h],
    }];
    // Front panel unless an open front was requested.
    if !open_front {
        face_instances.push(Instance {
            origin: [t, 0.0, t],
            size: [inner_w, t, inner_h],
        });
    }
    panels.push(Panel {
        label: if open_front { "back" } else { "front-back" }.into(),
        length_mm: bl,
        width_mm: bw,
        thickness_mm: t,
        material: mat,
        grain,
        instances: face_instances,
    });

    panels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductBrief;

    fn brief(kind: &str, extra_params: &str) -> ProductBrief {
        let json = format!(
            r#"{{"name":"t","kind":"{kind}","dimensions_mm":{{"width":800,"depth":300,"height":1000}},
                "material":{{"name":"ply","thickness_mm":18,"grain":true}},
                "parameters":{extra_params}}}"#
        );
        ProductBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn bookcase_has_sides_top_shelves_back() {
        let a = generate(&brief("bookcase", r#"{"shelves":2,"back_panel":true}"#));
        let labels: Vec<_> = a.panels.iter().map(|p| p.label.as_str()).collect();
        assert!(labels.contains(&"side"));
        assert!(labels.contains(&"top-bottom"));
        assert!(labels.contains(&"shelf"));
        assert!(labels.contains(&"back"));
        let shelf = a.panels.iter().find(|p| p.label == "shelf").unwrap();
        assert_eq!(shelf.qty(), 2);
    }

    #[test]
    fn bookcase_without_back_uses_full_depth_shelves() {
        let a = generate(&brief("shelf", r#"{"shelves":1,"back_panel":false}"#));
        assert!(a.panels.iter().all(|p| p.label != "back"));
        let shelf = a.panels.iter().find(|p| p.label == "shelf").unwrap();
        // Full-depth shelf: one in-plane dim equals the 300mm depth.
        assert!((shelf.width_mm - 300.0).abs() < 1e-6 || (shelf.length_mm - 300.0).abs() < 1e-6);
    }

    #[test]
    fn table_has_top_and_four_legs() {
        let a = generate(&brief("table", r#"{"apron":true}"#));
        let legs = a.panels.iter().find(|p| p.label == "leg").unwrap();
        assert_eq!(legs.qty(), 4);
        assert!(a.panels.iter().any(|p| p.label == "top"));
        assert!(a.panels.iter().any(|p| p.label == "apron"));
    }

    #[test]
    fn stool_has_seat_and_four_legs() {
        let a = generate(&brief("stool", "{}"));
        assert!(a.panels.iter().any(|p| p.label == "seat"));
        assert_eq!(a.panels.iter().find(|p| p.label == "leg").unwrap().qty(), 4);
    }

    #[test]
    fn carcass_closed_has_six_faces() {
        let a = generate(&brief("box", "{}"));
        assert_eq!(a.instance_count(), 6);
    }

    #[test]
    fn carcass_open_front_has_five_faces() {
        let a = generate(&brief("cabinet", r#"{"open_front":true}"#));
        assert_eq!(a.instance_count(), 5);
    }

    #[test]
    fn instances_stay_within_bounds() {
        let a = generate(&brief("bookcase", r#"{"shelves":3,"back_panel":true}"#));
        let b = a.bounds_mm();
        assert!(b[0] <= 800.0 + 1e-6);
        assert!(b[1] <= 300.0 + 1e-6);
        assert!(b[2] <= 1000.0 + 1e-6);
    }

    #[test]
    fn cut_dims_orders_length_first() {
        assert_eq!(cut_dims(10.0, 20.0), (20.0, 10.0));
        assert_eq!(cut_dims(30.0, 5.0), (30.0, 5.0));
    }

    #[test]
    fn panel_face_area_matches_dims() {
        let p = Panel {
            label: "x".into(),
            length_mm: 100.0,
            width_mm: 50.0,
            thickness_mm: 18.0,
            material: "ply".into(),
            grain: Grain::None,
            instances: vec![],
        };
        assert_eq!(p.face_area_mm2(), 5000.0);
        assert_eq!(Grain::Length.label(), "length");
        assert_eq!(Grain::None.label(), "none");
    }
}
