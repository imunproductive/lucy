//! Reliable combat notices shared by weapons and damage sources.
//!
//! The field layout follows ow174/game/combat.py for build 104319. Each
//! notice exposes the identity a recipient must know before it is queued.
//!
//! Client research (1.74):
//!
//! - ClientInCombat 25001 `{entity, u32 damage, u8 flags} u32 frame`
//!   (handler 0x7FF7895D2D40) tells the shooter of a hit: it goes in his
//!   component 59's hit list (+0x12D0, records in frame order), which shows
//!   the target's health bar for 5 s (0x7FF7895D8520: `int(1e6 / quanta) * 5`
//!   frames) and feeds the hit marker and its sound (event 7). Flag bit 0
//!   counts at +0x124, else +0x120; bit 1 at +0x128. Unverified: bit 0 =
//!   critical, bit 1 = kill (OW2 cross-check: 19 / 38 damage with 0 / 1, the
//!   last hit 2 / 3).
//! - 25005 `{u64, entity, entity victim, u8 score, bool assist}`
//!   (0x7FF7895D2B50, which reads only +0x84, +0x88 and +0x89) and 25004 `{u8,
//!   entity}` (0x7FF7895D2A70) post 0F76.025 and 0F7B.025 / 0F7C.025 to the
//!   game mode entity's statescript (0x7FF7895D7AF0, the first component-114
//!   entity). Its graph 1FC0 (a client-only child of 0CB6) shows
//!   "ELIMINATED: NAME" (1126.07C; +0x89 assist would be 01F0.07C, +0x88 a
//!   score shown when above 0), "YOU WERE KILLED BY NAME" (1127.07C) or
//!   "YOU DIED" (1128.07C).
//! - ClientInVoice 27500 `{entity speaker, entity, entity, u64 stimulus, u64
//!   line, u32, u64}` (handler B12CD0) queues a voice line: the client
//!   looks the speaker up for its voice set and plays the line whose key
//!   (`076`, the set's `m_voiceLineGuids`) is `line`, for the stimulus
//!   (`078`). leafall903/ORPHEUS (MIT), docs/MCCREE_COMBAT.txt and
//!   `voicelines.py` `queue_voice_notice`: +0x7C the listener's body, +0x80
//!   0xFFFFFFFF (no observer override), the rest 0; a Talon NPC's hit line
//!   played from it on the original client.
//! - 25008 `{f32 x, f32 y, f32 z, f32 w}` (Msg25008_Handler 0x7FF7895D4420)
//!   sets the local player's view (component 59 of the entity at
//!   `world+664`) to the quaternion, absolutely, through the set-view
//!   0x7FF7894E00A0, the only writer of the stored view besides the
//!   per-frame copy. Game check (refs/reports/RespawnFacing.md, "Game
//!   checks (2026-11-16)"): `(0, sin θ/2, 0, cos θ/2)` turned the stored yaw
//!   from 180° to +θ = 90°; Y is up and the yaw is the command's
//!   (forward = `(sin yaw, 0, cos yaw)`).
//! - 25000 `{i32 kind, entity, entity, vec3, vec3, vec3, u32[], f32, u32,
//!   u16, u16, f32, u8, f32[], u64[], u8}` (Msg25000_Handler
//!   0x7FF7895D39A0) queues a combat event on the local player's
//!   component 51 (+0x4C0). The damage direction indicator
//!   0x7FF78970A120 turns each event's source point (+0x9C) against the
//!   camera into one of 8 arcs and raises it by the amount (+0xC0).
//!   The event builder 0x7FF789B30550 reads 3 entries of +0xA8 and 2 of
//!   +0xD8 whatever their counts (refs/reports/PlayerHurt.md, "DayHitArcs").

use crate::{
    jam::codec::{Fields, Schemas, Value},
    messages::{self, ChannelError},
};

