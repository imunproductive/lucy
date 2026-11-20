//! What the frame sampler makes of the client's raw memory: the mover's
//! movement state, the camera's encrypted matrix and statescript values.
//! Layouts: `D:/Work/Coding/refs/reports/ClientProbe.md`.

use serde_json::{Value, json};

/// Component 31 (the mover): its movement state (the one an owner
/// correction restores, `0x7FF789B73E00`; Grapple.md "DayGrapple7").
pub const MOVEMENT_STATE: u64 = 0xD40;
/// Bytes of the movement state the sampler reads.
pub const MOVEMENT_BYTES: usize = 0xA0;
/// Movement flags of a body in the air: jumped, fell, no ground under it
/// (`orpheus_physics::mover::posture::IN_AIR`).
pub const IN_AIR: u32 = 0x0080_000C;

/// The movement state's fields the sampler records.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Movement {
    /// The command frame the state was simulated to (`+0x8`).
    pub frame: u32,
    /// The movement flags (`+0x0`, the server's `Movement::flags`).
    pub flags: u32,
    /// The frames since the body left the ground (`+0x10`).
    pub air_ticks: u32,
    /// The feet (`+0x70`).
    pub position: [f32; 3],
    /// `+0x90`, per second.
    pub velocity: [f32; 3],
}

fn f32x3(bytes: &[u8]) -> [f32; 3] {
    [0, 4, 8].map(|at| f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes")))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

/// The movement state keeps the feet as two words at `+0x70` encrypted with
/// the seed at `+0x80`: key `seed + table[seed & 7]`, stepping as
/// [`decrypt_words`] (the compare `0x7FF789B6D640` reads them so).
pub const POSITION_TABLE: [u64; 8] = [
    0x7E12_4173_6A5F_3F75,
    0x4741_9D50_8F1D_EF6B,
    0x65DA_6DD6_7068_2862,
    0x9A71_E90F_A067_A80B,
    0xB3AE_4BDC_4415_9626,
    0x6BC1_2670_1A3D_8EE0,
    0xD5CA_C8FA_03EE_D152,
    0x13EF_2256_6C3C_80B9,
];

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

impl Movement {
    /// From the first [`MOVEMENT_BYTES`] of the movement state.
    pub fn parse(state: &[u8]) -> Option<Self> {
        if state.len() < MOVEMENT_BYTES {
            return None;
        }
        let seed = u64_at(state, 0x80);
        let key = seed.wrapping_add(POSITION_TABLE[(seed & 7) as usize]);
        let feet = decrypt_words(
            [u64_at(state, 0x70), u64_at(state, 0x78)],
            key,
            &POSITION_TABLE,
        );
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&feet[0].to_le_bytes());
        bytes[8..].copy_from_slice(&feet[1].to_le_bytes());
        Some(Self {
            frame: u32_at(state, 0x8),
            flags: u32_at(state, 0x0),
            air_ticks: u32_at(state, 0x10),
            position: f32x3(&bytes),
            velocity: f32x3(&state[0x90..]),
        })
    }

    pub fn grounded(&self) -> bool {
        self.flags & IN_AIR == 0
    }

    pub fn json(&self) -> Value {
        json!({
            "frame": self.frame, "flags": self.flags, "air_ticks": self.air_ticks,
            "grounded": self.grounded(), "position": self.position, "velocity": self.velocity,
        })
    }
}

/// The camera object keeps its world matrix (4 rows of 4 floats: right, up,
/// forward, position) as 8 words, each XORed with a key that steps by a
/// table entry chosen by its low 3 bits (`0x7FF78970A120` decrypts it).
pub const CAMERA_MATRIX_TABLE: [u64; 8] = [
    0x912A_E5B4_1288_1E99,
    0xA879_3997_F7CA_CE87,
    0x8AE2_C911_08BF_098E,
    0x7549_4DC8_D8B0_89E7,
    0x5C96_EF1B_3CC2_B7CA,
    0x84F9_82B7_62EA_AF0C,
    0x3AF2_6C3D_7B39_F0BE,
    0xFCD7_8691_14EB_A155,
];

/// The words of an encrypted block: word `i` XOR the key, the key stepping by
/// `table[key & 7]` after each.
pub fn decrypt_words<const N: usize>(words: [u64; N], mut key: u64, table: &[u64; 8]) -> [u64; N] {
    words.map(|word| {
        let plain = word ^ key;
        key = key.wrapping_add(table[(key & 7) as usize]);
        plain
    })
}

/// The camera: where it is and where it looks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Camera {
    pub position: [f32; 3],
    /// Unit forward (the matrix's third row).
    pub forward: [f32; 3],
}

impl Camera {
    /// From the decrypted matrix words.
    pub fn from_matrix(words: [u64; 8]) -> Self {
        let floats: Vec<f32> = words
            .iter()
            .flat_map(|word| {
                let bytes = word.to_le_bytes();
                [
                    f32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")),
                    f32::from_le_bytes(bytes[4..].try_into().expect("4 bytes")),
                ]
            })
            .collect();
        Self {
            forward: [floats[8], floats[9], floats[10]],
            position: [floats[12], floats[13], floats[14]],
        }
    }

