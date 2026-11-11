//! A training bot's voice lines: the lines its voice set plays for the
//! criteria its graphs raise on it, which the server tells every client to
//! play (ClientInVoice 27500, [`CombatNotice::Voice`]).
//!
//! A hit the bot survives reaches its script as `0011.025`
//! ([`orpheus_script::bot::DAMAGED`]). Its graph `0C17` (rid 0 to 37) adds
//! the damage up over a second and, 0.1 s after the hit, raises criteria
//! `0608` when the sum is under a tenth of the health left, else `0609`, on
//! the bot (server-only `STUStatescriptActionCriteria` nodes, which the
//! client never runs). The six Practice Range bot bodies (`1403`, `143E`,
//! `146E`, `14A9`, `14D2`, `14ED`) all have the voice set `011D.05F`
//! (`STUVoiceSetComponent`), whose stimulus `0003.078` ("PnMnr") has
//! criteria `0608` and `0004.078` ("PnSvr") criteria `0609`. The client
//! looks a line up by its key in the set's `m_voiceLineGuids` (`076`), so
//! the server picks the line. Read from the 104319 install's `05F` and `078`
//! assets; that 27500 makes an NPC say a line comes from leafall903/ORPHEUS
//! (MIT), docs/MCCREE_COMBAT.txt (a Talon NPC's hit line, heard on the
//! original client).
//!
//! Assumptions: a stimulus waits its shorter cooldown (`m_A73F2EBE`; every
//! stimulus of the set has it at most its `m_DD27AC95`, the team lines both
//! 0) before it plays again for the same bot; the lines of a stimulus take
//! turns in the set's order (all weigh 1.0, and each line's own criteria,
//! "not `191E`", holds for a bot no graph gives `191E`); every client hears
//! the line (the stimulus's `m_radius`, 35 m, is left to the client).

use orpheus_wire::combat_messages::CombatNotice;

/// The voice set of the Practice Range bot bodies (`STUVoiceSetComponent`
/// `m_voiceDefinition`).
const BOT_BODIES: [u64; 6] = [
    0x0400_0000_0000_1403,
    0x0400_0000_0000_143e,
    0x0400_0000_0000_146e,
    0x0400_0000_0000_14a9,
    0x0400_0000_0000_14d2,
    0x0400_0000_0000_14ed,
];

/// One stimulus of voice set `011D.05F` the bot graphs call for.
struct Stimulus {
    /// The criteria (`01C`) the stimulus answers (`m_criteria`).
    criteria: u64,
    stimulus: u64,
    /// The shorter cooldown (`m_A73F2EBE`), in 16 ms frames, rounded up.
    cooldown: u32,
    /// The keys (`076`) of its lines, in the set's order.
    lines: &'static [u64],
}

const fn line(key: u64) -> u64 {
    0x0760_0000_0000_0000 | key
}

/// The pain lines of voice set `011D.05F`.
const STIMULI: [Stimulus; 2] = [
    // "PnMnr": 1.0 s.
    Stimulus {
        criteria: 0x0d80_0000_0000_0608,
        stimulus: 0x0ee0_0000_0000_0003,
        cooldown: 63,
        lines: &[
            line(0x06),
            line(0x07),
            line(0x08),
            line(0x09),
            line(0x0a),
            line(0x56),
            line(0x58),
            line(0x70),
            line(0x72),
        ],
    },
    // "PnSvr": 0.5 s.
    Stimulus {
        criteria: 0x0d80_0000_0000_0609,
        stimulus: 0x0ee0_0000_0000_0004,
        cooldown: 32,
        lines: &[
            line(0x0b),
            line(0x0c),
            line(0x0d),
            line(0x0e),
            line(0x0f),
            line(0x59),
            line(0x5a),
            line(0x5b),
            line(0x5c),
            line(0x5d),
        ],
    },
];

/// A line a bot says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BotVoice {
    pub speaker: u32,
    pub stimulus: u64,
    pub line: u64,
}

impl BotVoice {
    /// The notice for the client whose body is `listener`.
    pub fn notice(self, listener: u32) -> CombatNotice {
        CombatNotice::Voice {
            speaker: self.speaker,
            listener,
            stimulus: self.stimulus,
            line: self.line,
        }
    }
}

/// One bot lifetime's voice: when each stimulus last played and whose turn
/// its lines are.
#[derive(Debug, Default)]
pub(crate) struct BotVoiceState {
    last: [Option<u32>; STIMULI.len()],
    next: [usize; STIMULI.len()],
}

impl BotVoiceState {
    /// The line the bot with entity `speaker` and body `body` says at world
    /// tick `tick` for `criteria`, if its voice set answers it and the
    /// stimulus is not cooling down.
    pub(crate) fn answer(
        &mut self,
        body: u64,
        speaker: u32,
        criteria: u64,
        tick: u32,
    ) -> Option<BotVoice> {
        if !BOT_BODIES.contains(&body) {
            return None;
        }
        let index = STIMULI
            .iter()
            .position(|stimulus| stimulus.criteria == criteria)?;
        let stimulus = &STIMULI[index];
        if self.last[index].is_some_and(|last| tick < last.saturating_add(stimulus.cooldown)) {
            return None;
        }
        self.last[index] = Some(tick);
        let line = stimulus.lines[self.next[index] % stimulus.lines.len()];
        self.next[index] = self.next[index].wrapping_add(1);
        Some(BotVoice {
            speaker,
            stimulus: stimulus.stimulus,
            line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: u32 = 0xc000_1000;
    const BODY: u64 = 0x0400_0000_0000_1403;
    const MINOR: u64 = 0x0d80_0000_0000_0608;
    const SEVERE: u64 = 0x0d80_0000_0000_0609;

    /// The pain criteria get their stimulus's lines in turn, each stimulus
    /// on its own cooldown; other criteria and other bodies get none.
    #[test]
    fn pain_criteria_take_turns_through_their_lines_after_a_cooldown() {
        let mut voice = BotVoiceState::default();
        let said = |voice: &mut BotVoiceState, criteria, tick| {
            voice
                .answer(BODY, BOT, criteria, tick)
                .map(|said| (said.stimulus, said.line))
        };
        assert_eq!(
            said(&mut voice, SEVERE, 100),
            Some((0x0ee0_0000_0000_0004, 0x0760_0000_0000_000b))
        );
        assert_eq!(said(&mut voice, SEVERE, 131), None);
        assert_eq!(
            said(&mut voice, MINOR, 131),
            Some((0x0ee0_0000_0000_0003, 0x0760_0000_0000_0006))
        );
        assert_eq!(
            said(&mut voice, SEVERE, 132),
            Some((0x0ee0_0000_0000_0004, 0x0760_0000_0000_000c))
        );
        assert_eq!(said(&mut voice, MINOR, 193), None);
        assert_eq!(said(&mut voice, 0x0d80_0000_0000_191d, 500), None);
        assert_eq!(
            voice.answer(0x0400_0000_0000_0022, BOT, MINOR, 500),
            None,
            "Reaper's body has another voice set"
        );
    }
}