const HURT: u32 = 25000;
const HIT: u32 = 25001;
const DEATH: u32 = 25004;
const ELIMINATION: u32 = 25005;
const VIEW: u32 = 25008;
const VOICE: u32 = 27500;
/// ClientInVoice's "no observer override" (+0x80).
const NO_OBSERVER: u32 = 0xffff_ffff;
/// 25000's kind (+0x78) for damage taken: the client's own damage event
/// (0x7FF7894C0A60) uses it, its victim at +0x7C.
const DAMAGE_TAKEN: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CombatNotice {
    Hit {
        target: u32,
        dealt: f64,
        critical: bool,
        killed: bool,
        shot_frame: u32,
    },
    Death {
        source: u32,
        killed_by: bool,
    },
    /// 25005 to `killer` (his body): he eliminated `victim`, or with
    /// `assist` (`+0x89`) he assisted in its elimination: `1FC0` shows
    /// "ASSIST" (`01F0.07C`) in place of "ELIMINATED" (`1126.07C`).
    Elimination {
        killer: u32,
        victim: u32,
        assist: bool,
    },
    /// `speaker` says the voice line keyed `line` of its voice set, for
    /// `stimulus`, to the client whose body is `listener`.
    Voice {
        speaker: u32,
        listener: u32,
        stimulus: u64,
        line: u64,
    },
    /// Turn the player's view to `yaw` (1/65536 turns, the mover's and the
    /// command's convention), level: 25008 with the rotation about Y.
    View {
        yaw: i32,
    },
    /// 25000 to the client whose body `victim` took `dealt` from
    /// `attacker`, who stood at `from` (world): its damage direction arc
    /// and the hurt callbacks (kind 1 with `+0xD8[1] × dealt > 0`).
    Hurt {
        victim: u32,
        attacker: u32,
        from: [f32; 3],
        dealt: f64,
    },
}

impl CombatNotice {
    pub fn needs_mode_ui(self) -> bool {
        !matches!(
            self,
            Self::Hit { .. } | Self::Voice { .. } | Self::View { .. } | Self::Hurt { .. }
        )
    }

    pub fn required_identity(self) -> Option<u32> {
        match self {
            Self::Hit { target, .. } => Some(target),
            Self::Death { source, killed_by } => killed_by.then_some(source),
            Self::Elimination { victim, .. } => Some(victim),
            Self::Voice { speaker, .. } => Some(speaker),
            Self::View { .. } | Self::Hurt { .. } => None,
        }
    }