    /// The forward's heading in degrees: 0 along +z, 90 along +x (the view
    /// yaw's convention, `forward = (sin, 0, cos)`).
    pub fn yaw(&self) -> f32 {
        self.forward[0].atan2(self.forward[2]).to_degrees()
    }

    /// Degrees above the horizon.
    pub fn pitch(&self) -> f32 {
        self.forward[1].clamp(-1.0, 1.0).asin().to_degrees()
    }

    pub fn json(&self) -> Value {
        json!({
            "position": self.position, "forward": self.forward,
            "yaw": self.yaw(), "pitch": self.pitch(),
        })
    }
}

/// A statescript value (16 bytes: data, then the type at `+0xC`; the types
/// from `ScriptValue_ToBool` `0x7FF789E74940`) as JSON: bool, int, float,
/// vector, `{"entity": id}`, null when unset, else `{"type", "hex"}`.
pub fn script_value(bytes: &[u8; 16]) -> Value {
    let kind = u32_at(bytes, 0xC);
    match kind {
        1 => (bytes[0] != 0).into(),
        2 => (u32_at(bytes, 0) as i32).into(),
        3 => finite(f32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))),
        6 => f32x3(bytes).map(finite).to_vec().into(),
        0xA => json!({ "entity": u32_at(bytes, 0) }),
        0xFF => Value::Null,
        _ => json!({
            "type": kind,
            "hex": bytes[..12].iter().map(|b| format!("{b:02x}")).collect::<String>(),
        }),
    }
}

fn finite(value: f32) -> Value {
    if value.is_finite() {
        f64::from(value).into()
    } else {
        format!("{value}").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feet_decrypt_with_their_seed() {
        // A Practice Range spawn's movement state (Doomfist, 2026-11-20): feet (54.565, 1.0, 42.147).
        let mut state = vec![0u8; MOVEMENT_BYTES];
        let raw = [
            0xE020_5B27_783B_2D05u64,
            0x4B61_8197_16B6_6BE5,
            0x73DF_34B7_2023_DF25,
        ];
        for (index, word) in raw.iter().enumerate() {
            state[0x70 + 8 * index..0x78 + 8 * index].copy_from_slice(&word.to_le_bytes());
        }
        let feet = Movement::parse(&state).unwrap().position;
        assert_eq!(feet, [54.565_43, 1.0, 42.147_46]);
    }

    #[test]
    fn camera_words_decrypt_with_the_stepping_key() {
        let plain = [1, 2, 3, 4, 5, 6, 7, 8].map(|n: u64| n * 0x0101_0101_0101_0101);
        let mut key = 0x1234_5678_9ABC_DEF0u64;
        let start = key;
        let encrypted = plain.map(|word| {
            let cipher = word ^ key;
            key = key.wrapping_add(CAMERA_MATRIX_TABLE[(key & 7) as usize]);
            cipher
        });
        assert_eq!(decrypt_words(encrypted, start, &CAMERA_MATRIX_TABLE), plain);
    }

    #[test]
    fn a_camera_looking_along_x_and_up_has_yaw_90_and_its_pitch() {
        let row = |a: f32, b: f32| u64::from(a.to_bits()) | u64::from(b.to_bits()) << 32;
        let (s, c) = (0.5f32, 0.75f32.sqrt());
        let words = [
            row(0.0, 0.0),
            row(1.0, 0.0),
            row(0.0, 1.0),
            row(0.0, 0.0),
            row(c, s),
            row(0.0, 0.0),
            row(10.0, 2.0),
            row(-3.0, 1.0),
        ];
        let camera = Camera::from_matrix(words);
        assert_eq!(camera.position, [10.0, 2.0, -3.0]);
        assert!((camera.yaw() - 90.0).abs() < 1e-4);
        assert!((camera.pitch() - 30.0).abs() < 1e-4);
    }

    #[test]
    fn script_values_read_by_their_type() {
        let value = |data: &[u8], kind: u32| {
            let mut bytes = [0u8; 16];
            bytes[..data.len()].copy_from_slice(data);
            bytes[12..].copy_from_slice(&kind.to_le_bytes());
            script_value(&bytes)
        };
        assert_eq!(value(&[1], 1), json!(true));
        assert_eq!(value(&(-5i32).to_le_bytes(), 2), json!(-5));
        assert_eq!(value(&1.5f32.to_le_bytes(), 3), json!(1.5));
        let vector: Vec<u8> = [1.0f32, -2.0, 0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(value(&vector, 6), json!([1.0, -2.0, 0.5]));
        assert_eq!(
            value(&0xA000_0101u32.to_le_bytes(), 0xA),
            json!({"entity": 0xA000_0101u32})
        );
        assert_eq!(value(&[], 0xFF), Value::Null);
        assert_eq!(value(&[0xAB], 9)["type"], json!(9));
    }
}
