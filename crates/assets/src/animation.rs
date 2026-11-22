//! Animation clips (`006`): each animated bone's keyed scales, positions
//! and rotations, and the pose they give at a time.
//!
//! Ported from OWLib's TankLib `teAnimation` (MIT; see `CREDITS.md`). The
//! header (80 bytes): a flags word `u16` (OWLib's priority), bone count
//! `u16`, flags `u16`, an unknown `u16` and `i64`, the duration in seconds
//! `f32`, an unknown `f32`, the frame rate `f32`, an unknown `f32`, the
//! effect (`08F`) and its handle (`u64` each), then `i64` offsets: the bone
//! list at byte 48 and the track tables at byte 72. The clip has
//! `fps · duration + 1` frames (truncated). The bone list is one `i32` bone
//! id per animated bone; a track table follows for each, in the same order:
//! the scale, position and rotation key counts and a flags word (`u16`
//! each), then six `i32` offsets in 4-byte words from the table's start:
//! the three key frame lists, then the three value lists. A key frame is a
//! `u8` when the clip has at most 255 frames, else an `i16`, taken as its
//! absolute value modulo the frame count. A position is three `f32`; a
//! scale three `u16`, each `v/1024` (the client's sampler of a clip with
//! `u8` key frames, `0x7FF78A6937C0`, multiplies each part by
//! `0.0009765625`; OWLib `a1023ef` agrees. 1.74's 10,523 clips key 4.47M
//! scale parts, 1.08M of them not 1024, spread from 0 to above 4096, so
//! these are whole scales, not the near-1 deltas OWLib's earlier
//! `1 + (1 - v/1024)/1024` made of them); a
//! rotation three `u16` packing a unit quaternion's three smallest parts
//! (each 15 bits, the top bits of the first two naming the left-out part)
//! and the left-out part rebuilt as the positive square root.
//!
//! A track's values are the bone's transform relative to its parent, as
//! OWLib's exporter writes them (SEAnim, absolute): they replace the
//! skeleton's rest transform of that bone. Between keys we interpolate
//! (positions and scales linearly, rotations by normalised linear blend
//! along the shorter arc) and hold the first and last keys outside them
//! (assumption: OWLib exports keys only, and the client's sampling is not
//! in the data).
//!
//! **Track flags.** OWLib names bits of a track's flags word for newer
//! builds: `0x100` new scale packing, `0x200` the 12-byte positions (else
//! 10-byte ones), `0x400` new rotation packing. 1.74 does not follow that:
//! over its 10,523 clips `0x200` and `0x400` are never set, `0x100` is set on
//! most tracks, and every position list (with or without `0x100`) is 12
//! bytes a key (the same sampler reads 12 bytes a position key whatever
//! the flags). So we refuse only `0x400` (a packing we do not read) and do
//! not port OWLib's "no `0x200` means 10-byte positions".
//!
//! **Additive clips.** The first word is `0x100` on every clip the
//! Training Bot's blend trees play as its whole pose (they key all 43
//! bones) and has bit `0x4` set (`0x104`, `0x10C`) on every clip they play
//! as a layer (`391F`, `3950`, `3951`, `39FE`, `3A19`, `3A9D`, `27AD`,
//! `5E8D`), whose values are all near zero and the identity (the hover bob
//! `391F` moves bone `0x2` by −0.04 to +0.03 m, its rotations within 2° of
//! none). We read that bit as "the tracks are deltas to add to the pose"
//! (assumption: OWLib does not name it and the client's use of it is not in
//! the data).

use std::{collections::BTreeMap, f64::consts::SQRT_2};

use crate::props::i64_at;

/// The header's byte offsets.
const FLAGS: usize = 0;
const BONE_COUNT: usize = 2;
const DURATION: usize = 16;
const FPS: usize = 24;
const BONE_LIST: usize = 48;
const TRACKS: usize = 72;
/// A track table's size.
const TRACK_TABLE: usize = 32;
/// The flags word's bit for a clip of deltas (see the module notes).
const ADDITIVE: u16 = 0x4;
/// A track flag for rotations in OWLib's newer packing (see the module
/// notes).
const NEW_ROTATIONS: u16 = 0x400;

