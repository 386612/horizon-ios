//! A small, allocation-light reader for the public YSM container header.
//!
//! YSM payloads are encrypted. This crate deliberately separates safe
//! container/header recognition from payload decryption and model decoding;
//! accepting a header does not mean the model has been loaded.

use std::collections::BTreeMap;

/// The largest header accepted before the first NUL byte.
pub const MAX_HEADER_LEN: usize = 1024 * 1024;

/// Information that can be read without decrypting a YSM payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YsmHeader {
    /// Text between the YSGP marker and the NUL payload separator.
    pub raw: String,
    /// Header fields, with the surrounding angle brackets removed.
    pub fields: BTreeMap<String, String>,
    /// YSM container format version (`<format>`).
    pub format: u32,
    /// YSM payload crypto version (`<crypto>`).
    pub crypto: u32,
}

impl YsmHeader {
    /// Returns a header field, for example `name`, `author` or `license`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}

/// Why a byte slice could not be recognized as a supported YSM header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderError {
    TooShort,
    BadMagic,
    MissingPayloadSeparator,
    HeaderTooLarge,
    InvalidUtf8,
    MissingField(&'static str),
    InvalidNumber(&'static str),
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => f.write_str("file is too short to be a YSM container"),
            Self::BadMagic => f.write_str("missing YSGP container signature"),
            Self::MissingPayloadSeparator => f.write_str("YSM header has no NUL payload separator"),
            Self::HeaderTooLarge => f.write_str("YSM header exceeds the 1 MiB safety limit"),
            Self::InvalidUtf8 => f.write_str("YSM header is not valid UTF-8"),
            Self::MissingField(name) => write!(f, "YSM header is missing <{name}>"),
            Self::InvalidNumber(name) => write!(f, "YSM header field <{name}> is not an unsigned integer"),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Reads the unencrypted YSGP header from a `.ysm` file.
///
/// YSM files may begin with a UTF-8 BOM. The four-byte `YSGP` signature
/// follows it. A NUL byte separates the text header from the encrypted
/// payload. This function does not decrypt or interpret that payload.
pub fn parse_header(file: &[u8]) -> Result<YsmHeader, HeaderError> {
    if file.len() < 5 {
        return Err(HeaderError::TooShort);
    }

    let start = if file.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    if file.get(start..start + 4) != Some(b"YSGP".as_slice()) {
        return Err(HeaderError::BadMagic);
    }

    let body_start = start + 4;
    let relative_end = file[body_start..]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(HeaderError::MissingPayloadSeparator)?;
    if relative_end > MAX_HEADER_LEN {
        return Err(HeaderError::HeaderTooLarge);
    }

    let raw = std::str::from_utf8(&file[body_start..body_start + relative_end])
        .map_err(|_| HeaderError::InvalidUtf8)?
        .trim()
        .to_owned();

    let mut fields = BTreeMap::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(rest) = line.strip_prefix('<') else { continue };
        let Some((key, value)) = rest.split_once('>') else { continue };
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() {
            fields.insert(key.to_owned(), value.to_owned());
        }
    }

    let number = |key: &'static str| -> Result<u32, HeaderError> {
        fields
            .get(key)
            .ok_or(HeaderError::MissingField(key))?
            .parse()
            .map_err(|_| HeaderError::InvalidNumber(key))
    };

    Ok(YsmHeader {
        raw,
        fields,
        format: number("format")?,
        crypto: number("crypto")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bom_ysgp_header_and_metadata() {
        let bytes = b"\xEF\xBB\xBFYSGP<format> 4\n<crypto> 3\n<name> Aru\n<author> Author\n\0encrypted";
        let header = parse_header(bytes).expect("valid header");
        assert_eq!(header.format, 4);
        assert_eq!(header.crypto, 3);
        assert_eq!(header.get("name"), Some("Aru"));
        assert_eq!(header.get("author"), Some("Author"));
    }

    #[test]
    fn rejects_non_ysm_data() {
        assert_eq!(parse_header(b"not a model"), Err(HeaderError::BadMagic));
    }

    #[test]
    fn rejects_missing_separator() {
        assert_eq!(parse_header(b"YSGP<format> 4\n<crypto> 3"), Err(HeaderError::MissingPayloadSeparator));
    }

    #[test]
    fn rejects_missing_required_version_fields() {
        assert_eq!(
            parse_header(b"YSGP<format> 4\n\0payload"),
            Err(HeaderError::MissingField("crypto"))
        );
    }
}
