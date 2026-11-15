//! Conversion of decoded YSM geometry into Horizon's hierarchical mob model.
use super::{Part, Polygon};
use horizon_ysm::{YsmAnimationInfo, YsmMolangValue, YsmModel};
use std::{collections::HashMap, sync::Arc};
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

/// Applies the model-level YSM width/height scale to a model root or arm part.
/// YSM's renderer scales X/Z by width_scale and Y by height_scale; the default
/// for legacy models without metadata is 0.7 on both axes.
pub fn apply_ysm_model_scale(part: &mut Part, width_scale: f32, height_scale: f32) {
    let width = if width_scale.is_finite() { width_scale } else { 0.7 };
    let height = if height_scale.is_finite() { height_scale } else { 0.7 };
    part.pose[6] = width;
    part.pose[7] = height;
    part.pose[8] = width;
}

/// Converts a decoded YSM model to the same Part/Polygon representation used
/// by Horizon's mob renderer.
///
/// YSM stores face vertices in normalized model units (the upstream parser
/// multiplies them by 16 when rebuilding Blockbench geometry) and UVs as
/// normalized texture coordinates. Its
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
    for &root in &roots {
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
                                -vertex.position[0] * 16.0 - pivot[0],
                                vertex.position[1] * 16.0 - pivot[1],
                                vertex.position[2] * 16.0 - pivot[2],
                                vertex.uv[0],
                                vertex.uv[1],
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
    for &root in &roots {
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
    use horizon_ysm::{YsmBone, YsmCube, YsmFace, YsmVertex};

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
                YsmVertex { position: [4.0, 8.0, 12.0], uv: [0.5, 0.5] },
                YsmVertex { position: [4.0, 8.0, 14.0], uv: [0.75, 0.5] },
                YsmVertex { position: [4.0, 10.0, 14.0], uv: [0.75, 0.75] },
                YsmVertex { position: [4.0, 10.0, 12.0], uv: [0.5, 0.75] },
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
        assert_eq!(&vertex[..3], &[-62.0, 125.0, 188.0]);
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


static DEFAULT_PLAYER_MODEL: std::sync::OnceLock<
    std::sync::RwLock<Option<(Arc<Part>, String)>>,
> = std::sync::OnceLock::new();
static DEFAULT_PLAYER_ARMS: std::sync::OnceLock<
    std::sync::RwLock<Option<[Part; 2]>>,
> = std::sync::OnceLock::new();

/// Publishes the currently selected YSM geometry for both mannequin and player
/// render states, which share Horizon's avatar renderer.
pub fn register_default_player_model(model: Arc<Part>, texture: String) {
    let registry = DEFAULT_PLAYER_MODEL.get_or_init(|| std::sync::RwLock::new(None));
    *registry.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((model, texture));
}

/// Clears a previous world/resource selection before scanning the next one.
pub fn clear_registered_player_model() {
    let registry = DEFAULT_PLAYER_MODEL.get_or_init(|| std::sync::RwLock::new(None));
    *registry.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let arms = DEFAULT_PLAYER_ARMS.get_or_init(|| std::sync::RwLock::new(None));
    *arms.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let animations = DEFAULT_ANIMATIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    animations.write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// Applies the selected YSM avatar model to the player type created by the
/// perspective/resource bootstrap.
/// Registers the converted right/left first-person arm parts from the YSM arm model.
pub fn register_default_player_arms(arms: [Part; 2]) {
    let registry = DEFAULT_PLAYER_ARMS.get_or_init(|| std::sync::RwLock::new(None));
    *registry.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(arms);
}

/// Replaces the wide and slim first-person arm parts when a YSM arm model exists.
pub fn apply_registered_player_arms(arms: &mut [Option<Part>; 2]) {
    let registry = DEFAULT_PLAYER_ARMS.get_or_init(|| std::sync::RwLock::new(None));
    let guard = registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(replacement) = guard.as_ref() {
        arms[0] = Some(replacement[0].clone());
        arms[1] = Some(replacement[1].clone());
    }
}

pub fn apply_registered_player_model(mob_type: &mut super::MobType) {
    let registry = DEFAULT_PLAYER_MODEL.get_or_init(|| std::sync::RwLock::new(None));
    let guard = registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((model, texture)) = guard.as_ref() else {
        return;
    };
    mob_type.model = model.clone();
    mob_type.layers.insert("player_slim#main".to_owned(), model.clone());
    mob_type.texture = texture.clone();
}


static DEFAULT_ANIMATIONS: std::sync::OnceLock<
    std::sync::RwLock<HashMap<String, Vec<YsmAnimationInfo>>>,
> = std::sync::OnceLock::new();

/// Registers the default animation tracks for a loaded YSM avatar texture.
pub fn register_default_animations(texture: String, animations: Vec<YsmAnimationInfo>) {
    let registry = DEFAULT_ANIMATIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    registry.write().unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(texture, animations);
}

/// Applies the selected preview/default animation for the YSM model. Numeric keyframes
/// are interpolated; expression-valued Molang components remain available in
/// the parsed resource but are not evaluated by this numeric fallback.
pub fn apply_default_animation(
    texture: &str,
    age_ticks: f32,
    poses: &mut super::anim::Poses,
) {
    let registry = DEFAULT_ANIMATIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    let guard = registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(animation) = guard.get(texture).and_then(|animations| animations.first()) else {
        return;
    };
    let duration = animation.duration_ticks / 20.0;
    let mut time = age_ticks / 20.0;
    if duration > 0.0 && duration.is_finite() {
        time = match animation.loop_mode {
            1 => time.rem_euclid(duration),
            _ => time.min(duration),
        };
    }
    for bone in &animation.bones {
        for (channel_index, channel) in bone.channels.iter().enumerate() {
            let Some(value) = sample_numeric_channel(channel, time) else {
                continue;
            };
            let pose = poses.get(&bone.bone_name);
            match channel_index {
                0 => {
                    pose[3] += (-value[0]).to_radians();
                    pose[4] += (-value[1]).to_radians();
                    pose[5] += value[2].to_radians();
                }
                1 => {
                    pose[0] -= value[0];
                    pose[1] += value[1];
                    pose[2] += value[2];
                }
                2 => {
                    pose[6] *= value[0];
                    pose[7] *= value[1];
                    pose[8] *= value[2];
                }
                _ => {}
            }
        }
    }
}

fn sample_numeric_channel(
    channel: &horizon_ysm::YsmAnimationChannel,
    time: f32,
) -> Option<[f32; 3]> {
    let frames = &channel.keyframes;
    let first = frames.first()?;
    let numeric = |values: &[YsmMolangValue; 3]| -> Option<[f32; 3]> {
        let result = std::array::from_fn(|i| match &values[i] {
            YsmMolangValue::Number(value) => *value,
            YsmMolangValue::Expression(_) => f32::NAN,
        });
        result.iter().all(|value| value.is_finite()).then_some(result)
    };
    if time <= first.time {
        return numeric(&first.post);
    }
    for (index, pair) in frames.windows(2).enumerate() {
        let (from, to) = (&pair[0], &pair[1]);
        if time <= to.time {
            let a = numeric(&from.post)?;
            if from.lerp_mode == 1 || to.time <= from.time {
                return Some(a);
            }
            let b = numeric(to.pre.as_ref().unwrap_or(&to.post))?;
            let t = ((time - from.time) / (to.time - from.time)).clamp(0.0, 1.0);
            if from.lerp_mode == 2 {
                let p0 = if index == 0 {
                    a
                } else {
                    numeric(&frames[index - 1].post).unwrap_or(a)
                };
                let p3 = if index + 2 >= frames.len() {
                    b
                } else {
                    numeric(frames[index + 2].pre.as_ref().unwrap_or(&frames[index + 2].post))
                        .unwrap_or(b)
                };
                let t2 = t * t;
                let t3 = t2 * t;
                return Some(std::array::from_fn(|i| {
                    0.5 * ((2.0 * a[i])
                        + (-p0[i] + b[i]) * t
                        + (2.0 * p0[i] - 5.0 * a[i] + 4.0 * b[i] - p3[i]) * t2
                        + (-p0[i] + 3.0 * a[i] - 3.0 * b[i] + p3[i]) * t3)
                }));
            }
            return Some(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
        }
    }
    numeric(&frames.last()?.post)
}


#[cfg(test)]
mod animation_sampling_tests {
    use super::*;
    use horizon_ysm::{YsmAnimationChannel, YsmKeyframe, YsmMolangValue};

    fn frame(time: f32, value: f32, lerp_mode: u64) -> YsmKeyframe {
        YsmKeyframe {
            time,
            lerp_mode,
            post: std::array::from_fn(|_| YsmMolangValue::Number(value)),
            pre: None,
        }
    }

    #[test]
    fn samples_linear_and_step_keyframes() {
        let linear = YsmAnimationChannel {
            keyframes: vec![frame(0.0, 0.0, 0), frame(1.0, 10.0, 0)],
        };
        assert_eq!(sample_numeric_channel(&linear, 0.5), Some([5.0; 3]));

        let step = YsmAnimationChannel {
            keyframes: vec![frame(0.0, 2.0, 1), frame(1.0, 10.0, 0)],
        };
        assert_eq!(sample_numeric_channel(&step, 0.5), Some([2.0; 3]));
    }

    #[test]
    fn samples_catmull_rom_keyframes() {
        let channel = YsmAnimationChannel {
            keyframes: vec![
                frame(0.0, 0.0, 2),
                frame(1.0, 10.0, 0),
                frame(2.0, 0.0, 0),
            ],
        };
        assert_eq!(sample_numeric_channel(&channel, 1.0), Some([10.0; 3]));
        assert_eq!(sample_numeric_channel(&channel, 0.5), Some([5.625; 3]));
    }
}


#[cfg(test)]
mod model_scale_tests {
    use super::*;

    #[test]
    fn applies_ysm_width_and_height_scale_to_the_root_pose() {
        let mut part = Part {
            name: "root".into(),
            pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            polygons: Vec::new(),
            children: Vec::new(),
        };
        apply_ysm_model_scale(&mut part, 0.8, 0.65);
        assert_eq!(&part.pose[6..9], &[0.8, 0.65, 0.8]);
    }

    #[test]
    fn falls_back_for_non_finite_ysm_scale_values() {
        let mut part = Part {
            name: "root".into(),
            pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            polygons: Vec::new(),
            children: Vec::new(),
        };
        apply_ysm_model_scale(&mut part, f32::NAN, f32::INFINITY);
        assert_eq!(&part.pose[6..9], &[0.7, 0.7, 0.7]);
    }
}
