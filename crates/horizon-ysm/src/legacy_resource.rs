//! Legacy YSM resource table reader for format versions 4 through 9.
use crate::resource::{
    encode_rgba_texture, parse_model_record, ResourceError, ResourceReader, YsmAnimationInfo,
    YsmModel, YsmTexture,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YsmResource {
    pub format: u32,
    pub models: Vec<(String, YsmModel)>,
    pub textures: Vec<YsmTexture>,
    pub animations: Vec<YsmAnimationInfo>,
}

/// Parses the legacy resource layout used by YSM format versions 4-9.
/// Animation records are validated and indexed, but keyframes are not yet
/// converted into runtime animation tracks.
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
        let name = skip_animation(&mut reader)?;
        animations.push(YsmAnimationInfo { model_id, name });
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

fn skip_animation(reader: &mut ResourceReader<'_>) -> Result<String, ResourceError> {
    let name = reader.read_string()?;
    let _duration = reader.read_f32()?;
    let _loop_mode = reader.read_varint()?;
    for _ in 0..reader.read_count("animation bone count")? {
        let _bone_name = reader.read_string()?;
        for _ in 0..3 {
            let keyframes = reader.read_count("animation keyframe count")?;
            for _ in 0..keyframes {
                let _time = reader.read_f32()?;
                let _lerp = reader.read_varint()?;
                skip_molang_triplet(reader)?;
                let has_pre = reader.read_varint()?;
                if has_pre > 1 {
                    return Err(ResourceError::InvalidModel("invalid pre-keyframe flag"));
                }
                if has_pre == 1 {
                    skip_molang_triplet(reader)?;
                }
            }
        }
    }
    for _ in 0..reader.read_count("animation timeline count")? {
        for _ in 0..reader.read_count("animation timeline event count")? {
            let _event = reader.read_string()?;
        }
        let _time = reader.read_f32()?;
    }
    Ok(name)
}

fn skip_molang_triplet(reader: &mut ResourceReader<'_>) -> Result<(), ResourceError> {
    for _ in 0..3 {
        match reader.read_exact(1)?[0] {
            1 => {
                let _value = reader.read_f32()?;
            }
            2 => {
                let _expression = reader.read_string()?;
            }
            _ => return Err(ResourceError::InvalidModel("unknown Molang value type")),
        }
    }
    Ok(())
}
