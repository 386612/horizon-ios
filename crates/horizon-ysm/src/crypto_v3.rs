//! YSM Crypto v3 decryption pipeline.
//! The algorithm order and modified CityHash constants match OpenYSM/YSMParser.
use std::io::{Cursor, Read};

use crate::cityhash::city_hash64_with_seed;
use crate::mt19937_64::mt19937_64_xor;
use crate::{ContainerError, YsmContainer};

const SEED_KEY_DERIVATION: u64 = 0xD017_CBBA_7B5D_3581;
const SEED_RES_VERIFICATION: u64 = 0xA62B_1A2C_4384_2BC3;
const SEED_FILE_VERIFICATION: u64 = 0x9E55_99DB_80C6_7C29;
const MAX_DECOMPRESSED_SIZE: usize = 512 * 1024 * 1024;

#[derive(Debug)]
pub enum DecryptError {
    Container(ContainerError),
    FileHashMismatch { expected: u64, actual: u64 },
    PayloadTooShort,
    InvalidNonceLength,
    InvalidZstdFrame(&'static str),
    Zstd(String),
    DecompressedDataTooLarge,
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(e) => write!(f, "{e}"),
            Self::FileHashMismatch { expected, actual } =>
                write!(f, "YSM file hash mismatch (stored {expected:#018x}, calculated {actual:#018x})"),
            Self::PayloadTooShort => f.write_str("YSM encrypted payload is too short"),
            Self::InvalidNonceLength => f.write_str("YSM nonce length is outside the decrypted payload"),
            Self::InvalidZstdFrame(e) => write!(f, "invalid YSM Zstandard frame: {e}"),
            Self::Zstd(e) => write!(f, "Zstandard decompression failed: {e}"),
            Self::DecompressedDataTooLarge => f.write_str("decompressed YSM payload exceeds the 512 MiB safety limit"),
        }
    }
}
impl std::error::Error for DecryptError {}
impl From<ContainerError> for DecryptError {
    fn from(value: ContainerError) -> Self { Self::Container(value) }
}

/// Verifies and fully decrypts a Crypto v3 container, returning the
/// decompressed serialized YSM resource stream.
pub fn decrypt_container(file: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let container = crate::parse_container(file)?;
    decrypt_parsed(file, &container)
}

fn decrypt_parsed(file: &[u8], container: &YsmContainer<'_>) -> Result<Vec<u8>, DecryptError> {
    if file.len() < 8 { return Err(DecryptError::PayloadTooShort); }
    let actual_hash = city_hash64_with_seed(&file[..file.len() - 8], SEED_FILE_VERIFICATION);
    if actual_hash != container.stored_file_hash {
        return Err(DecryptError::FileHashMismatch {
            expected: container.stored_file_hash,
            actual: actual_hash,
        });
    }

    // Upstream YSMParser performs ModifiedChaChaDecrypt first, then MT19937 XOR.
    let chacha = modified_xchacha_decrypt(
        container.encrypted_payload,
        &container.key,
        &container.iv,
        SEED_RES_VERIFICATION,
    );
    let xorred = mt19937_64_xor(
        &chacha,
        city_hash64_with_seed(
            &[container.key.as_slice(), container.iv.as_slice()].concat(),
            SEED_KEY_DERIVATION,
        ),
    );
    if xorred.len() < 2 { return Err(DecryptError::PayloadTooShort); }
    let nonce_len = (u16::from_le_bytes([xorred[0], xorred[1]]) & 0x03ff) as usize;
    let data_start = 2usize.checked_add(nonce_len).ok_or(DecryptError::InvalidNonceLength)?;
    if data_start > xorred.len() { return Err(DecryptError::InvalidNonceLength); }
    decompress_ysm_zstd(&xorred[data_start..])
}

fn modified_xchacha_decrypt(data: &[u8], key: &[u8; 32], iv: &[u8; 24], seed: u64) -> Vec<u8> {
    let mut key_iv = [0u8; 56];
    key_iv[..32].copy_from_slice(key);
    key_iv[32..].copy_from_slice(iv);
    let hash2 = city_hash64_with_seed(&key_iv, seed);
    let mut next_round_size = (((hash2 & 0x3f) | 0x40) << 6) as usize;
    let initial_rounds = (10 * (hash2 % 3) + 10) as u32;
    set_rounds(initial_rounds);
    let mut state = xchacha_initial_state(key, iv, initial_rounds);
    let mut output = Vec::with_capacity(data.len());
    let mut offset = 0usize;

    while offset < data.len() {
        let take = next_round_size.min(data.len() - offset);
        let chunk = &data[offset..offset + take];
        let plain = chacha_xor(&mut state, chunk);
        let res_hash = city_hash64_with_seed(&plain, seed);
        next_round_size = update_xchacha_state(&mut state, res_hash);
        output.extend_from_slice(&plain);
        offset += take;
    }
    output
}

