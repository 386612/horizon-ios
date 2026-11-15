//! High-level loader that connects YSM container decryption and resource parsing.
use std::path::Path;

use crate::{
    decrypt_container, parse_header, parse_resource_v4_v15, DecryptError, HeaderError,
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
    if !(4..=15).contains(&header.format) {
        return Err(LoadError::UnsupportedFormat(header.format));
    }
    let decompressed = decrypt_container(bytes)?;
    let (resource, _consumed) = parse_resource_v4_v15(&decompressed, header.format)?;
    Ok(LoadedYsm { header, resource })
}


/// One result from scanning and loading the user's YSM model directory.
/// A broken file is kept as an individual error and does not prevent other
/// models in the same directory from loading.
#[derive(Debug)]
pub struct DiscoveredYsm {
    pub path: std::path::PathBuf,
    pub result: Result<LoadedYsm, LoadError>,
}

/// Discovers and attempts to load every .ysm file under config/ysm/models.
/// Directory access errors are returned immediately. Errors opening,
/// decrypting, or parsing an individual model are stored in that entry's
/// result, allowing usable models to load alongside per-file diagnostics.
pub fn load_ysm_directory(
    game_root: impl AsRef<Path>,
) -> std::io::Result<Vec<DiscoveredYsm>> {
    let paths = crate::discover_models(game_root)?;
    Ok(paths
        .into_iter()
        .map(|path| {
            let result = load_ysm_file(&path);
            DiscoveredYsm { path, result }
        })
        .collect())
}

#[cfg(test)]
mod directory_load_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_root() -> std::path::PathBuf {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "horizon-ysm-load-test-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn loads_directory_and_keeps_invalid_file_errors_per_entry() {
        let root = test_root();
        let directory = crate::ensure_model_directory(&root).unwrap();
        std::fs::write(directory.join("broken.ysm"), b"not a YSM file").unwrap();
        std::fs::write(directory.join("ignore.txt"), b"ignored").unwrap();

        let entries = load_ysm_directory(&root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].path.file_name().unwrap().to_string_lossy(),
            "broken.ysm"
        );
        assert!(matches!(
            entries[0].result.as_ref(),
            Err(LoadError::Header(crate::HeaderError::BadMagic))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_directory_load_returns_no_entries() {
        let root = test_root();
        let entries = load_ysm_directory(&root).unwrap();
        assert!(entries.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
