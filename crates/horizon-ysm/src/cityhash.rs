//! FFI boundary for the YSM-modified CityHash64 implementation.
unsafe extern "C" {
    fn CityHash64WithSeed(buf: *const std::ffi::c_char, len: usize, seed: u64) -> u64;
}

pub(crate) fn city_hash64_with_seed(data: &[u8], seed: u64) -> u64 {
    // CityHash accepts an arbitrary byte buffer and does not retain the pointer.
    let ptr = if data.is_empty() { std::ptr::NonNull::<u8>::dangling().as_ptr() } else { data.as_ptr() };
    unsafe { CityHash64WithSeed(ptr.cast(), data.len(), seed) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ysm_modified_cityhash_is_deterministic() {
        assert_eq!(
            city_hash64_with_seed(b"YSM", 0x9E5599DB80C67C29),
            city_hash64_with_seed(b"YSM", 0x9E5599DB80C67C29)
        );
    }
}
