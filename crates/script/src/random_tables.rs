//! The client's two random tables of 16,384 entries, which it fills once
//! at start-up and indexes with 14-bit masks: shot spread
//! (`orpheus_sim::weapon_spread`) and the statescript's table picks
//! (`STU_4EC41E8A`).
//!
//! The tables are made here, not read from the install. Ported from
//! leafall903/ORPHEUS (MIT), `src/ow174/game/weapon_spread.py` (commit
//! c239a84), which checked their whole output against a capture of the
//! client's tables (the fingerprints in the tests are upstream's); the
//! uniform table also matches the image dump (`0x7FF78C1519D0`).

use std::sync::LazyLock;

/// The entries of each table.
pub const TABLE: usize = 16_384;
const INDEX_MASK: u32 = TABLE as u32 - 1;

static UNIFORM: LazyLock<Box<[u32]>> = LazyLock::new(generate_uniform);
static FLOAT: LazyLock<Box<[f32]>> = LazyLock::new(generate_float);

/// The client's uniform table (`0x7FF78C1519D0`, filled by
/// 0x7FF7894ADF60 → 0x7FF78A75B370 from a thread RNG object, vtable
/// 0x7FF78B691A60): a 128-bit SFMT (SFMT-19937; seed 0x7FF78A75BE10,
/// refill 0x7FF78A75BF30) seeded with 1, each generation's 624 words read
/// from its end.
pub fn uniform_table() -> &'static [u32] {
    &UNIFORM
}

/// The client's float table (`184B280`): the sum of three signed xorshift
/// samples, each in [-1, 1), as a float32. Upstream inferred the seed
/// `03AF714A` from the captured end state and confirmed it by regenerating
/// all 16,384 captured values.
pub fn float_table() -> &'static [f32] {
    &FLOAT
}

/// An entry of the uniform table in `[low, end)` (0x7FF78A75B220): entry
/// `index & 0x3FFF` modulo the span, plus `low`, in unsigned 32-bit
/// arithmetic. A span of 2³² − 1 adds the entry as it is; the client
/// divides by zero on a span of 0 (`low` the least and `end` past the
/// greatest i32), which also takes the whole entry here.
pub fn uniform_pick(index: i32, low: i32, end: i32) -> i32 {
    let entry = uniform_table()[(index as u32 & INDEX_MASK) as usize];
    let span = (end as u32).wrapping_sub(low as u32);
    let offset = match span {
        0 | u32::MAX => entry,
        span => entry % span,
    };
    (low as u32).wrapping_add(offset) as i32
}

fn generate_uniform() -> Box<[u32]> {
    const WORDS: usize = 624;
    const BLOCKS: usize = WORDS / 4;
    const MASKS: [u32; 4] = [0xdfff_ffef, 0xddfe_cb7f, 0xbffa_ffff, 0xbfff_fff6];
    const PARITY: [u32; 4] = [1, 0, 0, 0x13c9_e684];
    let mut words = [0u32; WORDS];
    words[0] = 1;
    for index in 1..WORDS {
        let last = words[index - 1];
        words[index] = 1_812_433_253u32
            .wrapping_mul(last ^ (last >> 30))
            .wrapping_add(index as u32);
    }
    // The period certification: an even parity flips bit 0 of word 0.
    let mut inner = words
        .iter()
        .zip(PARITY)
        .fold(0, |inner, (word, mask)| inner ^ (word & mask));
    for shift in [16, 8, 4, 2, 1] {
        inner ^= inner >> shift;
    }
    if inner & 1 == 0 {
        words[0] ^= 1;
    }
    let mut state: Vec<u128> = words
        .chunks_exact(4)
        .map(|lanes| {
            lanes.iter().enumerate().fold(0, |block, (lane, &word)| {
                block | u128::from(word) << (32 * lane)
            })
        })
        .collect();
    let lane = |block: u128, lane: usize| (block >> (32 * lane)) as u32;
    let mut values = Vec::with_capacity(TABLE + WORDS);
    while values.len() < TABLE {
        let (mut previous, mut last) = (state[BLOCKS - 2], state[BLOCKS - 1]);
        for index in 0..BLOCKS {
            let (current, ahead) = (state[index], state[(index + 122) % BLOCKS]);
            let mut right = 0u128;
            let mut left = 0u128;
            for (at, mask) in MASKS.into_iter().enumerate() {
                right |= u128::from((lane(ahead, at) >> 11) & mask) << (32 * at);
                left |= u128::from(lane(last, at) << 18) << (32 * at);
            }
            let result = current ^ (current << 8) ^ (previous >> 8) ^ right ^ left;
            state[index] = result;
            (previous, last) = (last, result);
        }
        let start = values.len();
        values.extend(
            state
                .iter()
                .flat_map(|&block| (0..4).map(move |at| lane(block, at))),
        );
        values[start..].reverse();
    }
    values.truncate(TABLE);
    values.into_boxed_slice()
}