fn update_xchacha_state(state: &mut [u32; 16], hash: u64) -> usize {
    let rounds = 10 * (hash % 3) + 10;
    state[0] = state[0]; // Explicitly retain the HChaCha-derived state.
    for i in 4..16 {
        state[i] ^= if i % 2 == 0 { hash as u32 } else { (hash >> 32) as u32 };
    }
    // Rounds are carried separately in the counter slot's companion state.
    // The stream function reads this value from the side-channel below.
    set_rounds(rounds as u32);
    (((hash & 0x3f) | 0x40) << 6) as usize
}

thread_local! {
    static CURRENT_ROUNDS: std::cell::Cell<u32> = const { std::cell::Cell::new(20) };
}
fn set_rounds(rounds: u32) { CURRENT_ROUNDS.with(|v| v.set(rounds)); }
fn get_rounds() -> u32 { CURRENT_ROUNDS.with(std::cell::Cell::get) }

fn xchacha_initial_state(key: &[u8; 32], iv: &[u8; 24], rounds: u32) -> [u32; 16] {
    let mut input = [0u32; 16];
    input[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for i in 0..8 {
        input[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let subkey = hchacha20(&input, &iv[..16], rounds);
    input[4..12].copy_from_slice(&subkey[0..8]);
    input[12] = 0;
    input[13] = 0;
    input[14] = u32::from_le_bytes(iv[16..20].try_into().unwrap());
    input[15] = u32::from_le_bytes(iv[20..24].try_into().unwrap());
    input
}

fn hchacha20(base: &[u32; 16], nonce: &[u8], rounds: u32) -> [u32; 8] {
    let mut x = *base;
    for i in 0..4 {
        x[12 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }
    chacha_rounds(&mut x, rounds);
    [x[0], x[1], x[2], x[3], x[12], x[13], x[14], x[15]]
}

fn chacha_rounds(x: &mut [u32; 16], rounds: u32) {
    for _ in (0..rounds).step_by(2) {
        quarter_round(x, 0, 4, 8, 12);
        quarter_round(x, 1, 5, 9, 13);
        quarter_round(x, 2, 6, 10, 14);
        quarter_round(x, 3, 7, 11, 15);
        quarter_round(x, 0, 5, 10, 15);
        quarter_round(x, 1, 6, 11, 12);
        quarter_round(x, 2, 7, 8, 13);
        quarter_round(x, 3, 4, 9, 14);
    }
}

fn quarter_round(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[a] = x[a].wrapping_add(x[b]); x[d] = (x[d] ^ x[a]).rotate_left(16);
    x[c] = x[c].wrapping_add(x[d]); x[b] = (x[b] ^ x[c]).rotate_left(12);
    x[a] = x[a].wrapping_add(x[b]); x[d] = (x[d] ^ x[a]).rotate_left(8);
    x[c] = x[c].wrapping_add(x[d]); x[b] = (x[b] ^ x[c]).rotate_left(7);
}

fn chacha_xor(state: &mut [u32; 16], input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let rounds = get_rounds();
    for chunk in input.chunks(64) {
        let initial = *state;
        let mut block = initial;
        chacha_rounds(&mut block, rounds);
        for i in 0..16 { block[i] = block[i].wrapping_add(initial[i]); }
        let mut stream = [0u8; 64];
        for i in 0..16 { stream[i * 4..i * 4 + 4].copy_from_slice(&block[i].to_le_bytes()); }
        output.extend(chunk.iter().zip(stream.iter()).map(|(a, b)| a ^ b));
        state[12] = state[12].wrapping_add(1);
        if state[12] == 0 { state[13] = state[13].wrapping_add(1); }
    }
    output
}

fn decompress_ysm_zstd(input: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let washed = wash_ysm_zstd(input)?;
    let cursor = Cursor::new(washed);
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(cursor)
        .map_err(|e| DecryptError::Zstd(e.to_string()))?;
    let mut output = Vec::new();
    let mut limited = decoder.take((MAX_DECOMPRESSED_SIZE as u64) + 1);
    limited.read_to_end(&mut output).map_err(|e| DecryptError::Zstd(e.to_string()))?;
    if output.len() > MAX_DECOMPRESSED_SIZE { return Err(DecryptError::DecompressedDataTooLarge); }
    Ok(output)
}

fn wash_ysm_zstd(input: &[u8]) -> Result<Vec<u8>, DecryptError> {
    if input.len() < 5 { return Err(DecryptError::InvalidZstdFrame("frame is shorter than its header")); }
    if input[..4] != [0x28, 0xb5, 0x2f, 0xfd] {
        return Err(DecryptError::InvalidZstdFrame("unexpected magic number"));
    }
    let mut data = input.to_vec();
    let fhd = data[4];
    data[4] = fhd & 0xfb; // YSM toggles the standard checksum bit.
    let fcs_bits = (fhd >> 6) & 3;
    let single_segment = (fhd & 0x20) != 0;
    let dict_bits = fhd & 3;
    let dict_size = match dict_bits { 0 => 0, 1 => 1, 2 => 2, _ => 4 };
    let fcs_size = match fcs_bits { 0 if single_segment => 1, 0 => 0, 1 => 2, 2 => 4, _ => 8 };
    let frame_header_size = 1 + usize::from(!single_segment) + dict_size + fcs_size;
    let mut offset = 4usize.checked_add(frame_header_size).ok_or(DecryptError::InvalidZstdFrame("header length overflow"))?;
    if offset > data.len() { return Err(DecryptError::InvalidZstdFrame("truncated frame header")); }

    loop {
        if offset.checked_add(3).filter(|end| *end <= data.len()).is_none() {
            return Err(DecryptError::InvalidZstdFrame("truncated block header"));
        }
        let b0 = data[offset] as u32;
        let b1 = data[offset + 1] as u32;
        let b2 = data[offset + 2] as u32;
        let last = (b0 >> 7) & 1;
        let ysm_type = (b0 >> 5) & 3;
        let raw_size = ((b0 & 0x1f) << 16) | b1 | (b2 << 8);
        let compressed_size = raw_size ^ 0xd4e9;
        if compressed_size > 0x1ffff { return Err(DecryptError::InvalidZstdFrame("block size exceeds Zstandard limit")); }
        let std_type = match ysm_type {
            0 => 2u32, 1 => 1u32, 2 => return Err(DecryptError::InvalidZstdFrame("reserved block type")),
            3 => 0u32, _ => unreachable!(),
        };
        let std_header = last | (std_type << 1) | (compressed_size << 3);
        data[offset] = std_header as u8;
        data[offset + 1] = (std_header >> 8) as u8;
        data[offset + 2] = (std_header >> 16) as u8;
        let block_data_size = if std_type == 1 { 1 } else { compressed_size as usize };
        offset = offset.checked_add(3 + block_data_size).ok_or(DecryptError::InvalidZstdFrame("block offset overflow"))?;
        if offset > data.len() { return Err(DecryptError::InvalidZstdFrame("truncated block data")); }
        if last == 1 { break; }
    }
    Ok(data)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_ysm_raw_block_header_and_decompresses_frame() {
        // A single-segment Zstandard frame containing the raw bytes "abc",
        // with its block header transformed using the YSM v3 mapping.
        let ysm_frame = [
            0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x03,
            0xe0, 0xea, 0xd4, b'a', b'b', b'c',
        ];
        let washed = wash_ysm_zstd(&ysm_frame).expect("wash YSM Zstandard frame");
        assert_eq!(&washed[6..9], &[0x19, 0x00, 0x00]);
        assert_eq!(decompress_ysm_zstd(&ysm_frame).expect("decompress frame"), b"abc");
    }

    #[test]
    fn rejects_invalid_zstd_magic() {
        assert!(matches!(wash_ysm_zstd(b"nope!"), Err(DecryptError::InvalidZstdFrame(_))));
    }

    #[test]
    fn rejects_truncated_nonce_prefix() {
        assert!(matches!(decrypt_parsed(b"short", &dummy_container()), Err(DecryptError::PayloadTooShort)));
    }

    fn dummy_container<'a>() -> YsmContainer<'a> {
        let header = crate::parse_header(b"YSGP<format> 4\\n<crypto> 3\\n\\0x").unwrap();
        YsmContainer {
            header,
            crypto_version: 3,
            encrypted_payload: &[],
            key: [0; 32],
            iv: [0; 24],
            stored_file_hash: 0,
        }
    }
}