/// A decoded clip.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Animation {
    /// The tracks are deltas to add to a pose (see the module notes).
    pub additive: bool,
    pub duration: f32,
    pub fps: f32,
    /// `fps · duration + 1`, truncated: the frames the keys index.
    pub frames: usize,
    pub tracks: Vec<Track>,
}

/// One animated bone: its id and keys by frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Track {
    pub bone: u32,
    pub scales: BTreeMap<usize, [f32; 3]>,
    pub positions: BTreeMap<usize, [f32; 3]>,
    /// Quaternions (x, y, z, w).
    pub rotations: BTreeMap<usize, [f32; 4]>,
}

/// A bone's transform: rotation (x, y, z, w), scale, translation; in a
/// track, relative to the bone's parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bone {
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub translation: [f32; 3],
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .map(|bytes| bytes.try_into().expect("N bytes"))
        .ok_or_else(|| "truncated animation".to_owned())
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, String> {
    bytes(data, at).map(u16::from_le_bytes)
}

fn i32_at(data: &[u8], at: usize) -> Result<i32, String> {
    bytes(data, at).map(i32::from_le_bytes)
}

fn f32_at(data: &[u8], at: usize) -> Result<f32, String> {
    bytes(data, at).map(f32::from_le_bytes)
}

fn position(data: &[u8], at: usize) -> Result<usize, String> {
    usize::try_from(i64_at(data, at)?).map_err(|_| "bad animation offset".to_owned())
}

/// `base + 4 · words`, refusing to leave the file's start.
fn words(base: usize, words: i32) -> Result<usize, String> {
    usize::try_from(base as i64 + 4 * i64::from(words))
        .map_err(|_| "bad animation track offset".to_owned())
}

/// OWLib's `UnpackRotation` (which writes √2 as 1.41421).
fn rotation(a: u16, b: u16, c: u16) -> [f32; 4] {
    let axis = (b >> 15) << 1 | a >> 15;
    let x = SQRT_2 * (f64::from(a & 0x7FFF) - 16384.0) / 32768.0;
    let y = SQRT_2 * (f64::from(b & 0x7FFF) - 16384.0) / 32768.0;
    let z = SQRT_2 * (f64::from(c) - 32768.0) / 65536.0;
    let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    let q = match axis {
        0 => [w, x, y, z],
        1 => [x, w, y, z],
        2 => [x, y, w, z],
        _ => [x, y, z, w],
    };
    q.map(|v| v as f32)
}

/// OWLib's `UnpackScale`.
fn scale(a: u16, b: u16, c: u16) -> [f32; 3] {
    [a, b, c].map(|v| (f64::from(v) / 1024.0) as f32)
}

/// Reads a clip.
pub(crate) fn read(data: &[u8]) -> Result<Animation, String> {
    let count = usize::from(u16_at(data, BONE_COUNT)?);
    let duration = f32_at(data, DURATION)?;
    let fps = f32_at(data, FPS)?;
    // C#'s (int) cast truncates toward zero.
    let frames = (fps * duration) as usize + 1;
    let list = position(data, BONE_LIST)?;
    let tables = position(data, TRACKS)?;
    let frame_list = |at: usize, count: u16| -> Result<Vec<usize>, String> {
        (0..usize::from(count))
            .map(|k| {
                let raw = if frames <= 255 {
                    i32::from(*data.get(at + k).ok_or("truncated animation")?)
                } else {
                    i32::from(i16::from_le_bytes(bytes(data, at + 2 * k)?))
                };
                Ok(raw.unsigned_abs() as usize % frames)
            })
            .collect()
    };
    let mut tracks = Vec::with_capacity(count);
    for index in 0..count {
        let table = tables + TRACK_TABLE * index;
        let bone = i32_at(data, list + 4 * index)? as u32;
        let counts = [
            u16_at(data, table)?,
            u16_at(data, table + 2)?,
            u16_at(data, table + 4)?,
        ];
        if u16_at(data, table + 6)? & NEW_ROTATIONS != 0 {
            return Err(format!("bone {bone:X}: rotations in OWLib's newer packing"));
        }
        let offset = |k: usize| i32_at(data, table + 8 + 4 * k).and_then(|o| words(table, o));
        let [scale_count, position_count, rotation_count] = counts;
        let mut track = Track {
            bone,
            ..Track::default()
        };
        let at = offset(3)?;
        for (k, frame) in frame_list(offset(0)?, scale_count)?.into_iter().enumerate() {
            let at = at + 6 * k;
            let value = scale(
                u16_at(data, at)?,
                u16_at(data, at + 2)?,
                u16_at(data, at + 4)?,
            );
            track.scales.insert(frame, value);
        }
        let at = offset(4)?;
        for (k, frame) in frame_list(offset(1)?, position_count)?
            .into_iter()
            .enumerate()
        {
            let at = at + 12 * k;
            let value = [
                f32_at(data, at)?,
                f32_at(data, at + 4)?,
                f32_at(data, at + 8)?,
            ];
            track.positions.insert(frame, value);
        }
        let at = offset(5)?;
        for (k, frame) in frame_list(offset(2)?, rotation_count)?
            .into_iter()
            .enumerate()
        {
            let at = at + 6 * k;
            let value = rotation(
                u16_at(data, at)?,
                u16_at(data, at + 2)?,
                u16_at(data, at + 4)?,
            );
            track.rotations.insert(frame, value);
        }
        tracks.push(track);
    }
    Ok(Animation {
        additive: u16_at(data, FLAGS)? & ADDITIVE != 0,
        duration,
        fps,
        frames,
        tracks,
    })
}

