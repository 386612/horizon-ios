//! Legacy YSM resource table reader for format versions 4 through 9.
use crate::resource::{
    encode_rgba_texture, parse_model_record, ResourceError, ResourceReader, YsmModel, YsmTexture,
};

#[derive(Clone, Debug, PartialEq)]
pub struct YsmResource {
    pub format: u32,
    pub models: Vec<(String, YsmModel)>,
    pub textures: Vec<YsmTexture>,
    pub animations: Vec<YsmAnimationInfo>,
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
    pub default_texture: String,
    pub preview_animation: String,
    pub free: bool,
    pub render_layers_first: Option<bool>,
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
    pub bones: Vec<YsmBoneAnimation>,
    pub timeline: Vec<YsmTimelineEvent>,
}

/// Parses the legacy resource layout used by YSM format versions 4-9.
/// Animation records retain their numeric and Molang keyframes for runtime use.
pub fn parse_resource_v4_v9(
    bytes: &[u8],
    expected_format: u32,
) -> Result<(YsmResource, usize), ResourceError> {
    if !(4..=9).contains(&expected_format) {
        return Err(ResourceError::InvalidModel("resource layout supports formats 4 through 9"));
    }
    let mut reader = ResourceReader::new(bytes);
    if reader.read_u32()? != expected_format {
        return Err(ResourceError::InvalidModel("resource version differs from header"));
    }

    let prefix_len = reader.read_count("prefix length")?;
    reader.read_exact(prefix_len)?;

    let mut models = Vec::new();
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

    let mut animations = Vec::new();
    for _ in 0..reader.read_count("animation count")? {
        let model_id = u32::try_from(reader.read_varint()?)
            .map_err(|_| ResourceError::LengthLimit("animation model id"))?;
        let _unknown = reader.read_varint()?;
        let animation = parse_animation(&mut reader, model_id)?;
        animations.push(animation);
    }

    let mut textures = Vec::new();
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

    let metadata = parse_ysm_metadata(&mut reader, expected_format)?;
    Ok((
        YsmResource {
            format: expected_format,
            models,
            textures,
            animations,
            metadata,
        },
        reader.position(),
    ))
}

/// Reads the optional YSM model metadata footer used by legacy formats 4-9.
fn parse_ysm_metadata(
    reader: &mut ResourceReader<'_>,
    format: u32,
) -> Result<Option<YsmMetadata>, ResourceError> {
    let sha256 = reader.read_string()?;
    if reader.read_varint()? == 0 {
        return Ok(None);
    }
    // Reserved compatibility value present through format 15.
    let _reserved = reader.read_varint()?;
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
    let default_texture = reader.read_string()?;
    let preview_animation = reader.read_string()?;
    let free = reader.read_varint()? != 0;
    let render_layers_first = if format > 4 {
        Some(reader.read_varint()? != 0)
    } else {
        None
    };

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
        default_texture,
        preview_animation,
        free,
        render_layers_first,
    }))
}

fn parse_animation(
    reader: &mut ResourceReader<'_>,
    model_id: u32,
) -> Result<YsmAnimationInfo, ResourceError> {
    let name = reader.read_string()?;
    // Keep duration in source ticks; the runtime converts it to seconds.
    let duration_ticks = reader.read_f32()?;
    let loop_mode = reader.read_varint()?;
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
        let animation = parse_animation(&mut reader, 1).unwrap();
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
