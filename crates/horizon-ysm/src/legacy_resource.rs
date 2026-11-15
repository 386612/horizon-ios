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

    Ok((
        YsmResource {
            format: expected_format,
            models,
            textures,
            animations,
        },
        reader.position(),
    ))
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
