//! The voice lines bodies say: the line a body's voice set plays for a
//! criteria its graphs raise on it, which the server tells the clients to
//! play (ClientInVoice 27500, [`CombatNotice::Voice`]).
//!
//! Each hero and training bot body's voice set comes from the install
//! (`game_voice_sets_174.json`, [`orpheus_content::voice_sets`]). The
//! client looks a line up by its key in the set's `m_voiceLineGuids`
//! (`076`), so the server picks the line. That 27500 makes a body say a line
//! comes from leafall903/ORPHEUS (MIT), docs/MCCREE_COMBAT.txt (a Talon
//! NPC's hit line, heard on the original client) and
//! docs/VOICE_STIMULUS_STATE.txt (the Junkenstein mode lines).
//!
//! - Training bots: a hit the bot survives reaches its script as `0011.025`
//!   ([`orpheus_script::bot::DAMAGED`]). Its graph `0C17` (rid 0 to 37) adds
//!   the damage up over a second and, 0.1 s after the hit, raises criteria
//!   `0608` when the sum is under a tenth of the health left, else `0609`, on
//!   the bot (server-only `STUStatescriptActionCriteria` nodes). The six
//!   Practice Range bot bodies share voice set `011D.05F`, whose stimulus
//!   `0003.078` ("PnMnr") answers `0608` and `0004.078` ("PnSvr") `0609`.
//! - Players: the communication wheel's `0686` raises the choice's criteria
//!   on the body (Hello `0AEB` 0.3 s after the choice, Thanks `106C`, the
//!   configurable choices the identifier its map gives); each hero's voice
//!   set answers them with its own lines. A hit the body survives reaches
//!   its script as `0011.025` too ([`crate::combat::damage_message`]), and
//!   its graph `01CA` (or `14B1`) raises `0608` or `0609` 0.15 s later
//!   ([`orpheus_script::player::REACTION_GRAPHS`]), which every hero's set
//!   answers with its "PnMnr" or "PnSvr" lines.
//!
//! Who hears a line: the clients whose bodies the criteria action's
//! audience lists ([`RaisedCriteria::audience`]; everyone when it lists
//! none), within the stimulus's `m_radius` of the speaker. The pain
//! criteria list the hurt body and its attacker, so a bot's pain line after
//! another bot's hit (the Practice Range's firing bots shoot the other
//! team's bots) reaches no client, and after a player's hit only that
//! player, when he stands within 35 m. The wheel's lines name all players.
//!
//! Assumptions: a stimulus waits its shorter cooldown (`m_A73F2EBE`) before
//! it plays again for the same body (the wheel's stimuli have none; `05AD`
//! limits the wheel itself); the eligible lines of a stimulus take turns in
//! the set's order (most weigh 1.0); a line's criteria are judged with
//! nothing raised on the body, so "not `191E`" holds and a map, team or hero
//! criteria never does. The pain stimuli's other fields don't limit who
//! hears them: their play chance (`m_1CC5EF25`) is 1.0 and their priority
//! (`m_priority` 40) and category (`01E` `0002`) only matter against other
//! lines the same body says.

use std::collections::HashMap;

use orpheus_content::voice_sets::{Stimulus, VoiceSet};
use orpheus_core::tick::TICK_PERIOD;
use orpheus_script::vm::RaisedCriteria;
use orpheus_wire::combat_messages::CombatNotice;

/// A line a body says.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceLine {
    pub speaker: u32,
    pub stimulus: u64,
    pub line: u64,
    /// The stimulus's `m_radius`, in metres.
    pub radius: f64,
    /// The bodies that may hear it; none: every body.
    pub audience: Option<Vec<u32>>,
}

impl VoiceLine {
    /// The notice for the client whose body is `listener`.
    pub fn notice(&self, listener: u32) -> CombatNotice {
        CombatNotice::Voice {
            speaker: self.speaker,
            listener,
            stimulus: self.stimulus,
            line: self.line,
        }
    }

    /// Whether the body `listener` standing at `at` hears the line said at
    /// `speaker`: the audience lists it and it stands within the stimulus's
    /// radius.
    pub fn heard(&self, listener: u32, speaker: [f64; 3], at: [f64; 3]) -> bool {
        if self
            .audience
            .as_ref()
            .is_some_and(|audience| !audience.contains(&listener))
        {
            return false;
        }
        let distance = speaker
            .iter()
            .zip(at)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>();
        distance <= self.radius * self.radius
    }
}

