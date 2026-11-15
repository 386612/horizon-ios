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
    DEFAULT_CONTROLLER_STATES.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
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


#[derive(Clone, Debug, Default)]
struct YsmRuntimeAnimations {
    animations: Vec<YsmAnimationInfo>,
    controllers: Vec<horizon_ysm::YsmAnimationController>,
}

static DEFAULT_ANIMATIONS: std::sync::OnceLock<
    std::sync::RwLock<HashMap<String, YsmRuntimeAnimations>>,
> = std::sync::OnceLock::new();

static DEFAULT_CONTROLLER_STATES: std::sync::OnceLock<
    std::sync::Mutex<HashMap<(String, u32, String), String>>,
> = std::sync::OnceLock::new();

/// Registers decoded animation tracks and controller state graphs for a YSM avatar.
pub fn register_default_animations(
    texture: String,
    animations: Vec<YsmAnimationInfo>,
    controllers: Vec<horizon_ysm::YsmAnimationController>,
) {
    let registry = DEFAULT_ANIMATIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    registry.write().unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(texture, YsmRuntimeAnimations { animations, controllers });
}

/// Applies the selected preview/default animation for the YSM model. Numeric keyframes
/// are interpolated; expression-valued Molang components remain available in
/// the parser; supported arithmetic and common math functions are evaluated.
pub fn apply_default_animation(
    texture: &str,
    mob: &super::Mob,
    state: &super::anim::State,
    poses: &mut super::anim::Poses,
) {
    let registry = DEFAULT_ANIMATIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    let guard = registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(runtime) = guard.get(texture) else { return; };
    if runtime.animations.is_empty() { return; }

    let base_context = MolangContext {
        anim_time: state.age / 20.0,
        life_time: state.age / 20.0,
        modified_distance_moved: state.pos,
        ground_speed: state.speed,
        head_x_rotation: state.x_rot,
        head_y_rotation: state.y_rot,
        attack_time: state.swing,
        health: mob.health,
        vertical_speed: mob.velocity.y as f32,
        horizontal_speed: (mob.velocity.x * mob.velocity.x + mob.velocity.z * mob.velocity.z).sqrt() as f32,
        partial_tick: state.partial,
        is_in_water: mob.fluids.in_water,
        is_in_lava: mob.fluids.in_lava,
        is_underwater: mob.fluids.eye_in_water,
        is_on_fire: mob.fire_ticks > 0,
        is_moving: state.speed > 0.01,
        is_on_ground: mob.on_ground,
        is_sprinting: mob.extra.sprinting,
        is_sneaking: mob.extra.crouching,
        is_baby: state.baby,
        is_alive: mob.alive(),
        is_climbing: mob.climbing,
        is_passenger: mob.extra.passenger,
        ..MolangContext::default()
    };
    let mut selected_animations: Vec<(&YsmAnimationInfo, f32)> = Vec::new();
    if !runtime.controllers.is_empty() {
        let registry = DEFAULT_CONTROLLER_STATES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        let mut states = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for controller in &runtime.controllers {
            let key = (texture.to_owned(), mob.id, controller.name.clone());
            let current_state = states.get(&key).map(String::as_str)
                .unwrap_or(controller.initial_state.as_str());
            let next_state = controller_next_state(controller, current_state, base_context);
            states.insert(key, next_state.clone());
            let Some(animation_entries) = controller_state_animations(controller, &next_state) else { continue; };
            for (name, weight) in animation_entries {
                let animation_weight = if weight.is_empty() {
                    1.0
                } else {
                    eval_molang_with_context(weight, base_context).unwrap_or(1.0)
                };
                if animation_weight == 0.0 { continue; }
                let Some(animation) = runtime.animations.iter().find(|animation| {
                    animation.name.as_str() == name.as_str()
                        || animation.name.as_str() == name.strip_prefix("animation.").unwrap_or(name)
                }) else { continue; };
                if let Some((_, existing_weight)) = selected_animations.iter_mut()
                    .find(|(selected, _)| selected.name == animation.name)
                {
                    *existing_weight += animation_weight;
                } else {
                    selected_animations.push((animation, animation_weight));
                }
            }
        }
    }
    if selected_animations.is_empty() {
        selected_animations.push((&runtime.animations[0], 1.0));
    }

    for (animation, animation_weight) in selected_animations {
        let duration = animation.duration_ticks / 20.0;
        let mut time = base_context.life_time;
        if duration > 0.0 && duration.is_finite() {
            time = match animation.loop_mode {
                1 => time.rem_euclid(duration),
                _ => time.min(duration),
            };
        }
    for bone in &animation.bones {
        for (channel_index, channel) in bone.channels.iter().enumerate() {
            let Some(value) = sample_numeric_channel_with_context(
                channel,
                time,
                MolangContext { anim_time: time, ..base_context },
            ) else { continue; };
            let pose = poses.get(&bone.bone_name);
            match channel_index {
                0 => {
                    pose[3] += (-value[0] * animation_weight).to_radians();
                    pose[4] += (-value[1] * animation_weight).to_radians();
                    pose[5] += (value[2] * animation_weight).to_radians();
                }
                1 => {
                    pose[0] -= value[0] * animation_weight;
                    pose[1] += value[1] * animation_weight;
                    pose[2] += value[2] * animation_weight;
                }
                2 => {
                    pose[6] *= 1.0 + (value[0] - 1.0) * animation_weight;
                    pose[7] *= 1.0 + (value[1] - 1.0) * animation_weight;
                    pose[8] *= 1.0 + (value[2] - 1.0) * animation_weight;
                }
                _ => {}
            }
        }
    }
    }

}

