//! Conversion of decoded YSM geometry into Horizon's hierarchical mob model.
use super::{Part, Polygon};
use horizon_ysm::{YsmBone, YsmModel};
use std::collections::HashMap;
use std::fmt;

/// Errors that prevent a YSM bone hierarchy from becoming a renderable model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YsmModelError {
    TooManyBones,
    DuplicateBoneName(String),
    MissingParent { bone: String, parent: String },
    CyclicHierarchy,
    InvalidTextureDimensions,
}

impl fmt::Display for YsmModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyBones => f.write_str("YSM model has too many bones"),
            Self::DuplicateBoneName(name) => write!(f, "YSM model has duplicate bone name: {name}"),
            Self::MissingParent { bone, parent } => {
                write!(f, "YSM bone {bone} refers to missing parent {parent}")
            }
            Self::CyclicHierarchy => f.write_str("YSM bone hierarchy contains a cycle"),
            Self::InvalidTextureDimensions => f.write_str("YSM model has invalid texture dimensions"),
        }
    }
}

impl std::error::Error for YsmModelError {}

/// Converts a decoded YSM model to the same Part/Polygon representation used
/// by Horizon's mob renderer.
///
/// YSM stores face vertices in model pixels and UVs in texture pixels. Its
/// coordinate convention mirrors X; bone pivots and rotations are converted
/// to Horizon's model convention. Per-cube rotations are already represented
/// by the face vertices, so no lossy cube reconstruction is needed here.
pub fn convert_ysm_model(model: &YsmModel) -> Result<Part, YsmModelError> {
    const MAX_BONES: usize = 4096;
    if model.bones.len() > MAX_BONES {
        return Err(YsmModelError::TooManyBones);
    }
    let width = model.texture_width;
    let height = model.texture_height;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(YsmModelError::InvalidTextureDimensions);
    }

    let mut by_name = HashMap::with_capacity(model.bones.len());
    for (index, bone) in model.bones.iter().enumerate() {
        if by_name.insert(bone.name.as_str(), index).is_some() {
            return Err(YsmModelError::DuplicateBoneName(bone.name.clone()));
        }
    }

    let mut children = vec![Vec::<usize>::new(); model.bones.len()];
    let mut roots = Vec::new();
    for (index, bone) in model.bones.iter().enumerate() {
        if bone.parent.is_empty() {
            roots.push(index);
        } else {
            let Some(&parent) = by_name.get(bone.parent.as_str()) else {
                return Err(YsmModelError::MissingParent {
                    bone: bone.name.clone(),
                    parent: bone.parent.clone(),
                });
            };
            children[parent].push(index);
        }
    }

    // Iterative post-order construction avoids recursive stack growth on
    // malformed files with unusually deep parent chains.
    let mut state = vec![0u8; model.bones.len()];
    let mut built: Vec<Option<Part>> = (0..model.bones.len()).map(|_| None).collect();
    let mut visited = 0usize;
    for root in roots {
        let mut stack = vec![(root, false)];
        while let Some((index, expanded)) = stack.pop() {
            if expanded {
                let bone = &model.bones[index];
                let pivot = [-bone.pivot[0], bone.pivot[1], bone.pivot[2]];
                let rotation = [
                    -bone.rotation_radians[0],
                    -bone.rotation_radians[1],
                    bone.rotation_radians[2],
                ];
                let mut polygons = Vec::new();
                for cube in &bone.cubes {
                    for face in &cube.faces {
                        let vertices = std::array::from_fn(|vertex_index| {
                            let vertex = &face.vertices[vertex_index];
                            [
                                -vertex.position[0] - pivot[0],
                                vertex.position[1] - pivot[1],
                                vertex.position[2] - pivot[2],
                                vertex.uv[0] / width,
                                vertex.uv[1] / height,
                            ]
                        });
                        polygons.push(Polygon {
                            normal: [-face.normal[0], face.normal[1], face.normal[2]],
                            vertices,
                            cull_back: false,
                        });
                    }
                }
                let mut part_children = Vec::with_capacity(children[index].len());
                for &child in &children[index] {
                    let Some(part) = built[child].take() else {
                        return Err(YsmModelError::CyclicHierarchy);
                    };
                    part_children.push(part);
                }
                part_children.sort_by(|a, b| a.name.cmp(&b.name));
                built[index] = Some(Part {
                    name: bone.name.clone(),
                    pose: [
                        pivot[0], pivot[1], pivot[2],
                        rotation[0], rotation[1], rotation[2],
                        1.0, 1.0, 1.0,
                    ],
                    polygons,
                    children: part_children,
                });
                state[index] = 2;
                visited += 1;
                continue;
            }

            match state[index] {
                2 => continue,
                1 => return Err(YsmModelError::CyclicHierarchy),
                _ => {}
            }
            state[index] = 1;
            stack.push((index, true));
            for &child in children[index].iter().rev() {
                if state[child] == 1 {
                    return Err(YsmModelError::CyclicHierarchy);
                }
                if state[child] == 0 {
                    stack.push((child, false));
                }
            }
        }
    }

    if visited != model.bones.len() {
        return Err(YsmModelError::CyclicHierarchy);
    }

    let mut root_children = Vec::with_capacity(roots.len());
    for root in roots {
        let Some(part) = built[root].take() else {
            return Err(YsmModelError::CyclicHierarchy);
        };
        root_children.push(part);
    }
    root_children.sort_by(|a, b| a.name.cmp(&b.name));

    // Match Horizon's existing JSON model convention for models whose actual
    // top-level bone is itself named "root".
    let root_name = if root_children.iter().any(|part| part.name == "root") {
        String::new()
    } else {
        "root".to_owned()
    };
    Ok(Part {
        name: root_name,
        pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        polygons: Vec::new(),
        children: root_children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_ysm::{YsmCube, YsmFace, YsmVertex};

    fn model(bones: Vec<YsmBone>) -> YsmModel {
        YsmModel {
            sha256: None,
            identifier: "geometry.test".into(),
            texture_width: 64.0,
            texture_height: 32.0,
            visible_bounds_width: 2.0,
            visible_bounds_height: 2.0,
            visible_bounds_offset: vec![],
            scale: [1.0, 1.0],
            bones,
            legacy_info: None,
        }
    }

    #[test]
    fn converts_parent_hierarchy_coordinates_and_uvs() {
        let face = YsmFace {
            normal: [1.0, 0.0, 0.0],
            vertices: [
                YsmVertex { position: [4.0, 8.0, 12.0], uv: [32.0, 16.0] },
                YsmVertex { position: [4.0, 8.0, 14.0], uv: [48.0, 16.0] },
                YsmVertex { position: [4.0, 10.0, 14.0], uv: [48.0, 24.0] },
                YsmVertex { position: [4.0, 10.0, 12.0], uv: [32.0, 24.0] },
            ],
        };
        let root = YsmBone {
            name: "root".into(),
            parent: String::new(),
            cubes: vec![YsmCube { faces: vec![face] }],
            pivot: [2.0, 3.0, 4.0],
            rotation_radians: [0.1, 0.2, 0.3],
        };
        let child = YsmBone {
            name: "head".into(),
            parent: "root".into(),
            cubes: vec![],
            pivot: [0.0, 1.0, 0.0],
            rotation_radians: [0.0; 3],
        };
        let converted = convert_ysm_model(&model(vec![root, child])).unwrap();
        assert_eq!(converted.name, "");
        let root = &converted.children[0];
        assert_eq!(root.name, "root");
        assert_eq!(root.children[0].name, "head");
        assert_eq!(root.pose[0..3], [-2.0, 3.0, 4.0]);
        assert!((root.pose[3] + 0.1).abs() < 1e-6);
        let vertex = root.polygons[0].vertices[0];
        assert_eq!(&vertex[..3], &[-2.0, 5.0, 8.0]);
        assert_eq!(&vertex[3..], &[0.5, 0.5]);
        assert_eq!(root.polygons[0].normal, [-1.0, 0.0, 0.0]);
    }

    #[test]
    fn rejects_missing_parent_and_cycles() {
        let missing = YsmBone {
            name: "child".into(), parent: "absent".into(), cubes: vec![],
            pivot: [0.0; 3], rotation_radians: [0.0; 3],
        };
        assert!(matches!(convert_ysm_model(&model(vec![missing])), Err(YsmModelError::MissingParent { .. })));

        let a = YsmBone { name: "a".into(), parent: "b".into(), cubes: vec![], pivot: [0.0; 3], rotation_radians: [0.0; 3] };
        let b = YsmBone { name: "b".into(), parent: "a".into(), cubes: vec![], pivot: [0.0; 3], rotation_radians: [0.0; 3] };
        assert!(matches!(convert_ysm_model(&model(vec![a, b])), Err(YsmModelError::CyclicHierarchy)));
    }
}
