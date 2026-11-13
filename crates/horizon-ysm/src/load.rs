//! High-level loader that connects YSM container decryption and resource parsing.
use std::path::Path;

use crate::{
    decrypt_container, parse_header, parse_resource_v4_v9, DecryptError, HeaderError,
    ResourceError, YsmHeader, YsmResource,
};

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedYsm {
    pub header: YsmHeader,
    pub resource: YsmResource,
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Header(HeaderError),
    Decrypt(DecryptError),
    Resource(ResourceError),
    UnsupportedFormat(u32),
}
impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read YSM file: {e}"),
            Self::Header(e) => write!(f, "invalid YSM header: {e}"),
            Self::Decrypt(e) => write!(f, "could not decrypt YSM file: {e}"),
            Self::Resource(e) => write!(f, "could not parse YSM resources: {e}"),
            Self::UnsupportedFormat(v) => write!(f, "YSM format {v} is not yet supported by the resource decoder"),
        }
    }
}
impl std::error::Error for LoadError {}
impl From<HeaderError> for LoadError { fn from(e: HeaderError) -> Self { Self::Header(e) } }
impl From<DecryptError> for LoadError { fn from(e: DecryptError) -> Self { Self::Decrypt(e) } }
impl From<ResourceError> for LoadError { fn from(e: ResourceError) -> Self { Self::Resource(e) } }

pub fn load_ysm_file(path: impl AsRef<Path>) -> Result<LoadedYsm, LoadError> {
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;
    load_ysm_bytes(&bytes)
}

pub fn load_ysm_bytes(bytes: &[u8]) -> Result<LoadedYsm, LoadError> {
    let header = parse_header(bytes)?;
    if !(4..=9).contains(&header.format) {
        return Err(LoadError::UnsupportedFormat(header.format));
    }
    let decompressed = decrypt_container(bytes)?;
    let (resource, _consumed) = parse_resource_v4_v9(&decompressed, header.format)?;
    Ok(LoadedYsm { header, resource })
}