#[derive(Clone, Copy, Debug, Default)]
struct MolangContext {
    anim_time: f32,
    life_time: f32,
    modified_distance_moved: f32,
    ground_speed: f32,
    head_x_rotation: f32,
    head_y_rotation: f32,
    attack_time: f32,
    health: f32,
    vertical_speed: f32,
    horizontal_speed: f32,
    partial_tick: f32,
    is_in_water: bool,
    is_in_lava: bool,
    is_underwater: bool,
    is_on_fire: bool,
    is_moving: bool,
    is_on_ground: bool,
    is_sprinting: bool,
    is_sneaking: bool,
    is_baby: bool,
    is_alive: bool,
    is_climbing: bool,
    is_passenger: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum MolangToken {
    Number(f32),
    Ident(String),
    Plus, Minus, Star, Slash, Percent,
    Greater, Less, GreaterEqual, LessEqual, Equal, NotEqual, And, Or, Not,
    LeftParen, RightParen, Comma, Question, Colon,
}

fn tokenize_molang(source: &str) -> Option<Vec<MolangToken>> {
    let source = source.trim().strip_prefix("return ").unwrap_or(source.trim()).trim_end_matches(';').trim();
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() { i += 1; continue; }
        let paired = if i + 1 < bytes.len() {
            match (bytes[i], bytes[i + 1]) {
                (b'>', b'=') => Some(MolangToken::GreaterEqual),
                (b'<', b'=') => Some(MolangToken::LessEqual),
                (b'=', b'=') => Some(MolangToken::Equal),
                (b'!', b'=') => Some(MolangToken::NotEqual),
                (b'&', b'&') => Some(MolangToken::And),
                (b'|', b'|') => Some(MolangToken::Or),
                _ => None,
            }
        } else { None };
        if let Some(token) = paired { tokens.push(token); i += 2; continue; }
        let simple = match c {
            '+' => Some(MolangToken::Plus), '-' => Some(MolangToken::Minus),
            '*' => Some(MolangToken::Star), '/' => Some(MolangToken::Slash),
            '%' => Some(MolangToken::Percent), '>' => Some(MolangToken::Greater),
            '<' => Some(MolangToken::Less), '!' => Some(MolangToken::Not),
            '(' => Some(MolangToken::LeftParen), ')' => Some(MolangToken::RightParen),
            ',' => Some(MolangToken::Comma), '?' => Some(MolangToken::Question),
            ':' => Some(MolangToken::Colon),
            _ => None,
        };
        if let Some(token) = simple { tokens.push(token); i += 1; continue; }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
            }
            if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
                i += 1;
                if i < bytes.len() && matches!(bytes[i], b'+' | b'-') { i += 1; }
                let exponent_start = i;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
                if exponent_start == i { return None; }
            }
            let value = source.get(start..i)?.parse::<f32>().ok()?;
            if !value.is_finite() { return None; }
            tokens.push(MolangToken::Number(value));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            i += 1;
            while i < bytes.len() && ((bytes[i] as char).is_ascii_alphanumeric() || matches!(bytes[i] as char, '_' | '.')) { i += 1; }
            tokens.push(MolangToken::Ident(source.get(start..i)?.to_owned()));
            continue;
        }
        return None;
    }
    Some(tokens)
}

struct MolangParser {
    tokens: Vec<MolangToken>,
    cursor: usize,
    context: MolangContext,
}