/// One body lifetime's voice: when each stimulus last played and whose turn
/// its lines are, by the stimulus's index in the set.
#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    last: HashMap<usize, u32>,
    next: HashMap<usize, usize>,
}

impl VoiceState {
    /// The line the body with entity `speaker` and voice set `set` says at
    /// world tick `tick` for the criteria it raised, if the set answers it,
    /// the stimulus is not cooling down and it has a line that may play.
    /// A line no client can hear still plays (its stimulus cools down).
    pub(crate) fn answer(
        &mut self,
        set: Option<&VoiceSet>,
        speaker: u32,
        criteria: RaisedCriteria,
        tick: u32,
    ) -> Option<VoiceLine> {
        let (index, stimulus) = set?.answer(criteria.identifier)?;
        self.play(index, stimulus, speaker, criteria.audience, tick)
    }

    /// The line the body says at world tick `tick` for the voice stimulus
    /// `stimulus` a server-only `VoiceStimulus` state named
    /// ([`orpheus_script::vm::VoiceRequest`]), if its set has it, it is not
    /// cooling down and it has a line that may play. The stimulus's own
    /// criteria (what raises it from a criteria action) don't apply.
    pub(crate) fn say(
        &mut self,
        set: Option<&VoiceSet>,
        speaker: u32,
        stimulus: u64,
        audience: Option<Vec<u32>>,
        tick: u32,
    ) -> Option<VoiceLine> {
        let (index, stimulus) = set?.stimulus(stimulus)?;
        self.play(index, stimulus, speaker, audience, tick)
    }

    fn play(
        &mut self,
        index: usize,
        stimulus: &Stimulus,
        speaker: u32,
        audience: Option<Vec<u32>>,
        tick: u32,
    ) -> Option<VoiceLine> {
        if self
            .last
            .get(&index)
            .is_some_and(|&last| tick < last.saturating_add(cooldown_ticks(stimulus.cooldown)))
        {
            return None;
        }
        let lines: Vec<u64> = stimulus
            .lines
            .iter()
            .filter(|line| line.eligible())
            .map(|line| line.key)
            .collect();
        if lines.is_empty() {
            return None;
        }
        self.last.insert(index, tick);
        let turn = self.next.entry(index).or_default();
        let line = lines[*turn % lines.len()];
        *turn = turn.wrapping_add(1);
        Some(VoiceLine {
            speaker,
            stimulus: stimulus.guid,
            line,
            radius: stimulus.radius,
            audience,
        })
    }
}

/// A cooldown in whole ticks, rounded up (1 s is 63, 0.5 s 32).
fn cooldown_ticks(seconds: f64) -> u32 {
    (seconds / TICK_PERIOD.as_secs_f64()).ceil() as u32
}

/// A voice stimulus state's delay (`m_B0263D2D`) in whole ticks, rounded
/// up as a cooldown is; none for a negative one.
pub(crate) fn delay_ticks(seconds: f64) -> u32 {
    cooldown_ticks(seconds.max(0.0))
}

/// The death cries the hero's `01CA` raises on the body 0.2 s after it
/// died (rid62 → st19): `23E5` (mode `0008`), `05FC` (the killing hit
/// tagged `04B1`), `05FB` (`0644`), `1C92` (`074E`), else `05FA`; a
/// Training Bot's `0C17` raises `05FB` or `1C92` the same way 0.05 s after
/// (its other deaths leave the cry to the client). They are the only lines
/// a dead body says.
const DEATH_CRIES: [u64; 5] = [
    0x0d80_0000_0000_23e5,
    0x0d80_0000_0000_05fc,
    0x0d80_0000_0000_05fb,
    0x0d80_0000_0000_1c92,
    0x0d80_0000_0000_05fa,
];

