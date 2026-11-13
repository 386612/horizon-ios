//! The MT19937-64 byte-stream XOR stage used by YSM Crypto v3.
//!
//! YSM derives the generator seed from the 32-byte key and 24-byte IV using
//! its CityHash64 variant. This module accepts the already-derived seed so
//! the generator can be tested independently of that hash implementation.

const NN: usize = 312;
const MM: usize = 156;
const MATRIX_A: u64 = 0xB502_6F5A_A966_19E9;
const UM: u64 = 0xFFFF_FFFF_8000_0000;
const LM: u64 = 0x7FFF_FFFF;

#[derive(Clone)]
pub(crate) struct Mt19937_64 {
    state: [u64; NN],
    index: usize,
}

impl Mt19937_64 {
    pub(crate) fn new(seed: u64) -> Self {
        let mut state = [0u64; NN];
        state[0] = seed;
        for i in 1..NN {
            state[i] = 6_364_136_223_846_793_005u64
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Self { state, index: NN }
    }

    fn twist(&mut self) {
        for i in 0..NN {
            let x = (self.state[i] & UM) | (self.state[(i + 1) % NN] & LM);
            let mut xa = x >> 1;
            if x & 1 != 0 {
                xa ^= MATRIX_A;
            }
            self.state[i] = self.state[(i + MM) % NN] ^ xa;
        }
        self.index = 0;
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        if self.index >= NN {
            self.twist();
        }

        let mut x = self.state[self.index];
        self.index += 1;
        x ^= (x >> 29) & 0x5555_5555_5555_5555;
        x ^= (x << 17) & 0x71D6_7FFF_EDA6_0000;
        x ^= (x << 37) & 0xFFF7_EEE0_0000_0000;
        x ^= x >> 43;
        x
    }
}

/// XORs bytes with the little-endian byte stream produced by MT19937-64.
///
/// This is a reversible transform. It is not, by itself, complete YSM
/// decryption: callers must derive the seed and run the modified ChaCha stage
/// in the order specified by the YSM container format.
pub fn mt19937_64_xor(data: &[u8], seed: u64) -> Vec<u8> {
    let mut generator = Mt19937_64::new(seed);
    let mut output = Vec::with_capacity(data.len());

    for chunk in data.chunks(8) {
        let stream = generator.next_u64().to_le_bytes();
        output.extend(chunk.iter().zip(stream.iter()).map(|(byte, mask)| byte ^ mask));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_mt19937_64_reference_vector() {
        let mut generator = Mt19937_64::new(5489);
        assert_eq!(generator.next_u64(), 14_514_284_786_278_117_030);
        assert_eq!(generator.next_u64(), 4_620_546_740_167_642_908);
    }

    #[test]
    fn xor_transform_is_reversible() {
        let input = b"YSM Crypto Version 3";
        let encoded = mt19937_64_xor(input, 0x1234_5678_9ABC_DEF0);
        assert_ne!(encoded, input);
        assert_eq!(mt19937_64_xor(&encoded, 0x1234_5678_9ABC_DEF0), input);
    }

    #[test]
    fn handles_partial_final_word() {
        let input = [1, 2, 3];
        assert_eq!(mt19937_64_xor(&input, 7).len(), input.len());
    }
}