/// The keys around `frame` (a fractional frame) and how far between them.
fn around<T: Copy>(keys: &BTreeMap<usize, T>, frame: f64) -> Option<(T, T, f64)> {
    let below = keys.range(..=frame.floor() as usize).next_back();
    let above = keys.range(frame.ceil() as usize..).next();
    match (below, above) {
        (Some((&a, &va)), Some((&b, &vb))) if b > a => {
            Some((va, vb, (frame - a as f64) / (b - a) as f64))
        }
        (Some((_, &v)), _) | (None, Some((_, &v))) => Some((v, v, 0.0)),
        (None, None) => None,
    }
}

fn lerp<const N: usize>(a: [f32; N], b: [f32; N], t: f64) -> [f32; N] {
    std::array::from_fn(|k| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * t) as f32)
}

/// Normalised linear blend along the shorter arc; the key itself on a key.
fn blend(a: [f32; 4], mut b: [f32; 4], t: f64) -> [f32; 4] {
    if t == 0.0 {
        return a;
    }
    if (0..4).map(|k| a[k] * b[k]).sum::<f32>() < 0.0 {
        b = b.map(|v| -v);
    }
    let q = lerp(a, b, t);
    let norm = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / norm)
}

impl Track {
    /// The bone's transform at `frame` (fractional), the rest transform
    /// where the track keys nothing.
    pub fn sample(&self, frame: f64, rest: Bone) -> Bone {
        Bone {
            rotation: around(&self.rotations, frame)
                .map_or(rest.rotation, |(a, b, t)| blend(a, b, t)),
            scale: around(&self.scales, frame).map_or(rest.scale, |(a, b, t)| lerp(a, b, t)),
            translation: around(&self.positions, frame)
                .map_or(rest.translation, |(a, b, t)| lerp(a, b, t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One animated bone of a fixture clip: its id and keys (frame, packed
    /// value).
    struct Keys {
        bone: i32,
        scales: Vec<(u8, [u16; 3])>,
        positions: Vec<(u8, [f32; 3])>,
        rotations: Vec<(u8, [u16; 3])>,
    }

    /// A clip of `fps · duration + 1` frames with one track per bone.
    struct Clip {
        /// The header's first word (`0x104` for a layer of deltas).
        flags: u16,
        fps: f32,
        duration: f32,
        bones: Vec<Keys>,
    }

    impl Clip {
        fn bytes(&self) -> Vec<u8> {
            let mut data = vec![0; 80];
            data[0..2].copy_from_slice(&self.flags.to_le_bytes());
            data[2..4].copy_from_slice(&(self.bones.len() as u16).to_le_bytes());
            data[16..20].copy_from_slice(&self.duration.to_le_bytes());
            data[24..28].copy_from_slice(&self.fps.to_le_bytes());
            let list = data.len();
            data[48..56].copy_from_slice(&(list as i64).to_le_bytes());
            for Keys { bone, .. } in &self.bones {
                data.extend_from_slice(&bone.to_le_bytes());
            }
            let tables = data.len();
            data[72..80].copy_from_slice(&(tables as i64).to_le_bytes());
            data.resize(tables + TRACK_TABLE * self.bones.len(), 0);
            for (index, keys) in self.bones.iter().enumerate() {
                let Keys {
                    scales,
                    positions,
                    rotations,
                    ..
                } = keys;
                let table = tables + TRACK_TABLE * index;
                let counts = [scales.len(), positions.len(), rotations.len()];
                for (k, count) in counts.iter().enumerate() {
                    data[table + 2 * k..table + 2 * k + 2]
                        .copy_from_slice(&(*count as u16).to_le_bytes());
                }
                let mut offsets = [0i32; 6];
                // Frame lists, each padded to a word.
                let frames: [Vec<u8>; 3] = [
                    scales.iter().map(|k| k.0).collect(),
                    positions.iter().map(|k| k.0).collect(),
                    rotations.iter().map(|k| k.0).collect(),
                ];
                for (k, list) in frames.iter().enumerate() {
                    offsets[k] = ((data.len() - table) / 4) as i32;
                    data.extend_from_slice(list);
                    data.resize(data.len().next_multiple_of(4), 0);
                }
                offsets[3] = ((data.len() - table) / 4) as i32;
                for (_, value) in scales {
                    value
                        .iter()
                        .for_each(|v| data.extend_from_slice(&v.to_le_bytes()));
                }
                data.resize(data.len().next_multiple_of(4), 0);
                offsets[4] = ((data.len() - table) / 4) as i32;
                for (_, value) in positions {
                    value
                        .iter()
                        .for_each(|v| data.extend_from_slice(&v.to_le_bytes()));
                }
                offsets[5] = ((data.len() - table) / 4) as i32;
                for (_, value) in rotations {
                    value
                        .iter()
                        .for_each(|v| data.extend_from_slice(&v.to_le_bytes()));
                }
                data.resize(data.len().next_multiple_of(4), 0);
                for (k, offset) in offsets.iter().enumerate() {
                    let at = table + 8 + 4 * k;
                    data[at..at + 4].copy_from_slice(&offset.to_le_bytes());
                }
            }
            data
        }
    }

    /// Packs (x, y, z) with w the left-out part, as the client stores it.
    fn pack(x: f64, y: f64, z: f64) -> [u16; 3] {
        let a = (x * 32768.0 / SQRT_2 + 16384.0).round() as u16 | 0x8000;
        let b = (y * 32768.0 / SQRT_2 + 16384.0).round() as u16 | 0x8000;
        let c = (z * 65536.0 / SQRT_2 + 32768.0).round() as u16;
        [a, b, c]
    }

    #[test]
    fn a_clip_reads_its_bones_keys_and_values() {
        // A sixth of a turn about x: (sin 30°, 0, 0, cos 30°).
        let (x, w) = (0.5, 0.75f64.sqrt());
        let clip = Clip {
            flags: 0x100,
            fps: 30.0,
            duration: 1.0,
            bones: vec![
                Keys {
                    bone: 0x11,
                    // Whole scales, `v/1024`: 1, then half, double and none.
                    scales: vec![(0, [1024, 1024, 1024]), (15, [512, 2048, 0])],
                    positions: vec![(0, [0.0, 1.0, 0.0]), (30, [0.0, 2.0, 0.5])],
                    rotations: vec![(0, pack(0.0, 0.0, 0.0)), (30, pack(x, 0.0, 0.0))],
                },
                Keys {
                    bone: 0x2A,
                    scales: Vec::new(),
                    positions: vec![(10, [1.0, 0.0, 0.0])],
                    rotations: Vec::new(),
                },
            ],
        };
        let animation = read(&clip.bytes()).unwrap();
        assert!(!animation.additive);
        assert_eq!(animation.frames, 31);
        assert_eq!(animation.fps, 30.0);
        let [head, other] = &animation.tracks[..] else {
            panic!("{animation:?}")
        };
        assert_eq!(head.bone, 0x11);
        assert_eq!(head.scales[&0], [1.0; 3]);
        assert_eq!(head.scales[&15], [0.5, 2.0, 0.0]);
        assert_eq!(head.positions[&30], [0.0, 2.0, 0.5]);
        let turn = head.rotations[&30];
        assert!((turn[0] - x as f32).abs() < 1e-4, "{turn:?}");
        assert!((turn[3] - w as f32).abs() < 1e-4, "{turn:?}");
        assert_eq!(other.bone, 0x2A);
        assert_eq!(other.positions.keys().copied().collect::<Vec<_>>(), [10]);
        assert!(other.rotations.is_empty());
        let layer = Clip {
            flags: 0x104,
            ..clip
        };
        assert!(read(&layer.bytes()).unwrap().additive);
        // A track flagged with OWLib's newer rotation packing is refused;
        // `0x100` (set on most 1.74 tracks) is not.
        let mut flagged = layer.bytes();
        let tables = i64::from_le_bytes(flagged[TRACKS..TRACKS + 8].try_into().unwrap()) as usize;
        flagged[tables + 6..tables + 8].copy_from_slice(&0x0111u16.to_le_bytes());
        assert!(read(&flagged).is_ok());
        flagged[tables + 6..tables + 8].copy_from_slice(&0x0511u16.to_le_bytes());
        assert!(read(&flagged).unwrap_err().contains("newer packing"));
    }

    /// Every 1.74 clip reads: no track has the newer rotation packing, and
    /// the scales are whole scales around 1 (mostly 1, some far from it).
    #[test]
    #[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
    fn every_clip_of_the_install_reads() {
        let install = crate::Install::open(&lucy_testkit::game_dir(), "enUS").unwrap();
        let clips = install.guids(0x006);
        assert_eq!(clips.len(), 10_523);
        let (mut parts, mut unit, mut doubled) = (0usize, 0usize, 0usize);
        for clip in clips {
            let animation = read(&install.raw(clip).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", crate::guid_hex(clip)));
            for scale in animation
                .tracks
                .iter()
                .flat_map(|track| track.scales.values())
            {
                parts += 3;
                unit += scale.iter().filter(|part| **part == 1.0).count();
                doubled += scale.iter().filter(|part| **part >= 2.0).count();
            }
        }
        assert!(unit * 3 > parts * 2, "{unit} of {parts} parts are 1");
        assert!(doubled > 1000, "{doubled} parts at least 2");
    }

    #[test]
    fn rotations_unpack_each_left_out_part() {
        // The left-out part is the one the top bits name, rebuilt positive.
        let [a, b, c] = pack(0.0, 0.0, 0.0);
        assert_eq!(rotation(a & 0x7FFF, b & 0x7FFF, c), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(rotation(a, b & 0x7FFF, c), [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(rotation(a & 0x7FFF, b, c), [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(rotation(a, b, c), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn sampling_interpolates_between_keys_and_holds_outside() {
        let rest = Bone {
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            translation: [9.0, 9.0, 9.0],
        };
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let track = Track {
            bone: 1,
            positions: BTreeMap::from([(10, [0.0, 1.0, 0.0]), (20, [0.0, 3.0, 0.0])]),
            rotations: BTreeMap::from([(0, [0.0, 0.0, 0.0, 1.0]), (20, [half, 0.0, 0.0, half])]),
            ..Track::default()
        };
        let middle = track.sample(15.0, rest);
        assert_eq!(middle.translation, [0.0, 2.0, 0.0]);
        assert_eq!(middle.scale, [1.0; 3]);
        // Halfway to a quarter turn about x: an eighth turn.
        let at10 = track.sample(10.0, rest);
        let eighth = (std::f32::consts::PI / 8.0).sin();
        assert!((at10.rotation[0] - eighth).abs() < 1e-6, "{at10:?}");
        assert_eq!(track.sample(0.0, rest).translation, [0.0, 1.0, 0.0]);
        assert_eq!(track.sample(25.0, rest).translation, [0.0, 3.0, 0.0]);
        assert_eq!(track.sample(25.0, rest).rotation, [half, 0.0, 0.0, half]);
    }
}
