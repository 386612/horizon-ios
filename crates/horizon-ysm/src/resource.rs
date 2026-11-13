//! Bounded readers and record decoders for the decompressed YSM resource stream.
use std::io::Cursor;

const MAX_COLLECTION_ITEMS: usize = 1_000_000;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub enum ResourceError {
    UnexpectedEof,
    VarintOverflow,
    LengthLimit(&'static str),
    InvalidUtf8,
    InvalidImageDimensions,
    InvalidImageDataLength,
    InvalidImage(String),
    InvalidModel(&'static str),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("unexpected end of YSM resource data"),
            Self::VarintOverflow => f.write_str("YSM varint exceeds 64 bits"),
            Self::LengthLimit(what) => write!(f, "YSM {what} exceeds the safety limit"),
            Self::InvalidUtf8 => f.write_str("YSM resource string is not valid UTF-8"),
            Self::InvalidImageDimensions => f.write_str("YSM texture has invalid dimensions"),
            Self::InvalidImageDataLength => f.write_str("YSM RGBA texture byte count does not match its dimensions"),
            Self::InvalidImage(e) => write!(f, "could not encode YSM texture as PNG: {e}"),
            Self::InvalidModel(e) => write!(f, "invalid YSM model record: {e}"),
        }
    }
}
impl std::error::Error for ResourceError {}

