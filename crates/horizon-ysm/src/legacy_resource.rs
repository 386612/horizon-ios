//! YSM resource table readers for legacy formats 4-15 and modern formats 16-25.
use crate::resource::{
    encode_rgba_texture, parse_model_record, ResourceError, ResourceReader, YsmModel, YsmTexture,
};

#[derive(Clone, Debug, PartialEq)]
pub struct YsmResource {
    pub format: u32,
    pub models: Vec<(String, YsmModel)>,
    pub textures: Vec<YsmTexture>,
    pub animations: Vec<YsmAnimationInfo>,
    pub controllers: Vec<YsmAnimationController>,
    pub sounds: Vec<YsmSound>,
    pub metadata: Option<YsmMetadata>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmAuthor {
    pub name: String,
    pub role: String,
    pub contacts: Vec<(String, String)>,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmMetadata {
    pub sha256: String,
    pub name: String,
    pub tips: String,
    pub license_type: String,
    pub license_description: String,
    pub authors: Vec<YsmAuthor>,
    pub links: Vec<(String, String)>,
    pub width_scale: f32,
    pub height_scale: f32,
    pub extra_animations: Vec<(String, String)>,
    pub extra_animation_buttons: Vec<YsmExtraAnimationButton>,
    pub extra_animation_classify: Vec<YsmExtraAnimationClassify>,
    pub default_texture: String,
    pub preview_animation: String,
    pub free: bool,
    pub render_layers_first: Option<bool>,
    pub all_cutout: Option<bool>,
    pub disable_preview_rotation: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmExtraAnimationConfigForm {
    pub kind: String,
    pub title: String,
    pub description: String,
    pub value: String,
    pub step: f32,
    pub min: f32,
    pub max: f32,
    pub labels: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmExtraAnimationButton {
    pub id: String,
    pub name: String,
    pub config_forms: Vec<YsmExtraAnimationConfigForm>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmExtraAnimationClassify {
    pub id: String,
    pub extra_animations: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmSound {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct YsmControllerState {
    pub animations: Vec<(String, String)>,
    pub transitions: Vec<(String, String)>,
    pub on_entry: Vec<String>,
    pub on_exit: Vec<String>,
    pub blend_transition: Option<f32>,
    pub blend_transitions: Vec<(f32, f32)>,
    pub blend_via_shortest_path: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct YsmAnimationController {
    pub name: String,
    pub initial_state: String,
    pub states: Vec<(String, YsmControllerState)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum YsmMolangValue {
    Number(f32),
    Expression(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmKeyframe {
    /// Keyframe timestamp in seconds (the file stores ticks at 20 TPS).
    pub time: f32,
    /// YSM lerp enum: 0 linear, 1 step, 2 catmull-rom.
    pub lerp_mode: u64,
    pub post: [YsmMolangValue; 3],
    pub pre: Option<[YsmMolangValue; 3]>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct YsmAnimationChannel {
    pub keyframes: Vec<YsmKeyframe>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmBoneAnimation {
    pub bone_name: String,
    /// Rotation, position, scale, in that order.
    pub channels: [YsmAnimationChannel; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmTimelineEvent {
    pub events: Vec<String>,
    /// Timeline timestamp in seconds.
    pub time: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmAnimationInfo {
    pub model_id: u32,
    pub name: String,
    /// Duration in file ticks (20 ticks per second).
    pub duration_ticks: f32,
    pub loop_mode: u64,
    pub blend_weight: Option<YsmMolangValue>,
    pub bones: Vec<YsmBoneAnimation>,
    pub timeline: Vec<YsmTimelineEvent>,
}

/// Parses the legacy resource layout used by YSM format versions 4-15.
/// Animation records retain their numeric and Molang keyframes for runtime use.
pub fn parse_resource_v4_v15(
    bytes: &[u8],
    expected_format: u32,
) -> Result<(YsmResource, usize), ResourceError> {
    if !(4..=15).contains(&expected_format) {
        return Err(ResourceError::InvalidModel("resource layout supports formats 4 through 15"));
    }
    let mut reader = ResourceReader::new(bytes);
    if reader.read_u32()? != expected_format {
        return Err(ResourceError::InvalidModel("resource version differs from header"));
    }

    let prefix_len = reader.read_count("prefix length")?;
    reader.read_exact(prefix_len)?;

    // The legacy table owns its model and animation collections. Keep them
    // separate from the format 16+ parser's similarly named tables below.
    let mut models = Vec::new();
    let mut animations = Vec::new();

    for _ in 0..reader.read_count("model count")? {
        let id = reader.read_varint()?;
        let _unknown = reader.read_varint()?;
        let start = reader.position();
        let (model, consumed) = parse_model_record(&bytes[start..], expected_format)?;
        reader.read_exact(consumed)?;
        let name = match id {
            1 => "main",
            2 => "arm",
            3 => "arrow",
            _ => return Err(ResourceError::InvalidModel("unknown model id")),
        };
        models.push((name.to_owned(), model));
    }

    for _ in 0..reader.read_count("animation blob count")? {
        let model_id = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::LengthLimit("animation model id"))?;
        let _unknown = reader.read_varint()?;
        animations.extend(parse_animation_group(&mut reader, model_id, expected_format)?);
    }

    // Formats 10-15 add animation controllers before texture records.
    let mut controllers = Vec::new();
    if expected_format > 9 {
        for _ in 0..reader.read_count("animation controller count")? {
            // In formats through 15 the outer controller is stored under a
            // generic name and carries an unused hash/index varint.
            let _controller_hash = reader.read_varint()?;
            controllers.extend(parse_controller_group(&mut reader, expected_format)?);
        }
        for _ in 0..reader.read_count("animation controller hash table count")? {
            let _name = reader.read_string()?;
            let _hash = reader.read_string()?;
        }
    }

    let mut textures = Vec::new();
    let mut sounds = Vec::new();
    for _ in 0..reader.read_count("texture count")? {
        let name = reader.read_string()?;
        let rgba = reader.read_bytes()?;
        let width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        textures.push(encode_rgba_texture(name.clone(), width, height, &rgba)?);

        for _ in 0..reader.read_count("subtexture count")? {
            let kind = reader.read_varint()?;
            let rgba = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let suffix = match kind {
                1 => "_normal",
                2 => "_specular",
                _ => "_special",
            };
            textures.push(encode_rgba_texture(format!("{name}{suffix}"), width, height, &rgba)?);
        }
    }

    if expected_format > 9 {
        for _ in 0..reader.read_count("sound count")? {
            let name = reader.read_string()?;
            let data = reader.read_bytes()?;
            sounds.push(YsmSound { name, data });
        }
        for _ in 0..reader.read_count("sound hash table count")? {
            let _name = reader.read_string()?;
            let _hash = reader.read_string()?;
        }
    }

    for _ in 0..reader.read_count("extra texture count")? {
        let name = reader.read_string()?;
        let rgba = reader.read_bytes()?;
        let width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        textures.push(encode_rgba_texture(format!("avatar/{name}"), width, height, &rgba)?);
    }

    for _ in 0..reader.read_count("model hash table count")? {
        let _id = reader.read_varint()?;
        let _hash = reader.read_string()?;
    }
    for _ in 0..reader.read_count("animation hash table count")? {
        let _id = reader.read_varint()?;
        let _hash = reader.read_string()?;
    }
    for _ in 0..reader.read_count("texture hash table count")? {
        let _name = reader.read_string()?;
        let _hash = reader.read_string()?;
        for _ in 0..reader.read_count("subtexture hash count")? {
            let _kind = reader.read_varint()?;
            let _hash = reader.read_string()?;
        }
    }

    let metadata = parse_ysm_metadata(&mut reader, expected_format, None)?;
    Ok((
        YsmResource {
            format: expected_format,
            models,
            textures,
            animations,
            controllers,
            sounds,
            metadata,
        },
        reader.position(),
    ))
}

fn shared_model_name(id: u64, format: u32) -> String {
    let name = match id {
        1 => "main",
        2 => "arm",
        3 => "extra",
        4 => "tac",
        5 => "arrow",
        6 => "carryon",
        7 => "parcool",
        8 => "swem",
        9 => "slashblade",
        10 => "tlm",
        11 if format >= 32 => "fp.arm",
        11 => "fp_arm",
        12 => "immersive_melodies",
        13 if format >= 32 => "iss",
        13 => "irons_spell_books",
        _ => return format!("unk_{id}"),
    };
    name.to_owned()
}

/// Parses the modern YSM resource layout used by formats 16 through 25.
pub fn parse_resource_v16_v25(
    bytes: &[u8],
    expected_format: u32,
) -> Result<(YsmResource, usize), ResourceError> {
    if !(16..=25).contains(&expected_format) {
        return Err(ResourceError::InvalidModel("modern resource layout supports formats 16 through 25"));
    }
    let mut reader = ResourceReader::new(bytes);
    if reader.read_u32()? != expected_format {
        return Err(ResourceError::InvalidModel("resource version differs from header"));
    }

    // Shared files preceding the entity data: sounds, functions and language tables.
    let mut sounds = Vec::new();
    for _ in 0..reader.read_count("sound count")? {
        let name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let data = reader.read_bytes()?;
        sounds.push(YsmSound { name, data });
    }
    for _ in 0..reader.read_count("function count")? {
        let _name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let _data = reader.read_bytes()?;
    }
    for _ in 0..reader.read_count("language file count")? {
        let _name = reader.read_string()?;
        let _hash = reader.read_string()?;
        for _ in 0..reader.read_count("language node count")? {
            let _key = reader.read_string()?;
            let _value = reader.read_string()?;
        }
    }

    let mut animations = Vec::new();
    let mut controllers = Vec::new();
    let mut textures = Vec::new();
    let mut models = Vec::new();
    // Format 16-25 has one named sub-entity section. Its model is duplicated
    // in the final model table, where ids 1 and 2 identify main and arm.
    for _ in 0..reader.read_count("sub-entity count")? {
        let raw_sub_entity_name = reader.read_string()?;
        let sub_entity_name = raw_sub_entity_name.rsplit(':').next().unwrap_or(&raw_sub_entity_name);
        let sub_entity_model_id = match sub_entity_name {
            "main" => 1,
            "arm" => 2,
            _ => 3,
        };
        if reader.read_varint()? != 0 {
            animations.extend(parse_animation_group(&mut reader, sub_entity_model_id, expected_format)?);
        }
        if reader.read_varint()? != 0 {
            let _controller_hash = reader.read_string()?;
            controllers.extend(parse_controller_group(&mut reader, expected_format)?);
        }
        let _texture_hash = reader.read_string()?;
        let texture_data = reader.read_bytes()?;
        let texture_width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let texture_height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let _image_format = reader.read_varint()?;
        let _unknown_flag = reader.read_varint()?;
        if !texture_data.is_empty() {
            textures.push(parse_modern_image(sub_entity_name.to_owned(), texture_data, texture_width, texture_height)?);
        }
        for _ in 0..reader.read_count("sub-entity subtexture count")? {
            let kind = reader.read_varint()?;
            let _hash = reader.read_string()?;
            let data = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let _image_format = reader.read_varint()?;
            let _unknown_flag = reader.read_varint()?;
            if !data.is_empty() {
                let suffix = match kind {
                    1 => "_normal",
                    2 => "_specular",
                    _ => "_special",
                };
                textures.push(parse_modern_image(format!("{sub_entity_name}{suffix}"), data, width, height)?);
            }
        }
        let start = reader.position();
        let (model, consumed) = parse_model_record(&bytes[start..], expected_format)?;
        reader.read_exact(consumed)?;
        let model_name = if matches!(sub_entity_name, "main" | "arm") {
            sub_entity_name.to_owned()
        } else {
            format!("subentity/{sub_entity_name}")
        };
        models.push((model_name, model));
    }
    let _footer_flag = reader.read_varint()?;
    if reader.read_varint()? != 1 {
        return Err(ResourceError::InvalidModel("invalid modern sub-entity section marker"));
    }

    for _ in 0..reader.read_count("animation blob count")? {
        let model_id = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::LengthLimit("animation model id"))?;
        animations.extend(parse_animation_group(&mut reader, model_id, expected_format)?);
    }

    for _ in 0..reader.read_count("animation controller count")? {
        let _controller_name = reader.read_string()?;
        let _controller_hash = reader.read_string()?;
        controllers.extend(parse_controller_group(&mut reader, expected_format)?);
    }

    for _ in 0..reader.read_count("texture count")? {
        let name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let data = reader.read_bytes()?;
        let width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let _image_format = reader.read_varint()?;
        let _unknown_flag = reader.read_varint()?;
        if !data.is_empty() {
            textures.push(parse_modern_image(name.clone(), data, width, height)?);
        }
        for _ in 0..reader.read_count("texture subtexture count")? {
            let kind = reader.read_varint()?;
            let _hash = reader.read_string()?;
            let data = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let _image_format = reader.read_varint()?;
            let _unknown_flag = reader.read_varint()?;
            let suffix = match kind {
                1 => "_normal",
                2 => "_specular",
                _ => "_special",
            };
            if !data.is_empty() {
                textures.push(parse_modern_image(format!("{name}{suffix}"), data, width, height)?);
            }
        }
    }

    // Keep the geometry parsed from the sub-entity section above. The final
    // model table adds shared player/arm/extra geometry; it must not shadow
    // and discard the category-specific models.
    for _ in 0..reader.read_count("model count")? {
        let id = reader.read_varint()?;
        let start = reader.position();
        let (model, consumed) = parse_model_record(&bytes[start..], expected_format)?;
        reader.read_exact(consumed)?;
        let name = shared_model_name(id, expected_format);
        models.push((name, model));
    }

    let metadata = parse_ysm_metadata(&mut reader, expected_format, Some(&mut textures))?;
    Ok((YsmResource {
        format: expected_format,
        models,
        textures,
        animations,
        controllers,
        sounds,
        metadata,
    }, reader.position()))
}

/// Parses the category-based resource layout introduced in YSM format 26.
/// Format 26 keeps each sub-entity name before its payload; later formats
/// place one shared model first and list its names after the model record.
pub fn parse_resource_v26_plus(
    bytes: &[u8],
    expected_format: u32,
) -> Result<(YsmResource, usize), ResourceError> {
    if expected_format < 26 {
        return Err(ResourceError::InvalidModel("category resource layout requires format 26 or newer"));
    }
    let mut reader = ResourceReader::new(bytes);
    if reader.read_u32()? != expected_format {
        return Err(ResourceError::InvalidModel("resource version differs from header"));
    }

    let mut sounds = Vec::new();
    for _ in 0..reader.read_count("sound count")? {
        let name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let data = reader.read_bytes()?;
        sounds.push(YsmSound { name, data });
    }
    for _ in 0..reader.read_count("function count")? {
        let _name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let _data = reader.read_bytes()?;
    }
    for _ in 0..reader.read_count("language file count")? {
        let _name = reader.read_string()?;
        let _hash = reader.read_string()?;
        for _ in 0..reader.read_count("language node count")? {
            let _key = reader.read_string()?;
            let _value = reader.read_string()?;
        }
    }

    let mut models = Vec::new();
    let mut textures = Vec::new();
    let mut animations = Vec::new();
    let mut controllers = Vec::new();
    parse_category_sub_entities(
        &mut reader, bytes, expected_format, "vehicle", 1,
        &mut models, &mut textures, &mut animations, &mut controllers,
    )?;
    parse_category_sub_entities(
        &mut reader, bytes, expected_format, "projectiles", 2,
        &mut models, &mut textures, &mut animations, &mut controllers,
    )?;

    if reader.read_varint()? != 1 {
        return Err(ResourceError::InvalidModel("invalid category resource section marker"));
    }

    for _ in 0..reader.read_count("animation blob count")? {
        let model_id = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::LengthLimit("animation model id"))?;
        animations.extend(parse_animation_group(&mut reader, model_id, expected_format)?);
    }
    for _ in 0..reader.read_count("animation controller count")? {
        let _controller_name = reader.read_string()?;
        let _controller_hash = reader.read_string()?;
        controllers.extend(parse_controller_group(&mut reader, expected_format)?);
    }

    for _ in 0..reader.read_count("texture count")? {
        let name = reader.read_string()?;
        let _hash = reader.read_string()?;
        let data = reader.read_bytes()?;
        let width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let _image_format = reader.read_varint()?;
        let _unknown_flag = reader.read_varint()?;
        if !data.is_empty() {
            textures.push(parse_modern_image(name.clone(), data, width, height)?);
        }
        for _ in 0..reader.read_count("texture subtexture count")? {
            let kind = reader.read_varint()?;
            let _hash = reader.read_string()?;
            let data = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let _image_format = reader.read_varint()?;
            let _unknown_flag = reader.read_varint()?;
            let suffix = match kind {
                1 => "_normal",
                2 => "_specular",
                _ => "_special",
            };
            if !data.is_empty() {
                textures.push(parse_modern_image(format!("{name}{suffix}"), data, width, height)?);
            }
        }
    }

    for _ in 0..reader.read_count("shared model count")? {
        let id = reader.read_varint()?;
        let start = reader.position();
        let (model, consumed) = parse_model_record(&bytes[start..], expected_format)?;
        reader.read_exact(consumed)?;
        let name = shared_model_name(id, expected_format);
        models.push((name, model));
    }

    let metadata = parse_ysm_metadata(&mut reader, expected_format, Some(&mut textures))?;
    Ok((YsmResource {
        format: expected_format,
        models,
        textures,
        animations,
        controllers,
        sounds,
        metadata,
    }, reader.position()))
}

fn parse_category_sub_entities(
    reader: &mut ResourceReader<'_>,
    bytes: &[u8],
    format: u32,
    category: &str,
    category_id: u32,
    models: &mut Vec<(String, YsmModel)>,
    textures: &mut Vec<YsmTexture>,
    animations: &mut Vec<YsmAnimationInfo>,
    controllers: &mut Vec<YsmAnimationController>,
) -> Result<(), ResourceError> {
    for index in 0..reader.read_count("category sub-entity count")? {
        let header_name = if format == 26 {
            Some(reader.read_string()?)
        } else {
            None
        };
        let animation_model_id = 0x8000_0000u32
            | (category_id << 20)
            | u32::try_from(index).map_err(|_| ResourceError::LengthLimit("sub-entity index"))?;

        if reader.read_varint()? != 0 {
            animations.extend(parse_animation_group(reader, animation_model_id, format)?);
        }
        let mut entity_controllers = Vec::new();
        if reader.read_varint()? != 0 {
            let _controller_hash = reader.read_string()?;
            entity_controllers = parse_controller_group(reader, format)?;
        }

        let _texture_hash = reader.read_string()?;
        let base_data = reader.read_bytes()?;
        let base_width = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let base_height = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::InvalidImageDimensions)?;
        let _base_format = reader.read_varint()?;
        let _base_flag = reader.read_varint()?;

        let mut image_blobs: Vec<(String, Vec<u8>, u32, u32)> = Vec::new();
        if !base_data.is_empty() {
            image_blobs.push((String::new(), base_data, base_width, base_height));
        }
        for _ in 0..reader.read_count("category subtexture count")? {
            let kind = reader.read_varint()?;
            let _hash = reader.read_string()?;
            let data = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let _image_format = reader.read_varint()?;
            let _unknown_flag = reader.read_varint()?;
            let suffix = match kind {
                1 => "_normal",
                2 => "_specular",
                _ => "_special",
            };
            if !data.is_empty() {
                image_blobs.push((suffix.to_owned(), data, width, height));
            }
        }

        let start = reader.position();
        let (model, consumed) = parse_model_record(&bytes[start..], format)?;
        reader.read_exact(consumed)?;
        let raw_names = if let Some(name) = header_name {
            vec![name]
        } else {
            let mut names = Vec::new();
            for _ in 0..reader.read_count("category model name count")? {
                names.push(reader.read_string()?);
            }
            names
        };

        for raw_name in raw_names {
            let short_name = raw_name.rsplit(':').next().unwrap_or(&raw_name);
            let model_name = format!("{category}/{short_name}");
            models.push((model_name.clone(), model.clone()));
            // Preserve category-local controllers for future vehicle/projectile
            // rendering, but namespace them so the player runtime can exclude them.
            for mut controller in entity_controllers.iter().cloned() {
                controller.name = format!("{model_name}/{}", controller.name);
                controllers.push(controller);
            }
            for (suffix, data, width, height) in &image_blobs {
                textures.push(parse_modern_image(
                    format!("{model_name}{suffix}"),
                    data.clone(),
                    *width,
                    *height,
                )?);
            }
        }
    }
    Ok(())
}

fn parse_modern_image(
    name: String,
    data: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<YsmTexture, ResourceError> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        if width == 0 || height == 0 {
            return Err(ResourceError::InvalidImageDimensions);
        }
        Ok(YsmTexture { name, width, height, png_bytes: data })
    } else {
        encode_rgba_texture(name, width, height, &data)
    }
}

/// Reads the YSM model metadata footer used by YSM formats 4 and newer.
fn parse_ysm_metadata(
    reader: &mut ResourceReader<'_>,
    format: u32,
    mut textures: Option<&mut Vec<YsmTexture>>,
) -> Result<Option<YsmMetadata>, ResourceError> {
    let sha256 = reader.read_string()?;
    if reader.read_varint()? == 0 {
        return Ok(None);
    }
    // Formats through 15 include a reserved compatibility varint here.
    if format <= 15 { let _reserved = reader.read_varint()?; }
    let name = reader.read_string()?;
    let tips = reader.read_string()?;
    let license_type = reader.read_string()?;
    let license_description = reader.read_string()?;

    let mut authors = Vec::new();
    for _ in 0..reader.read_count("metadata author count")? {
        let author_name = reader.read_string()?;
        let role = reader.read_string()?;
        let mut contacts = Vec::new();
        for _ in 0..reader.read_count("metadata author contact count")? {
            contacts.push((reader.read_string()?, reader.read_string()?));
        }
        let comment = reader.read_string()?;
        authors.push(YsmAuthor {
            name: author_name,
            role,
            contacts,
            comment,
        });
    }

    let mut links = Vec::new();
    for _ in 0..reader.read_count("metadata link count")? {
        links.push((reader.read_string()?, reader.read_string()?));
    }

    let width_scale = reader.read_f32()?;
    let height_scale = reader.read_f32()?;
    let mut extra_animations = Vec::new();
    for _ in 0..reader.read_count("metadata extra animation count")? {
        extra_animations.push((reader.read_string()?, reader.read_string()?));
    }
    let mut extra_animation_buttons = Vec::new();
    let mut extra_animation_classify = Vec::new();
    if format > 9 {
        for _ in 0..reader.read_count("extra animation button count")? {
            let id = reader.read_string()?;
            let button_name = reader.read_string()?;
            if reader.read_varint()? != 0 {
                return Err(ResourceError::InvalidModel("unsupported extra-animation button flag"));
            }
            let mut config_forms = Vec::new();
            for _ in 0..reader.read_count("extra animation config form count")? {
                let kind = reader.read_string()?;
                let title = reader.read_string()?;
                let description = reader.read_string()?;
                let value = reader.read_string()?;
                let step = reader.read_f32()?;
                let min = reader.read_f32()?;
                let max = reader.read_f32()?;
                let mut labels = Vec::new();
                for _ in 0..reader.read_count("extra animation form label count")? {
                    labels.push((reader.read_string()?, reader.read_string()?));
                }
                config_forms.push(YsmExtraAnimationConfigForm {
                    kind, title, description, value, step, min, max, labels,
                });
            }
            extra_animation_buttons.push(YsmExtraAnimationButton {
                id, name: button_name, config_forms,
            });
        }
        for _ in 0..reader.read_count("extra animation classify count")? {
            let id = reader.read_string()?;
            let mut entries = Vec::new();
            for _ in 0..reader.read_count("extra animation classify entry count")? {
                entries.push((reader.read_string()?, reader.read_string()?));
            }
            extra_animation_classify.push(YsmExtraAnimationClassify {
                id, extra_animations: entries,
            });
        }
    }
    let default_texture = reader.read_string()?;
    let preview_animation = reader.read_string()?;
    let free = reader.read_varint()? != 0;
    let render_layers_first = if format > 4 {
        Some(reader.read_varint()? != 0)
    } else {
        None
    };
    let all_cutout = if format >= 15 {
        Some(reader.read_varint()? != 0)
    } else {
        None
    };
    let disable_preview_rotation = if format >= 15 {
        Some(reader.read_varint()? != 0)
    } else {
        None
    };
    if format > 15 {
        let _gui_no_lighting = reader.read_varint()?;
        if format >= 32 { let _merge_multiline_expr = reader.read_varint()?; }
        let gui_foreground = reader.read_string()?;
        let gui_background = reader.read_string()?;
        for _ in 0..reader.read_count("metadata avatar count")? {
            let name = reader.read_string()?;
            let data = reader.read_bytes()?;
            let width = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let height = u32::try_from(reader.read_varint()?)
                .map_err(|_| ResourceError::InvalidImageDimensions)?;
            let _image_format = reader.read_varint()?;
            let _unknown_flag = reader.read_varint()?;
            if !data.is_empty() {
                if let Some(textures) = textures.as_deref_mut() {
                    textures.push(parse_modern_image(format!("avatar/{name}"), data, width, height)?);
                }
            }
        }
        if !gui_foreground.is_empty() || !gui_background.is_empty() {
            for _ in 0..reader.read_count("metadata background image count")? {
                let name = reader.read_string()?;
                let data = reader.read_bytes()?;
                let width = u32::try_from(reader.read_varint()?)
                    .map_err(|_| ResourceError::InvalidImageDimensions)?;
                let height = u32::try_from(reader.read_varint()?)
                    .map_err(|_| ResourceError::InvalidImageDimensions)?;
                let _image_format = reader.read_varint()?;
                let _unknown_flag = reader.read_varint()?;
                if !data.is_empty() {
                    if let Some(textures) = textures.as_deref_mut() {
                        textures.push(parse_modern_image(format!("background/{name}"), data, width, height)?);
                    }
                }
            }
        }
    }

    Ok(Some(YsmMetadata {
        sha256,
        name,
        tips,
        license_type,
        license_description,
        authors,
        links,
        width_scale,
        height_scale,
        extra_animations,
        extra_animation_buttons,
        extra_animation_classify,
        default_texture,
        preview_animation,
        free,
        render_layers_first,
        all_cutout,
        disable_preview_rotation,
    }))
}

fn parse_controller_group(
    reader: &mut ResourceReader<'_>,
    format: u32,
) -> Result<Vec<YsmAnimationController>, ResourceError> {
    let mut controllers = Vec::new();
    for _ in 0..reader.read_count("controller count")? {
        let name = reader.read_string()?;
        let initial_state = reader.read_string()?;
        let mut states = Vec::new();
        for _ in 0..reader.read_count("controller state count")? {
            let state_name = reader.read_string()?;
            let mut state = YsmControllerState::default();
            for _ in 0..reader.read_count("controller animation count")? {
                state.animations.push((reader.read_string()?, reader.read_string()?));
            }
            for _ in 0..reader.read_count("controller transition count")? {
                state.transitions.push((reader.read_string()?, reader.read_string()?));
            }
            for _ in 0..reader.read_count("controller on-entry count")? {
                state.on_entry.push(reader.read_string()?);
            }
            for _ in 0..reader.read_count("controller on-exit count")? {
                state.on_exit.push(reader.read_string()?);
            }
            if reader.read_varint()? != 0 {
                state.blend_transition = Some(reader.read_f32()?);
            } else {
                for _ in 0..reader.read_count("controller blend transition count")? {
                    state.blend_transitions.push((reader.read_f32()?, reader.read_f32()?));
                }
            }
            state.blend_via_shortest_path = reader.read_varint()? != 0;
            if format > 26 {
                for _ in 0..reader.read_count("controller sound effect count")? {
                    let _effect_name = reader.read_string()?;
                }
            }
            states.push((state_name, state));
        }
        controllers.push(YsmAnimationController { name, initial_state, states });
    }
    Ok(controllers)
}

fn parse_animation_group(
    reader: &mut ResourceReader<'_>,
    model_id: u32,
    format: u32,
) -> Result<Vec<YsmAnimationInfo>, ResourceError> {
    if format > 15 {
        let _animation_hash = reader.read_string()?;
    }
    let mut animations = Vec::new();
    for _ in 0..reader.read_count("animation record count")? {
        animations.push(parse_animation(reader, model_id, format)?);
    }
    Ok(animations)
}

fn parse_animation(
    reader: &mut ResourceReader<'_>,
    model_id: u32,
    format: u32,
) -> Result<YsmAnimationInfo, ResourceError> {
    let name = reader.read_string()?;
    // Keep duration in source ticks; the runtime converts it to seconds.
    let duration_ticks = reader.read_f32()?;
    let loop_mode = reader.read_varint()?;
    let blend_weight = if format > 9 {
        let first_flag = reader.read_varint()?;
        let second_flag = reader.read_varint()?;
        if first_flag != 0 || second_flag != 0 {
            return Err(ResourceError::InvalidModel("unsupported animation compatibility flags"));
        }
        let mut blend_weight = None;
        for _ in 0..reader.read_count("animation blend weight count")? {
            blend_weight = Some(match reader.read_exact(1)?[0] {
                1 => YsmMolangValue::Number(reader.read_f32()?),
                2 => YsmMolangValue::Expression(reader.read_string()?),
                _ => return Err(ResourceError::InvalidModel("unknown animation blend weight type")),
            });
        }
        if reader.read_varint()? != 0 {
            return Err(ResourceError::InvalidModel("unsupported animation compatibility field"));
        }
        blend_weight
    } else {
        None
    };
    let mut bones = Vec::new();
    for _ in 0..reader.read_count("animation bone count")? {
        let bone_name = reader.read_string()?;
        let rotation = parse_channel(reader)?;
        let position = parse_channel(reader)?;
        let scale = parse_channel(reader)?;
        bones.push(YsmBoneAnimation {
            bone_name,
            channels: [rotation, position, scale],
        });
    }
    let mut timeline = Vec::new();
    for _ in 0..reader.read_count("animation timeline count")? {
        let mut events = Vec::new();
        for _ in 0..reader.read_count("animation timeline event count")? {
            events.push(reader.read_string()?);
        }
        timeline.push(YsmTimelineEvent {
            events,
            time: reader.read_f32()? / 20.0,
        });
    }
    Ok(YsmAnimationInfo {
        model_id,
        name,
        duration_ticks,
        loop_mode,
        blend_weight,
        bones,
        timeline,
    })
}

fn parse_channel(
    reader: &mut ResourceReader<'_>,
) -> Result<YsmAnimationChannel, ResourceError> {
    let mut keyframes = Vec::new();
    for _ in 0..reader.read_count("animation keyframe count")? {
        // YSM stores keyframe timestamps in game ticks (20 ticks per second).
        let time = reader.read_f32()? / 20.0;
        let lerp_mode = reader.read_varint()?;
        let first = read_molang_triplet(reader)?;
        let has_pre = reader.read_varint()?;
        if has_pre > 1 {
            return Err(ResourceError::InvalidModel("invalid pre-keyframe flag"));
        }
        // With a pre value, the binary stores pre first and post second.
        let (post, pre) = if has_pre == 1 {
            (read_molang_triplet(reader)?, Some(first))
        } else {
            (first, None)
        };
        keyframes.push(YsmKeyframe { time, lerp_mode, post, pre });
    }
    Ok(YsmAnimationChannel { keyframes })
}

fn read_molang_triplet(
    reader: &mut ResourceReader<'_>,
) -> Result<[YsmMolangValue; 3], ResourceError> {
    let mut values = Vec::with_capacity(3);
    for _ in 0..3 {
        values.push(match reader.read_exact(1)?[0] {
            1 => YsmMolangValue::Number(reader.read_f32()?),
            2 => YsmMolangValue::Expression(reader.read_string()?),
            _ => return Err(ResourceError::InvalidModel("unknown Molang value type")),
        });
    }
    values.try_into()
        .map_err(|_| ResourceError::InvalidModel("animation vector must have three values"))
}

#[cfg(test)]
mod format_16_tests {
    use super::*;

    fn f32le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn animation_record(out: &mut Vec<u8>, format: u32) {
        string(out, "animation.test");
        f32le(out, 40.0);
        varint(out, 1);
        if format > 9 {
            varint(out, 0); varint(out, 0); // compatibility flags
            varint(out, 0); // blend weight count
            varint(out, 0); // trailing compatibility field
        }
        varint(out, 0); // bones
        varint(out, 0); // timeline
    }

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }

    fn model_record(out: &mut Vec<u8>) {
        string(out, "model-sha");
        varint(out, 1); // bones
        string(out, ""); // parent
        varint(out, 1); // cubes
        varint(out, 1); // faces
        for value in [0.0, 0.0, 1.0] { f32le(out, value); }
        for index in 0..4 {
            for value in [index as f32, 0.0, 0.0, index as f32, 0.0] {
                f32le(out, value);
            }
        }
        for _ in 0..3 { varint(out, 0); } // reserved cube fields
        string(out, "root");
        for _ in 0..5 { varint(out, 0); }
        for _ in 0..6 { f32le(out, 0.0); }
        string(out, "geometry.test");
        for value in [64.0, 64.0, 2.0, 2.0] { f32le(out, value); }
        varint(out, 3);
        for _ in 0..3 { f32le(out, 0.0); }
        f32le(out, 1.0);
        f32le(out, 1.0);
        varint(out, 0); // no legacy info
        for _ in 0..3 { varint(out, 0); }
    }

    #[test]
    fn retains_category_sub_entity_models_alongside_shared_player_models() {
        let mut data = Vec::new();
        data.extend_from_slice(&16u32.to_le_bytes());
        for _ in 0..3 { varint(&mut data, 0); } // sounds, functions, languages
        varint(&mut data, 1); // sub-entity count
        string(&mut data, "minecraft:player");
        varint(&mut data, 0); // no sub-entity animations
        varint(&mut data, 0); // no sub-entity controller
        string(&mut data, ""); // texture hash
        varint(&mut data, 0); // empty texture data
        for _ in 0..4 { varint(&mut data, 0); } // width, height, format, flag
        varint(&mut data, 0); // no subtextures
        model_record(&mut data);
        varint(&mut data, 1); // sub-entity footer flag
        varint(&mut data, 1); // modern section marker
        varint(&mut data, 0); // global animation groups
        varint(&mut data, 0); // global controllers
        varint(&mut data, 0); // global textures
        varint(&mut data, 1); // shared model table
        varint(&mut data, 1); // main model id
        model_record(&mut data);
        string(&mut data, ""); varint(&mut data, 0); // no metadata

        let (resource, consumed) = parse_resource_v16_v25(&data, 16).unwrap();
        assert_eq!(consumed, data.len());
        assert!(resource.models.iter().any(|(name, _)| name == "subentity/player"));
        assert!(resource.models.iter().any(|(name, _)| name == "main"));
        assert_eq!(resource.models.len(), 2);
    }

    #[test]
    fn parses_modern_png_texture_files() {
        let png = encode_rgba_texture("main", 1, 1, &[255, 0, 0, 255]).unwrap().png_bytes;
        let mut data = Vec::new();
        data.extend_from_slice(&16u32.to_le_bytes());
        for _ in 0..4 { varint(&mut data, 0); } // sound/function/language/sub-entity counts
        varint(&mut data, 1); // sub-entity footer
        varint(&mut data, 1); // modern section marker
        varint(&mut data, 0); // animations
        varint(&mut data, 0); // controllers
        varint(&mut data, 1); // textures
        string(&mut data, "main");
        string(&mut data, "texture-hash");
        varint(&mut data, png.len() as u64);
        data.extend_from_slice(&png);
        varint(&mut data, 1); // width
        varint(&mut data, 1); // height
        varint(&mut data, 0); // image format
        varint(&mut data, 0); // unknown flag
        varint(&mut data, 0); // subtextures
        varint(&mut data, 0); // models
        string(&mut data, ""); varint(&mut data, 0); // no metadata

        let (resource, consumed) = parse_resource_v16_v25(&data, 16).unwrap();
        assert_eq!(consumed, data.len());
        assert_eq!(resource.textures.len(), 1);
        assert_eq!(resource.textures[0].name, "main");
        assert_eq!(resource.textures[0].png_bytes, png);
    }

    #[test]
    fn parses_modern_animation_groups_and_format_specific_blend_fields() {
        let mut data = Vec::new();
        data.extend_from_slice(&16u32.to_le_bytes());
        for _ in 0..4 { varint(&mut data, 0); } // sounds, functions, languages, sub-entities
        varint(&mut data, 1); // sub-entity footer
        varint(&mut data, 1); // modern section marker
        varint(&mut data, 1); // animation blob count
        varint(&mut data, 1); // main model id
        string(&mut data, "animation-hash");
        varint(&mut data, 1); // animation record count
        animation_record(&mut data, 16);
        varint(&mut data, 0); // controllers
        varint(&mut data, 0); // textures
        varint(&mut data, 0); // models
        string(&mut data, ""); varint(&mut data, 0); // no metadata

        let (resource, consumed) = parse_resource_v16_v25(&data, 16).unwrap();
        assert_eq!(consumed, data.len());
        assert_eq!(resource.animations.len(), 1);
        assert_eq!(resource.animations[0].name, "animation.test");
        assert_eq!(resource.animations[0].blend_weight, None);
    }

    #[test]
    fn parses_an_empty_modern_format_sixteen_resource() {
        let mut data = Vec::new();
        data.extend_from_slice(&16u32.to_le_bytes());
        // sounds, functions, language files, sub-entities, then the sub-entity footer
        for _ in 0..4 { data.push(0); }
        data.push(1);
        data.push(1); // modern section marker
        // animations, controllers, textures and model table
        for _ in 0..4 { data.push(0); }
        // metadata hash and "no metadata" flag
        data.push(0);
        data.push(0);
        let (resource, consumed) = parse_resource_v16_v25(&data, 16).unwrap();
        assert_eq!(consumed, data.len());
        assert_eq!(resource.format, 16);
        assert!(resource.models.is_empty());
        assert!(resource.textures.is_empty());
        assert!(resource.animations.is_empty());
        assert!(resource.controllers.is_empty());
        assert!(resource.sounds.is_empty());
        assert!(resource.metadata.is_none());
    }
}

#[cfg(test)]
mod shared_model_name_tests {
    use super::*;

    #[test]
    fn keeps_unknown_model_ids_distinct_and_uses_versioned_first_person_names() {
        assert_eq!(shared_model_name(11, 25), "fp_arm");
        assert_eq!(shared_model_name(11, 32), "fp.arm");
        assert_eq!(shared_model_name(13, 25), "irons_spell_books");
        assert_eq!(shared_model_name(13, 32), "iss");
        assert_eq!(shared_model_name(99, 32), "unk_99");
    }
}

#[cfg(test)]
mod format_26_plus_tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }
    fn f32le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn model_record(out: &mut Vec<u8>) {
        string(out, "model-sha");
        varint(out, 1);
        string(out, "");
        varint(out, 1);
        varint(out, 1);
        for value in [0.0, 0.0, 1.0] { f32le(out, value); }
        for index in 0..4 {
            for value in [index as f32, 0.0, 0.0, index as f32, 0.0] {
                f32le(out, value);
            }
        }
        for _ in 0..3 { varint(out, 0); }
        string(out, "root");
        for _ in 0..5 { varint(out, 0); }
        for _ in 0..6 { f32le(out, 0.0); }
        string(out, "geometry.test");
        for value in [64.0, 64.0, 2.0, 2.0] { f32le(out, value); }
        varint(out, 3);
        for _ in 0..3 { f32le(out, 0.0); }
        f32le(out, 1.0);
        f32le(out, 1.0);
        varint(out, 0);
        for _ in 0..3 { varint(out, 0); }
    }
    fn empty_modern_tail(out: &mut Vec<u8>) {
        varint(out, 1); // category resource section marker
        for _ in 0..4 { varint(out, 0); } // animations, controllers, textures, shared models
        string(out, "");
        varint(out, 0); // no metadata
    }

    #[test]
    fn parses_format_26_named_vehicle_sub_entities() {
        let mut data = Vec::new();
        data.extend_from_slice(&26u32.to_le_bytes());
        for _ in 0..3 { varint(&mut data, 0); }
        varint(&mut data, 1);
        string(&mut data, "minecraft:cart");
        varint(&mut data, 0); // no local animations
        varint(&mut data, 1); // local controller
        string(&mut data, "controller-hash");
        varint(&mut data, 1); // controller count
        string(&mut data, "controller.animation.cart");
        string(&mut data, "default");
        varint(&mut data, 1); // state count
        string(&mut data, "default");
        for _ in 0..4 { varint(&mut data, 0); } // animations, transitions, entry, exit
        varint(&mut data, 0); // no direct blend transition
        varint(&mut data, 0); // no blend transition map
        varint(&mut data, 0); // shortest-path blending off
        string(&mut data, ""); varint(&mut data, 0);
        for _ in 0..4 { varint(&mut data, 0); }
        varint(&mut data, 0);
        model_record(&mut data);
        varint(&mut data, 0);
        empty_modern_tail(&mut data);

        let (resource, consumed) = parse_resource_v26_plus(&data, 26).unwrap();
        assert_eq!(consumed, data.len());
        assert!(resource.models.iter().any(|(name, _)| name == "vehicle/cart"));
        assert_eq!(
            resource.controllers[0].name,
            "vehicle/cart/controller.animation.cart"
        );
    }

    #[test]
    fn parses_format_27_shared_sub_model_name_table() {
        let mut data = Vec::new();
        data.extend_from_slice(&27u32.to_le_bytes());
        for _ in 0..3 { varint(&mut data, 0); }
        varint(&mut data, 1);
        varint(&mut data, 0);
        varint(&mut data, 0);
        string(&mut data, ""); varint(&mut data, 0);
        for _ in 0..4 { varint(&mut data, 0); }
        varint(&mut data, 0);
        model_record(&mut data);
        varint(&mut data, 1);
        string(&mut data, "minecraft:cart");
        varint(&mut data, 0);
        empty_modern_tail(&mut data);

        let (resource, consumed) = parse_resource_v26_plus(&data, 27).unwrap();
        assert_eq!(consumed, data.len());
        assert!(resource.models.iter().any(|(name, _)| name == "vehicle/cart"));
    }
}

#[cfg(test)]
mod format_10_tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }
    fn f32le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn parses_format_fifteen_extra_animation_ui_and_metadata_flags() {
        let mut data = Vec::new();
        data.extend_from_slice(&15u32.to_le_bytes());
        for _ in 0..12 { varint(&mut data, 0); }
        string(&mut data, "resource-sha");
        varint(&mut data, 1); // metadata is present
        varint(&mut data, 0); // reserved
        string(&mut data, "Example");
        string(&mut data, "tips");
        string(&mut data, "CC0");
        string(&mut data, "license");
        varint(&mut data, 0); // authors
        varint(&mut data, 0); // links
        f32le(&mut data, 0.8);
        f32le(&mut data, 0.65);
        varint(&mut data, 1); // extra animations
        string(&mut data, "wave");
        string(&mut data, "animation.wave");
        varint(&mut data, 1); // buttons
        string(&mut data, "emote");
        string(&mut data, "Wave");
        varint(&mut data, 0); // reserved button flag
        varint(&mut data, 1); // config forms
        string(&mut data, "range");
        string(&mut data, "Speed");
        string(&mut data, "Animation speed");
        string(&mut data, "1");
        f32le(&mut data, 0.1);
        f32le(&mut data, 0.0);
        f32le(&mut data, 2.0);
        varint(&mut data, 1); string(&mut data, "Fast"); string(&mut data, "query.speed > 1");
        varint(&mut data, 1); // classify groups
        string(&mut data, "emotes");
        varint(&mut data, 1); string(&mut data, "wave"); string(&mut data, "animation.wave");
        string(&mut data, "main"); // default texture
        string(&mut data, "idle"); // preview animation
        varint(&mut data, 1); // free
        varint(&mut data, 0); // render layers first
        varint(&mut data, 1); // all cutout
        varint(&mut data, 1); // disable preview rotation

        let (resource, consumed) = parse_resource_v4_v15(&data, 15).unwrap();
        assert_eq!(consumed, data.len());
        let metadata = resource.metadata.unwrap();
        assert_eq!(metadata.name, "Example");
        assert_eq!(metadata.extra_animations, vec![("wave".into(), "animation.wave".into())]);
        assert_eq!(metadata.extra_animation_buttons[0].config_forms[0].kind, "range");
        assert_eq!(metadata.extra_animation_buttons[0].config_forms[0].labels[0].0, "Fast");
        assert_eq!(metadata.extra_animation_classify[0].id, "emotes");
        assert_eq!(metadata.default_texture, "main");
        assert_eq!(metadata.preview_animation, "idle");
        assert_eq!(metadata.all_cutout, Some(true));
        assert_eq!(metadata.disable_preview_rotation, Some(true));
    }

    #[test]
    fn parses_format_ten_controllers_and_sound_records() {
        let mut data = Vec::new();
        data.extend_from_slice(&10u32.to_le_bytes());
        varint(&mut data, 0); // prefix
        varint(&mut data, 0); // models
        varint(&mut data, 0); // animations
        varint(&mut data, 1); // controllers
        varint(&mut data, 0); // legacy controller hash/index
        varint(&mut data, 1); // controller animation entries
        string(&mut data, "controller.animation.test");
        string(&mut data, "default");
        varint(&mut data, 1); // states
        string(&mut data, "default");
        varint(&mut data, 1); string(&mut data, "animation.test"); string(&mut data, "");
        varint(&mut data, 1); string(&mut data, "query.is_running"); string(&mut data, "default");
        varint(&mut data, 1); string(&mut data, "event.start");
        varint(&mut data, 1); string(&mut data, "event.stop");
        varint(&mut data, 1); f32le(&mut data, 0.25); // blend transition
        varint(&mut data, 1); // shortest path
        varint(&mut data, 1); string(&mut data, "controller"); string(&mut data, "controller-hash");
        varint(&mut data, 0); // textures
        varint(&mut data, 1); // sounds
        string(&mut data, "sound.test");
        varint(&mut data, 3); data.extend_from_slice(b"ogg");
        varint(&mut data, 1); string(&mut data, "sound.test"); string(&mut data, "sound-hash");
        varint(&mut data, 0); // extra textures
        varint(&mut data, 0); // model hash table
        varint(&mut data, 0); // animation hash table
        varint(&mut data, 0); // texture hash table
        string(&mut data, ""); varint(&mut data, 0); // no metadata footer

        let (resource, consumed) = parse_resource_v4_v15(&data, 10).unwrap();
        assert_eq!(consumed, data.len());
        assert_eq!(resource.controllers.len(), 1);
        assert_eq!(resource.controllers[0].name, "controller.animation.test");
        assert_eq!(resource.controllers[0].initial_state, "default");
        let state = &resource.controllers[0].states[0].1;
        assert_eq!(state.animations[0].0, "animation.test");
        assert_eq!(state.transitions[0].0, "query.is_running");
        assert_eq!(state.blend_transition, Some(0.25));
        assert!(state.blend_via_shortest_path);
        assert_eq!(resource.sounds[0].name, "sound.test");
        assert_eq!(resource.sounds[0].data, b"ogg");
        assert!(resource.metadata.is_none());
    }
}

#[cfg(test)]
mod format_4_animation_group_tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }

    #[test]
    fn parses_legacy_animation_blob_groups() {
        let mut data = Vec::new();
        data.extend_from_slice(&4u32.to_le_bytes());
        varint(&mut data, 0); // prefix
        varint(&mut data, 0); // models
        varint(&mut data, 1); // animation blobs
        varint(&mut data, 1); // main model id
        varint(&mut data, 0); // legacy unknown
        varint(&mut data, 1); // animation records in blob
        string(&mut data, "animation.idle");
        data.extend_from_slice(&20.0f32.to_le_bytes());
        varint(&mut data, 1); // loop
        varint(&mut data, 0); // bones
        varint(&mut data, 0); // timeline
        for _ in 0..5 { varint(&mut data, 0); } // textures, extras and three hash tables
        string(&mut data, ""); varint(&mut data, 0); // no metadata

        let (resource, consumed) = parse_resource_v4_v15(&data, 4).unwrap();
        assert_eq!(consumed, data.len());
        assert_eq!(resource.animations.len(), 1);
        assert_eq!(resource.animations[0].name, "animation.idle");
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }
    fn f32le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn channel(out: &mut Vec<u8>) {
        varint(out, 1);
        f32le(out, 2.5);
        varint(out, 1);
        out.push(1); f32le(out, 12.0);
        out.push(2); string(out, "query.anim_time");
        out.push(1); f32le(out, -4.0);
        varint(out, 1);
        out.push(1); f32le(out, 24.0);
        out.push(2); string(out, "query.post_time");
        out.push(1); f32le(out, -8.0);
    }

    #[test]
    fn retains_numeric_and_molang_animation_channels_and_timeline() {
        let mut bytes = Vec::new();
        string(&mut bytes, "animation.test");
        f32le(&mut bytes, 40.0);
        varint(&mut bytes, 1);
        varint(&mut bytes, 1);
        string(&mut bytes, "head");
        channel(&mut bytes);
        channel(&mut bytes);
        channel(&mut bytes);
        varint(&mut bytes, 1);
        varint(&mut bytes, 1);
        string(&mut bytes, "sound.test");
        f32le(&mut bytes, 8.0);

        let mut reader = ResourceReader::new(&bytes);
        let animation = parse_animation(&mut reader, 1, 4).unwrap();
        assert_eq!(animation.name, "animation.test");
        assert_eq!(animation.duration_ticks, 40.0);
        assert_eq!(animation.loop_mode, 1);
        assert_eq!(animation.bones.len(), 1);
        assert_eq!(animation.bones[0].bone_name, "head");
        assert_eq!(animation.bones[0].channels[0].keyframes.len(), 1);
        let keyframe = &animation.bones[0].channels[0].keyframes[0];
        assert!((keyframe.time - 0.125).abs() < 1e-6);
        assert_eq!(keyframe.pre.as_ref().unwrap()[1],
            YsmMolangValue::Expression("query.anim_time".into()));
        assert_eq!(keyframe.post[1],
            YsmMolangValue::Expression("query.post_time".into()));
        assert_eq!(animation.timeline[0].events.len(), 1);
        assert_eq!(animation.timeline[0].events[0], "sound.test");
        assert!((animation.timeline[0].time - 0.4).abs() < 1e-6);
        assert_eq!(reader.remaining(), 0);
    }
}


#[cfg(test)]
mod metadata_tests {
    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn string(out: &mut Vec<u8>, value: &str) {
        varint(out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }
    fn f32le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn parses_format_four_model_metadata_and_extra_animation_names() {
        let mut bytes = Vec::new();
        string(&mut bytes, "sha256-value");
        varint(&mut bytes, 1);
        varint(&mut bytes, 0);
        string(&mut bytes, "Sample Model");
        string(&mut bytes, "Model tips");
        string(&mut bytes, "CC BY-NC-SA 4.0");
        string(&mut bytes, "Attribution required");
        varint(&mut bytes, 1);
        string(&mut bytes, "Author");
        string(&mut bytes, "Model");
        varint(&mut bytes, 1);
        string(&mut bytes, "site");
        string(&mut bytes, "https://example.invalid");
        string(&mut bytes, "comment");
        varint(&mut bytes, 1);
        string(&mut bytes, "homepage");
        string(&mut bytes, "https://example.invalid");
        f32le(&mut bytes, 1.25);
        f32le(&mut bytes, 1.5);
        varint(&mut bytes, 1);
        string(&mut bytes, "dance");
        string(&mut bytes, "animation.dance");
        string(&mut bytes, "texture.default");
        string(&mut bytes, "animation.preview");
        varint(&mut bytes, 1);

        let mut reader = ResourceReader::new(&bytes);
        let metadata = parse_ysm_metadata(&mut reader, 4).unwrap().unwrap();
        assert_eq!(metadata.sha256, "sha256-value");
        assert_eq!(metadata.name, "Sample Model");
        assert_eq!(metadata.license_type, "CC BY-NC-SA 4.0");
        assert_eq!(metadata.authors[0].name, "Author");
        assert_eq!(metadata.authors[0].contacts[0].0, "site");
        assert_eq!(metadata.extra_animations[0],
            ("dance".to_owned(), "animation.dance".to_owned()));
        assert_eq!(metadata.default_texture, "texture.default");
        assert_eq!(metadata.preview_animation, "animation.preview");
        assert!(metadata.free);
        assert_eq!(metadata.render_layers_first, None);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn accepts_legacy_metadata_absence_flag() {
        let mut bytes = Vec::new();
        string(&mut bytes, "sha256-value");
        varint(&mut bytes, 0);
        let mut reader = ResourceReader::new(&bytes);
        assert_eq!(parse_ysm_metadata(&mut reader, 4).unwrap(), None);
        assert_eq!(reader.remaining(), 0);
    }
}