/// Whether a dead body may say the line of the criteria `identifier`.
pub(crate) fn is_death_cry(identifier: u64) -> bool {
    DEATH_CRIES.contains(&identifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_content::voice_sets::VoiceCatalog;

    const BOT: u32 = 0xc000_1000;
    const BODY: u64 = 0x0400_0000_0000_1403;
    const MINOR: u64 = 0x0d80_0000_0000_0608;
    const SEVERE: u64 = 0x0d80_0000_0000_0609;

    /// A criteria the graphs raised with no audience.
    fn raised(identifier: u64) -> RaisedCriteria {
        RaisedCriteria {
            identifier,
            target: BOT,
            audience: None,
        }
    }

    /// The bots' pain lines as the install has them (the first three lines
    /// of each stimulus; "not `191E`" on each).
    const SET: &str = r#"{
        "bodies": {"0x0400000000001403": "0x07A000000000011D"},
        "sets": {"0x07A000000000011D": {
            "0x0EE0000000000003": {
                "criteria": {"statescript": "0x0D80000000000608", "not": false},
                "cooldown": 1.0, "cooldown_max": 2.5, "radius": 35.0,
                "lines": [
                    {"key": "0x0760000000000006", "criteria": {"statescript": "0x0D8000000000191E", "not": true}},
                    {"key": "0x0760000000000007", "criteria": {"statescript": "0x0D8000000000191E", "not": true}}
                ]
            },
            "0x0EE0000000000004": {
                "criteria": {"statescript": "0x0D80000000000609", "not": false},
                "cooldown": 0.5, "cooldown_max": 1.0, "radius": 35.0,
                "lines": [
                    {"key": "0x076000000000000B", "criteria": {"statescript": "0x0D8000000000191E", "not": true}},
                    {"key": "0x076000000000000C", "criteria": {"statescript": "0x0D8000000000191E", "not": true}}
                ]
            }
        }}
    }"#;

    /// The pain criteria get their stimulus's lines in turn, each stimulus
    /// on its own cooldown (1 s is 63 ticks, 0.5 s 32); other criteria and
    /// bodies without a set get none.
    #[test]
    fn criteria_take_turns_through_their_lines_after_a_cooldown() {
        let catalog = VoiceCatalog::from_json(SET).unwrap();
        let set = catalog.for_body(BODY);
        let mut voice = VoiceState::default();
        let mut said = |criteria, tick| {
            voice
                .answer(set, BOT, raised(criteria), tick)
                .map(|said| (said.stimulus, said.line))
        };
        assert_eq!(
            said(SEVERE, 100),
            Some((0x0ee0_0000_0000_0004, 0x0760_0000_0000_000b))
        );
        assert_eq!(said(SEVERE, 131), None);
        assert_eq!(
            said(MINOR, 131),
            Some((0x0ee0_0000_0000_0003, 0x0760_0000_0000_0006))
        );
        assert_eq!(
            said(SEVERE, 132),
            Some((0x0ee0_0000_0000_0004, 0x0760_0000_0000_000c))
        );
        assert_eq!(said(MINOR, 193), None);
        assert_eq!(
            said(MINOR, 194),
            Some((0x0ee0_0000_0000_0003, 0x0760_0000_0000_0007))
        );
        assert_eq!(said(0x0d80_0000_0000_191d, 500), None);
        assert_eq!(
            VoiceState::default().answer(
                catalog.for_body(0x0400_0000_0000_0022),
                BOT,
                raised(MINOR),
                500
            ),
            None,
            "Reaper's body isn't in this catalog"
        );
    }

    /// A line is heard within its stimulus's radius of the speaker, by the
    /// bodies its audience lists: the bot's pain line after a player's hit
    /// by that player alone, after another bot's hit by no player.
    #[test]
    fn a_line_is_heard_by_its_audience_within_its_radius() {
        const PLAYER: u32 = 0xa000_0101;
        const OTHER: u32 = 0xa000_0102;
        const SHOOTER: u32 = 0xc000_1001;
        let catalog = VoiceCatalog::from_json(SET).unwrap();
        let set = catalog.for_body(BODY);
        let near = [21.0, 0.0, 28.0];
        let far = [21.0, 1.0, 28.0];
        let everyone = VoiceState::default()
            .answer(set, BOT, raised(SEVERE), 100)
            .unwrap();
        assert_eq!(everyone.radius, 35.0);
        assert!(everyone.heard(PLAYER, [0.0; 3], near));
        assert!(!everyone.heard(PLAYER, [0.0; 3], far));
        let pain = |attacker| {
            VoiceState::default()
                .answer(
                    set,
                    BOT,
                    RaisedCriteria {
                        identifier: SEVERE,
                        target: BOT,
                        audience: Some(vec![BOT, attacker]),
                    },
                    100,
                )
                .unwrap()
        };
        let hurt_by_player = pain(PLAYER);
        assert!(hurt_by_player.heard(PLAYER, [0.0; 3], near));
        assert!(!hurt_by_player.heard(PLAYER, [0.0; 3], far));
        assert!(!hurt_by_player.heard(OTHER, [0.0; 3], near));
        let hurt_by_bot = pain(SHOOTER);
        assert!(!hurt_by_bot.heard(PLAYER, [0.0; 3], near));
        assert!(!hurt_by_bot.heard(OTHER, [0.0; 3], [0.0; 3]));
    }
}
