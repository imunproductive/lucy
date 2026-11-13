//! The announcer's multikill lines ("Double kill" to "Six kills").
//!
//! Neither Python nor upstream has them. Research (104319 data):
//!
//! - The PvP controller graph `0C90` runs graph `0215` under its st116
//!   (a BooleanSwitch on the owner) as st117 (SubScript). No other graph
//!   names `0215`, and no other graph names its five voice stimuli.
//! - The Practice Range: in retail the announcer says these lines there,
//!   for kills of Training Bots too (seen in the game). The map (`0688`) has
//!   two modes. In `0018` (the solo Range, which Python ran) the player's
//!   controller is `13C1`, and nothing in that mode runs `0215`: not 13C1 or
//!   the graphs it starts (`20E4`, `288A`), not the mode script `13C0`, the
//!   team body script `0C8E`, the hero's body graphs or the bot graphs. In
//!   `005A` (the group Range, six players, ruleset `0034.0C0`) both teams
//!   have 0C90, so there the player's 0C90 runs `0215`. The server runs the
//!   Range as `005A`, so the lines come as in the PvP modes.
//! - `0215` (server-only part): every elimination the controller gets
//!   (`0093.025`: killer `0012`, victim `0015`) of an enemy hero
//!   (rid15: filter with target tags `06AD` or `031D`) adds 1 to its count
//!   #2023 (rid17) and re-arms the wait st4 of #2036 = 2 s (`m_0DBAFD7F`:
//!   re-armed on a re-entry). When that wait ends, 2 s after the last kill,
//!   the count goes back to 0 (rid44). On each kill, unless #1954 or #21925
//!   (rid54), `ActionSwitch` rid30 on the count sets #10122 to the voice
//!   stimulus of the count (2 `058A`, 3 `058B`, 4 `058C`, 5 `058D`, 6
//!   `058E`; no other count has one) and enters the `ClientOnlyPulser` st21.
//! - The client: the pulse runs st21's client-only plug: if the local
//!   view is the controller's own (rid71), after `STU_30667BB6` it starts the
//!   client-only `VoiceStimulus` st23 on #10122, targets `STU_A3DEF368()` (the
//!   game mode entity, as for the kill feed's `0765.025`), delay 0.2 s. The
//!   mode entity (`16B2`) has the announcer's voice set `001D.05F`, which has
//!   one line for each of the five stimuli. #10122 reaches the client through
//!   st31 (`ClientOnlySharedVars`, presence bit).
//!
//! So the server sends no voice line: it pulses st21 with #10122 set, and the
//! killer's own client says the line. The mode entity is `16B2` in every
//! mode, the Practice Range too.
//!
//! The primer (2026-11-13): st21 is on, at count 0 and not fresh, from the
//! controller's first frame on, so every multikill moves an on pulser's
//! count, which pulses the client as often (reader 0x7FF78990D760). Before,
//! `0215` came with the first multikill, its pulser on at count 1 and
//! fresh, relying on the client pulsing a state it reads for the first time
//! with the fresh bit set; in the game no line played, in the Practice
//! Range or a real match (as for the kill feed,
//! `ProjectedSnapshot::gate_kill_feed`). In retail `0215` runs from the
//! start too (0C90 st116 is on for a human owner), but st21 comes on only
//! with the first multikill: the primer is an assumption, as is the rule
//! it avoids. Assumptions: an elimination is the final
//! blow (the server credits no assists; `0215` rid57 counts any elimination,
//! or with `STU_A3DEF368().id(12421)` only `0091.026`, likely the final
//! blow); every enemy player's body or bot passes rid15. The Range's Training
//! Bots (`1403`, `14ED`) carry only the target tag `02ED`; `031D` is on two
//! other Training Bots only (`14A9`, `14D2`, placed on the Tutorial map
//! `0661`), and `06AD` is on no hero body (only on B.O.B., `25C3`), so the
//! game gives heroes `06AD` itself; since retail counts Training Bot kills,
//! the bots must get it too, or pass rid15 some other way. #1954 (set on
//! `069C.025`) and #21925 (a mode entity variable) stay off. The wait ends
//! before a kill of the same tick is counted. Not done: the hero's own
//! reaction 1.6 s after a third kill (`00C9.078` on the controller, rid85: no
//! hero's voice set has a line for it), the multikill career stats (rid28,
//! `0345`/`0346`/`0348.062`), the "N PLAYER KILL STREAK!" text and the team
//! kill (see `docs/notes/behavior.md`).

use orpheus_core::tick::TICK_PERIOD;

use crate::replication::kill_feed_fresh;

/// `0215` st4's wait, #2036 = 2 s, in world ticks (2000 / 16 = 125).
pub const MULTIKILL_WINDOW_TICKS: u32 = 2000 / TICK_PERIOD.as_millis() as u32;

