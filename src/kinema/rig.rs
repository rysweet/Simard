//! Rig derivation.
//!
//! Rigging turns each object into an armature the renderer can pose. Characters
//! get a full stick-figure skeleton (root → torso → head, arms, legs); simple
//! shapes get a single-bone transform rig. The rig is derived deterministically
//! from the brief and persisted as `rig.json` — the "shot brief → rig" step.

use serde::{Deserialize, Serialize};

use super::brief::{ObjectKind, ShotBrief};

/// A single bone: a named offset from its parent, in local rig units where the
/// figure spans roughly `[-0.5, 0.5]` vertically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    pub parent: Option<String>,
    /// Offset from the parent's tail, in local rig units (x right, y down).
    pub offset: [f64; 2],
    /// Bone length in local rig units (0 for point bones like the root).
    pub length: f64,
}

/// An armature for one object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Armature {
    pub object: String,
    pub kind: String,
    pub bones: Vec<Bone>,
}

impl Armature {
    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }
}

/// The rig for the whole shot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rig {
    pub shot: String,
    pub armatures: Vec<Armature>,
}

impl Rig {
    pub fn total_bones(&self) -> usize {
        self.armatures.iter().map(Armature::bone_count).sum()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

fn bone(name: &str, parent: Option<&str>, offset: [f64; 2], length: f64) -> Bone {
    Bone {
        name: name.to_string(),
        parent: parent.map(str::to_string),
        offset,
        length,
    }
}

/// A humanoid stick-figure skeleton in local rig units.
fn character_bones() -> Vec<Bone> {
    vec![
        bone("root", None, [0.0, 0.0], 0.0),
        bone("torso", Some("root"), [0.0, 0.0], 0.32),
        bone("head", Some("torso"), [0.0, -0.10], 0.16),
        bone("arm.L", Some("torso"), [0.0, -0.26], 0.24),
        bone("arm.R", Some("torso"), [0.0, -0.26], 0.24),
        bone("leg.L", Some("root"), [0.0, 0.06], 0.30),
        bone("leg.R", Some("root"), [0.0, 0.06], 0.30),
    ]
}

/// Build the rig for a shot.
pub fn build_rig(brief: &ShotBrief) -> Rig {
    let armatures = brief
        .objects
        .iter()
        .map(|obj| {
            let kind = obj.normalized_kind();
            let bones = match kind {
                ObjectKind::Character => character_bones(),
                // Shapes get a single transform bone.
                ObjectKind::Circle | ObjectKind::Rect => {
                    vec![bone("root", None, [0.0, 0.0], 0.0)]
                }
            };
            Armature {
                object: obj.name.clone(),
                kind: kind.label().to_string(),
                bones,
            }
        })
        .collect();

    Rig {
        shot: brief.name.clone(),
        armatures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(kind: &str) -> ShotBrief {
        let json = format!(
            r#"{{"name":"rigtest","style":"2d","fps":12,"duration_s":1.0,
            "resolution":{{"width":64,"height":64}},
            "objects":[{{"name":"o","kind":"{kind}","keyframes":[{{"t":0,"x":0.5,"y":0.5}}]}}]}}"#
        );
        ShotBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn character_gets_full_skeleton() {
        let rig = build_rig(&brief("character"));
        assert_eq!(rig.armatures.len(), 1);
        assert_eq!(rig.armatures[0].kind, "character");
        assert_eq!(rig.armatures[0].bone_count(), 7);
        assert_eq!(rig.total_bones(), 7);
    }

    #[test]
    fn shape_gets_single_bone() {
        let rig = build_rig(&brief("rect"));
        assert_eq!(rig.armatures[0].bone_count(), 1);
        assert_eq!(rig.armatures[0].bones[0].name, "root");
    }

    #[test]
    fn every_non_root_bone_has_a_parent() {
        let rig = build_rig(&brief("character"));
        for b in &rig.armatures[0].bones {
            if b.name != "root" {
                assert!(b.parent.is_some(), "bone {} should have a parent", b.name);
            }
        }
    }

    #[test]
    fn rig_serializes_to_json() {
        let rig = build_rig(&brief("character"));
        let json = rig.to_json();
        assert!(json.contains("\"armatures\""));
        assert!(json.contains("head"));
    }
}