fn generate_float() -> Box<[f32]> {
    let mut state: u64 = 0x03af_714a;
    let mut sample = || {
        let old = state;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state.wrapping_add(old) as i64 as f64 * 2f64.powi(-63)
    };
    (0..TABLE)
        .map(|_| {
            let (first, second, third) = (sample(), sample(), sample());
            ((first + second) + third) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Upstream `test_generated_tables_match_complete_native_capture_fingerprints`:
    /// the SHA-256 of both whole tables (little-endian) as the client's.
    #[test]
    fn the_tables_match_the_captured_fingerprints() {
        let uniform: Vec<u8> = uniform_table()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let float: Vec<u8> = float_table()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let hex = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(&uniform),
            "4f99ff210621528fff6737582c39f0ed5464cdf1c9980f03d503dcf311b356f6"
        );
        assert_eq!(
            hex(&float),
            "b3d0a3ea584bfc99c11bc39a490e7a2765c9ef74e55db6d8aafb1803c6a25281"
        );
        let float = |at: usize| f64::from(float_table()[at]);
        assert_eq!(
            [float(0), float(1), float(2), float(3), float(TABLE - 1)],
            [
                0.837_978_243_827_819_8,
                -0.488_391_011_953_353_9,
                0.258_714_139_461_517_33,
                -1.135_897_397_994_995_1,
                -0.101_589_024_066_925_05
            ]
        );
    }

    /// Entries of the uniform table as the image dump holds them
    /// (`python -m tools.owsearch read 0x7FF78C1519D0+4i --dword`), and
    /// 0x7FF78A75B220's pick from them.
    #[test]
    fn the_uniform_table_and_its_pick_are_the_dumps() {
        let dump = [
            (0, 0x9cd0_8d8b),
            (1, 0xba46_25bc),
            (2, 0x137f_3ac7),
            (3, 0xb74f_6e34),
            (100, 0x5d34_89a1),
            (700, 0x0dac_73d4),
            (5000, 0xd68d_d39f),
            (9999, 0x66ac_bb7c),
            (TABLE - 1, 0x0b41_7fac),
        ];
        for (at, entry) in dump {
            assert_eq!(uniform_table()[at], entry, "entry {at}");
        }
        // 0x9CD08D8B = 2630913419 = 3 × 876971139 + 2; 0x0DAC73D4 =
        // 229405652 is 0 modulo 2 and 2 modulo 3.
        assert_eq!(uniform_pick(0, 0, 3), 2);
        assert_eq!(uniform_pick(700, 1, 3), 1);
        assert_eq!(uniform_pick(700, 0, 3), 2);
        // The index keeps its low 14 bits (a negative one too).
        assert_eq!(uniform_pick(700 + 16_384, 0, 3), 2);
        assert_eq!(uniform_pick(-16_384, 0, 3), 2);
        // The whole 32-bit span adds the entry as it is.
        assert_eq!(uniform_pick(0, 0, -1), 0x9cd0_8d8b_u32 as i32);
        assert_eq!(
            uniform_pick(1, 5, 4),
            5_i32.wrapping_add(0xba46_25bc_u32 as i32)
        );
    }
}
