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

use crate::{
    jam::codec::{Fields, Schemas, Value},
    messages::{self, ChannelError},
};

const HIT: u32 = 25001;
const DEATH: u32 = 25004;
const ELIMINATION: u32 = 25005;
const VOICE: u32 = 27500;
/// ClientInVoice's "no observer override" (+0x80).
const NO_OBSERVER: u32 = 0xffff_ffff;

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
    Elimination {
        killer: u32,
        victim: u32,
    },
    /// `speaker` says the voice line keyed `line` of its voice set, for
    /// `stimulus`, to the client whose body is `listener`.
    Voice {
        speaker: u32,
        listener: u32,
        stimulus: u64,
        line: u64,
    },
}

impl CombatNotice {
    pub fn needs_mode_ui(self) -> bool {
        !matches!(self, Self::Hit { .. } | Self::Voice { .. })
    }

    pub fn required_identity(self) -> Option<u32> {
        match self {
            Self::Hit { target, .. } => Some(target),
            Self::Death { source, killed_by } => killed_by.then_some(source),
            Self::Elimination { victim, .. } => Some(victim),
            Self::Voice { speaker, .. } => Some(speaker),
        }
    }

    pub fn encode(self, schemas: &Schemas) -> Result<Vec<u8>, ChannelError> {
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
            Self::Elimination { killer, victim } => (
                ELIMINATION,
                Fields::from([
                    (0x78, Value::Unsigned(0)),
                    (0x80, entity(killer)),
                    (0x84, entity(victim)),
                    (0x88, Value::Unsigned(0)),
                    (0x89, Value::Bool(false)),
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
        };
        messages::encode(schemas, id, &fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combat_notice_messages_match_python() {
        const KILLER: u32 = 0xa000_0101;
        const TARGET: u32 = 0xa004_0010;
        let schemas = Schemas::game();
        let cases: [(CombatNotice, Option<u32>, &[u8]); 4] = [
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
                },
                Some(TARGET),
                b"\x02\x05\x00\x00\x00\x00\x00\x00\x00\x00\x01\x01\x00\xa0\x10\x00\x04\xa0\x00\x00",
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
            assert_eq!(notice.encode(&schemas).unwrap(), expected);
        }
    }

    /// The voice notice as upstream's `queue_voice_notice` lays it out: on
    /// channel 3 (ClientInVoice), message 0, the speaker, the listener's
    /// body, no observer override, the stimulus and the line key; the
    /// client must know the speaker first.
    #[test]
    fn a_voice_notice_names_the_speaker_stimulus_and_line() {
        const BOT: u32 = 0xc000_1000;
        const BODY: u32 = 0xa000_0101;
        let schemas = Schemas::game();
        let notice = CombatNotice::Voice {
            speaker: BOT,
            listener: BODY,
            stimulus: 0x0ee0_0000_0000_0004,
            line: 0x0760_0000_0000_000b,
        };
        assert_eq!(notice.required_identity(), Some(BOT));
        assert!(!notice.needs_mode_ui());
        let bytes = notice.encode(&schemas).unwrap();
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
}