/// Checked cursor for the little-endian, varint-based YSM resource stream.
pub struct ResourceReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> ResourceReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, offset: 0 } }
    pub fn position(&self) -> usize { self.offset }
    pub fn remaining(&self) -> usize { self.bytes.len().saturating_sub(self.offset) }

    pub fn read_exact(&mut self, len: usize) -> Result<&'a [u8], ResourceError> {
        let end = self.offset.checked_add(len).ok_or(ResourceError::UnexpectedEof)?;
        if end > self.bytes.len() { return Err(ResourceError::UnexpectedEof); }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub fn read_varint(&mut self) -> Result<u64, ResourceError> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = self.read_exact(1)?[0];
            if index == 9 && byte > 1 { return Err(ResourceError::VarintOverflow); }
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 { return Ok(value); }
        }
        Err(ResourceError::VarintOverflow)
    }

    pub fn read_count(&mut self, label: &'static str) -> Result<usize, ResourceError> {
        let count = usize::try_from(self.read_varint()?).map_err(|_| ResourceError::LengthLimit(label))?;
        if count > MAX_COLLECTION_ITEMS { return Err(ResourceError::LengthLimit(label)); }
        Ok(count)
    }

    pub fn read_string(&mut self) -> Result<String, ResourceError> {
        let len = usize::try_from(self.read_varint()?).map_err(|_| ResourceError::LengthLimit("string"))?;
        if len > MAX_STRING_BYTES { return Err(ResourceError::LengthLimit("string")); }
        String::from_utf8(self.read_exact(len)?.to_vec()).map_err(|_| ResourceError::InvalidUtf8)
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, ResourceError> {
        let len = usize::try_from(self.read_varint()?).map_err(|_| ResourceError::LengthLimit("byte sequence"))?;
        if len > MAX_IMAGE_BYTES { return Err(ResourceError::LengthLimit("byte sequence")); }
        Ok(self.read_exact(len)?.to_vec())
    }

    pub fn read_u32(&mut self) -> Result<u32, ResourceError> {
        Ok(u32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    pub fn read_f32(&mut self) -> Result<f32, ResourceError> {
        Ok(f32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    pub fn read_vec3(&mut self) -> Result<[f32; 3], ResourceError> {
        Ok([self.read_f32()?, self.read_f32()?, self.read_f32()?])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct YsmVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}
#[derive(Clone, Debug, PartialEq)]
pub struct YsmFace {
    pub normal: [f32; 3],
    pub vertices: [YsmVertex; 4],
}
#[derive(Clone, Debug, PartialEq)]
pub struct YsmCube {
    pub faces: Vec<YsmFace>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct YsmBone {
    pub name: String,
    pub parent: String,
    pub cubes: Vec<YsmCube>,
    pub pivot: [f32; 3],
    pub rotation_radians: [f32; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub struct YsmModel {
    pub sha256: Option<String>,
    pub identifier: String,
    pub texture_width: f32,
    pub texture_height: f32,
    pub visible_bounds_width: f32,
    pub visible_bounds_height: f32,
    pub visible_bounds_offset: Vec<f32>,
    pub scale: [f32; 2],
    pub bones: Vec<YsmBone>,
    pub legacy_info: Option<YsmLegacyInfo>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct YsmLegacyInfo {
    pub name: String,
    pub tips: String,
    pub extra_animation_names: Vec<String>,
    pub authors: Vec<String>,
    pub license: String,
    pub free: bool,
}

/// Parses one YSM model record as defined by YSMParserV3::ParseModels.
/// The caller supplies the format version and handles the surrounding
/// resource tables/record boundaries.
pub fn parse_model_record(bytes: &[u8], format: u32) -> Result<(YsmModel, usize), ResourceError> {
    let mut reader = ResourceReader::new(bytes);
    let sha256 = if format > 15 { Some(reader.read_string()?) } else { None };
    let bone_count = reader.read_count("bone count")?;
    let mut bones = Vec::with_capacity(bone_count);

    for _ in 0..bone_count {
        let parent = reader.read_string()?;
        let cube_count = reader.read_count("cube count")?;
        let mut cubes = Vec::with_capacity(cube_count);
        for _ in 0..cube_count {
            let face_count = reader.read_count("face count")?;
            if face_count > 6 { return Err(ResourceError::InvalidModel("a cube has more than six faces")); }
            let mut faces = Vec::with_capacity(face_count);
            for _ in 0..face_count {
                let normal = reader.read_vec3()?;
                let mut vertices = Vec::with_capacity(4);
                for _ in 0..4 {
                    vertices.push(YsmVertex {
                        position: reader.read_vec3()?,
                        uv: [reader.read_f32()?, reader.read_f32()?],
                    });
                }
                let vertices: [YsmVertex; 4] = vertices.try_into()
                    .map_err(|_| ResourceError::InvalidModel("face must have four vertices"))?;
                faces.push(YsmFace { normal, vertices });
            }
            // Three reserved values follow every cube. YSMParser currently
            // accepts them but does not assign semantics.
            let _reserved = [reader.read_varint()?, reader.read_varint()?, reader.read_varint()?];
            cubes.push(YsmCube { faces });
        }

        let name = reader.read_string()?;
        let _unknown0 = reader.read_varint()?;
        for _ in 0..3 {
            if reader.read_varint()? != 0 {
                return Err(ResourceError::InvalidModel("non-zero reserved bone field"));
            }
        }
        let _unknown1 = reader.read_varint()?;
        let pivot = reader.read_vec3()?;
        let rotation_radians = reader.read_vec3()?;
        bones.push(YsmBone { name, parent, cubes, pivot, rotation_radians });
    }

    let identifier = reader.read_string()?;
    let texture_height = reader.read_f32()?;
    let texture_width = reader.read_f32()?;
    let visible_bounds_height = reader.read_f32()?;
    let visible_bounds_width = reader.read_f32()?;
    let offset_count = reader.read_count("visible bounds offset")?;
    if offset_count > 16 { return Err(ResourceError::InvalidModel("too many visible bounds offset values")); }
    let mut visible_bounds_offset = Vec::with_capacity(offset_count);
    for _ in 0..offset_count { visible_bounds_offset.push(reader.read_f32()?); }
    let scale = [reader.read_f32()?, reader.read_f32()?];

    let has_legacy_info = reader.read_varint()? != 0;
    let legacy_info = if has_legacy_info {
        let name = reader.read_string()?;
        let tips = reader.read_string()?;
        let mut extra_animation_names = Vec::new();
        for _ in 0..reader.read_count("legacy extra animation")? {
            extra_animation_names.push(reader.read_string()?);
        }
        let mut authors = Vec::new();
        for _ in 0..reader.read_count("legacy author")? { authors.push(reader.read_string()?); }
        let license = reader.read_string()?;
        let free = reader.read_varint()? != 0;
        Some(YsmLegacyInfo { name, tips, extra_animation_names, authors, license, free })
    } else { None };

    // Three trailing flags/fields are reserved in the currently parsed layout.
    let _tail = [reader.read_varint()?, reader.read_varint()?, reader.read_varint()?];
    Ok((YsmModel {
        sha256, identifier, texture_width, texture_height, visible_bounds_width,
        visible_bounds_height, visible_bounds_offset, scale, bones, legacy_info,
    }, reader.position()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YsmTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
}

/// Converts YSMParser's raw RGBA8 texture payload into PNG bytes.
pub fn encode_rgba_texture(name: impl Into<String>, width: u32, height: u32, rgba: &[u8]) -> Result<YsmTexture, ResourceError> {
    if width == 0 || height == 0 { return Err(ResourceError::InvalidImageDimensions); }
    let expected = (width as usize).checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ResourceError::InvalidImageDimensions)?;
    if expected > MAX_IMAGE_BYTES { return Err(ResourceError::LengthLimit("texture")); }
    if rgba.len() != expected { return Err(ResourceError::InvalidImageDataLength); }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut png_bytes), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| ResourceError::InvalidImage(e.to_string()))?;
        writer.write_image_data(rgba).map_err(|e| ResourceError::InvalidImage(e.to_string()))?;
    }
    Ok(YsmTexture { name: name.into(), width, height, png_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 { out.push((value as u8) | 0x80); value >>= 7; }
        out.push(value as u8);
    }
    fn push_string(out: &mut Vec<u8>, value: &str) {
        push_varint(out, value.len() as u64); out.extend_from_slice(value.as_bytes());
    }
    fn push_f32(out: &mut Vec<u8>, value: f32) { out.extend_from_slice(&value.to_le_bytes()); }

    #[test]
    fn reads_varints_and_utf8_strings() {
        let mut bytes = Vec::new(); push_varint(&mut bytes, 300); push_string(&mut bytes, "骨");
        let mut reader = ResourceReader::new(&bytes);
        assert_eq!(reader.read_varint().unwrap(), 300);
        assert_eq!(reader.read_string().unwrap(), "骨");
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn parses_a_minimal_format_four_model_record() {
        let mut bytes = Vec::new();
        push_varint(&mut bytes, 1); // bones
        push_string(&mut bytes, ""); // parent
        push_varint(&mut bytes, 1); // cubes
        push_varint(&mut bytes, 1); // faces
        for v in [0.0, 0.0, 1.0] { push_f32(&mut bytes, v); } // normal
        for i in 0..4 {
            for v in [i as f32, 0.0, 0.0, i as f32, 0.0] { push_f32(&mut bytes, v); }
        }
        for _ in 0..3 { push_varint(&mut bytes, 0); }
        push_string(&mut bytes, "root");
        for v in [0, 0, 0, 0, 0] { push_varint(&mut bytes, v); }
        for v in [0.0, 0.0, 0.0, 0.0, 0.0, 0.0] { push_f32(&mut bytes, v); }
        push_string(&mut bytes, "geometry.test");
        for v in [64.0, 64.0, 2.0, 2.0] { push_f32(&mut bytes, v); }
        push_varint(&mut bytes, 3);
        for v in [0.0, 0.0, 0.0] { push_f32(&mut bytes, v); }
        push_f32(&mut bytes, 1.0); push_f32(&mut bytes, 1.0);
        push_varint(&mut bytes, 0); // no legacy info
        for _ in 0..3 { push_varint(&mut bytes, 0); }

        let (model, consumed) = parse_model_record(&bytes, 4).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(model.identifier, "geometry.test");
        assert_eq!(model.bones[0].name, "root");
        assert_eq!(model.bones[0].cubes[0].faces.len(), 1);
    }

    #[test]
    fn encodes_rgba_texture_as_png() {
        let texture = encode_rgba_texture("test", 1, 1, &[255, 0, 0, 255]).unwrap();
        assert_eq!(&texture.png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn rejects_wrong_rgba_length() {
        assert!(matches!(encode_rgba_texture("bad", 2, 2, &[0; 4]), Err(ResourceError::InvalidImageDataLength)));
    }
}