impl MolangParser {
    fn expression(&mut self) -> Option<f32> {
        let condition = self.logical_or()?;
        if self.tokens.get(self.cursor) == Some(&MolangToken::Question) {
            self.cursor += 1;
            let when_true = self.expression()?;
            if self.tokens.get(self.cursor) != Some(&MolangToken::Colon) { return None; }
            self.cursor += 1;
            let when_false = self.expression()?;
            Some(if condition != 0.0 { when_true } else { when_false })
        } else {
            Some(condition)
        }
    }
    fn logical_or(&mut self) -> Option<f32> {
        let mut value = self.logical_and()?;
        while self.tokens.get(self.cursor) == Some(&MolangToken::Or) {
            self.cursor += 1;
            let rhs = self.logical_and()?;
            value = if value != 0.0 || rhs != 0.0 { 1.0 } else { 0.0 };
        }
        Some(value)
    }
    fn logical_and(&mut self) -> Option<f32> {
        let mut value = self.equality()?;
        while self.tokens.get(self.cursor) == Some(&MolangToken::And) {
            self.cursor += 1;
            let rhs = self.equality()?;
            value = if value != 0.0 && rhs != 0.0 { 1.0 } else { 0.0 };
        }
        Some(value)
    }
    fn equality(&mut self) -> Option<f32> {
        let mut value = self.comparison()?;
        loop {
            match self.tokens.get(self.cursor) {
                Some(MolangToken::Equal) => { self.cursor += 1; value = if value == self.comparison()? { 1.0 } else { 0.0 }; }
                Some(MolangToken::NotEqual) => { self.cursor += 1; value = if value != self.comparison()? { 1.0 } else { 0.0 }; }
                _ => break,
            }
        }
        Some(value)
    }
    fn comparison(&mut self) -> Option<f32> {
        let mut value = self.additive()?;
        loop {
            match self.tokens.get(self.cursor) {
                Some(MolangToken::Greater) => { self.cursor += 1; value = if value > self.additive()? { 1.0 } else { 0.0 }; }
                Some(MolangToken::Less) => { self.cursor += 1; value = if value < self.additive()? { 1.0 } else { 0.0 }; }
                Some(MolangToken::GreaterEqual) => { self.cursor += 1; value = if value >= self.additive()? { 1.0 } else { 0.0 }; }
                Some(MolangToken::LessEqual) => { self.cursor += 1; value = if value <= self.additive()? { 1.0 } else { 0.0 }; }
                _ => break,
            }
        }
        Some(value)
    }
    fn additive(&mut self) -> Option<f32> {
        let mut value = self.multiplicative()?;
        loop {
            match self.tokens.get(self.cursor) {
                Some(MolangToken::Plus) => { self.cursor += 1; value += self.multiplicative()?; }
                Some(MolangToken::Minus) => { self.cursor += 1; value -= self.multiplicative()?; }
                _ => break,
            }
        }
        Some(value)
    }
    fn multiplicative(&mut self) -> Option<f32> {
        let mut value = self.unary()?;
        loop {
            match self.tokens.get(self.cursor) {
                Some(MolangToken::Star) => { self.cursor += 1; value *= self.unary()?; }
                Some(MolangToken::Slash) => { self.cursor += 1; let rhs = self.unary()?; if rhs == 0.0 { return None; } value /= rhs; }
                Some(MolangToken::Percent) => { self.cursor += 1; let rhs = self.unary()?; if rhs == 0.0 { return None; } value %= rhs; }
                _ => break,
            }
        }
        Some(value)
    }
    fn unary(&mut self) -> Option<f32> {
        match self.tokens.get(self.cursor) {
            Some(MolangToken::Plus) => { self.cursor += 1; self.unary() }
            Some(MolangToken::Minus) => { self.cursor += 1; Some(-self.unary()?) }
            Some(MolangToken::Not) => { self.cursor += 1; Some(if self.unary()? == 0.0 { 1.0 } else { 0.0 }) }
            _ => self.primary(),
        }
    }
    fn primary(&mut self) -> Option<f32> {
        match self.tokens.get(self.cursor)?.clone() {
            MolangToken::Number(value) => { self.cursor += 1; Some(value) }
            MolangToken::LeftParen => {
                self.cursor += 1;
                let value = self.expression()?;
                if self.tokens.get(self.cursor) != Some(&MolangToken::RightParen) { return None; }
                self.cursor += 1;
                Some(value)
            }
            MolangToken::Ident(name) => {
                self.cursor += 1;
                if self.tokens.get(self.cursor) == Some(&MolangToken::LeftParen) {
                    self.cursor += 1;
                    let mut args = Vec::new();
                    if self.tokens.get(self.cursor) != Some(&MolangToken::RightParen) {
                        loop {
                            args.push(self.expression()?);
                            if self.tokens.get(self.cursor) == Some(&MolangToken::Comma) { self.cursor += 1; continue; }
                            break;
                        }
                    }
                    if self.tokens.get(self.cursor) != Some(&MolangToken::RightParen) { return None; }
                    self.cursor += 1;
                    return match (name.as_str(), args.as_slice()) {
                        ("math.sin", [x]) => Some(x.to_radians().sin()),
                        ("math.cos", [x]) => Some(x.to_radians().cos()),
                        ("math.abs", [x]) => Some((*x).abs()),
                        ("math.floor", [x]) => Some((*x).floor()),
                        ("math.ceil", [x]) => Some((*x).ceil()),
                        ("math.round", [x]) => Some((*x).round()),
                        ("math.sqrt", [x]) if *x >= 0.0 => Some((*x).sqrt()),
                        ("math.pow", [a,b]) => Some(a.powf(*b)),
                        ("math.mod", [a,b]) if *b != 0.0 => Some(a.rem_euclid(*b)),
                        ("math.atan", [x]) => Some(x.atan().to_degrees()),
                        ("math.asin", [x]) if (-1.0..=1.0).contains(x) => Some(x.asin().to_degrees()),
                        ("math.acos", [x]) if (-1.0..=1.0).contains(x) => Some(x.acos().to_degrees()),
                        ("math.atan2", [y,x]) => Some(y.atan2(*x).to_degrees()),
                        ("math.ln", [x]) if *x > 0.0 => Some(x.ln()),
                        ("math.exp", [x]) => Some(x.exp()),
                        ("math.min", [a,b]) => Some((*a).min(*b)),
                        ("math.max", [a,b]) => Some((*a).max(*b)),
                        ("math.clamp", [x,a,b]) if *a <= *b => Some((*x).clamp(*a,*b)),
                        ("math.lerp", [a,b,t]) => Some(*a + (*b-*a)* *t),
                        _ => None,
                    };
                }
                match name.as_str() {
                    "query.anim_time" => Some(self.context.anim_time),
                    "query.life_time" | "query.time" => Some(self.context.life_time),
                    "query.modified_distance_moved" => Some(self.context.modified_distance_moved),
                    "query.ground_speed" => Some(self.context.ground_speed),
                    "query.head_x_rotation" => Some(self.context.head_x_rotation),
                    "query.head_y_rotation" => Some(self.context.head_y_rotation),
                    "query.attack_time" => Some(self.context.attack_time),
                    "query.health" => Some(self.context.health),
                    "query.vertical_speed" => Some(self.context.vertical_speed),
                    "query.horizontal_speed" => Some(self.context.horizontal_speed),
                    "query.frame_alpha" => Some(self.context.partial_tick),
                    "query.is_in_water" => Some(self.context.is_in_water as u8 as f32),
                    "query.is_in_lava" => Some(self.context.is_in_lava as u8 as f32),
                    "query.is_underwater" => Some(self.context.is_underwater as u8 as f32),
                    "query.is_on_fire" => Some(self.context.is_on_fire as u8 as f32),
                    "query.is_moving" => Some(self.context.is_moving as u8 as f32),
                    "query.is_on_ground" => Some(self.context.is_on_ground as u8 as f32),
                    "query.is_sprinting" => Some(self.context.is_sprinting as u8 as f32),
                    "query.is_sneaking" | "query.is_crouching" => Some(self.context.is_sneaking as u8 as f32),
                    "query.is_baby" => Some(self.context.is_baby as u8 as f32),
                    "query.is_alive" => Some(self.context.is_alive as u8 as f32),
                    "query.is_climbing" => Some(self.context.is_climbing as u8 as f32),
                    "query.is_passenger" | "query.is_riding" => Some(self.context.is_passenger as u8 as f32),
                    "math.pi" => Some(std::f32::consts::PI),
                    "math.tau" => Some(std::f32::consts::TAU),
                    "math.e" => Some(std::f32::consts::E),
                    "true" => Some(1.0),
                    "false" => Some(0.0),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

fn controller_next_state(
    controller: &horizon_ysm::YsmAnimationController,
    current_state: &str,
    context: MolangContext,
) -> String {
    let mut next_state = current_state.to_owned();
    if let Some((_, state)) = controller.states.iter().find(|(name, _)| name == current_state) {
        // Serialized transitions are (target state, condition), in declaration order.
        for (target, condition) in &state.transitions {
            if eval_molang_with_context(condition, context).is_some_and(|value| value != 0.0) {
                next_state = target.clone();
                break;
            }
        }
    } else {
        next_state = controller.initial_state.clone();
    }
    next_state
}

fn controller_state_animations<'a>(
    controller: &'a horizon_ysm::YsmAnimationController,
    state_name: &str,
) -> Option<&'a [(String, String)]> {
    controller.states.iter()
        .find(|(name, _)| name == state_name)
        .map(|(_, state)| state.animations.as_slice())
}


fn eval_molang(expression: &str, anim_time: f32) -> Option<f32> {
    eval_molang_with_context(expression, MolangContext {
        anim_time,
        life_time: anim_time,
        ..MolangContext::default()
    })
}

fn eval_molang_with_context(expression: &str, context: MolangContext) -> Option<f32> {
    let mut parser = MolangParser { tokens: tokenize_molang(expression)?, cursor: 0, context };
    let value = parser.expression()?;
    (parser.cursor == parser.tokens.len() && value.is_finite()).then_some(value)
}

fn sample_numeric_channel(
    channel: &horizon_ysm::YsmAnimationChannel,
    time: f32,
) -> Option<[f32; 3]> {
    sample_numeric_channel_with_context(channel, time, MolangContext {
        anim_time: time,
        life_time: time,
        ..MolangContext::default()
    })
}

fn sample_numeric_channel_with_context(
    channel: &horizon_ysm::YsmAnimationChannel,
    time: f32,
    context: MolangContext,
) -> Option<[f32; 3]> {
    let frames = &channel.keyframes;
    let first = frames.first()?;
    let numeric = |values: &[YsmMolangValue; 3]| -> Option<[f32; 3]> {
        let result = std::array::from_fn(|i| match &values[i] {
            YsmMolangValue::Number(value) => value.is_finite().then_some(*value),
            YsmMolangValue::Expression(expression) => eval_molang_with_context(expression, context),
        });
        let sampled = [result[0]?, result[1]?, result[2]?];
        sampled.iter().all(|value| value.is_finite()).then_some(sampled)
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
    fn selects_controller_initial_or_transition_animation() {
        let controller = horizon_ysm::YsmAnimationController {
            name: "controller.test".into(),
            initial_state: "idle".into(),
            states: vec![
                ("idle".into(), horizon_ysm::YsmControllerState {
                    animations: vec![("animation.idle".into(), String::new())],
                    transitions: vec![("run".into(), "query.anim_time > 1".into())],
                    ..Default::default()
                }),
                ("run".into(), horizon_ysm::YsmControllerState {
                    animations: vec![("animation.run".into(), String::new())],
                    ..Default::default()
                }),
            ],
        };
        let idle = controller_next_state(&controller, "idle", MolangContext { anim_time: 0.5, life_time: 0.5, ..MolangContext::default() });
        assert_eq!(idle, "idle");
        assert_eq!(controller_state_animations(&controller, &idle).unwrap()[0].0, "animation.idle");
        let running = controller_next_state(&controller, &idle, MolangContext { anim_time: 1.5, life_time: 1.5, ..MolangContext::default() });
        assert_eq!(running, "run");
        assert_eq!(controller_state_animations(&controller, &running).unwrap()[0].0, "animation.run");
        // A state remains active until one of its own transitions changes it.
        assert_eq!(controller_next_state(&controller, &running, MolangContext::default()), "run");
    }

    #[test]
    fn evaluates_common_molang_math_and_animation_queries() {
        assert_eq!(eval_molang("query.anim_time * 2 + 1", 3.0), Some(7.0));
        assert_eq!(eval_molang("query.anim_time > 2 && query.anim_time <= 4", 3.0), Some(1.0));
        assert_eq!(eval_molang("!(query.anim_time < 2)", 3.0), Some(1.0));
        assert_eq!(eval_molang("query.anim_time > 2 ? 10 : 0", 3.0), Some(10.0));
        let context = MolangContext {
            is_on_ground: true,
            is_in_water: true,
            is_sneaking: true,
            ground_speed: 0.8,
            ..MolangContext::default()
        };
        assert_eq!(eval_molang_with_context("query.is_on_ground && query.ground_speed > 0.5", context), Some(1.0));
        assert_eq!(eval_molang_with_context("query.is_in_water && query.is_crouching", context), Some(1.0));
        assert_eq!(eval_molang("math.sin(90)", 0.0), Some(1.0));
        assert_eq!(eval_molang("math.clamp(query.life_time, 0, 2)", 3.0), Some(2.0));
        assert_eq!(eval_molang("unknown.query", 1.0), None);
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