    pub fn encode(self, schemas: &Schemas) -> Result<messages::Encoded, ChannelError> {
        let entity = |id: u32| Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]));
        let (id, fields) = match self {
            Self::Hit {
                target,
                dealt,
                critical,
                killed,
                shot_frame,
            } => (
                HIT,
                Fields::from([
                    (
                        0x78,
                        Value::Struct(Fields::from([
                            (0, entity(target)),
                            (
                                4,
                                Value::Unsigned(u64::from(dealt.round_ties_even().max(1.0) as u32)),
                            ),
                            (
                                8,
                                Value::Unsigned(u64::from(critical) | (u64::from(killed) << 1)),
                            ),
                        ])),
                    ),
                    (0x84, Value::Unsigned(u64::from(shot_frame))),
                ]),
            ),
            Self::Death { source, killed_by } => (
                DEATH,
                Fields::from([
                    (0x78, Value::Unsigned(2 * u64::from(killed_by))),
                    (0x7c, entity(source)),
                ]),
            ),
            Self::Elimination {
                killer,
                victim,
                assist,
            } => (
                ELIMINATION,
                Fields::from([
                    (0x78, Value::Unsigned(0)),
                    (0x80, entity(killer)),
                    (0x84, entity(victim)),
                    (0x88, Value::Unsigned(0)),
                    (0x89, Value::Bool(assist)),
                ]),
            ),
            Self::Voice {
                speaker,
                listener,
                stimulus,
                line,
            } => (
                VOICE,
                Fields::from([
                    (0x78, entity(speaker)),
                    (0x7c, entity(listener)),
                    (0x80, entity(NO_OBSERVER)),
                    (0x88, Value::Unsigned(stimulus)),
                    (0x90, Value::Unsigned(line)),
                    (0x98, Value::Unsigned(0)),
                    (0xa0, Value::Unsigned(0)),
                ]),
            ),
            Self::View { yaw } => {
                let half = f64::from(yaw) * std::f64::consts::PI / 65536.0;
                let (sin, cos) = half.sin_cos();
                (
                    VIEW,
                    Fields::from([(
                        0x78,
                        Value::Struct(Fields::from([
                            (0, Value::Float32(0.0)),
                            (4, Value::Float32(sin as f32)),
                            (8, Value::Float32(0.0)),
                            (12, Value::Float32(cos as f32)),
                        ])),
                    )]),
                )
            }
            Self::Hurt {
                victim,
                attacker,
                from,
                dealt,
            } => {
                let vector = |[x, y, z]: [f32; 3]| {
                    Value::Struct(Fields::from([
                        (0, Value::Float32(x)),
                        (4, Value::Float32(y)),
                        (8, Value::Float32(z)),
                    ]))
                };
                (
                    HURT,
                    Fields::from([
                        (0x78, Value::Unsigned(DAMAGE_TAKEN)),
                        (0x7c, entity(victim)),
                        (0x80, entity(attacker)),
                        // The hit's normal, and its point in the victim's
                        // frame (the body's origin): the hit effect's,
                        // which no contact set (+0xCA) asks for.
                        (0x84, vector([0.0; 3])),
                        (0x90, vector([0.0; 3])),
                        (0x9c, vector(from)),
                        (0xa8, Value::Array(vec![Value::Unsigned(0); 3])),
                        (0xc0, Value::Float32(dealt as f32)),
                        (0xc4, Value::Unsigned(0)),
                        (0xc8, Value::Unsigned(0)),
                        (0xca, Value::Unsigned(0)),
                        (0xcc, Value::Float32(0.0)),
                        (0xd0, Value::Unsigned(0)),
                        // The builder's defaults (0x7FF7894BFCF0).
                        (0xd8, Value::Array(vec![Value::Float32(1.0); 2])),
                        // No damage tags: the indicator's settings for the
                        // pool hit (0x7FF78954B990).
                        (0xf0, Value::Array(Vec::new())),
                        // Not predicted by the client (its component 80).
                        (0x108, Value::Unsigned(0)),
                    ]),
                )
            }
        };
        messages::encode(schemas, id, &fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn combat_notice_messages_match_python() {
        const KILLER: u32 = 0xa000_0101;
        const TARGET: u32 = 0xa004_0010;
        let schemas = Schemas::game(&lucy_testkit::runtime_dir()).unwrap();
        let cases: [(CombatNotice, Option<u32>, &[u8]); 5] = [
            (
                CombatNotice::Hit {
                    target: TARGET,
                    dealt: 12.5,
                    critical: true,
                    killed: true,
                    shot_frame: 1042,
                },
                Some(TARGET),
                b"\x02\x01\x10\x00\x04\xa0\x0c\x00\x00\x00\x03\x12\x04\x00\x00",
            ),
            (
                CombatNotice::Elimination {
                    killer: KILLER,
                    victim: TARGET,
                    assist: false,
                },
                Some(TARGET),
                b"\x02\x05\x00\x00\x00\x00\x00\x00\x00\x00\x01\x01\x00\xa0\x10\x00\x04\xa0\x00\x00",
            ),
            // An assist: `+0x89` set, the last byte.
            (
                CombatNotice::Elimination {
                    killer: KILLER,
                    victim: TARGET,
                    assist: true,
                },
                Some(TARGET),
                b"\x02\x05\x00\x00\x00\x00\x00\x00\x00\x00\x01\x01\x00\xa0\x10\x00\x04\xa0\x00\x01",
            ),
            (
                CombatNotice::Death {
                    source: KILLER,
                    killed_by: true,
                },
                Some(KILLER),
                b"\x02\x04\x02\x01\x01\x00\xa0",
            ),
            (
                CombatNotice::Death {
                    source: TARGET,
                    killed_by: false,
                },
                None,
                b"\x02\x04\x00\x10\x00\x04\xa0",
            ),
        ];
        for (notice, identity, expected) in cases {
            assert_eq!(notice.required_identity(), identity);
            assert_eq!(notice.encode(&schemas).unwrap().bytes(), expected);
        }
    }

    /// The voice notice as upstream's `queue_voice_notice` lays it out: on
    /// channel 3 (ClientInVoice), message 0, the speaker, the listener's
    /// body, no observer override, the stimulus and the line key; the
    /// client must know the speaker first.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn a_voice_notice_names_the_speaker_stimulus_and_line() {
        const BOT: u32 = 0xc000_1000;
        const BODY: u32 = 0xa000_0101;
        let schemas = Schemas::game(&lucy_testkit::runtime_dir()).unwrap();
        let notice = CombatNotice::Voice {
            speaker: BOT,
            listener: BODY,
            stimulus: 0x0ee0_0000_0000_0004,
            line: 0x0760_0000_0000_000b,
        };
        assert_eq!(notice.required_identity(), Some(BOT));
        assert!(!notice.needs_mode_ui());
        let encoded = notice.encode(&schemas).unwrap();
        let bytes = encoded.bytes();
        assert_eq!(bytes[..2], [3, 0]);
        let (_, crc, _) = messages::locate(VOICE).unwrap();
        let fields = schemas.decode(crc, VOICE, &bytes[2..], true).unwrap();
        let entity = |id: u32| Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]));
        assert_eq!(
            fields,
            Fields::from([
                (0x78, entity(BOT)),
                (0x7c, entity(BODY)),
                (0x80, entity(0xffff_ffff)),
                (0x88, Value::Unsigned(0x0ee0_0000_0000_0004)),
                (0x90, Value::Unsigned(0x0760_0000_0000_000b)),
                (0x98, Value::Unsigned(0)),
                (0xa0, Value::Unsigned(0)),
            ])
        );
    }

    /// 25008 on ClientInCombat (channel 2, message 8), the schema's four
    /// f32 at +0x78: a quarter turn is the quaternion GameChecks sent
    /// (yaw +90°), half a turn faces -Z.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn a_view_notice_is_the_rotation_about_y() {
        let schemas = Schemas::game(&lucy_testkit::runtime_dir()).unwrap();
        let (_, crc, _) = messages::locate(VIEW).unwrap();
        let rotation = |yaw| {
            let notice = CombatNotice::View { yaw };
            assert_eq!(notice.required_identity(), None);
            assert!(!notice.needs_mode_ui());
            let encoded = notice.encode(&schemas).unwrap();
            let bytes = encoded.bytes();
            assert_eq!(bytes[..2], [2, 8]);
            let fields = schemas.decode(crc, VIEW, &bytes[2..], true).unwrap();
            let Some(Value::Struct(quaternion)) = fields.get(&0x78) else {
                panic!("no quaternion in {fields:?}");
            };
            assert_eq!(fields.len(), 1);
            [0, 4, 8, 12].map(|at| match quaternion[&at] {
                Value::Float32(value) => value,
                ref other => panic!("not an f32: {other:?}"),
            })
        };
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(rotation(16384), [0.0, half, 0.0, half]);
        assert_eq!(rotation(0), [0.0, 0.0, 0.0, 1.0]);
        let [x, y, z, w] = rotation(-32768);
        assert_eq!([x, y, z], [0.0, -1.0, 0.0]);
        assert!(w.abs() < 1e-7);
    }

    /// 25000 on ClientInCombat (channel 2, message 0) in the handler's
    /// order (0x7FF7895D39A0): kind 1, the victim, the attacker, a zero
    /// normal and local point, the attacker's world position, the three
    /// u32 and two f32 (1.0, 1.0) the builder reads whatever the counts,
    /// the amount, no tags and not predicted.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn a_hurt_notice_is_the_clients_damage_taken_event() {
        let notice = CombatNotice::Hurt {
            victim: 0xa000_0101,
            attacker: 0xc000_1000,
            from: [1.5, -2.0, 30.25],
            dealt: 8.0,
        };
        assert_eq!(notice.required_identity(), None);
        assert!(!notice.needs_mode_ui());
        let expected: Vec<u8> = [
            &[2, 0][..],
            &[1, 0, 0, 0],                                        // +0x78 kind
            &[0x01, 0x01, 0x00, 0xa0],                            // +0x7C victim
            &[0x00, 0x10, 0x00, 0xc0],                            // +0x80 attacker
            &[0; 12],                                             // +0x84 normal
            &[0; 12],                                             // +0x90 point (victim's frame)
            &[0, 0, 0xc0, 0x3f, 0, 0, 0, 0xc0, 0, 0, 0xf2, 0x41], // +0x9C
            &[3, 0, 0, 0],                                        // +0xA8 count
            &[0; 12],
            &[0, 0, 0, 0x41], // +0xC0 amount
            &[0; 4],          // +0xC4
            &[0; 2],          // +0xC8
            &[0; 2],          // +0xCA contact set
            &[0; 4],          // +0xCC
            &[0],             // +0xD0
            &[2, 0, 0, 0],    // +0xD8 count
            &[0, 0, 0x80, 0x3f, 0, 0, 0x80, 0x3f],
            &[0, 0, 0, 0], // +0xF0 tags
            &[0],          // +0x108 predicted
        ]
        .concat();
        assert_eq!(
            notice
                .encode(&Schemas::game(&lucy_testkit::runtime_dir()).unwrap())
                .unwrap()
                .bytes(),
            expected
        );
    }
}