/// `0215` rid30 to rid35: the voice stimulus (`078`) the announcer says for
/// each count of kills.
const STIMULI: [(u32, u64); 5] = [
    (2, 0x0ee0_0000_0000_058a),
    (3, 0x0ee0_0000_0000_058b),
    (4, 0x0ee0_0000_0000_058c),
    (5, 0x0ee0_0000_0000_058d),
    (6, 0x0ee0_0000_0000_058e),
];

/// One player's multikills: `0215`'s count #2023, its pulser st21 and
/// #10122.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Multikills {
    /// Kills in the running streak.
    count: u32,
    /// The world tick of its last kill.
    last: Option<u32>,
    /// st21's pulses so far (its 8-bit count) and the tick of the last.
    pulses: u8,
    pulsed: Option<u32>,
    /// #10122: the stimulus of the last pulse.
    stimulus: u64,
}

/// What `0215`'s pulser and #10122 send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultikillPulse {
    /// st21's pulse count (8 bits on the wire).
    pub count: u8,
    /// Its fresh bit: the last pulse is at most 500 ms old (the pulser
    /// policy, [`kill_feed_fresh`]). A client that reads st21 for the first
    /// time pulses once with the bit set, else takes the count as seen.
    pub fresh: bool,
    /// #10122, once st21 pulsed.
    pub stimulus: Option<u64>,
}

impl MultikillPulse {
    /// st21 before its first pulse: on, at count 0, not fresh, with no
    /// #10122 (the primer, see the module notes).
    pub const PRIMER: Self = Self {
        count: 0,
        fresh: false,
        stimulus: None,
    };
}

impl Multikills {
    /// The player eliminated an enemy at world tick `tick`. Returns whether
    /// st21 pulsed: the count is one the announcer has a line for.
    pub fn eliminated(&mut self, tick: u32) -> bool {
        if self
            .last
            .is_none_or(|last| tick.saturating_sub(last) >= MULTIKILL_WINDOW_TICKS)
        {
            self.count = 0;
        }
        self.count += 1;
        self.last = Some(tick);
        let Some(&(_, stimulus)) = STIMULI.iter().find(|(count, _)| *count == self.count) else {
            return false;
        };
        self.pulses = self.pulses.wrapping_add(1);
        self.pulsed = Some(tick);
        self.stimulus = stimulus;
        true
    }

    /// st21 and #10122 at world tick `tick`: the primer until st21 pulsed.
    pub fn pulse(&self, tick: u32) -> MultikillPulse {
        let Some(pulsed) = self.pulsed else {
            return MultikillPulse::PRIMER;
        };
        MultikillPulse {
            count: self.pulses,
            fresh: kill_feed_fresh(tick, pulsed),
            stimulus: Some(self.stimulus),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kills each less than 2 s (125 ticks) after the one before make a
    /// streak; the second to the sixth pulse with their stimulus, the first
    /// and the seventh don't; a kill 125 ticks after the last starts over.
    #[test]
    fn kills_within_two_seconds_of_each_other_pulse_the_announcer() {
        let mut kills = Multikills::default();
        assert!(!kills.eliminated(1000));
        assert_eq!(kills.pulse(1000), MultikillPulse::PRIMER);
        let mut tick = 1000;
        for (pulses, stimulus) in [
            (1, 0x0ee0_0000_0000_058a),
            (2, 0x0ee0_0000_0000_058b),
            (3, 0x0ee0_0000_0000_058c),
            (4, 0x0ee0_0000_0000_058d),
            (5, 0x0ee0_0000_0000_058e),
        ] {
            tick += MULTIKILL_WINDOW_TICKS - 1;
            assert!(kills.eliminated(tick));
            assert_eq!(
                kills.pulse(tick),
                MultikillPulse {
                    count: pulses,
                    fresh: true,
                    stimulus: Some(stimulus),
                }
            );
        }
        // A seventh kill has no line: st21 keeps its count and #10122.
        tick += 1;
        assert!(!kills.eliminated(tick));
        assert_eq!(kills.pulse(tick).count, 5);
        // The pulse stays fresh for 500 ms: 31 ticks of 16 ms, not 32.
        let pulsed = tick - 1;
        assert!(kills.pulse(pulsed + 31).fresh);
        assert!(!kills.pulse(pulsed + 32).fresh);
        // The wait ended 2 s after the last kill: a new streak.
        tick += MULTIKILL_WINDOW_TICKS;
        assert!(!kills.eliminated(tick));
        assert!(kills.eliminated(tick + 1));
        assert_eq!(
            kills.pulse(tick + 1),
            MultikillPulse {
                count: 6,
                fresh: true,
                stimulus: Some(0x0ee0_0000_0000_058a),
            }
        );
    }
}
